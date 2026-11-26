# Cyber worked example and `gecko run --args` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:**
- `gecko run <id> --args <json>` gives a playbook a deep-frozen `gecko.args`.
- A cyber sample bundle walks through the whole detection lifecycle against a synced
  ATT&CK database, and a test keeps that walkthrough from going stale.

**Architecture:**
- **PR 1 (N6a)**, Tasks 1–2:
  - the host validates the JSON object and carries it on `PlaybookRun.args`;
  - `WasmExecutor` puts it in the guest's WASI environment as `GECKO_ARGS`;
  - the Boa guest parses it once into a deep-frozen, non-writable `gecko.args`.
- **PR 2 (N6)**, Tasks 3–4: the bundle `extensions/cyber-gecko/sample-bundle/`, a sensor file
  written from ATT&CK 20.1, and a README walkthrough. A spawned-binary test runs the
  walkthrough's own commands against a small fixture with real ATT&CK ids.

**Tech Stack:** Rust 2024, wasmtime + wasmtime-wasi (p1), Boa, TypeDB 3.15, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-11-25-cyber-worked-example-design.md`

## Global Constraints

- **Code rules:**
  - all TQL is parameterized;
  - comments are greenfield;
  - never fabricate success over bad input;
  - no `#[allow(dead_code)]`.
- **Integration tests** run with `-j 1` and never touch the database named `gecko`.
- **Before each commit:**
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`;
  - the same clippy run with `--features cyber`;
  - and, when the guest changes, `cargo clippy -p js-sandbox --target wasm32-wasip1 -- -D warnings`.
- **The guest rebuild.** After editing `core/js-sandbox/src/main.rs`, run `just build-guest`
  and commit `core/gecko-engine/src/sandbox/quickjs.wasm` in the same commit.
- **The args cap** is exactly **1 MiB** (`1024 * 1024` bytes), on the host and in the executor.
- **The environment variable** is named exactly `GECKO_ARGS`.
- **The walkthrough's ATT&CK release** is Enterprise **20.1**. The bundle is staged at
  `/private/tmp/claude-501/-Users-eddie-Documents-repos-gecko/b58caccd-4178-4fc7-a581-9b53edddbe9b/scratchpad/attack/ea.json`.
- **Frozen fixtures** under `*/tests/fixtures/` keep their text. New files are fine.
- **Commit trailer:** `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **`--args` with a non-object value** (`[]`, `"x"`, `null`, `3`) is refused before TypeDB
   is contacted. Task 2 pins it by running against an unreachable address and checking that
   the error is the args error, not a connection error.
2. **An args value over 1 MiB** reaching `WasmExecutor` from a library caller is refused
   with an error result, and the guest never starts. Task 1 pins it.
3. **Mutating nested arrays and objects** inside `gecko.args` has no effect. Task 1 pins
   `list.push` and `a.b =`, in strict and sloppy mode.
4. **A playbook run without `--args`** whose code destructures `gecko.args` sees `{}`, not
   `undefined`. Task 1 pins it.
5. **README drift.** A walkthrough command edited in the README but not in the test fails
   the test. Task 4 parses the `gecko run` commands from the README itself, and asserts that
   the README contains the exact `sync-stix`, `sync-sensors` and `sync` paths the test uses.

---

### Task 0: Branches

- [ ] Commit this plan on `feat/cyber-worked-example`, which already holds the design commits.
- [ ] Create and check out `feat/gecko-run-args` from that head. Tasks 1–2 commit there
  (PR 1).
- [ ] Before Task 3, check out `feat/cyber-worked-example` and fast-forward it to
  `feat/gecko-run-args` (or rebase it onto `main` once PR 1 has merged). Tasks 3–4 commit
  there (PR 2).

### Task 1: `GECKO_ARGS` through the executor into a frozen `gecko.args`

**Files:**
- Modify:
  - `core/gecko-engine/src/pipeline.rs:27-48`: `PlaybookRun` gains `args`, and
    `from_concept` sets `args: None`;
  - `core/gecko-engine/src/sandbox/wasm_executor.rs`: `evaluate` takes a `SandboxRun` and
    sets the env var;
  - `core/gecko-engine/src/sandbox/wasm_pool.rs:53`: `SandboxPool::execute` takes a
    `SandboxRun` and passes it through;
  - `core/js-sandbox/src/main.rs`: reads, parses and freezes the value, and registers the
    property. While there, tidy the comment wrap near `:89` if it reads badly;
  - `core/gecko-engine/src/sandbox/quickjs.wasm`: rebuilt.
