# A failed run is reported as a failure — design

Closes owed-work ledger item **N7**, and two gaps beside it that the user chose to fold in.
It's a small change, so this is the short design.

## Intent

The guest prints exactly one JSON line to stdout: the playbook's result, or
`{"success": false, "error": …}`. `console.log` goes to stderr, so a playbook's logging
never mixes into that line.

Today a failed run can still be reported as a success, in three ways:

- **N7.** `WasmExecutor::evaluate` (`core/gecko-engine/src/sandbox/wasm_executor.rs:457`)
  falls back to `success: true` with `{"raw_output": …}` when stdout isn't JSON. Normal
  JavaScript can't cause this; it happens when the guest breaks:
  - it crashes before printing, so stdout is empty;
  - its output passes the 10 MiB cap: the full pipe refuses the write and the guest aborts,
    which `evaluate` reports as `guest output exceeded the 10 MiB limit`, not as a wasm
    backtrace;
  - the prebuilt `quickjs.wasm` doesn't match the host's output format.
- **Gap 1.** `{"success": false}` with no string `error` fails silently, and `gecko run`
  prints just "Execution failed!". The same code also fills `error` from a successful
  result that happens to have an `error` field, such as `{ "error": "none" }`.
- **Gap 2.** `gecko run` exits 0 after printing "Execution failed!", so a script or CI job
  can't tell a failed run from a good one.

Done when:
- a guest that prints non-JSON fails the run, with the text in the error;
- a guest failure always carries a message;
- a failed `gecko run` exits non-zero, with the same stdout as today.

## Approaches

- **Chosen: one pure stdout reader, plus a typed exit signal.**
  - `evaluate` hands stdout to a pure function that decides the output and the error.
  - `cmd_run` signals a failed playbook with a marker error, which `main` maps to
    `ExitCode::FAILURE` without logging it.
- **Rejected: `cmd_run` returns a plain `anyhow` error.** `main` logs every error to
  stderr at `ERROR`, so a failed playbook would also show up as a host error there.
- **Rejected: `run()` returns an `ExitCode`.** That works, but it touches every command arm
  to carry one command's exit status. The marker error keeps the change inside `Run`.

## Design

- **`read_guest_output(stdout: &str) -> GuestOutput`** in `wasm_executor.rs`, where
  `struct GuestOutput { output: serde_json::Value, error: Option<String> }`. The run
  succeeds exactly when `error` is `None`.
  - **Empty or whitespace-only stdout:** `output` is `Null`, and `error` is
    `guest wrote no output`.
  - **Non-JSON:** `output` is `Null`, and `error` is `guest output is not JSON: <quote>`.
    - The quote is the trimmed text, cut to at most `MAX_QUOTED_OUTPUT_LEN` (1024 bytes)
      on a UTF-8 char boundary, using the same loop as `stash_outcome`.
    - When the text is cut, the quote ends with `…`.
  - **JSON whose top-level `success` is `false`:** a failure.
    - `error` is the `error` field when it is a non-blank string.
    - Otherwise (missing, not a string, empty or whitespace-only) it is
      `guest reported failure without an error message`.
    - `output` keeps the parsed JSON, as today.
  - **Any other JSON:** a success with `error` `None`, even when the value has an `error`
    field. A `success` that isn't a bool counts as success, as today; the playbook's result
    is its own business.
- **`evaluate`** uses it in step 6, so `raw_output` is gone. No existing test reads
  `raw_output`.
- **`gecko run`'s exit code.**
  - `struct PlaybookFailed;` in `gecko-bin/src/main.rs`, with `Display` "the playbook
    failed" and `std::error::Error`.
  - `cmd_run` renders the result exactly as today, then returns `Err(PlaybookFailed.into())`
    when `!execution.success`.
  - In `main`, the `Err(e)` arm returns `ExitCode::FAILURE` without calling `error!` when
    `e.downcast_ref::<PlaybookFailed>()` is `Some`. Every other error is logged as today.
  - stdout is unchanged: "Execution failed!" followed by "Error: …".
- **The pipeline's failure log** (`core/gecko-engine/src/pipeline.rs:135-139`) moves from
  `error!` to `warn!`. A failed playbook is the playbook's outcome, not a host error, so
  stderr carries no `ERROR` line for it. The log keeps the message, and its
  `unwrap_or("unknown")` default goes: every failure now carries a message, so it logs
  `error = ?execution.error`.
- **Quotes can span lines.** A non-JSON quote keeps its newlines. The error is shown as
  written and is not collapsed onto one line.
- **Stale comments.** The doc on `run_output` in `gecko-bin/tests/run_args_cli_test.rs:41`
  and `gecko-bin/tests/cyber_sample_test.rs:87` says a failed playbook still exits 0. It is
  rewritten to say what the helper checks now.
- **Docs.** `docs/refactor/guest-wasm.md` doesn't describe stdout yet, so it gets a short
  "Run output" section: the one JSON line, the `success: false` convention, and the three
  host-side failures above. The ledger moves N7 to Landed.

## Tests

- **`read_guest_output`:** Tier-1 unit tests in `wasm_executor.rs`'s `mod tests`:
  - `""` and `"  \n"` → `guest wrote no output`;
  - `"hello"` → `guest output is not JSON: hello`;
  - text over 1 KiB with a multi-byte character across the 1024-byte boundary → the quote
    is valid UTF-8, at most 1024 bytes plus `…`;
  - `{"success": false}`, `{"success": false, "error": 7}` and
    `{"success": false, "error": "  "}` → the no-message error;
  - `{"success": false, "error": "boom"}` → `boom`, and `output` is the parsed object;
  - `{"error": "x"}` with no `success` → success, `error` `None`;
  - `{"result": 42}` and `null` → success.
- **The real guest:** a test in `core/gecko-engine/tests/wasm_tests.rs` runs
  `({ success: false })` through `quickjs.wasm` and gets `success == false` with the
  no-message error. No guest rebuild is needed.
- **The CLI:**
  - `a_failed_playbook_exits_non_zero` in `gecko-bin/tests/run_args_cli_test.rs` syncs a
    playbook that throws `new Error("boom")` and runs it. The run exits non-zero, stdout
    holds "Execution failed!" and "Error: JS Execution Error: …boom", and stderr holds
    neither an `ERROR`-level line nor "the playbook failed".
  - `a_playbook_times_out_at_its_timeout_ms` also asserts `!out.status.success()`.

## Non-goals

- Parsing more than one stdout line, or sending stderr back to the caller.
- Changing the guest. `quickjs.wasm` isn't rebuilt.
- Giving `success` a stricter type check than `success == false`.
