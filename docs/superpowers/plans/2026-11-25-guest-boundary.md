# The guest boundary refuses bad input — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every `mem.*` and `gecko.callExtension` call either runs exactly once with valid input or
fails with an error naming what was wrong.

**Architecture:** The work has three parts:
- The host validates every mem payload field against an allow-list and a type table.
- The sandbox ABI becomes two-step: `host_call_extension` runs the callback once and stashes the
  outcome, and `host_read_reply` takes it. The S3 gate moves ahead of the args read.
- The guest checks only shape (options bags, unknown keys, non-JSON values) and forwards values
  unconverted.

**Tech Stack:** Rust 2024, wasmtime 50, Boa 0.22 (guest, `wasm32-wasip1`), TypeDB 3.15,
`cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-11-25-guest-boundary-refuses-bad-input-design.md`

## Global Constraints

- Never fabricate success over bad input. An absent field takes its documented default; a present
  malformed one is an error naming its key.
- The guest is untrusted. Cap every guest-supplied length before allocating. The S3 scope gate
  runs before any args check.
- Comments are greenfield: no plan tags and no changelog phrasing.
- No `#[allow(dead_code)]`.
- Before every commit, these are clean:
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`;
  - the same with `--features cyber`;
  - `cargo clippy -p js-sandbox --target wasm32-wasip1 -- -D warnings`.
- After any edit to `core/js-sandbox/src/main.rs`, run `just build-guest` and commit
  `core/gecko-engine/src/sandbox/quickjs.wasm` in the same commit. The guest changes once in this
  plan (Task 2), so the PR carries one new blob.
- Integration tests run with `-j 1` / `--test-threads=1`. Never touch the database named `gecko`.
  Don't disrupt a TypeDB you didn't start.
- Ratified docs (INVARIANTS, SPEC-W2-W3, SPEC-W5, CONTRACT-W8, CONFLICT-REPORT) and historical
  docs are not edited.
- The 16 MiB cap message is exactly `host response exceeded 16 MiB`.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **A whole-number float.** `maxChunks: Math.floor(x)` arrives as `5.0` and must be accepted.
   `1e20` must be refused, not saturated. (Task 1: `positive_int` tests.)
2. **A `callExtension("mem", …)` payload** that skips the guest's checks: a non-object, unknown
   keys, or `claims: null`. The host alone must refuse each. (Task 1.)
3. **An ungranted call with junk args** must report "not granted", never an args error. (Task 2:
   `ungranted_call_with_oversized_args_reports_not_granted`.)
4. **A refused or repeated `host_read_reply`** must never return a reply twice or leak a stale one
   into the next call. (Task 2: `take_reply` unit tests, plus the stale-stash clear.)
5. **`undefined` inside an options bag** (`{ asOf: maybeDate }`) must stay idiomatic and mean
   absent, while `{ maxChunk: undefined }` is refused. (Task 2 guest-shape tests.)

---

### Task 1: The host refuses what it cannot use

**Files:**
- Modify: `core/gecko-extension-api/src/epistemic.rs`:
  - add `DerivationMethod::ALL`;
  - remove `ContextBudget::max_tokens`.
- Modify: `core/gecko-engine/src/sandbox/mem_host.rs`: dispatch validation and its unit tests.
- Modify, removing every `max_tokens: None,` struct-literal line:
  - `core/mem-gecko/tests/writer_reader_integration.rs`
  - `core/mem-gecko/tests/semantic_index_integration.rs`
  - `core/mem-gecko/src/writer/tests.rs`
  - `gecko-bin/tests/schema_init_test.rs`
  - `gecko-bin/tests/belief_tier_reconciliation_test.rs`

  Use `grep -rn max_tokens core extensions gecko-bin` for the full list.

**Interfaces:**
- Produces:
  - `pub const ALL: [DerivationMethod; 7]` on `DerivationMethod`, in declaration order;
  - `ContextBudget { max_chunks: usize }`;
  - unchanged signatures for `dispatch_mem_call` and `dispatch_mem_recall`.
- Private helpers in `mem_host.rs`; names and shapes are the builder's, behaviour is fixed:
  - `payload_object(func, p) -> Result<&Map<String, Value>, String>`: refuses a non-object with
    `"mem.<func> payload must be a JSON object"`.
  - `check_keys(func, obj, allowed: &[&str]) -> Result<(), String>`: names the first unknown key
    and lists the allowed ones.
  - `opt_present(p, key) -> Option<&Value>`: `None` for missing or `null`.
  - `opt_str`, `opt_number(f64)`, `opt_rfc3339(DateTime)` and `opt_positive_int(usize)`: each
    returns `Result<Option<T>, String>`.
  - `mem_ids_required(p, key)`: refuses missing and `null`.
- The allow-lists, after the reserved provenance keys are stripped:

  | Function | Allowed keys |
  |---|---|
  | `remember` | `text`, `event_time` |
  | `derive` | `text`, `evidence`, `method`, `confidence` |
  | `supersede` | `old`, `text`, `reason`, `confidence` |
  | `contest` | `claims` |
  | `recall` | `text`, `max_chunks`, `as_of` |

- `opt_positive_int` accepts either:
  - `as_u64()` that is `>= 1` and fits `usize`; or
  - an `as_f64()` `n` with `n.is_finite() && n.fract() == 0.0 && n >= 1.0 && n <= 9007199254740991.0`.

  Everything else is refused.

- [ ] **Step 1: Write the failing tests** in `mem_host.rs`'s test module. Reuse `ProbeWriter`
  (extend it to record each call kind) and `MockReader`. Each refusal asserts that the error
  contains the wire key and that the writer or reader never ran:
  - `a_non_object_payload_is_refused_by_both_dispatchers`: `json!("bare")` to
    `dispatch_mem_call(Derive)` and `dispatch_mem_recall`. The error contains
    `must be a JSON object`; today it says missing `text`, so the test is RED.
  - `an_unknown_key_is_refused_for_each_mem_function`: `max_tokens` on `recall` and `max_chunk` on
    `recall`, then one unknown key on each of `remember`, `derive`, `supersede` and `contest`.
  - `remember_refuses_an_unparseable_event_time`: `"yesterday"` is refused, `42` is refused, and
    absent reaches `observe` with `ctx.occurred_at`.
  - `derive_refuses_an_unknown_method_and_lists_the_accepted_ones`: `"guess"` is refused, the
    error contains every `DerivationMethod::ALL` `as_str()`, and `7` is refused. Absent reaches
    the writer as `LlmSynthesis`.
  - `derive_and_supersede_refuse_a_non_number_confidence`: `"0.5"` is refused on both, and an
    absent value reaches the writer as `None`.
  - `supersede_refuses_a_non_string_reason`: `5` is refused, and absent reaches the writer as
    `""`.
  - `contest_requires_claims`: missing is refused, and so is `null`.
  - `recall_max_chunks_must_be_a_positive_integer`: `5.0` is accepted as 5, and `5` as 5. These
    are refused: `0`, `-1`, `2.5`, `1e20`, `"5"`, `true`. `null` and absent reach the reader as 8.
  - Rewrite `recall_defaults_max_chunks_when_omitted` so it no longer reads `max_tokens`.
  - Rewrite `mem_ids_treats_an_absent_key_as_empty` so it stays valid for `evidence`.

- [ ] **Step 2: Run them and watch them fail**

  Run: `cargo nextest run -p gecko-engine --no-default-features --lib sandbox::mem_host`

  Expected: the new tests FAIL. Each malformed value is accepted today and the writer runs.

- [ ] **Step 3: Implement.**
  - Add `DerivationMethod::ALL`, and make `from_str` iterate it (`ALL.into_iter().find(|m| m.as_str() == s)`)
    so the list stays single-sourced.
  - Remove `ContextBudget::max_tokens` and its literals.
  - In `dispatch_mem_call` and `dispatch_mem_recall`, run `payload_object`, then `check_keys`, then
    the field helpers, before any writer or reader call.
  - Delete `opt_dt`. `as_of` uses `opt_rfc3339`, keeping its current error text, which includes
    `as_of`.

- [ ] **Step 4: Run them and watch them pass,** along with the crates whose literals changed.

  Run:
  ```
  cargo nextest run -p gecko-engine --no-default-features --lib
  cargo nextest run -p gecko-extension-api -p mem-gecko --no-default-features -j 1
  ```

  Expected: PASS. Live TypeDB is needed for the `mem-gecko` integration tests.

- [ ] **Step 5: Commit**: `mem host refuses malformed and unknown payload fields; cut max_tokens`.

### Task 2: The two-step reply ABI and the guest's shape checks

**Files:**
- Modify: `core/gecko-engine/src/sandbox/wasm_executor.rs`
- Modify: `core/js-sandbox/src/main.rs`
- Regenerate: `core/gecko-engine/src/sandbox/quickjs.wasm` (`just build-guest`)
- Test: `core/gecko-engine/tests/wasm_tests.rs`, and the unit tests in `wasm_executor.rs`

**Interfaces:**
- Consumes: Task 1's host refusals; guest-shape tests rely on `contest()` being refused by the host.
- Produces the wasm imports, module `env`:
  - `host_call_extension(ext_ptr: u32, ext_len: u32, func_ptr: u32, func_len: u32, args_ptr: u32, args_len: u32) -> i32`
  - `host_read_reply(out_ptr: u32, out_cap: u32) -> i32`
- `ExecutorCtx` gains `pending_reply: Option<Vec<u8>>`.
- Host-side pure helpers in `wasm_executor.rs`, which the import closures wrap:
  - `const MAX_REPLY_LEN: usize = 16 * 1024 * 1024;`
  - `fn stash_outcome(res: Result<String, String>) -> (Vec<u8>, i32)`. It:
    - replaces an over-cap `Ok` with `Err("host response exceeded 16 MiB")`;
    - replaces an empty `Err` with a fixed non-empty message;
    - truncates an over-cap `Err` at a UTF-8 char boundary;
    - returns the bytes and `+len` for success or `-len` for an error.
  - `fn take_reply(pending: &mut Option<Vec<u8>>, out_cap: u32) -> Result<Vec<u8>, i32>`. It
    takes first, then returns `Err(-1)` for none and `Err(-2)` for a short `out_cap`. The closure
    maps a failed `memory.write` to `-3`.
- `host_call_extension`'s order:
  1. clear `pending_reply`;
  2. get memory, or trap. Both import closures return
     `Box<dyn Future<Output = wasmtime::Result<i32>> + Send + '_>`, and the trap is
     `wasmtime::Error::msg(..)`. `wasmtime::Error` is not `anyhow::Error`, and this file imports
     `anyhow::Result`, so name the wasmtime type explicitly. `host_read_reply` traps on missing
     memory too;
  3. read `ext` and `func`;
  4. apply the S3 gate;
  5. read the args;
  6. parse the JSON;
  7. run the callback;
  8. call `stash_outcome` and store the bytes.

  A failure in steps 3–6 goes through `stash_outcome` as an `Err` with a message:
  - `extension name exceeds 256 bytes or is not UTF-8`;
  - `function name …`;
  - `args exceed 8 MiB or are not UTF-8`;
  - `invalid args JSON: …`;
  - `scope '…' not granted to this concept`.
