# `timeout-ms` reaches the run Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A playbook's `timeout-ms` is validated, stored by sync, and honoured by `gecko run`.
`consumes` and `produces` stop being special fields.

**Architecture:**
- **Task 1, the engine:**
  - `MAX_TIMEOUT_MS` and the executor's range check;
  - a named timeout error;
  - strict parsing;
  - the `consumes`/`produces` cut.
- **Task 2, persistence:**
  - the schema attribute;
  - sync's integer writer and its range check;
  - `fetch_playbook`'s guarded read;
  - `gecko run` wiring and the docs.

**Tech Stack:** Rust 2024, wasmtime, TypeDB 3.15, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-11-26-timeout-ms-design.md`

## Global Constraints

- **Code rules:**
  - all TQL is parameterized;
  - comments are greenfield;
  - never fabricate success over bad input;
  - no `#[allow(dead_code)]`.
- **Integration tests** run with `-j 1` and never touch the database named `gecko`.
- **Before each commit:** `cargo fmt --all -- --check`, and both clippy runs (with and
  without `--features cyber`) with `--all-targets -- -D warnings`.
- **The ceiling.** `pub const MAX_TIMEOUT_MS: u64 = 600_000;` in
  `core/gecko-engine/src/sandbox/wasm_executor.rs`. Every layer enforces
  `1..=MAX_TIMEOUT_MS`. The ceiling is never baked into the schema.
