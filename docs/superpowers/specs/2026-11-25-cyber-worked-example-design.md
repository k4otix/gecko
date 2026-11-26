# Cyber worked example and `gecko run --args` — design

Closes owed-work ledger item **N6**, including **N6a**. It ships as two PRs from this one
design: N6a (`gecko run --args`) first, then the sample bundle and the README.

## Intent

cyber-gecko's host imports work end to end, but nothing shows a person how to use them.
No sample bundle exercises them, and the README documents only three of the six
(`coverage_gaps`, `blinded`, `recommend_sources`). A playbook also has no input: to record
a verdict on a different detection, an analyst would have to edit the playbook's code.

Success, from the ledger:
- the README walkthrough runs against a synced ATT&CK database;
- a verdict is recorded without editing a playbook.

## Approaches

How a run's arguments reach the guest:

- **Chosen: a WASI environment variable, `GECKO_ARGS`.** `WasiCtxBuilder::new()` starts with
  an empty environment, so this needs no new wasm import, and stdin stays the program's
  channel alone.
- **Rejected: frame the arguments on stdin ahead of the program.** It changes the stdin
  contract that every guest test relies on, for no gain.
- **Rejected: a host call such as `gecko.callExtension("gecko", "args")`.** It would go
  through the S3 scope gate and the reply buffer for data the host already holds at
  instantiation.
- **Rejected: splice the arguments into the program text.** That's an injection path.

Shipping, agreed with the user: one design and two PRs. N6a is engine, CLI and guest work
with a `quickjs.wasm` rebuild, and reviews best without sample markdown mixed in.

## N6a: `gecko run <concept-id> --args <json>`

### The host

- **The flag.** `--args <json>` is optional on `gecko run`, and its value is a literal JSON
  string.
- **Validation.** The host checks the value before it connects to TypeDB or instantiates
  wasm:
  - it must parse as JSON;
  - it must be a JSON **object**; an array, string, number, bool or null is refused;
  - its length must be at most **1 MiB**, checked before parsing.

  Each failure is a CLI error that names the problem. Nothing is defaulted.
- **The run.** The parsed object travels on `PlaybookRun` as
  `args: Option<&'a serde_json::Map<String, Value>>`, through `execute_playbook` to the
  executor. `PlaybookRun` stays `Copy`, and a non-object is unrepresentable at the engine
  boundary.
- **The executor checks the cap too.** `WasmExecutor` checks the 1 MiB limit on the
  serialized string, so a library caller gets the same guarantee as the CLI.
- **The environment variable.** The executor always sets `GECKO_ARGS`:
  - to the re-serialized object (`serde_json::to_string`), so the value is canonical JSON
    with no NUL bytes;
  - to `{}` when `--args` is omitted.
- **Other callers.** Every other `PlaybookRun` constructor passes an empty object.

### The guest (`core/js-sandbox/src/main.rs`)

- **Reading it.** The guest reads `GECKO_ARGS` once at startup and parses it.
- **Refusals.** A missing variable, invalid JSON or a non-object fails the run with an
  error result before any program code runs. The host always sets a valid object, so any
  of these means the host and guest disagree, and the guest must not invent `{}`.
- **The property.** `gecko.args` is the parsed value, **deep-frozen** by a recursive Rust
  walk over the `JsObject` that freezes nested objects and arrays bottom-up. No JS helper
  is evaluated before the program, so nothing leaks into global scope. The property is registered on `gecko` as non-writable and
  non-configurable (`Attribute::READONLY | Attribute::ENUMERABLE`). A playbook can
  destructure it without a null check.
- **The rebuild.** Run `just build-guest` and commit `quickjs.wasm` with the guest change.
  While there, tidy the comment wrap near `main.rs:89` if it reads badly.

### Scope and provenance

- **No S3 scope gates `gecko.args`.** The data comes from the operator, not from a host
  capability.
- **The arguments aren't persisted on the doc-run.** A belief a playbook records from them
  carries the values itself: `disposition`'s belief text and edge hold the logic id and
  the verdict. This is my call; see "Left to the builder".

### N6a tests