- Guest helpers in `main.rs`:
  - `fn to_wire(v: &JsValue, name: &str, ctx) -> JsResult<Option<Value>>`. Its rules, in order:
    - `undefined` → `None`;
    - a non-finite number → `TypeError`;
    - `is_callable()` → `TypeError`. Boa 0.22's `to_json` serializes a function as an object and
      never returns `Ok(None)` for one.
    - `to_json` `Err` → `TypeError`. This covers `Symbol`, cyclic objects and `BigInt`.
    - otherwise `Some(json)`.
  - `fn read_bag(args, idx, param: &str, allowed: &[(&str, &str)], ctx) -> JsResult<Map<String, Value>>`.
    It maps camelCase to snake_case. It checks keys first through `JsObject::own_property_keys`:
    a `PropertyKey::Index` key is unknown, and `Symbol` keys are ignored. It then reads each
    accepted key with `get` and passes it through `to_wire`.
- Guest `host_call`:
  - `ret == 0` → error `host call returned no reply`;
  - otherwise a buffer of `ret.unsigned_abs()` bytes, then `host_read_reply`;
  - a negative read → `host reply could not be read (code N)`.

  Delete `HOST_CALL_CAPACITIES` and the sentinel.

- [ ] **Step 1: Write the failing host unit tests** in `wasm_executor.rs` (`#[cfg(test)] mod tests`):
  - `stash_outcome_caps_an_oversized_reply`: an `Ok` of `MAX_REPLY_LEN + 1` bytes →
    `(b"host response exceeded 16 MiB", -29)`.
  - `stash_outcome_gives_an_empty_error_a_message`: the result is negative and non-zero.
    (`Ok("")` cannot occur, because a serialized `Value` is never empty, so it is not special-cased.)
  - `stash_outcome_truncates_an_oversized_error_on_a_char_boundary`: build it from `"é"`
    repeated; the result is valid UTF-8 and at most `MAX_REPLY_LEN`.
  - `take_reply_reads_once`: the first take returns the bytes, and the second returns `Err(-1)`.
  - `take_reply_clears_on_a_short_buffer`: `Err(-2)`, then `Err(-1)`.