- **The schema attribute** is `timeout-ms`, `value integer @range(1..)`, with an `@doc`.
  `concept` owns it `@card(0..1)`. If TypeDB refuses `@range` on the value type at define
  time, move it to the ownership instead (`owns timeout-ms @card(0..1) @range(1..)`, the
  repo's existing pattern), and ledger a `Ruling:`.
- **The timeout error text** is exactly `execution timed out after {ms} ms`.
- **Frozen fixtures** under `*/tests/fixtures/` keep their text.
- **Commit trailer:** `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **YAML `null` or a blank `timeout-ms:`** is refused, not read as absent. Task 1 pins it.
2. **A stored value outside the range** (written by an older or foreign writer) makes
   `gecko run` fail with an error, not fall back to the default. Task 2 pins it with a raw
   insert of `0`.
3. **An older database whose schema lacks `timeout-ms`** still syncs (no query for an absent
   attribute) and still runs a playbook. Task 2 pins the run, and the existing
   `tightened_schema_applies_to_a_populated_database` covers the sync.
4. **Removing `timeout-ms` from a playbook** and re-syncing leaves no stored value. Task 2 pins
   it.
5. **A timed-out run is distinguishable from other traps.** Task 1 pins the message.

---

### Task 1: The ceiling, the named timeout, strict parsing, and the `consumes`/`produces` cut

**Files:**
- Modify:
  - `core/gecko-engine/src/sandbox/wasm_executor.rs`: the constant, the range check, and the
    interrupt mapping;
  - `core/gecko-engine/src/okf/parser.rs:174-179`: strict `timeout-ms`, no `consumes` or
    `produces`. Add the `ParseError` variant.
  - `core/gecko-extension-api/src/okf.rs`: drop `consumes` and `produces`, and fix the module
    doc at `:4`;
  - the struct literals at `extensions/cyber-gecko/src/sensors.rs:263-264`,
    `extensions/cyber-gecko/src/stix/mod.rs:499-500` and
    `core/gecko-engine/src/syncer/bundle.rs:1298-1299`.
- Test: `core/gecko-engine/tests/wasm_tests.rs`, and the parser's test module.

**Interfaces:**
- Produces:
  - `pub const MAX_TIMEOUT_MS: u64 = 600_000;` in `gecko_engine::sandbox::wasm_executor`;
  - `ParseError::InvalidTimeout { path: String, value: String }`, with the message
    `Concept file '{path}' has an invalid 'timeout-ms' ({value}); it must be an integer from 1 to {MAX_TIMEOUT_MS}`.
    The message is built from the constant. `value` is `serde_json::to_string` of the YAML
    value, as `extra_metadata` renders it, so null shows as `null`.

- [ ] **Step 1: Write the failing tests.**
  - **Executor,** in `wasm_tests.rs`:
    - `a_zero_timeout_is_refused` and `a_timeout_over_the_ceiling_is_refused`, with
      `Some(0)` and `Some(MAX_TIMEOUT_MS + 1)`. The result has `success == false` and its
      error contains `"timeout"` and `"600000"`. A granted `extension_callback` flag stays
      false, as in `oversized_run_args_are_refused`.
    - `the_ceiling_itself_is_accepted`: `Some(MAX_TIMEOUT_MS)` runs `1 + 1` to `2`.
    - Extend `test_wasm_epoch_timeout` to assert the error equals
      `"execution timed out after 50 ms"`.
  - **Parser,** in its test module, on the frontmatter of a minimal playbook:
    - `timeout-ms: 500` gives `Some(500)`, and `timeout-ms: 600000` gives `Some(600000)`;
    - an absent key gives `None`;
    - each of these values is `ParseError::InvalidTimeout`, and the message names the file:
      `0`, `-1`, `5.0`, `"500ms"`, `null`, a blank `timeout-ms:`, and `600001`;
    - update the existing test at `parser.rs:668-685`: keep `consumes: [alert, asset]` and
      `produces: [containment-result]` in its fixture, and assert that both land in
      `extra_metadata` as JSON strings (`"[\"alert\",\"asset\"]"` and
      `"[\"containment-result\"]"`).
- [ ] **Step 2: Run** `cargo nextest run -p gecko-engine`. Expected: compile failure (no
  `MAX_TIMEOUT_MS`, no `InvalidTimeout`).
- [ ] **Step 3: Implement.**
  - **The range check.** In `evaluate`, before building WASI, a `Some(t)` outside
    `1..=MAX_TIMEOUT_MS` returns
    `fail(format!("timeout-ms {t} is out of range; it must be from 1 to {MAX_TIMEOUT_MS}"))`.
  - **The interrupt mapping.** Where `start_func.call_async` errors, an error whose
    `downcast_ref::<wasmtime::Trap>()` is `Some(Trap::Interrupt)` becomes
    `fail(format!("execution timed out after {ms} ms"))`, where `ms` is the effective timeout.
    Every other error keeps the `JS Execution trap: {e}` text.
  - **The parser.** Absent is `None`. A present value must be a YAML integer in
    `1..=MAX_TIMEOUT_MS`. A blank `timeout-ms:` is `Some(Value::Null)`, not absent, so it is
    refused. Anything else is `InvalidTimeout`.
  - **The cut.** Remove `consumes` and `produces` from `OkfConcept` and every literal. They then
    fall into `extra_metadata` like any unknown key.
  - **Doc comments.** `SandboxRun.timeout_ms`, `PlaybookRun.timeout_ms` and
    `OkfConcept.timeout_ms` (`okf.rs:102`) state the `1..=MAX_TIMEOUT_MS` range.
- [ ] **Step 4: Run** `cargo nextest run -p gecko-engine`, and
  `cargo build --workspace --exclude js-sandbox --no-default-features --features cyber --tests`.
  Expected: PASS, and it compiles.
- [ ] **Step 5: Commit** "timeout-ms: one ceiling, refused out of range, and a named timeout".

### Task 2: Store `timeout-ms` and honour it in `gecko run`

**Files:**
- Modify:
  - `core/gecko-engine/schema/core_schema.tql`: the attribute, and `concept owns timeout-ms @card(0..1)`;
  - `core/gecko-engine/src/syncer/bundle.rs`: `attach_integer`, its call in
    `attach_concept_content`, and the range check at the start of `sync_bundle`. Add the
    `DbError` variant in `core/gecko-engine/src/db/router.rs`.
  - `gecko-bin/src/main.rs`: `fetch_playbook`'s guarded projection and strict read, and
    `cmd_run` passing `timeout_ms`. Fix the stale comment at `:1370`.
  - `docs/refactor/KNOWN-LIMITATIONS.md`: the unchanged-hash caveat;
  - `docs/refactor/OWED-WORK-LEDGER.md`: N5 moves to Landed;
  - `README.md`, if it lists frontmatter keys; check with `grep -n "scopes:" README.md`.
- Test: `gecko-bin/tests/bundle_sync_test.rs`, and `gecko-bin/tests/run_args_cli_test.rs`,
  which already has the CLI pattern, `TestDb`, and no feature gate.
- Keep the core schema's `    owns retired-at @card(0..1),` line byte-identical, and add the
  new `owns` as a line of its own: `gecko_run_works_on_a_database_without_retired_at`
  string-replaces that line.

**Interfaces:**
- Consumes: `MAX_TIMEOUT_MS` from Task 1.
- Produces:
  - `DbError::InvalidTimeout { concept_id: String, value: u64 }`;
  - `FetchedPlaybook.timeout_ms: Option<u64>`.

- [ ] **Step 1: Write the failing tests.**
  - **`bundle_sync_test.rs`:**
    - `timeout_ms_round_trips`: sync a playbook with `timeout-ms: 500`, and read
      `$c has timeout-ms $t` back as `500`. Edit it to `750` and re-sync: the value is `750`,
      and there is exactly one value. Remove the key and re-sync: there is no value.
    - `an_out_of_range_timeout_is_refused_before_writing`: build an `OkfConcept` with
      `timeout_ms: Some(MAX_TIMEOUT_MS + 1)` and call `sync_bundle`. It is
      `DbError::InvalidTimeout`, and the concept count is unchanged (0).
  - **`run_args_cli_test.rs`** (rename the file's doc comment to cover run inputs):
    - `a_playbook_times_out_at_its_timeout_ms`: a playbook with `timeout-ms: 500` and the
      program `while (true) {}`. `gecko run` prints `Execution failed!` and
      `execution timed out after 500 ms`, in under 25 s of wall time (below the 30 s default).
    - `a_stored_timeout_out_of_range_fails_the_run`: sync a playbook (no `timeout-ms`), then
      raw-insert `has timeout-ms 700000` on its concept. That value passes `@range(1..)` but is
      over the ceiling. Then `gecko run`: it exits non-zero, and the error names `timeout-ms`
      and the concept id. Without the strict read, the run would exit 0 with an executor
      refusal, so the test isn't vacuous.
    - `a_playbook_runs_on_a_schema_without_timeout_ms`. Copy
      `gecko_run_works_on_a_database_without_retired_at`
      (`belief_tier_reconciliation_test.rs:1126-1172`):
      - strip the `attribute timeout-ms …;` line and the `owns timeout-ms @card(0..1),` line
        from `CORE_SCHEMA`, and assert the result no longer contains `timeout-ms`;
      - apply it, plus `mem_gecko::extension().schema()`;
      - raw-insert a playbook concept, and `gecko run` it successfully.
- [ ] **Step 2: Run**
  `cargo nextest run -p gecko-bin --no-default-features -j 1 --test bundle_sync_test --test run_args_cli_test`.
  Expected: FAIL, from a compile error (no `DbError::InvalidTimeout`, no `MAX_TIMEOUT_MS`
  use). Once it compiles, the new tests fail except `a_playbook_runs_on_a_schema_without_timeout_ms`.
  That one is a regression guard and passes now. To see it red once, implement the
  `fetch_playbook` projection without the `defines_attribute` guard first, watch it fail,
  then add the guard.
- [ ] **Step 3: Implement.**
  - **The schema:**
    `attribute timeout-ms @doc("The longest a concept's code block may run, in milliseconds; gecko run refuses a value over its ceiling."), value integer @range(1..);`.
    `concept` owns it `@card(0..1)`.
  - **`attach_integer`.** `attach_integer(tx, concepts, "timeout-ms", |c| c.timeout_ms)` mirrors
    `attach_datetime`:
    - `filter_map` to rows;
    - the query `given $id: string, $val: integer; … insert $c has {attr} == $val;`;
    - it goes through `run_rows`, which sends nothing for zero rows. A sync against a schema
      without `timeout-ms` must never send the query.
    - It binds `Value::Integer(i64::try_from(v)?)`. The range check makes the conversion
      infallible, but handle the error rather than casting.
  - **`sync_bundle`** checks every concept's `timeout_ms` against `1..=MAX_TIMEOUT_MS` before
    opening the write transaction, and returns `InvalidTimeout` for the first offender, by
    concept id.
  - **`fetch_playbook`:**
    - when `defines_attribute(&schema, "timeout-ms")` is true, add `"timeout": [ $c.timeout-ms ]`;
    - zero values give `None`, and one integer in range gives `Some`;
    - anything else is an error: a non-integer, out of range, or more than one value. The
      error is `"concept '{id}' has an invalid stored timeout-ms ({v}); re-sync its bundle"`.
      The more-than-one case is unreachable under `@card(0..1)`. Keep it as a defence, with
      no test.
    - Update the `FetchedPlaybook` and `fetch_playbook` doc comments (`main.rs:1096-1110`).
  - **`cmd_run`** sets `timeout_ms: playbook.timeout_ms`.
- [ ] **Step 4: Run** step 2's command. Expected: PASS.
- [ ] **Step 5: Docs.**
  - **KNOWN-LIMITATIONS:** one entry.
    - A playbook synced before `timeout-ms` was stored keeps its file hash, so a re-sync skips
      it, and its timeout stays at the 30 s default until its content changes. Change
      detection hashes the content, so only editing the file's content fixes it; touching it
      doesn't.
    - The same holds for `consumes` and `produces`: an unchanged, already-synced file doesn't
      gain them in `metadata-json` until it is edited.
    - A bundle with a `timeout-ms` the old parser tolerated (`0`, `"5s"`) now fails
      `parse_bundle`, and so the whole sync, naming the file.
  - **The ledger:** N5 moves to Landed with its tests and the design path, and "Next up"
    renumbers.
  - **README:** if it documents frontmatter, add `timeout-ms` (1 to 600000 ms, default
    30000).
- [ ] **Step 6:** Run fmt, both clippy runs and the full suite:
  `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1`.
  Expected: all pass. **Commit** "timeout-ms is stored by sync and honoured by gecko run (N5)".

## Finish

- Final whole-branch review on Fable, with one fix pass.
- A PR against `main`, with the CI checks watched.
- Merge only when the user asks, gating on the required checks.