- **Host validation**, as unit tests in `gecko-bin`: a non-object is refused, invalid JSON
  is refused, and an oversized value is refused before parsing.
- **Guest**, in the engine's sandbox tests:
  - the program sees the object in `gecko.args`, including a nested value;
  - with no `--args`, it sees `{}`;
  - assigning to `gecko.args`, to `gecko.args.x`, or to a nested `gecko.args.a.b` leaves
    the values unchanged (sloppy mode) or throws (strict);
  - `gecko.args.list.push(..)` on a nested array fails, and the length is unchanged;
  - `delete gecko.args` fails.
- **End to end**: `gecko run <id> --args '{"k":"v"}'` returns output that echoes `v`. Use
  the existing CLI end-to-end test pattern.

### N6a docs

- **`docs/refactor/guest-wasm.md`:** the `GECKO_ARGS` contract (always set, a JSON object,
  read once).
- **The root `README.md` Usage section:** `--args`.

## N6: the cyber sample bundle and the README

### Placement

- **The bundle lives at `extensions/cyber-gecko/sample-bundle/`**, with `bundle.json` name
  `cyber-sample`.
- **`sample-bundle/` stays cyber-free.** `no_drag_test.rs` syncs it with no extensions
  enabled.
- **The sensor file** for the walkthrough is `extensions/cyber-gecko/sample-sensors.yaml`,
  outside the bundle, so the OKF parser never reads it.

### Contents

All technique ids are real ATT&CK Enterprise ids.

- **Two detection concepts,** plain OKF concepts whose concept ids serve as `logic_id`:
  - `detections/encoded_powershell`, for `T1059.001`;
  - `detections/scheduled_task_persistence`, for `T1053.005`.
- **Playbooks.** Each one reads its inputs from `gecko.args` and grants only the scopes
  it calls:
  - `playbooks/claim_detection`: `claim`, with the args `logic_id`, `technique_id` (an
    ATT&CK id), `method` and `confidence`;
  - `playbooks/record_verdict`: `disposition`, with the args `logic_id`, `verdict` and an
    optional `evidence`;
  - `playbooks/coverage_report`: `coverage_gaps`, `blinded` and `recommend_sources`,
    returning a short summary, for example the `by_tactic` counts and the blind and
    recommended counts;
  - `playbooks/detection_precision`: `precision`, with the args `logic_id` and `window`;
  - `playbooks/withdraw_claim`: `mem.retract`, with the args `belief_id` and `reason`,
    granting `mem:retract`.
- **A missing argument.** The playbook passes it through, and the host import's error
  surfaces. No playbook invents a default for a required argument.

### The walkthrough (`extensions/cyber-gecko/README.md`)

A new `## Worked example` section. It first states the prerequisite: a `gecko` built with
`--features cyber` (the release binaries are), and `enabled = ["cyber"]` under
`[extensions]` in `gecko.toml`. Without it, step 4 fails because the `cyber-gecko` import
isn't found. Then it runs in order:

1. `sync-stix` (Enterprise v18 or later) into `--database attack`.
2. `sync-sensors` with `sample-sensors.yaml`.
3. `gecko sync extensions/cyber-gecko/sample-bundle --database attack`.
4. Claim both detections with `gecko run playbooks/claim_detection --args …`.
5. `coverage_report`.
6. Record a verdict with `gecko run playbooks/record_verdict --args …`. This is the
   ledger's success line.
7. `detection_precision`.

It ends with withdrawing a claim: `gecko run playbooks/withdraw_claim --args …` with the
`belief_id` from step 4. Every command carries `--database attack`; without it, `run`
targets the reserved `gecko` database.

**Real output, from a pinned release.** Before N6's PR, the builder runs the walkthrough by
hand against ATT&CK Enterprise **20.1**. It uses the bundle already staged locally for the
S4 check, a throwaway database on the local TypeDB, and never the database named `gecko`.

- **The README's abbreviated outputs are copied from that run.** The README names the
  release, and says that counts differ on other releases.