- [ ] **Step 2: Write the failing executor tests** in `wasm_tests.rs`. Each uses a counting
  callback, an `Arc<AtomicUsize>`, and the right scope:
  - `a_reply_over_one_mib_arrives_intact_and_runs_once`: the callback returns `{"blob": s}`,
    where `s` is 2 MiB of `"abcdefgh"` repeated. The program returns `{ len: r.blob.length, sum }`,
    where `sum` is the char-code sum over every 4096th char. Asserts `len == 2097152`, `sum`
    equal to the host-computed value, and `count == 1`.
  - `a_reply_over_the_cap_is_refused_after_one_run`: the callback returns
    `"x".repeat(17 << 20)` as a JSON string. Asserts that the error contains
    `host response exceeded 16 MiB` and `count == 1`.
  - `a_callback_error_reaches_the_guest` (characterization; it passes today):
    `Err("boom")` → the error contains `Host call failed: boom`.
  - `an_empty_callback_error_arrives_with_a_message`: `Err("")` → the error is not just
    `Host call failed: `. It is RED today, since the guest reads 0 bytes.
  - `ungranted_call_with_oversized_args_reports_not_granted`: the program passes
    `{ s: "x".repeat(9 << 20) }` with no scope. The error contains `not granted`, and
    `count == 0`.
  - `call_extension_requires_string_names`: `gecko.callExtension(undefined, "f", {})` and
    `gecko.callExtension("e", 5, {})` each give a `TypeError`, and `count == 0`.
  - `call_extension_refuses_unconvertible_args`: cyclic `args` (`const o = {}; o.o = o;`) and
    `args = () => 1` each give a `TypeError`, and `count == 0`.
  - `call_extension_sends_absent_args_as_an_empty_object` (characterization): the callback
    receives `{}`.