- Update every `evaluate(`, `SandboxPool::execute(` and `PlaybookRun {` call site. Each
  passes `args: None`:
  - `core/gecko-engine/tests/wasm_tests.rs`;
  - `core/gecko-engine/tests/pipeline_tests.rs`;
  - `gecko-bin/tests/mem_recall_e2e_test.rs`;
  - `gecko-bin/tests/sandbox_test.rs`;
  - `gecko-bin/src/main.rs:1348`.
- Test: `core/gecko-engine/tests/wasm_tests.rs`.

**Interfaces:**
- Produces:
  - `PlaybookRun<'a> { …, pub args: Option<&'a serde_json::Map<String, serde_json::Value>> }`.
    It stays `Copy`.
  - `pub struct SandboxRun<'a> { pub code: &'a str, pub scopes: &'a [String], pub timeout_ms: Option<u64>, pub args: Option<&'a serde_json::Map<String, Value>> }`,
    `Copy`, in `core/gecko-engine/src/sandbox/wasm_executor.rs`.
  - `WasmExecutor::evaluate(&self, run: SandboxRun<'_>, handle_id: Uuid, host_imports: &HostImports, extension_callback: Option<ExtensionCallback>) -> ExecutionResult`,
    with `SandboxPool::execute` taking the same shape.
  - Adding a plain eighth parameter would trip clippy's `too_many_arguments`, which is why
    the parameters move into a struct.
  - `execute_playbook` builds the `SandboxRun` from its `PlaybookRun`.
  - `pub const MAX_RUN_ARGS_LEN: usize = 1024 * 1024;`, exported from `wasm_executor` for
    Task 2.

