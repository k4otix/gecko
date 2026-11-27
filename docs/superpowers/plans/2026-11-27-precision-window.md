# `precision` reads `window` safely — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `precision.window` accepts a whole-number float (N8) and refuses, rather than panics on, a window past a named 100-year ceiling (N9).

**Architecture:** The mem bridge's whole-number rule and its `shown` quoting move into `gecko-extension-api::guest_value`, shared by mem and cyber. `precision` reads `window` through a pure parser bounded by `MAX_WINDOW_SECONDS`, and computes its cutoff with checked chrono math.

**Tech Stack:** Rust 2024, serde_json, chrono, TypeDB 3.15 (the integration test only), cargo nextest.

**Spec:** `docs/superpowers/specs/2026-11-27-precision-window-design.md`

## Global Constraints

- Comments are greenfield. Never use `#[allow(dead_code)]`. All TQL is parameterized.
- Never fabricate success over bad input: every refused `window` is an `Err`, never a default.
- `MAX_WINDOW_SECONDS: u64 = 3_155_760_000`. Its comment says this is `100 * 365.25 * 86_400`, that is, 100 Julian years.
- `MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0`.
- `MAX_SHOWN_VALUE_LEN` stays 64.
- `Duration::seconds` is never used on a guest-derived value. Use `Duration::try_seconds`, then `checked_sub_signed`.
- Integration tests run with `-j 1`, against TypeDB on `localhost:1729`. Never touch the `gecko` database.
- Before each commit:
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`;
  - the same clippy run with `--features cyber`.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- **`-0.0`** must be refused as 0. `whole_number(-0.0)` returns `Some(0)`, and the range check rejects it. Pinned in Task 2.
- **A float just over the ceiling,** such as `3155760001.0`, must take the same path as the integer. Pinned in Task 2.
- **A huge refused value in the error.** A 10 KB string `window` must not be echoed back in full. `shown` caps it; pinned in Task 2.
- **`occurred_at` near chrono's minimum.** The ceiling must still give an error, not a panic. Pinned in Task 2 with `DateTime::<Utc>::MIN_UTC`.
- **A missing `logic_id` combined with a bad `window`.** The existing order stands: `logic_id` is checked first. No new test is needed; the existing `req_str` behaviour is unchanged.

---

### Task 1: `guest_value` in `gecko-extension-api`

**Files:**
- Create: `core/gecko-extension-api/src/guest_value.rs`
- Modify: `core/gecko-extension-api/src/lib.rs` (add `pub mod guest_value;` and `pub use guest_value::{MAX_SAFE_INTEGER, shown, whole_number};`)
- Modify: `core/gecko-engine/src/sandbox/mem_host.rs`:
  - delete `MAX_SAFE_INTEGER`, `MAX_SHOWN_VALUE_LEN` and `fn shown` (`:151-171`);
  - import them from `gecko_extension_api`;
  - `opt_positive_int` calls `whole_number`.

**Interfaces:**
- Produces:
  - `pub const MAX_SAFE_INTEGER: f64`;
  - `pub fn whole_number(v: &serde_json::Value) -> Option<u64>`;
  - `pub fn shown(v: &serde_json::Value) -> String`.

- [ ] **Step 1: Write the failing unit tests** in `guest_value.rs`'s `#[cfg(test)] mod tests`:
  - `whole_number_accepts_integers_and_whole_floats`:
    - `json!(5)` → `Some(5)`;
    - `json!(5.0)` → `Some(5)`;
    - `json!(-0.0)` → `Some(0)`;
    - `json!(9_007_199_254_740_991.0_f64)` → `Some(9_007_199_254_740_991)`;
    - `json!(u64::MAX)` → `Some(u64::MAX)`.
  - `whole_number_refuses_everything_else`. Each of these → `None`:
    - `json!(5.5)`, `json!(-1)`, `json!(-5.0)`, `json!(1e20)`;
    - `json!(9_007_199_254_740_993.0_f64)`;
    - `json!("5")`, `json!(null)`, `json!(true)`, `json!([5])`.
  - `shown_quotes_a_short_value`: `shown(&json!("0.5")) == "\"0.5\""`.
  - `shown_names_a_long_value_by_type_and_size`: `shown(&json!("x".repeat(100)))` is `"a string of 102 bytes"`.
- [ ] **Step 2: Run them.**
  - Run: `cargo nextest run -p gecko-extension-api guest_value`
  - Expected: a compile failure, because the functions aren't defined.
- [ ] **Step 3: Implement the module.**
  - `whole_number` is mem_host's existing match, `(v.as_u64(), v.as_f64())`, returning `u64`.
  - `shown` is moved verbatim, with `MAX_SHOWN_VALUE_LEN` kept private to the module.
  - Then rewire `mem_host`:
    - `opt_positive_int` becomes `whole_number(v).and_then(|n| usize::try_from(n).ok())` and keeps its own error text;
    - every `shown(` call uses the import.
- [ ] **Step 4: Run the tests.**
  - Run: `cargo nextest run -p gecko-extension-api guest_value && cargo nextest run -p gecko-engine --no-default-features mem_host`
  - Expected: PASS. That includes `recall_max_chunks_must_be_a_positive_integer` and `a_short_value_is_still_shown_in_a_refusal`, unchanged.