- Guest-shape tests: `mem.*` through a recording callback. The callback records `(ext, func, args)`
  and answers `{"id": "x"}`, or `{"chunks": []}` for `recall`. Each refused case asserts a
  `TypeError` mentioning the named thing and that nothing was recorded:
  - `mem_bags_must_be_objects`: `mem.recall("q", 5)`, and `mem.derive("t", [], "opts")`.
  - `mem_bags_refuse_unknown_keys`:
    - `{ maxChunk: 3 }` and `{ maxChunk: undefined }` are refused;
    - `{ maxTokens: 5 }` is refused as unknown.
  - `mem_values_must_be_json`:
    - `{ maxChunks: NaN }`, `{ maxChunks: () => 5 }` and `{ confidence: Symbol() }` are refused;
    - so are `mem.derive("t", NaN)` and `mem.supersede("o", "t", Symbol())`.
  - `mem_undefined_option_means_absent` (characterization): `mem.recall("q", { asOf: undefined, maxChunks: 3 })`
    records args `{ "text": "q", "max_chunks": 3 }` with no `as_of` key.
  - `mem_values_are_forwarded_unconverted`: `mem.recall("q", { maxChunks: 2.5 })` records
    `max_chunks: 2.5`.
  - `mem_recall_requires_a_chunks_array`: replies `{}` and `{"chunks": "bad"}` each throw.
  - `mem_contest_forwards_missing_claims_for_the_host_to_refuse`: through the recording
    callback, `mem.contest()` records `{}` and `mem.contest(null)` records `{"claims": null}`.
    The refusal itself is Task 1's `contest_requires_claims`. Today the guest sends `[]`, so this
    is RED.