- **`sample-sensors.yaml` is written from 20.1's real analytics.** It names the platforms
  and log sources those analytics read for `T1059.001` and `T1053.005`, so `blinded` and
  `recommend_sources` give meaningful results. It must also sync against the test fixture;
  the fixture is built to match the file, not the other way round.
- **`coverage_report` must finish inside the default 30 s timeout** on 20.1. S4 timed
  `blinded` at 0.59 s and `recommend_sources` at 0.56 s on that release, so this is
  expected. If it doesn't, the playbook splits in two. N5 is not pulled forward.

### The host-import reference (same README)

- **New sections** for `claim`, `disposition` and `precision`, in the shape of the existing
  three. Each lists its argument names, types and errors, and its reply.
- **Argument notes:**
  - `technique_id` is an ATT&CK id such as `T1059.001`, not a concept id;
  - `logic_id` is a concept id;
  - `method` takes the kebab-case `DerivationMethod` names (`external-tool`,
    `human-assertion`, and so on);
  - `verdict` is one of `true-positive`, `false-positive` or `benign-true`;
  - `window` is an integer number of seconds; `3600.0` is refused (ledger N8).
- **`claim` specifics:**
  - `evidence` is optional;
  - `confidence` is in `[0.0, 1.0]`, and must be below `1.0` for `llm-synthesis`;
  - both `logic_id` and `technique_id` must already exist in the graph.
- **`precision` specifics:**
  - `window` must be greater than 0;
  - `benign-true` counts as neither a true nor a false positive;
  - `precision` is `null` when both counts are 0.
- **Withdrawing a claim.** A short note: `mem.retract(belief_id, reason)` retracts the
  belief and closes its validity.
- **The import list** in the README's intro links each name to its section.

### N6 test: the walkthrough doesn't rot

New `gecko-bin/tests/cyber_sample_test.rs`, under `--features cyber`. Against a throwaway
database:

- **Setup:**
  - sync a small STIX fixture holding the walkthrough's real ids (`T1059.001`, its parent
    `T1059`, `T1053.005`, their tactics, and one detection strategy, analytic and data
    component, so `sync-sensors` accepts it). The fixture is a new file; it isn't a frozen
    one, since nothing earlier recorded it;
  - sync `sample-sensors.yaml`;
  - sync the sample bundle.
- **Then it runs each playbook** through the `gecko` binary (`CARGO_BIN_EXE_gecko`, the
  existing CLI end-to-end pattern, since `cmd_run` is private), with the README's
  own `--args` values, and asserts:
  - the claim returns a belief id;
  - `coverage_report` reflects the claim;
  - the verdict records;
  - `precision` counts it;
  - `withdraw_claim` retracts the claim: its technique returns to `coverage_gaps`, and the
    claim leaves `blinded`.
- **The README stays in sync.** The test reads each `--args` value it passes from the
  README's code blocks, so the two can't drift. The one exception is
  `withdraw_claim`'s `belief_id`, which exists only at run time: the README shows a
  placeholder, and the test substitutes the id the claim returned. If extracting from
  markdown proves fragile,
  the builder may keep the values in one shared constants file the test and the README
  both cite.

### N6 docs

- **The ledger:** N6 moves to Landed.
- **The root `README.md`:** a link to the walkthrough.

## Non-goals

- `--args @file`, or args from stdin.
- Declaring the expected arguments in playbook frontmatter, or schema-validating `gecko.args`.
- Persisting a run's arguments.
- Running the walkthrough against full ATT&CK in CI.

## Constraints

- All TQL is parameterized.
- Comments are greenfield.
- Never fabricate success over bad input.
- No `#[allow(dead_code)]`.
- Integration tests run with `-j 1` and never touch the database named `gecko`.
- Fmt and both clippy runs are clean, plus `cargo clippy -p js-sandbox --target wasm32-wasip1`
  for the guest.
- The `quickjs.wasm` rebuild is committed with its guest change.

## Left to the builder

- The sample's detection prose, and the coverage summary's exact fields.
- Arguments are not persisted on the doc-run (confirmed with the user). Invariant 2 needs
  only `run_id`, `actor`, `source` and `occurred_at`, and not storing raw arguments avoids
  leaking any secrets they carry.
