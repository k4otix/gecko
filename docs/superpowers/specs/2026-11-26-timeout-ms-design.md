# `timeout-ms` reaches the run — design

Closes owed-work ledger item **N5**. It's a small change, so this is the short design.

## Intent

A playbook's frontmatter `timeout-ms` is meant to bound its run. Today:
- the parser reads it with `.as_u64()`, so an invalid value silently becomes "no timeout";
- sync never stores it;
- `gecko run` always passes `None`, so every run gets the 30 s default.

`consumes` and `produces` are parsed into fields nothing reads.

Done when a playbook with `timeout-ms: 500` times out at 500 ms under `gecko run`.

## Approaches

- **Chosen, with the user: store it on the concept.** A `timeout-ms` attribute in the core
  schema, written by sync, read by `gecko run`.
- **Rejected: read it from `metadata-json`.** That would leave a typed execution limit inside
  an untyped blob, with no schema check.
- **Rejected for `consumes` and `produces`: store them.** Nothing reads them (YAGNI). They are
  no longer parsed specially, so they land in `metadata-json` like any unknown frontmatter key.
  For an already-synced file, that happens once its content next changes.

## Design

- **The ceiling.** `pub const MAX_TIMEOUT_MS: u64 = 600_000;` (10 minutes) lives in
  `core/gecko-engine/src/sandbox/wasm_executor.rs`. The ceiling stays in Rust, so tuning it
  needs no schema migration. Every layer enforces `1..=MAX_TIMEOUT_MS`:
  - **The parser** refuses a `timeout-ms` that isn't an integer in range, with a `ParseError`
    naming the file and the value. That covers `0`, negatives, floats, strings, YAML `null`,
    a blank value, and anything over the ceiling. An absent key is `None`.
  - **`sync_bundle`** refuses an `OkfConcept.timeout_ms` out of range, for a programmatic
    caller, before writing anything. It never casts unchecked to `i64`.
  - **`WasmExecutor::evaluate`** refuses `Some(0)` or `Some(> MAX_TIMEOUT_MS)` before the
    guest starts, as it does for `MAX_RUN_ARGS_LEN`. Zero no longer clamps silently to one
    epoch.
- **The timeout error names itself.** An epoch interrupt (`wasmtime::Trap::Interrupt`) is
  reported as `execution timed out after <ms> ms`, not as a generic trap, so a timed-out run
  says what happened. This is my call: the acceptance test needs it to tell a timeout from any
  other trap.
- **The schema.** In `core_schema.tql`, a new attribute:
  `attribute timeout-ms @doc("…"), value integer @range(1..);`. `concept` owns it,
  `@card(0..1)`. It's an additive `define`, so no migration is needed.
- **Sync.** `attach_concept_content` writes it through a new `attach_integer` helper, next to
  `attach_attr` and `attach_datetime`. `clear_concept_content` already deletes every non-key
  attribute of an updated concept, so editing or removing `timeout-ms` cleans up by itself.
- **The run.** `fetch_playbook` projects `[ $c.timeout-ms ]` only when the schema defines the
  attribute, the same guard it uses for `retired-at`, so an older database still runs. A
  stored value is validated strictly: anything outside `1..=MAX_TIMEOUT_MS` is an error,
  never a fallback to `None`. `cmd_run` passes the value to `PlaybookRun.timeout_ms`.
- **Cleanup:**
  - drop `consumes` and `produces` from `OkfConcept`, the parser, and the struct literals
    (`extensions/cyber-gecko/src/sensors.rs`, `extensions/cyber-gecko/src/stix/mod.rs`,
    `core/gecko-engine/src/syncer/bundle.rs`);
  - fix the stale comments at `gecko-bin/src/main.rs:1370` and
    `core/gecko-extension-api/src/okf.rs:4`.
- **The caveat.** A playbook synced before this change keeps its file hash, so a re-sync skips
  it and its `timeout-ms` stays unstored until the file changes. This goes in
  `docs/refactor/KNOWN-LIMITATIONS.md`.

## Tests

- **Parser:** each refusal (`0`, `-1`, `5.0`, `"500ms"`, `null`, a blank value, `600001`);
  `500` and `600000` are accepted; an absent key is `None`; `consumes` lands in
  `extra_metadata`.
- **Executor:** `Some(0)` and `Some(600_001)` are refused before the guest runs; `Some(600_000)`
  is accepted.
- **Sync:**
  - the value round-trips;
  - editing it replaces the stored value, and removing it leaves none;
  - an out-of-range programmatic value is refused before any write.
- **CLI end to end:** a `timeout-ms: 500` playbook that loops forever fails with
  `execution timed out after 500 ms`, well under the 30 s default.

## Non-goals

- A `gecko run --timeout-ms` override.
- Making the ceiling configurable.
- Re-syncing unchanged files to pick up the new attribute.
