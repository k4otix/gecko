# `precision` reads `window` safely — design

Closes owed-work ledger items **N8** and **N9** together. Both concern how cyber-gecko's
`precision` reads its `window` argument, in the same three lines of
`extensions/cyber-gecko/src/detect/mod.rs`. This is the short design.

## Intent

`precision` counts the verdicts recorded on a detection over a trailing window of `window`
seconds. Today it reads the argument with `Value::as_u64`. That has two consequences:

- **N8.** A whole number the guest sends as a float (`5.0`) is refused. Boa keeps the result of
  `Math.floor` as a float, so a playbook that computes its window hits this.
- **N9.** A large integer reaches `ctx.occurred_at - chrono::Duration::seconds(window as i64)`
  unchecked. `1e14` seconds panics the host when the subtraction overflows, and a value over
  `i64::MAX` wraps negative.

Done when:
- `window: 5.0` is read as 5 and the reply says `"window_seconds": 5`;
- `window: 100000000000000` is an error naming the limit, not a panic.

## Approaches

- **Chosen, with the user: one shared whole-number rule, plus a named ceiling.** The rule
  already used by the mem bridge (`opt_positive_int` in
  `core/gecko-engine/src/sandbox/mem_host.rs`) moves to `gecko-extension-api`, and
  `precision` uses it. That keeps one rule for what counts as an integer from the guest. A
  fixed `MAX_WINDOW_SECONDS` of 100 years bounds the window.
- **Rejected: copy the rule into cyber-gecko.** Two copies of the same rule would drift.
- **Rejected: bound only by what chrono can represent.** That limit depends on
  `occurred_at`, so it can't be stated as a single number, and an error couldn't tell a
  playbook author what the limit is.

## Design

- **The shared rule.** In `gecko-extension-api`, a new module `guest_value` holds:
  - `pub const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;`
  - `pub fn shown(v: &serde_json::Value) -> String`, which moves from `mem_host` with its
    `MAX_SHOWN_VALUE_LEN` cap (64 characters): the value's JSON text when short, otherwise
    its type and size;
  - `pub fn whole_number(v: &serde_json::Value) -> Option<u64>`, which accepts:
    - `v.as_u64()` for a JSON integer;
    - otherwise `v.as_f64()` when `f.is_finite() && f.fract() == 0.0 && (0.0..=MAX_SAFE_INTEGER).contains(&f)`.

  Because of the range check, a huge float like `1e20` never saturates through `as u64`.
- **Re-exports.** `gecko-extension-api` re-exports all three from its root. `mem_host`
  uses them and drops its private `MAX_SAFE_INTEGER`, `MAX_SHOWN_VALUE_LEN` and `shown`;
  `opt_positive_int` calls `whole_number`. Its behaviour doesn't change, and its existing tests stay as they are.
- **The ceiling.** `MAX_WINDOW_SECONDS: u64 = 3_155_760_000` in `detect/mod.rs`. Its comment
  says it is `100 * 365.25 * 86_400`, that is, 100 Julian years.
- **Reading `window`.** `precision` accepts the window only when `whole_number` returns a
  value in `1..=MAX_WINDOW_SECONDS`.
  - Everything else is refused with one error naming the field, the accepted range, and the
    value as written. That covers a missing or `null` window, `0`, `0.0`, `-5`, `5.5`, `"60"`,
    `1e20`, `100000000000000` and `MAX_WINDOW_SECONDS + 1`.
  - The value is quoted through `shown`, so a large payload is never echoed back.
- **Checked date math.** The cutoff is computed as
  `chrono::Duration::try_seconds(i64)` followed by `ctx.occurred_at.checked_sub_signed(..)`.
  Either one returning `None` is an error, never a panic. `Duration::seconds` itself panics
  on overflow, so it isn't used.
- **The reply.** `window_seconds` is the integer that was read, so `5.0` comes back as `5`.
- **The docs.** The `window` row in `extensions/cyber-gecko/README.md` (`## precision`) says:
  - an integer number of seconds, from 1 to 3155760000 (100 years);
  - a whole number written as a float, such as `3600.0`, counts.

  It no longer says that `3600.0` is refused. The ledger moves N8 and N9 to Landed.

## Tests

- **`whole_number`:** a unit test in `gecko-extension-api`. It accepts `5`, `5.0` and
  `MAX_SAFE_INTEGER`, and refuses `5.5`, `-1`, `1e20`, `"5"`, `null` and `MAX_SAFE_INTEGER + 2.0`.
  A unit test of `shown` checks that a short value is quoted and a long one is named by type
  and size. The mem bridge's refusal-size tests stay where they are and pass unchanged.
- **The window parser:** unit tests in `detect/mod.rs`.
  - Accepted: `5.0` → 5, `MAX_WINDOW_SECONDS`.
  - Refused, each with an error naming the limit: `MAX_WINDOW_SECONDS + 1`, `0`, `0.0`, `-5`,
    `5.5`, missing, `null`, `"60"`, `1e20`, `100000000000000`.
- **The cutoff:** a unit test that the ceiling subtracted from a real `occurred_at` gives the
  expected datetime. A second test feeds a window chrono can't subtract and checks that the
  result is an error, not a panic.
- **End to end:** an integration test in `gecko-bin/tests/cyber_detect_test.rs`.
  - `window: 5.0` replies `"window_seconds": 5`.
  - `window: 100000000000000` returns an error through the callback, and the process
    doesn't panic.
- **The mem bridge:** its existing `max_chunks` tests (`5.0` accepted, `1e20` refused) pass
  unchanged.

## Non-goals

- Changing any other argument's integer rule. `precision` is cyber-gecko's only integer
  argument.
- Making the ceiling configurable.