- [ ] **Step 1: Write the failing guest tests** in `wasm_tests.rs`. Each one evaluates a
  program with `args` set, and asserts on `result.output` and `result.success`:
  - `run_args_reach_the_guest`:
    - args are `{"logic_id":"detections/x","n":3,"a":{"b":1},"list":[1,2]}`;
    - the program is `({ id: gecko.args.logic_id, n: gecko.args.n, b: gecko.args.a.b, len: gecko.args.list.length })`;
    - the output equals `{"id":"detections/x","n":3,"b":1,"len":2}`.
  - `absent_run_args_are_an_empty_object`: with `args: None`,
    `(() => { const { x } = gecko.args; return { keys: Object.keys(gecko.args).length, x: x === undefined }; })()`
    gives `{"keys":0,"x":true}`.
  - `run_args_are_deep_frozen_in_sloppy_mode`: with the same args as the first test, the
    program assigns `gecko.args = 1`, `gecko.args.n = 9`, `gecko.args.a.b = 9`,
    `gecko.args.extra = 1`, then calls `try { gecko.args.list.push(3) } catch (e) {}`. It
    returns `{ n, b, len, extra: "extra" in gecko.args, same: typeof gecko.args }`, which
    equals `{"n":3,"b":1,"len":2,"extra":false,"same":"object"}`.
  - `run_args_are_deep_frozen_in_strict_mode`: under `"use strict"`, each of these throws a
    `TypeError`, collected in a try/catch per statement:
    - `gecko.args.n = 9`;
    - `gecko.args.extra = 1`;
    - `gecko.args.a.b = 9`;
    - `gecko.args.list.push(3)`;
    - `gecko.args = 1`;
    - `delete gecko.args`.

    The output is six `true`s.
  - `oversized_run_args_are_refused`:
    - the args are one string field of `MAX_RUN_ARGS_LEN` bytes;
    - the scopes are `["t:hit"]`, and an `extension_callback` sets an `Arc<AtomicBool>`;
    - the program calls `gecko.callExtension("t", "hit", {})`.

    The result has `success == false` and `output == Null`, its error contains `"GECKO_ARGS"`
    and `"1048576"`, and the flag stays false, so the guest never ran. (Guest `console.log`
    goes to an uncaptured stderr, so it can't serve as the check.)
  - `run_args_just_under_the_cap_reach_the_guest`: one string field sized so that the
    serialized object is `MAX_RUN_ARGS_LEN - 16` bytes. The program returns
    `gecko.args.s.length`, which equals the field's length.
- [ ] **Step 2: Run** `cargo nextest run -p gecko-engine --test wasm_tests run_args`.
  Expected: compile failure, because `SandboxRun` doesn't exist.
- [ ] **Step 3: Implement the host side.**
  - In `evaluate`, serialize `args` with `serde_json::to_string`, or use `"{}"` for `None`.
  - If the length is over `MAX_RUN_ARGS_LEN`, return the `fail` result
    `"run args (GECKO_ARGS) are {len} bytes; the limit is 1048576"` before building WASI.
  - Otherwise set `wasi_builder.env("GECKO_ARGS", &json)`.
  - Add `SandboxRun`, and add `args` to `PlaybookRun`. Build the `SandboxRun` in
    `execute_playbook`, pass it through `SandboxPool::execute`, and update the call sites
    listed above.
- [ ] **Step 4: Implement the guest side** in `core/js-sandbox/src/main.rs`, before the
  program runs:
  - read the variable with `std::env::var("GECKO_ARGS")`. If it is missing, not valid JSON,
    or not an object, call `print_json_result(false, …)` naming `GECKO_ARGS` and exit
    without running the program;
  - convert the value to a `JsValue` with Boa's JSON conversion;
  - freeze it bottom-up with `fn deep_freeze(obj: &JsObject, context: &mut Context) -> JsResult<()>`,
    which recurses into own enumerable property values that are objects (arrays included),
    then calls `obj.set_integrity_level(IntegrityLevel::Frozen, context)`;
  - treat a `false` from `set_integrity_level` as an error;
  - add it to the `gecko` `ObjectInitializer` as `args` with
    `Attribute::READONLY | Attribute::ENUMERABLE` (so neither writable nor configurable).
- [ ] **Step 5: Rebuild and check.**
  - Run `just build-guest`, then
    `cargo clippy -p js-sandbox --target wasm32-wasip1 -- -D warnings`.
  - Run the step 2 command. Expected: all 6 PASS.
  - Then run `cargo nextest run -p gecko-engine` and
    `cargo nextest run -p gecko-bin --no-default-features -j 1 --test sandbox_test --test mem_recall_e2e_test`.
    Expected: PASS.
- [ ] **Step 6: Commit** "gecko.args: run arguments reach the guest frozen, through GECKO_ARGS",
  including `quickjs.wasm`.

### Task 2: `gecko run --args` and its docs (PR 1)

**Files:**
- Modify:
  - `gecko-bin/src/main.rs`: `Commands::Run` gains `#[arg(long)] args: Option<String>`.
    `cmd_run` takes the parsed map and sets `PlaybookRun.args`;
  - `docs/refactor/guest-wasm.md`: the `GECKO_ARGS` contract;
  - `README.md`: the Usage section shows `--args`.
- Test: unit tests in `gecko-bin/src/main.rs`'s test module (or the module that holds the
  parser), and a new `gecko-bin/tests/run_args_cli_test.rs`.

**Interfaces:**
- Consumes: `PlaybookRun.args` and `MAX_RUN_ARGS_LEN` from Task 1.
- Produces: `fn parse_run_args(raw: &str) -> anyhow::Result<serde_json::Map<String, Value>>`.

- [ ] **Step 1: Write the failing unit tests** for `parse_run_args`:
  - `{"a":1}` gives a map with one key;
  - each of `[]`, `"x"`, `null` and `3` is an error containing `"must be a JSON object"`;
  - `{"a":` is an error containing `"not valid JSON"`;
  - a value of `MAX_RUN_ARGS_LEN + 1` bytes is an error containing `"1048576"`. The length is
    checked first, so a non-JSON oversized string also gives the size error;
  - an object whose raw text is under the cap but whose `serde_json::to_string` form is
    over it is refused too: `parse_run_args` checks both lengths. Build it from many `1e15`
    numbers, which re-serialize longer.
