# The guest boundary refuses bad input — design

Closes owed-work ledger item **N2**, including A11's guest half (`evidence` and `claims`). N4
(`mem.retract`) and N6a (`gecko.args`) each get their own guest rebuild later.

## Intent

A playbook that passes a malformed argument to `mem.*` or `gecko.callExtension` today gets a
silent default and a "successful" call. The failure shows up far downstream, or never. That breaks
the "never fabricate success over bad input" rule.

- **The guest** (`core/js-sandbox/src/main.rs`) drops or coerces fields before the host sees them:
  - a non-array `evidence` is discarded;
  - a missing or malformed `claims` becomes `[]`;
  - `maxChunks: "5"` is dropped, `-3` becomes `0` and `2.7` becomes `2`;
  - a non-string `asOf`, `eventTime`, `method` or `reason`, and a non-number `confidence`, are
    dropped;
  - a non-object options bag (`mem.recall(q, 5)`) is ignored, and so is a typo'd key
    (`{ maxChunk: 3 }`);
  - a value `to_json` cannot convert is treated as absent;
  - a reply with no `chunks` becomes `[]`;
  - `callExtension` turns a missing extension or function name into `""`, and an unconvertible
    `args` into `{}`.
- **The host** (`core/gecko-engine/src/sandbox/mem_host.rs`) falls back to defaults:
  - an unparseable `event_time` becomes "now";
  - an unknown `method` becomes `llm-synthesis`;
  - a non-integer `max_chunks` becomes 8;
  - a non-number `confidence` becomes "none";
  - a non-string `reason` becomes `""`.
- **`maxTokens` is accepted and never enforced.** `ContextBudget.max_tokens` is set by the bridge
  and read nowhere in `mem-gecko`, so a token budget is reported as honoured when it is not.
- **The boundary reads args before the scope gate.** `wasm_executor.rs` reads the args (refusing
  over-cap, out-of-bounds or non-UTF-8 input with `-4`) before it checks S3. An ungranted call can
  therefore learn whether its payload was well-formed.
- **The reply buffer runs a call more than once.** `host_call_extension` runs the callback, finds
  the reply too big for the guest's buffer and returns `-5`. The guest retries at 4 MiB, then at
  16 MiB, and each retry runs the callback again. An oversized `mem.recall` mints up to three
  `retrieval-event`s, and an oversized write would commit up to three times.
  (`KNOWN-LIMITATIONS.md` names this caveat and points at N2.)

## Approaches

The argument handling has one sensible shape; see Goals 2 and 3. The reply ABI had two
candidates:

- **Fetch only on overflow.** Keep a 1 MiB first buffer. On overflow the host stashes the reply and
  returns the size it needs, and a new import fetches it. The common case stays one crossing, but
  the return value must still tell an overflow apart from an error length, so a reserved code
  remains.
- **Chosen by the user: always two-step.** Every call runs the callback once and stashes its
  outcome. The guest allocates exactly that size and reads it. This removes the capacity ladder,
  the zero-buffer sentinel and the ambiguous `-5`, and it gives boundary failures a real message.
  It costs one extra in-process crossing per call, which is negligible next to a TypeDB round trip.

## Goals

### 1. The reply ABI: run once, read exactly

- **`host_call_extension(ext_ptr, ext_len, func_ptr, func_len, args_ptr, args_len) -> i32`.** It
  loses its two out-buffer parameters. It does the following, in order:
  1. Clears any pending reply, so a guest can never read a stale one.
  2. Reads `ext` and `func` under `MAX_IMPORT_NAME_LEN`.
  3. Applies the **S3 scope gate**.
  4. Only then reads the args under `MAX_ARGS_JSON_LEN` and parses them.
  5. Runs the callback **at most once**.

  An ungranted call is refused as "not granted" whatever its args are: over the cap, out of
  bounds, non-UTF-8 or not JSON.
- **The outcome is stashed** in `ExecutorCtx` as `pending_reply: Option<Vec<u8>>`:
  - success → the reply JSON's bytes, returning `len` (`>= 1`);
  - failure → the UTF-8 error message, returning `-len` (`<= -1`).

  Every failure carries a message. That includes the boundary failures that today return bare
  codes: a name or args over its cap, out of bounds, or non-UTF-8. A callback error that is empty
  is replaced by a fixed message, so `ret` is never 0.