- [ ] **Step 5: Commit** with the message `guest_value: one whole-number rule and refusal quoting for every bridge`.

### Task 2: `precision` reads `window` through the shared rule, with a ceiling

**Files:**
- Modify: `extensions/cyber-gecko/src/detect/mod.rs`:
  - `precision` at `:1117-1130`;
  - the new `MAX_WINDOW_SECONDS`, `window_seconds` and `window_cutoff` beside it;
  - the tests in `mod tests`.
- Test: `gecko-bin/tests/cyber_detect_test.rs`.

**Interfaces:**
- Consumes: `gecko_extension_api::{whole_number, shown}` (Task 1).
- Produces, private to `detect`:
  - `const MAX_WINDOW_SECONDS: u64`;
  - `fn window_seconds(args: &Value) -> Result<u64, String>`;
  - `fn window_cutoff(occurred_at: DateTime<Utc>, window: u64) -> Result<DateTime<Utc>, String>`.

- [ ] **Step 1: Write the failing unit tests** in `detect/mod.rs` `mod tests`:
  - `a_whole_float_window_is_read_as_an_integer`:
    - `window_seconds(&json!({"window": 5.0})) == Ok(5)`;
    - `window_seconds(&json!({"window": MAX_WINDOW_SECONDS})) == Ok(MAX_WINDOW_SECONDS)`.
  - `a_window_outside_the_range_is_refused_naming_the_limit`. Each of these is `Err`, and the error contains `"3155760000"` and `"window"`:
    - `{"window": 3155760001}` and `{"window": 3155760001.0}`;
    - `{"window": 0}`, `{"window": 0.0}`, `{"window": -0.0}`, `{"window": -5}`, `{"window": 5.5}`;
    - `{}` and `{"window": null}`;
    - `{"window": "60"}`;
    - `{"window": 1e20}` and `{"window": 100000000000000u64}`.
  - `a_refused_window_is_shown_without_echoing_a_large_value`: `{"window": "x".repeat(10_000)}` errors in under 300 bytes.
  - `the_ceiling_subtracts_from_a_real_time`: `window_cutoff(t, MAX_WINDOW_SECONDS) == Ok(t - Duration::try_seconds(3_155_760_000).unwrap())`, for `t = 2026-11-27T00:00:00Z`.
  - `a_cutoff_chrono_cannot_represent_is_an_error`: `window_cutoff(DateTime::<Utc>::MIN_UTC, MAX_WINDOW_SECONDS).is_err()`.
- [ ] **Step 2: Run them.**
  - Run: `cargo nextest run -p cyber-gecko window`
  - Expected: a compile failure, because the functions aren't defined.
- [ ] **Step 3: Implement.**
  - **`window_seconds`:**
    - `args.get("window")`, `whole_number`, then a filter on `(1..=MAX_WINDOW_SECONDS).contains`;
    - the error is `precision 'window' must be an integer number of seconds from 1 to 3155760000 (100 years), got <shown>`;
    - missing reads as `null` for display.
  - **`window_cutoff`:** `i64::try_from(window)`, then `Duration::try_seconds`, then `occurred_at.checked_sub_signed`. Any `None` gives an error naming the window.
  - **`precision`:** uses both helpers, and `window_seconds` keeps returning the parsed `u64`.
  - **The constant:** `MAX_WINDOW_SECONDS` carries the Julian-years comment.
- [ ] **Step 4: Run the unit tests.**
  - Run: `cargo nextest run -p cyber-gecko`
  - Expected: PASS.
- [ ] **Step 5: Write the failing integration test** `precision_reads_a_whole_float_window_and_refuses_one_past_the_ceiling` in `cyber_detect_test.rs`. Reuse `seed_detection_graph`, `detect_handles`, `run_ctx` and `call`.
  - `call(&cb, "precision", json!({"logic_id": LOGIC_ID, "window": 5.0}))` is `Ok`, and `["window_seconds"] == 5`.
  - `call(... "window": 100000000000000u64 ...)` is `Err` containing `"3155760000"`.

  It passes against Step 3's code. To see it fail first, stash the Step 3 change (`git stash`), run the test, and watch the `5.0` assert fail. Then `git stash pop`.
- [ ] **Step 6: Run the integration test.**
  - Run: `cargo nextest run -p gecko-bin --no-default-features --features cyber --test cyber_detect_test -j 1`
  - Expected: PASS, including the three existing `precision_*` tests.
- [ ] **Step 7: Update the docs.**
  - **`extensions/cyber-gecko/README.md`, the `## precision` table row** for `window`:
    - "The window in seconds: an integer from 1 to 3155760000 (100 years).";
    - "A whole number written as a float, such as `3600.0`, counts."
  - **`docs/refactor/OWED-WORK-LEDGER.md`:**
    - move N8 and N9 to Landed, naming their tests;
    - "Next up" becomes 1 N7.
- [ ] **Step 8: Commit** with the message `precision reads window through the shared integer rule, bounded at 100 years (N8, N9)`.

## Verification

- `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1` passes. The baseline is 835 passed; the new tests add to it.
- fmt and both clippy configurations are clean.
- `rg 'Duration::seconds' extensions/cyber-gecko/src` finds no guest-derived use.
