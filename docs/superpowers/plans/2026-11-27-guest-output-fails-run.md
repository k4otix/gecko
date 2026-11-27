# A failed run is reported as a failure — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Non-JSON or empty guest stdout fails the run (N7), every guest failure carries a message, and a failed `gecko run` exits non-zero.

**Architecture:** A pure `read_guest_output` in `wasm_executor.rs` replaces `evaluate`'s inline parsing. `cmd_run` returns a `PlaybookFailed` marker error after rendering a failed result, and `main` maps it to `ExitCode::FAILURE` without logging.

**Tech Stack:** Rust 2024, serde_json, anyhow, wasmtime 51, cargo nextest, TypeDB 3.15 (the CLI tests only).

**Spec:** `docs/superpowers/specs/2026-11-27-guest-output-fails-run-design.md`

## Global Constraints

- **Exact error strings:**
  - `guest wrote no output`;
  - `guest output is not JSON: <quote>`;
  - `guest reported failure without an error message`.
- `MAX_QUOTED_OUTPUT_LEN: usize = 1024`. A cut quote ends with `…`.
- `PlaybookFailed`'s `Display` is `the playbook failed`.
- The guest is not changed, and `quickjs.wasm` is not rebuilt.
- Comments are greenfield. No `#[allow(dead_code)]`. Never fabricate success over bad input.
- Integration tests run with `-j 1`. Never touch the `gecko` database.
- **Before each commit:**
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`;
  - the same clippy run with `--features cyber`.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- **A JSON line followed by trailing garbage,** such as `{"a":1}x`, must be refused as non-JSON, not partly parsed. `serde_json::from_str` already refuses trailing characters; this is pinned in Task 1.
- **Valid JSON padded with whitespace or newlines,** which `println!` adds, must still parse. Pinned in Task 1.
- **A quote cut exactly on a char boundary** (exactly 1024 ASCII bytes) must not gain `…`. Pinned in Task 1.
- **Error paths other than a failed playbook** must still exit non-zero and log to stderr, such as an unknown concept id. The existing CLI tests (`a_stored_timeout_out_of_range_fails_the_run`) cover this; Task 2 must not route them through `PlaybookFailed`.
- **A failure the host reports before the guest runs,** through `fail()`, must also exit non-zero, such as a timeout or oversized args. This goes through the same `!success` path, and is pinned by the timeout test in Task 2.

---

### Task 1: `read_guest_output`

**Files:**
- Modify: `core/gecko-engine/src/sandbox/wasm_executor.rs`:
  - step 6 of `evaluate`, at `:456-480`;
  - the new `MAX_QUOTED_OUTPUT_LEN`, `GuestOutput` and `read_guest_output` beside `stash_outcome`;
  - `mod tests`.
- Test: `core/gecko-engine/tests/wasm_tests.rs`.
- Modify: `docs/refactor/guest-wasm.md`, adding a "Run output" section after "Run arguments".

**Interfaces:**
- Produces, private to the module:
  - `struct GuestOutput { output: serde_json::Value, error: Option<String> }`;
  - `fn read_guest_output(stdout: &str) -> GuestOutput`.

- [ ] **Step 1: Write the failing unit tests** in `mod tests`. Each asserts `(output, error)`:
  - `empty_stdout_is_no_output`: `""` and `"  \n"` → `(Null, Some("guest wrote no output"))`.
  - `non_json_stdout_fails_with_a_quote`:
    - `"hello\n"` → `(Null, Some("guest output is not JSON: hello"))`;
    - `"{\"a\":1}x"` → its error starts with `guest output is not JSON: `.
  - `a_long_non_json_quote_is_cut_on_a_char_boundary`: input is `"a".repeat(1023) + "é" + "b".repeat(10)`, where `é` is 2 bytes spanning byte 1024.
    - The quote after the prefix ends with `…`.
    - Without the `…`, it is at most 1024 bytes and equals `"a".repeat(1023)`.
  - `a_quote_of_exactly_the_limit_is_not_marked_cut`: `"a".repeat(1024)` → the quote is exactly that, with no `…`.
  - `a_failure_without_a_message_gets_one`. Each of these gives `error == Some("guest reported failure without an error message")`, with `output` the parsed value:
    - `{"success": false}`;
    - `{"success": false, "error": 7}`;
    - `{"success": false, "error": "  "}`.
  - `a_failure_keeps_its_message_and_output`: `{"success": false, "error": "boom"}` → `error == Some("boom")`, and `output` is the parsed object.
  - `json_that_is_not_a_failure_succeeds`. Each of these gives `error == None`, with `output` the parsed value:
    - `{"error": "x"}`;
    - `{"result": 42}`;
    - `null`;
    - `"  {\"result\": 1}\n"`.
- [ ] **Step 2: Write the failing guest test** `a_guest_failure_without_a_message_is_reported`, in `wasm_tests.rs`, using the `test_wasm_simple_execution` shape.
  - The code is `({ success: false })`.
  - It asserts `!result.success` and `result.error.as_deref() == Some("guest reported failure without an error message")`.
- [ ] **Step 3: Run the tests.**
  - Run: `cargo nextest run -p gecko-engine --no-default-features --test wasm_tests a_guest_failure_without_a_message`
    - Expected: FAIL, because `error` is `None`.
    - An integration test builds the library without its `cfg(test)` module, so the unit tests' compile error doesn't block this run.
  - Run: `cargo nextest run -p gecko-engine --no-default-features --lib`
    - Expected: a compile failure, because `read_guest_output` isn't defined.
- [ ] **Step 4: Implement** `read_guest_output` as the spec's Design states.
  - `GuestOutput` derives `Debug, PartialEq`, so the tests compare whole values.
  - Trim with `str::trim`.
  - The cut reuses `stash_outcome`'s `is_char_boundary` loop.
  - `evaluate` keeps `String::from_utf8_lossy` and passes the `&str`.
  - Step 6 builds `ExecutionResult` field by field: `output`, `success: error.is_none()` and `error` from it, with `duration_ms` and `engine` as they are written today.
- [ ] **Step 5: Run the tests.**
  - Run: `cargo nextest run -p gecko-engine --no-default-features`
  - Expected: PASS, including every existing guest test.
- [ ] **Step 6: Write the "Run output" section in `guest-wasm.md`.** It covers:
  - the guest prints one JSON line;
  - a top-level `success: false` with a string `error` fails the run;
  - empty output, non-JSON output, and a failure without a message each fail with the exact strings in Global Constraints.
- [ ] **Step 7: Commit** with the message `A guest whose output is not JSON, or that fails without a message, fails the run (N7)`.

### Task 2: a failed `gecko run` exits non-zero

**Files:**
- Modify: `core/gecko-engine/src/pipeline.rs:135-139`. The failure log becomes `warn!(concept_id = %run.concept_id, error = ?execution.error, "Pipeline: execution failed, not recording")`.
- Modify: `gecko-bin/src/main.rs`:
  - the `match run(cli)` in `main` at `:246-252`;
  - `cmd_run` at `:1384`, whose last lines (`:1436-1437`) render the result and return `Ok(())`;
  - the new `struct PlaybookFailed`.
- Test: `gecko-bin/tests/run_args_cli_test.rs`.
- Modify: the doc comment on `run_output` in `gecko-bin/tests/run_args_cli_test.rs:41` and in `gecko-bin/tests/cyber_sample_test.rs:87`.
- Modify: `docs/refactor/OWED-WORK-LEDGER.md`.

**Interfaces:**
- Consumes: `ExecutionResult.success` and `ExecutionResult.error` as Task 1 sets them.
- Produces: `#[derive(Debug)] struct PlaybookFailed;`, implementing `Display` and `std::error::Error`.