- **The 16 MiB cap (`MAX_REPLY_LEN`) applies to the stash,** for replies and error messages alike
  (error messages can echo guest values).
  - A reply over the cap is replaced by the error `"host response exceeded 16 MiB"`. The callback
    has already run, once.
  - An error message over the cap is truncated at a UTF-8 boundary.
- **A missing `memory` export traps** in both imports. Neither import can deliver a message without
  memory, and the guest always exports it.
- **`host_read_reply(out_ptr, out_cap) -> i32`,** a new import.
  - It **takes** the pending reply (`Option::take`) **before any check**, so every call clears it,
    a refused read included, and a reply can be read at most once.
  - It then checks `out_cap >= len`, writes the bytes into guest memory and returns `len`.
  - Otherwise it returns a negative code and writes nothing:
    - `-1`: no reply is pending;
    - `-2`: `out_cap` is too small;
    - `-3`: the write is out of bounds.
- **The guest's `host_call`:**
  - calls `host_call_extension`;
  - treats `ret == 0` as the error `host call returned no reply`;
  - otherwise allocates `vec![0u8; ret.unsigned_abs()]` once and calls `host_read_reply`;
  - parses a positive result as JSON;
  - surfaces a negative result as `Host call failed: <message>`;
  - turns a failed `host_read_reply` into the error `host reply could not be read (code N)`.

  `HOST_CALL_CAPACITIES` and the zero-buffer sentinel are deleted.
- **The guest is the only wasm caller** of either import. The host and guest change in one PR, and
  `quickjs.wasm` is rebuilt with `just build-guest`.

### 2. The guest checks shape and forwards values unconverted

- **Options bags.** `recall`'s budget, and `remember`'s, `derive`'s and `supersede`'s opts, must
  each be absent, `undefined`, `null` or an object. The check runs on the `JsValue`, never on
  `to_json` output. An array, a function or a primitive is a `TypeError` naming the parameter.
  - **Keys are checked on the `JsObject` first.** The bag's own keys (`own_property_keys`; an index
    key such as `0` counts as unknown, symbol keys are ignored) are checked
    against the allow-list before any value is read. A key the method does not accept is a
    `TypeError` naming the key and the accepted set. So `{ maxChunk: 3 }` fails rather than
    recalling 8, and so does `{ maxChunk: undefined }`, which `to_json` would drop.
  - **Then each accepted key's value is read** with `get` and passed through `to_wire` (below).
  - A `Date` or `Map` passed as a bag has no own enumerable keys and reads as an empty bag. This
    is stated, not guarded.
  - The accepted sets:
    - `recall`: `maxChunks`, `asOf`;
    - `remember`: `eventTime`;
    - `derive`: `method`, `confidence`;
    - `supersede`: `confidence`.
- **`to_wire(value, name) -> JsResult<Option<Value>>`** is the guest's one conversion helper. Every
  option value and every optional positional argument goes through it: `evidence`, `claims`,
  `reason` and `callExtension`'s `args`.
  - `undefined` → absent: the key is omitted, so `{ asOf: maybeDate }` with an undefined
    `maybeDate` is idiomatic and works.
  - `null` → JSON `null`: forwarded, and the host reads it as absent.
  - A non-finite number (`NaN`, `±Infinity`) → `TypeError`. Boa's `to_json` would turn it into
    `null`.
  - A function (`is_callable`) → `TypeError`, checked before `to_json`, which in Boa 0.22
    serializes a function as an object.
  - `to_json` returning `Err` (a `Symbol`, a cyclic object, a `BigInt`) → `TypeError`.
  - Each `TypeError` names the argument or key.
  - The helper checks only these top-level conditions. A function nested inside an array is
    serialized as an object, and a nested `NaN` becomes `null`. For the only nested wire values,
    `evidence` and `claims`, the host then refuses the entry as not an id string.
- **Otherwise values are forwarded as they are,** renamed to the snake_case wire key, with no
  `as_f64` or `as u64` coercion and no type filter. The host is the single authority on types and
  ranges, as in Goal 3.
- **Positional arguments** go through `to_wire`, and the host validates each:
  - `derive`'s `evidence`;
  - `contest`'s `claims`; absent is sent as absent, and the host refuses it;
  - `supersede`'s `reason`.