- [ ] **Step 2: Write the failing CLI test** in `run_args_cli_test.rs`.
  - **Setup:**
    - spawn `CARGO_BIN_EXE_gecko`;
    - use the `gecko.toml` shape from `run_missing_database_test.rs`, with no `[extensions]`,
      since this test runs without `cyber`;
    - set `GECKO_CACHE_DIR` to a tempdir, as that test does.
  - **Output and exit status.** A failed playbook still exits 0: `cmd_run` prints
    "Execution failed!" and returns `Ok`. So add a helper
    `fn run_output(out: &std::process::Output) -> serde_json::Value`, which requires
    `Execution successful!` on stdout and parses the compact JSON after the `Output: ` line.
  - `run_args_reach_the_playbook`:
    - sync a one-playbook bundle whose program is `({ echoed: gecko.args.k })` into a
      `TestDb`;
    - run `gecko run <id> --database <db> --args '{"k":"v"}'`;
    - `run_output` equals `{"echoed":"v"}`;
    - the test also uses `db.router()` to assert the bundle's concept exists, so `common`
      has no dead code.
  - `non_object_args_fail_before_connecting`:
    `gecko --config <tempdir config> --address 127.0.0.1:1 run x --args '[]'` exits
    non-zero. Its stderr contains `"must be a JSON object"` and no connection error.
- [ ] **Step 3: Run** the unit tests and
  `cargo nextest run -p gecko-bin --test run_args_cli_test --no-default-features -j 1`.
  Expected: FAIL.
- [ ] **Step 4: Implement.**
  - Call `parse_run_args` in the `Commands::Run` arm before `build_extensions` and before
    any connection.
  - Pass the map into `cmd_run`, and set `args: parsed.as_ref()`.
- [ ] **Step 5: Run** step 3's commands. Expected: PASS.
- [ ] **Step 6: Docs.**
  - **`guest-wasm.md`:** `GECKO_ARGS` is always set; it holds a JSON object of at most
    1 MiB; the guest reads it once and exposes it as the deep-frozen `gecko.args`; it is
    `{}` when `--args` is omitted; it is not gated by any S3 scope.
  - **`README.md` Usage:** one example, `gecko run playbooks/x --database db --args '{"k":"v"}'`.
- [ ] **Step 7:** Run fmt, both clippy runs and the full suite:
  `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1`.
  Expected: all pass. **Commit** "gecko run --args: validated run arguments for a playbook".
- [ ] **Step 8: PR 1.**
  - Run the final whole-branch review on Fable over `main..HEAD`, with one fix pass.
  - The branch is `feat/gecko-run-args`. It carries the design and plan commits, and it is
    created from the current `feat/cyber-worked-example` head before Task 1.
  - Open the PR against `main` with the title "gecko run --args (N6a)".
  - Merge only when the user asks, gating on the required checks.

### Task 3: The sample bundle and sensor file, from a real 20.1 run (PR 2)

**Files:**
- Create:
  - `extensions/cyber-gecko/sample-bundle/bundle.json`, with `{"name":"cyber-sample", …}`;
  - `extensions/cyber-gecko/sample-bundle/detections/encoded_powershell.md` (`T1059.001`);
  - `extensions/cyber-gecko/sample-bundle/detections/scheduled_task_persistence.md`
    (`T1053.005`);
  - `extensions/cyber-gecko/sample-bundle/playbooks/claim_detection.md`, scope
    `cyber-gecko:claim`;
  - `extensions/cyber-gecko/sample-bundle/playbooks/record_verdict.md`, scope
    `cyber-gecko:disposition`;
  - `extensions/cyber-gecko/sample-bundle/playbooks/coverage_report.md`, scopes
    `cyber-gecko:coverage_gaps`, `cyber-gecko:blinded` and `cyber-gecko:recommend_sources`;
  - `extensions/cyber-gecko/sample-bundle/playbooks/detection_precision.md`, scope
    `cyber-gecko:precision`;
  - `extensions/cyber-gecko/sample-bundle/playbooks/withdraw_claim.md`, scope `mem:retract`;
  - `extensions/cyber-gecko/sample-sensors.yaml`.

**Interfaces:**
- Consumes: `gecko.args` from Task 1.
- Produces: the five playbook concept ids above, the two detection concept ids, and the
  manual 20.1 run's output, saved to `<workspace>/attack-20.1-run.md` for Task 4.

- [ ] **Step 1: Read 20.1.** With `jq` over `ea.json`, find the platforms and the log
  sources (data-component name and channel) that the analytics of the detection strategies
  for `T1059.001` and `T1053.005` read on Windows.