- [ ] **Step 1: Write the failing CLI test** `a_failed_playbook_exits_non_zero` in `run_args_cli_test.rs`.
  - Reuse `sync_playbook`, `run` and `text`, with a `THROWING_PLAYBOOK` const in the `LOOPING_PLAYBOOK` shape whose body is `throw new Error("boom");`.
  - It asserts:
    - `!out.status.success()`;
    - stdout contains `Execution failed!`, `Error: JS Execution Error:` and `boom`;
    - stderr contains neither `" ERROR "` (tracing's level field) nor `the playbook failed`.
  - Also add `assert!(!out.status.success(), …)` to `a_playbook_times_out_at_its_timeout_ms`.
- [ ] **Step 2: Run the tests.**
  - Run: `cargo nextest run -p gecko-bin --no-default-features --test run_args_cli_test -j 1`
  - Expected: both tests FAIL on the exit status.
- [ ] **Step 3: Implement.**
  - **`cmd_run`:** after `render_execution_result`, return `Err(PlaybookFailed.into())` when `!pipeline_result.execution.success`.
  - **`main`:**
    ```rust
    Err(e) if e.downcast_ref::<PlaybookFailed>().is_some() => ExitCode::FAILURE,
    ```
    This arm goes before the logging arm.
  - **`PlaybookFailed`:** a one-sentence doc saying that its result is already printed, so `main` exits non-zero without logging it again.
- [ ] **Step 4: Run the tests.**
  - Run: `cargo nextest run -p gecko-bin --no-default-features --test run_args_cli_test -j 1`
  - Expected: PASS, including `a_stored_timeout_out_of_range_fails_the_run`, which goes through the logging path.
- [ ] **Step 5: Fix the stale comments and the ledger.**
  - The `run_output` doc in both test files becomes `The JSON output of a successful gecko run: it exited 0 and printed "Execution successful!".`
  - The ledger moves N7 to Landed and names the tests.
    - Under `## Open work`, the "Engine and CLI" table is removed and replaced with the line `Nothing open.`
    - The "Next up" table is replaced with the line `Nothing queued.`
- [ ] **Step 6: Commit** with the message `A failed gecko run exits non-zero`.

## Verification

- `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1` passes. The baseline is 845; the new tests add to it.
- fmt and both clippy configurations are clean.
- `grep -rn raw_output core gecko-bin` finds nothing.