- **`mem.recall`** throws unless the reply's `chunks` is an array. It checks the
  `serde_json::Value` before `JsValue::from_json`, so a missing `chunks` and `{ chunks: "bad" }`
  both throw instead of returning `[]` or a string.
- **`gecko.callExtension(ext, func, args?)`.**
  - `ext` and `func` must be strings, or it throws a `TypeError`.
  - `args` that is absent, `undefined` or `null` is sent as `{}`.
  - Otherwise `args` goes through `to_wire`. A non-object `args` is forwarded, and the receiving
    handler judges it.

### 3. The host refuses what it cannot use

`gecko.callExtension("mem", …)` reaches `dispatch_mem_*` under the same scope as `mem.*`, so the
host cannot assume the guest's checks ran.

- **The payload must be a JSON object.** `dispatch_mem_call` and `dispatch_mem_recall` check
  `p.as_object()` first. A non-object payload (`callExtension("mem", "recall", "bare")`) is refused
  as "payload must be a JSON object", never as a confusing missing `text`. `bind_mem_call` still
  leaves a non-object payload untouched; it is the dispatchers that refuse it.
- **Each mem function refuses a payload key outside its allow-list,** naming the key. The
  allow-lists are the table's fields plus `text` (and `old` for `supersede`). The reserved
  provenance keys are stripped before the check, as today.
- **Each optional field** is either **absent** (missing or `null`: the documented default applies)
  or **valid**. Anything else is an error naming the wire key and what it must be. The writer and
  reader never run.

| Field | Absent | Must be |
|---|---|---|
| `remember.event_time` | `ctx.occurred_at` | an RFC 3339 string |
| `derive.method` | `llm-synthesis` | a string `DerivationMethod::from_str` accepts. The error lists the accepted names from a new `DerivationMethod::ALL` |
| `derive.confidence`, `supersede.confidence` | none | a number. The writer's existing `[0.0, 1.0]` and invariant-7 checks are unchanged |
| `derive.evidence` | `[]` | an array of id strings (`mem_ids`, as today) |
| `contest.claims` | — (**required**: missing and `null` are both refused before the writer runs) | an array of id strings. The writer's "at least two" check is unchanged |
| `supersede.reason` | `""` | a string |
| `recall.max_chunks` | 8 | an integer `>= 1` |
| `recall.as_of` | present-state | an RFC 3339 string (as today) |

- **Integers.** A JSON number counts as an integer when either:
  - `as_u64()` succeeds and fits `usize`; or
  - it is a float `n` with `n.is_finite() && n.fract() == 0.0 && n <= 2^53 - 1`
    (`Number.MAX_SAFE_INTEGER`).

  The float form is needed because Boa serializes a whole number held as `Float64` as `5.0`, and
  `Math.floor` returns one. The upper bound stops `1e20`, whose `fract()` is `0.0`, from saturating
  to `usize::MAX` on its way into the index query. The `>= 1` floor then applies. `2.5`, `-1`,
  `0`, `1e20` and strings are refused.
- **The absent defaults are documented contract, not fabrication.** They are unchanged.
- `opt_dt`'s silent fallback is replaced by a helper that returns `Result<Option<DateTime>>`, and
  `as_of` reuses it.
- **`max_tokens` is cut.** `ContextBudget.max_tokens` is removed from `gecko-extension-api`, along
  with its test literals. The wire key and the `maxTokens` option are then refused like any
  unknown key. A token budget returns when the reader enforces one.

### 4. Docs and the ledger

- `KNOWN-LIMITATIONS.md`:
  - the reply-buffer caveat is replaced by the bound as it now stands: a reply over 16 MiB is
    refused after the call ran once;
  - the `just build-guest` caveat stays.
- `docs/refactor/guest-wasm.md` describes the two imports, their order of checks and their return
  codes.
- `extensions/README.md`'s `callExtension` section states the `ext`, `func` and `args` rules.
- **`OWED-WORK-LEDGER.md`:**
  - N2 moves to "Landed since the review". The entry says that `ContextBudget.max_tokens` and
    `maxTokens` were cut, not validated, because nothing enforced them.
  - "Next up" becomes: 1 N4, 2 S6, 3 N6 (with N6a), 4 N5.
  - N4's and N6a's "Needs" become "its own guest rebuild".
  - New open rows, out of scope here:
    - unparseable guest stdout is reported as `success: true` with `raw_output`
      (`wasm_executor.rs` result parsing);
    - cyber's `precision.window` refuses a whole-number float (`5.0`), and should reuse the
      integer helper.