- [ ] **Step 3: Run them and watch them fail**

  Run:
  ```
  cargo nextest run -p gecko-engine --no-default-features --lib sandbox::wasm_executor
  cargo nextest run -p gecko-engine --no-default-features --test wasm_tests
  ```

  Expected: the unit tests fail to compile, because the helpers don't exist yet. The executor
  tests FAIL on today's guest: the 1 MiB test runs twice, the S3 test reports a garbage error,
  the bag tests succeed silently, and so on.

- [ ] **Step 4: Implement** the host ABI and the guest, then run `just build-guest`.

- [ ] **Step 5: Run the guest-facing suites.**

  Run:
  ```
  cargo nextest run -p gecko-engine --no-default-features
  cargo nextest run -p gecko-bin --no-default-features --features cyber -j 1 --test mem_recall_e2e_test --test multi_bundle_test --test belief_tier_reconciliation_test --test cyber_detect_test --test epistemic_bridge_test --test index_per_database_cli_test
  ```

  Expected: PASS. If an existing playbook used a now-refused shape, the fix is to the playbook.
  Record it as a ruling.

- [ ] **Step 6: Commit** the source together with the regenerated `quickjs.wasm`:
  `Two-step host reply ABI; guest checks argument shape and forwards values unconverted`.

### Task 3: An oversized recall mints one retrieval-event

**Files:**
- Test: `gecko-bin/tests/mem_recall_e2e_test.rs`

**Interfaces:**
- Consumes: Task 2's ABI, and the file's existing `Fixture`, `provenance_writer` and
  `run_callback`.

- [ ] **Step 1: Probe.** Write the test's seeding first and run it alone. Seed 12 beliefs, each
  about 100 KiB: a distinct prefix plus a repeated filler word. Assert them as `agent-1` through
  the writer.

  Expected: every `assert_belief` succeeds, which confirms that TypeDB 3.15 takes a 100 KiB
  string. If it refuses, rule on a smaller text with more beliefs, keeping the reply over 1 MiB,
  and ledger the ruling.

- [ ] **Step 2: Write the test** `an_oversized_recall_mints_one_retrieval_event`. The program:

  ```js
  const c = mem.recall("filler", { maxChunks: 12 });
  ({ count: c.length, totalBytes: c.reduce((n, x) => n + x.text.length, 0) })
  ```

  Asserts:
  - `count == 12`;
  - `totalBytes > 1 << 20`;
  - exactly one `retrieval-event` (the same `raw_fetch` as `recall_then_derive_records_correlation`).

- [ ] **Step 3: Verify that it would fail without Task 2.** Run it against the previous guest:
  `git stash` is not usable once Task 2 is committed, so check out `quickjs.wasm` and
  `wasm_executor.rs` from `main` temporarily. Expect 2 retrieval-events. Then restore both from
  `HEAD`.

  Expected: FAIL on `main`'s ABI, PASS on `HEAD`.

  Run: `cargo nextest run -p gecko-bin --no-default-features --test mem_recall_e2e_test -j 1`

- [ ] **Step 4: Commit**: `An oversized mem.recall runs once`.

### Task 4: Docs and the ledger

**Files:**
- Modify: `docs/refactor/KNOWN-LIMITATIONS.md`: replace the reply-buffer caveat, around lines
  37–48.
- Modify: `docs/refactor/guest-wasm.md`: describe the two imports, their check order and their
  codes.
- Modify: `extensions/README.md`: the `callExtension` rules (`ext`, `func`, `args`).
- Modify: `docs/refactor/OWED-WORK-LEDGER.md`, as the spec's Goal 4 says:
  - N2 moves to "Landed since the review", noting that `max_tokens` and `maxTokens` were cut;
  - "Next up" becomes 1 N4, 2 S6, 3 N6, 4 N5;
  - N4's and N6a's "Needs" become "its own guest rebuild";
  - two new open rows: unparseable guest stdout reported as success, and cyber's
    `precision.window` refusing `5.0`.
- Modify: the module docs in `mem_host.rs`, if they describe a removed default.

- [ ] **Step 1:** Edit the docs.

  Then run: `grep -rn "max_tokens\|maxTokens\|HOST_CALL_CAPACITIES\|grow-and-retry" core extensions gecko-bin docs/refactor README.md`

  Expected: no hits, outside ratified or historical docs and the ledger's landed entry.

- [ ] **Step 2: Run the full suite.**

  Run: `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1`

  Expected: PASS.

- [ ] **Step 3: Commit**: `docs: the guest boundary and ledger after N2`.