- [ ] **Step 2: Write `sample-sensors.yaml`** in the shape the cyber README's sensor-file
  section documents.
  - **What 20.1 holds** (the review checked `ea.json`):
    - `T1059.001`'s Windows analytic, AN1252, reads `WinEventLog:Sysmon` and
      `WinEventLog:PowerShell`;
    - `T1053.005`'s Windows analytic, AN1221, reads `WinEventLog:Security` and
      `WinEventLog:Sysmon`.
  - **The file:**
    - it declares `Windows`;
    - its one sensor collects `WinEventLog:Sysmon` and `WinEventLog:PowerShell`;
    - it omits `WinEventLog:Security`.
  - **The effect:** `T1059.001` is covered. `T1053.005` is blind, because at least one of
    its sources isn't collected, and `recommend_sources` recommends `WinEventLog:Security`.
    Confirm all of this in Step 4.
- [ ] **Step 3: Write the playbooks.**
  - Each passes `gecko.args` fields straight to its import, and returns the import's reply.
    No playbook defaults a required argument.
  - `coverage_report` returns
    `{ undetected: [attack ids from undetected_techniques], by_tactic: [{attack_id, live, undetected}], blind: [logic ids of blinded_detections rows with a non-empty blind_platforms], recommended: [the log sources in recommend_sources' steps] }`.
    Every list is sorted, and these replies' field names are `undetected_techniques`,
    `blinded_detections` and `steps`.
  - `withdraw_claim` calls `mem.retract(gecko.args.belief_id, gecko.args.reason)`.
- [ ] **Step 4: Run the walkthrough by hand against 20.1.**
  - Use a release build with `--features cyber`, a temporary `gecko.toml` with
    `enabled = ["cyber"]`, and a throwaway database named `gecko_n6_walkthrough_<suffix>`.
    Never use `gecko`.
  - Steps, in order:
    1. sync-stix;
    2. sync-sensors;
    3. sync the bundle;
    4. claim both detections: `encoded_powershell` with `method: "human-assertion"` and
       `confidence: 0.8`, and `scheduled_task_persistence` with the same values;
    5. coverage_report (also run once before step 4, as the baseline);
    6. record_verdict `true-positive` on `encoded_powershell`;
    7. detection_precision with `window: 86400`;
    8. withdraw_claim on the `scheduled_task_persistence` belief, with reason
       `"superseded by vendor rule"`.
  - Record every command and its output in `attack-20.1-run.md`, plus the time of step 5.
    Expected: every step succeeds, step 5 finishes inside 30 s, and `blind` is
    `["detections/scheduled_task_persistence"]` with `WinEventLog:Security` in `recommended`. If it doesn't, split
    `coverage_report` into `coverage_gaps_report` and `blind_report`, and ledger a `Ruling:`.
  - Drop the database afterwards.
- [ ] **Step 5: Commit** "cyber sample bundle and sensor file for the worked example".

### Task 4: The README walkthrough, the host-import reference, and the drift test

**Files:**
- Modify:
  - `extensions/cyber-gecko/README.md`: a new `## Worked example`, new `## claim`,
    `## disposition` and `## precision` sections, and linked import names in the intro;
  - `README.md`: one line linking the walkthrough;
  - `docs/refactor/OWED-WORK-LEDGER.md`.
- Create:
  - `gecko-bin/tests/cyber_sample_test.rs`, with `#![cfg(feature = "cyber")]`;
  - a self-contained fixture builder, `gecko-bin/tests/common/walkthrough.rs`, included with
    `#[path = "common/walkthrough.rs"] mod walkthrough;`. `structure.rs`'s helpers are
    private, and including `stix_objects.rs` would leave `sync_stix_fixture` unused.

**Interfaces:**
- Consumes: Task 3's ids and its `attack-20.1-run.md`.
- Produces: the README's walkthrough commands. Each is one line of the form
  `gecko run <concept-id> --database attack [--args '<json>']` inside a ```bash block under
  `## Worked example`. `--args` is absent for `coverage_report`. `withdraw_claim`'s args hold the literal placeholder `"<belief_id>"`.

- [ ] **Step 1: Write the README sections.**
  - **The prerequisite paragraph:** a build with `--features cyber`, and `enabled = ["cyber"]`.
  - **Steps 1–8** as in Task 3, with abbreviated outputs copied from `attack-20.1-run.md`.
    Name Enterprise 20.1, and note that counts differ on other releases.
  - **The reference sections**, carrying the spec's argument notes verbatim in substance:
    - `technique_id` is an ATT&CK id, and `logic_id` is a concept id;
    - `method` takes the kebab-case names, and `confidence` is in `[0.0, 1.0]`, below `1.0`
      for `llm-synthesis`;
    - both ids must already exist in the graph, and `evidence` is optional;
    - `verdict` is one of the three values;
    - `window` is an integer number of seconds greater than 0, and `3600.0` is refused
      (ledger N8);
    - `benign-true` counts as neither a true nor a false positive, and `precision` is
      `null` when both counts are 0;
    - a claim is withdrawn with `mem.retract`.