## Non-goals

- **N4 and N6a.** No `mem.retract` and no `gecko.args` stub enters the guest: shipping a surface
  before its host side exists would be dead surface and would lock N6a's wire format early.
- **Cyber's host handlers.** They parse their own args; `claim` already refuses an unknown
  `method`. Beyond the ledger row above, they are not audited here.
- **A11's remaining entry:** `set_retrieval_provenance_op`'s row count.
- **Raising or making configurable** the 16 MiB reply cap or the 8 MiB args cap.
- **Guarding the bag check against exotic objects** (`Date`, `Map`, accessors).

## Constraints

- Never fabricate success over bad input.
- The guest is untrusted: cap every guest-supplied length before allocating, and keep S3 ahead of
  every payload check.
- Comments are greenfield.
- No `#[allow(dead_code)]`.
- fmt and both clippy runs are clean; `cargo clippy -p js-sandbox` is clean too.
- `quickjs.wasm` is rebuilt with `just build-guest` and committed with the guest source.
- Integration tests run with `-j 1` and never touch the database named `gecko`.
- Ratified docs are not edited.

## Verification

- **Host unit tests** in `mem_host.rs`:
  - **one per row of Goal 3's table.** The malformed value is refused with an error naming the key,
    and the probe writer or reader did not run. The absent case reaches the writer or reader with
    the default.
  - `max_chunks: 5.0` is accepted as 5, and `1e20` and `2.5` are refused.
  - An unknown key (`max_tokens`, `max_chunk`) is refused for each mem function.
  - A non-object payload (`json!("bare")`) is refused by both `dispatch_mem_call` and
    `dispatch_mem_recall`.
  - `contest` with `claims` missing and with `claims: null` is refused, and the writer never ran.
- **Executor tests** in `core/gecko-engine/tests/wasm_tests.rs`, with a counting callback:
  - a 2 MiB reply arrives intact, and the callback ran once. The playbook returns the reply's
    length and a checksum, since guest stdout is capped at 10 MiB.
  - a reply over 16 MiB is refused with the cap message, and the callback ran once;
  - an error message over 16 MiB is truncated;
  - a callback error reaches the guest as `Host call failed: <message>`, and an empty one as the
    fixed message;
  - an ungranted call whose args exceed `MAX_ARGS_JSON_LEN` reports "not granted", and the
    callback never ran;
  - `callExtension` with a missing `ext` or `func`, or with cyclic `args`, throws a `TypeError`,
    and the callback never ran.
- **Guest-shape tests,** through the executor:
  - a non-object budget;
  - an unknown option key, including `{ maxChunk: undefined }`;
  - `maxChunks: NaN`;
  - a function or a `Symbol` as an option value (`{ maxChunks: () => 5 }`);
  - a positional `NaN` (`mem.derive("t", NaN)`);
  - a `{ chunks: "bad" }` recall reply;
  - `contest()` and `contest(null)`, which the host refuses.

  Each throws an error naming what was wrong, and the writer or reader never ran.
- **`host_read_reply` unit tests** on the executor's import logic: a second read returns `-1`, and
  a short buffer returns `-2` and clears the reply.
- **End to end** in `gecko-bin/tests/mem_recall_e2e_test.rs`:
  - about 12 beliefs of about 100 KiB each, recalled with `maxChunks: 12` on the **indexed**
    present-state path, return every chunk (over 1 MiB) and mint exactly one `retrieval-event`;
  - the playbook returns a small summary, `{ count, totalBytes }`, rather than the raw chunks;
  - the plan probes first that TypeDB 3.15 accepts a 100 KiB string attribute;
  - as-of recall mints no event, and fallback recall returns empty text, so neither path can
    serve this test.
- **The existing guest tests pass after `just build-guest`:** `wasm_tests.rs`,
  `pipeline_tests.rs`, `mem_recall_e2e_test.rs`, `multi_bundle_test.rs`,
  `belief_tier_reconciliation_test.rs` and the cyber detect tests.
- The full candle-free suite passes.

## Left to the builder

- The exact error strings, apart from the 16 MiB cap message.
- The integer helper's name and its home in `mem_host.rs`.
- Whether the `host_read_reply` unit tests drive a hand-written wasm module or a host-side helper
  the import wraps.