- [ ] **Step 2: Write the fixture builder.**
  `walkthrough::bundle() -> serde_json::Value` builds a STIX bundle holding:
  - the tactics `execution` (`TA0002`) and `persistence` (`TA0003`);
  - `T1059`, with `T1059.001` as its sub-technique, in execution;
  - `T1053`, with `T1053.005` as its sub-technique, in execution and persistence;
  - for each sub-technique, one detection strategy and one Windows analytic, whose log
    sources are the ones Task 3 found:
    - `T1059.001` reads `WinEventLog:Sysmon` and `WinEventLog:PowerShell`;
    - `T1053.005` reads `WinEventLog:Security` and `WinEventLog:Sysmon`, so Sysmon is shared;
  - the `x-mitre-data-component` objects those log-source references point to.

  `sample-sensors.yaml` then syncs against it, with the same blind and covered split.

  Reuse the object helpers' style from `common/structure.rs`.
- [ ] **Step 3: Write the failing test** `the_readme_walkthrough_runs`:
  - **Setup:**
    - write `walkthrough::bundle()` to a tempdir;
    - write a `gecko.toml` with `enabled = ["cyber"]`;
    - against a `TestDb`, spawn `gecko` for `sync-stix`, `sync-sensors` (with
      `extensions/cyber-gecko/sample-sensors.yaml`), and `sync` of the sample bundle.
  - **Check the setup paths.** Assert that the README contains the exact `sync-sensors`
    path `extensions/cyber-gecko/sample-sensors.yaml` and the `sync` path
    `extensions/cyber-gecko/sample-bundle`.
  - **Parse** the README's `## Worked example` section for every line starting
    `gecko run `. Take its concept id and the optional single-quoted `--args` JSON.
    Expected: exactly 6 lines (2 claims, report, verdict, precision, withdraw).
  - **Run** each one with `--database <test db>` in place of `attack`. Substitute
    `"<belief_id>"` with the `belief_id` the `scheduled_task_persistence` claim returned.
    Read every reply with Task 2's `run_output` helper, copied into this test.
  - **Assert,** on the four-technique fixture (`undetected-techniques()` doesn't credit a
    parent for a claimed sub-technique):
    - **the baseline report, before any claim:** `undetected` is
      `["T1053","T1053.005","T1059","T1059.001"]`;
    - **both claims** return a `belief_id`;
    - **after the claims:**
      - `undetected` is `["T1053","T1059"]`;
      - `TA0002` has `live` 4 and `undetected` 2;
      - `TA0003` has `live` 2 and `undetected` 1;
      - `blind` is `["detections/scheduled_task_persistence"]`;
      - `recommended` contains `WinEventLog:Security`;
    - **the verdict reply** has `logic_id` `detections/encoded_powershell`;
    - **precision** parses as a float equal to `1.0`, with `true_positives` 1;
    - **after the withdrawal,** a fresh report has `undetected` equal to
      `["T1053","T1053.005","T1059"]` and an empty `blind`;
    - **through `db.router()`,** the withdrawn claim's belief is in the `retracted` state.
  - A README edit that breaks a command breaks this test.
- [ ] **Step 4: Run**
  `cargo nextest run -p gecko-bin --test cyber_sample_test --no-default-features --features cyber -j 1`.
  Expected: PASS, after fixing whatever the first run exposes. Then check the guard: put a
  typo in one README `--args` key, see the test fail, and revert.
- [ ] **Step 5: Update the ledger and docs.**
  - The ledger moves N6 (with N6a) to Landed, listing the tests and the design path.
    "Next up" renumbers.
  - The root `README.md` links the walkthrough.
- [ ] **Step 6:** Run fmt, both clippy runs and the full suite. Expected: all pass. **Commit**
  "docs: the cyber worked example and the full host-import reference (N6)".
- [ ] **Step 7: PR 2.**
  - Run the final whole-branch review on Fable, with one fix pass.
  - Open the PR against `main` once PR 1 has merged, or stacked on `feat/gecko-run-args`
    if it hasn't. Its title is "Cyber worked example (N6)".
  - Merge only when the user asks.
