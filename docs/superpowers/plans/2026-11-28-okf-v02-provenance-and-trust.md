# OKF v0.2 provenance, trust and lifecycle (O2 + O4) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Read OKF v0.2's `generated`, `verified`, `status`, `stale_after` and the
bundle-root `okf_version` into typed graph facts. A malformed value produces a warning,
stays in `metadata-json` as written, and never refuses the bundle or invents a value.

**Architecture:** A new parser module, `okf/provenance.rs`, takes the v0.2 keys out of a
concept's frontmatter map. It returns typed values and warnings, and leaves any key it
could not wholly understand in the map, so the key falls through to `metadata-json`.

`parse_bundle` collects the warnings and reads the root `index.md`'s `okf_version`.

`sync_bundle` writes the new attributes and the unary `verification` relation. It guards
the deletes it adds behind one schema read, so a library caller on an older schema still
syncs.

**Prerequisite:** PR #101, the Q11 link fix, lands first. This branch
(`feat/okf-v02-trust`) is rebased onto `main` after it merges. Nothing here depends on
its code.

**Tech Stack:** Rust 2024, serde_yaml 0.9, chrono, TypeDB 3.15 / TypeQL, cargo nextest.

**Spec:** `docs/superpowers/specs/2026-11-28-okf-v02-provenance-and-trust-design.md`

## Global Constraints

- **All TQL is parameterized** (`given` rows). No value is ever interpolated; only
  validated labels are.
- **Comments are greenfield.** No `#[allow(dead_code)]`.
- **Every new schema attribute, relation and role carries a one-sentence greenfield
  `@doc`.**
- **Never refuse a bundle for a malformed v0.2 provenance value; never type a value
  invented for one.** `engine`, `timeout-ms` and `scopes` keep refusing.
- **Timestamps are RFC 3339 with an explicit offset.** Date-only and offset-less values
  are malformed.
- **Trust tiers are `unverified`, `machine-confirmed` and `human-reviewed`.**
  - `human:` matching is case-sensitive and needs a non-empty id after the colon.
  - `trust-tier` is written only when `verified` is present.
- **v0.1 bundles, the STIX import and sensor files sync exactly as before.**
- **Integration tests run with `-j 1`.** Never touch the database named `gecko`.
- **Before each commit, each of these exits 0, checked on its own exit code:**
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`;
  - the same with `--features cyber`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Ratified docs and frozen `*/tests/fixtures/` files are never edited.**

## Review Focus

1. **Unquoted YAML timestamps** (`stale_after: 2026-06-01T00:00:00Z`) read the same as
   quoted ones. Pinned by `unquoted_and_quoted_timestamps_read_alike` in Task 1.
2. **A non-UTC offset** (`+09:00`) is stored as the same instant in UTC. Pinned by
   `an_offset_timestamp_is_the_same_instant` in Task 1.
3. **Whitespace-only strings** are blank and therefore malformed: `status: "  "` and
   `by: "  "`. Pinned by `blank_strings_are_malformed` in Task 1.
4. **A sync that changes only the root `index.md`** still updates `okf-version`, because
   the bundle entity is upserted on every sync. Pinned by
   `okf_version_follows_the_root_index` in Task 3.
5. **Exact duplicate `verified` events** are written as one relation. Pinned by
   `a_duplicate_verification_event_is_written_once` in Task 1 (parser) and by
   `verification_events_round_trip` in Task 3.

---

### Task 1: Concept provenance types and parsing

**Files:**
- Modify: `core/gecko-extension-api/src/okf.rs` (the new types, and new fields on
  `OkfConcept` and `OkfBundle`)
- Create: `core/gecko-engine/src/okf/provenance.rs`
- Modify: `core/gecko-engine/src/okf/mod.rs` (add `mod provenance;`)
- Modify: `core/gecko-engine/src/okf/parser.rs` (`parse_concept`, `extract_timestamp`
  removed, `parse_bundle` collects warnings)
- Modify: the `OkfConcept` / `OkfBundle` literals in
  `core/gecko-engine/src/syncer/bundle.rs`, `extensions/cyber-gecko/src/sensors.rs` and
  `extensions/cyber-gecko/src/stix/mod.rs` (`provenance: Default::default()`,
  `okf_version: None`, `warnings: Vec::new()`)

**Interfaces:**
- Produces, in `gecko_extension_api::okf`:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  pub enum TrustTier { Unverified, MachineConfirmed, HumanReviewed }
  impl TrustTier { pub fn as_str(self) -> &'static str } // "unverified" | "machine-confirmed" | "human-reviewed"

  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  pub struct Verification { pub by: String, pub at: DateTime<Utc> }

  #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
  pub struct OkfProvenance {
      pub generated_by: Option<String>,
      pub trust_tier: Option<TrustTier>,
      pub status: Option<String>,
      pub stale_after: Option<DateTime<Utc>>,
      pub verifications: Vec<Verification>, // deduplicated, in file order
  }
  // OkfConcept gains: #[serde(default)] pub provenance: OkfProvenance
  // OkfBundle gains:  pub okf_version: Option<String>, pub warnings: Vec<String>
  ```
- Produces, in `gecko_engine::okf::provenance` (crate-private):
  ```rust
  pub(crate) struct ConceptProvenance { pub timestamp: Option<DateTime<Utc>>, pub provenance: OkfProvenance }
  pub(crate) fn take_provenance(
      map: &mut HashMap<String, serde_yaml::Value>,
      path: &str,
      warnings: &mut Vec<String>,
  ) -> ConceptProvenance
  ```
- **Produces:** `parse_concept(file_path, bundle_root) -> Result<(OkfConcept, Vec<String>), ParseError>`.
  The second element holds that file's warnings. `parse_bundle` appends them to
  `OkfBundle.warnings` in file order.

**The rule, per key, which `take_provenance` implements:**
- **Removing the key.** `take_provenance` removes a key from `map` only when the key is
  wholly valid and holds only known subkeys. Otherwise it puts the key back, unchanged,
  so it falls through to `metadata-json`.
- **The warning.** It warns only for a malformed part, never for an unknown subkey. The
  text is exactly:
  `Concept file '{path}' has an invalid '{key}' ({value}); it is kept in metadata-json and not typed`.
  - `{value}` is `yaml_as_written` of the offending value; move that helper to
    `pub(crate)`.
  - `{key}` is `generated.by`, `generated.at`, `generated`, `timestamp`, `verified`,
    `verified[{i}]`, `status` or `stale_after`.
- **Timestamps** are read with `DateTime::parse_from_rfc3339` from a `Value::String`, then
  converted to UTC. Anything else is malformed.
- **`generated`.**
  - It must be a mapping. Its known subkeys are `by` (a non-blank string) and `at` (a
    timestamp).
  - A missing `by` is malformed, with the key `generated.by`; the warning's `{value}` is
    the whole `generated` mapping, as written.
  - The valid parts are still typed. `at` becomes `ConceptProvenance.timestamp`.
- **`timestamp`** is read only when there is no valid `generated.at`. When it isn't read,
  it stays in `map` untouched, with no warning.
- **`verified`.**
  - A mapping is treated as a one-element list.
  - Any value that is neither a list nor a mapping is malformed as a whole: the tier is
    `None` and there are no events.
  - An entry is a mapping with a non-blank string `by` and a timestamp `at`; unknown
    subkeys are allowed.
  - A bad entry is warned about as `verified[{i}]` and skipped.
  - The tier:
    - `[]` gives `Unverified`;
    - any valid `by` that starts with `human:` and has a non-blank id after it gives
      `HumanReviewed`;
    - other valid entries give `MachineConfirmed`;
    - a non-empty list with no valid entry gives `None`.
- **`status`.** A non-blank string is stored as written. Anything else is malformed.
- **`stale_after`.** A timestamp; anything else is malformed.

- [ ] **Step 1: Write the failing parser tests** in `parser.rs`'s test module.
  - Each uses `parse_with`, which now returns `(OkfConcept, Vec<String>)`. A malformed
    case asserts the warning contains the file name, the key and the value as written,
    and that `extra_metadata` still holds the key.
  - `generated_fills_timestamp_and_generated_by`:
    `generated: { by: openwiki/0.9.0, at: "2026-11-28T02:42:19.865Z" }` gives
    `timestamp == Some(2026-11-28T02:42:19.865Z)` and
    `provenance.generated_by == Some("openwiki/0.9.0")`. It produces no warnings, and
    `generated` is not in `extra_metadata`.
  - `an_unconventional_actor_is_accepted`: `by: claude-code` is stored, with no warning.
  - `generated_wins_over_a_legacy_timestamp`: given `generated.at` A and `timestamp` B,
    the timestamp is A, `extra_metadata["timestamp"]` holds B, and there are no warnings.
  - `a_legacy_timestamp_is_read_without_generated`.
  - `a_legacy_timestamp_is_read_when_generated_at_is_bad`:
    `generated: { by: x/1, at: soon }` beside a valid `timestamp` gives the legacy
    timestamp, which is removed from the map, and one warning for `generated.at`.
  - `an_unknown_verified_subkey_is_kept_without_warning`: an event with `note: n` is
    typed, `verified` is kept in `extra_metadata`, and there are no warnings.
  - `generated_without_by_keeps_its_at`: `generated: { at: … }` gives the timestamp, one
    warning naming `generated.by`, and `generated` kept raw.
  - `an_unknown_generated_subkey_is_kept_without_warning`:
    `generated: { by: x/1, at: …, model: m }` types both parts, keeps `generated` in
    `extra_metadata`, and produces no warnings.
  - `a_malformed_timestamp_warns_and_is_kept`: covers `timestamp: yesterday`,
    `stale_after: next tuesday`, `stale_after: "2026-06-01"` (date-only) and
    `stale_after: "2026-06-01T00:00:00"` (no offset). Each gives `None`, one warning and
    the raw value kept.
  - `unquoted_and_quoted_timestamps_read_alike` (Review Focus 1).
  - `an_offset_timestamp_is_the_same_instant`: `+09:00` gives the same UTC instant
    (Review Focus 2).
  - `trust_tiers_follow_the_verifiers`:
    - absent gives `None`;
    - `[]` gives `Unverified`;
    - a bare `{ by: openwiki/0.9.0, at: … }` gives `MachineConfirmed` with one event;
    - machine plus `human:alice` gives `HumanReviewed`;
    - `HUMAN:alice` gives `MachineConfirmed`;
    - a bare `"human:"` gives `MachineConfirmed` (it must be quoted, because unquoted
    `human:` is not a YAML string);
  - `"human:  "` gives `MachineConfirmed`.
  - `a_malformed_verified_writes_no_tier`: `verified: 5` gives a `None` tier, no events,
    one warning, and the raw value kept.
  - `a_bad_verified_entry_is_skipped`: `[{by: a/1, at: …}, {by: b/1}]` gives one event,
    `MachineConfirmed`, and one warning naming `verified[1]`. A list with only bad entries
    gives a `None` tier.
  - `a_duplicate_verification_event_is_written_once` (Review Focus 5).
  - `status_is_stored_as_written`: `published` is stored; `status: 3` gives a warning and
    `None`.
  - `blank_strings_are_malformed`: `status: "  "` and `by: "  "` (Review Focus 3).
  - `a_v01_concept_has_no_provenance`: a concept without any v0.2 key gives
    `provenance == OkfProvenance::default()` and no warnings.
  - `parse_bundle_collects_warnings_in_file_order`: two files with bad `stale_after`
    values give `bundle.warnings` in sorted-path order.

- [ ] **Step 2: Run the tests to verify they fail.**
  Run: `cargo test -p gecko-engine --no-default-features --lib okf::parser`
  Expected: the build fails because `provenance`, `OkfProvenance` and the tuple return
  are not defined.

- [ ] **Step 3: Implement the types, `take_provenance`, and the `parse_concept` and
  `parse_bundle` changes.** `extract_timestamp` is deleted; `take_provenance` owns
  `timestamp`. Update every existing `parse_concept` / `parse_with` call site in the
  tests to take `.0`.

- [ ] **Step 4: Run the tests to verify they pass.**
  Run: `cargo test -p gecko-engine --no-default-features --lib okf::`, then
  `cargo build --workspace --exclude js-sandbox --no-default-features --features cyber --all-targets`.
  Expected: everything passes and the workspace builds.

- [ ] **Step 5: Commit** — `OKF v0.2 provenance is parsed; a malformed value warns and stays raw (O2, O4)`.

### Task 2: The bundle-root `okf_version`

**Files:**
- Modify: `core/gecko-engine/src/okf/parser.rs` (`parse_bundle`, plus a new
  `fn read_okf_version(bundle_root: &Path, warnings: &mut Vec<String>) -> Option<String>`)

**Interfaces:**
- **Consumes:** `OkfBundle.okf_version` and `OkfBundle.warnings` (Task 1).
- **Produces:** `parse_bundle` sets `okf_version`.

**Behaviour.**
- **What is read.** Only the root entry that `is_reserved_file` takes as `index.md` is
  read, matched case-insensitively like the reservation, so `Index.md` counts. `split_frontmatter` gives its
  YAML, and only `okf_version` is taken from it.
- **What is stored.**
  - A non-blank string is stored as written.
  - A YAML number is stored as `yaml_as_written` (unquoted `0.2` gives `"0.2"`).
- **No warning, and `None`, for** a missing file, no frontmatter, or no key.
- **A warning, and `None`, for:**
  - YAML that won't parse, or that parses but is not a mapping (a bare scalar), which is
    intended: `Bundle index '{path}' has frontmatter that is not valid YAML; its okf_version is not recorded`;
  - any other value (`null`, blank, `""`, a boolean, a list or a map):
    `Bundle index '{path}' has an invalid 'okf_version' ({value}); it is not recorded`.
- **`index.md` stays reserved,** and is never a concept.

- [ ] **Step 1: Write the failing tests** in `parser.rs`.
  - `okf_version_is_read_from_the_root_index`: `"0.2"` quoted gives `Some("0.2")`; `0.2`
    unquoted gives `Some("0.2")`; `"9.9"` gives `Some("9.9")`.
  - `a_missing_or_keyless_root_index_records_no_version`: no file, a body-only file, and
    frontmatter without the key each give `None` and no warnings.
  - `an_invalid_okf_version_warns`: `null`, a blank value, `""`, `true` and `[0.2]` each
    give `None` and one warning. Frontmatter that won't parse gives the YAML warning, and
    the bundle still parses.
  - `a_subdirectory_index_version_is_ignored`: `sub/index.md` with `okf_version: "0.2"`
    and no root index gives `None`.
- [ ] **Step 2: Run them to verify they fail.**
  Run: `cargo test -p gecko-engine --no-default-features --lib okf::parser`
  Expected: FAIL for `okf_version_is_read_from_the_root_index` and
  `an_invalid_okf_version_warns`. The two "no version" tests already pass, because
  `okf_version` is `None` today; they are regression guards, not RED.
- [ ] **Step 3: Implement `read_okf_version`** and call it from `parse_bundle`.
- [ ] **Step 4: Run `okf::` again.**
  Expected: PASS.
- [ ] **Step 5: Commit** — `The bundle-root okf_version is read (O4)`.

### Task 3: Schema and sync

**Files:**
- Modify: `core/gecko-engine/schema/core_schema.tql`
- Modify: `core/gecko-engine/src/syncer/bundle.rs` (`sync_bundle`,
  `attach_concept_content`, `clear_concept_content`, `upsert_bundle`)
- Test: `gecko-bin/tests/bundle_sync_test.rs`

**Interfaces:**
- **Consumes:** `OkfConcept.provenance`, `OkfBundle.okf_version` (Tasks 1–2).
- **Produces, as schema labels:**
  - the attributes `generated-by` (string), `trust-tier` (string,
    `@values("unverified", "machine-confirmed", "human-reviewed")`), `okf-status`
    (string), `stale-after` (datetime), `verified-by` (string), `verified-at` (datetime)
    and `okf-version` (string);
  - `relation verification`, with `relates verified-concept @card(1)` and
    `owns verified-by @card(1)`, `owns verified-at @card(1)`;
  - `concept` owns `generated-by`, `trust-tier`, `okf-status` and `stale-after`, each
    `@card(0..1)`, and plays `verification:verified-concept`;
  - `bundle` owns `okf-version @card(0..1)`.
  - The `@doc` for `trust-tier` says absence means `unverified`. The `@doc` for
    `okf-status` says absence means `stable`.

**Sync changes.**
- **One schema read.** Read the schema once at the start of `sync_bundle` (before
  `begin_write`), and reuse it for the existing `required_attributes` call at
  `bundle.rs:352`.
  - Keep the `required_attributes(...)?` parse lazy, inside the existing
    `if !retire_ids.is_empty()`, so a schema that it cannot parse doesn't start failing
    every sync.
  - Compute `defines_attribute(&schema, "verified-by")`, which stands for the
    `verification` relation because both are defined together.
  - Compute `defines_attribute(&schema, "okf-version")`.
- **`clear_concept_content`.**
  - Takes the `verification` flag. When it is set, it also deletes
    `$v isa verification, links (verified-concept: $c)`.
  - Its attribute delete already clears the new attributes.
- **`attach_concept_content`.**
  - Writes `generated-by`, `trust-tier` (`as_str`) and `okf-status` with `attach_attr`.
  - Writes `stale-after` with `attach_datetime`. The `timestamp` write is unchanged.
  - Writes one `verification` per event in a single `run_rows` call:
    `given $id: string, $by: string, $at: datetime; match … ; insert $v isa verification, links (verified-concept: $c), has verified-by == $by, has verified-at == $at;`.
  - Runs no query when nothing is written.
- **`upsert_bundle`.**
  - Takes the `okf-version` flag. When it is set, it deletes the bundle's `okf-version`.
  - Then, when `manifest.okf_version` is `Some`, it inserts it. It runs on every sync.

- [ ] **Step 1: Write the failing integration tests** in `bundle_sync_test.rs`.
  - Use the existing `TestDb`, `apply_core(&mut db, true)` and `count` helpers.
  - `write_concepts` hard-codes its frontmatter, so add a helper that writes a concept
    with given frontmatter lines (for example `write_concept_with(dir, path, frontmatter)`).
  - `provenance_attributes_round_trip`: a concept with `generated`, `status: draft`,
    `stale_after` and a human verifier stores `generated-by`, `timestamp`, `okf-status`,
    `stale-after` and `trust-tier "human-reviewed"`. A re-sync after removing them leaves
    none of them.
  - `verification_events_round_trip`:
    - two events plus one exact duplicate store two `verification` relations, each with
      its own by/at pairing;
    - after an edit to one event, exactly the new set is stored (Review Focus 5).
  - `gc_and_retirement_leave_no_verification`:
    - **GC:** assert at least 1 `verification` before the drop; deleting the verified
      concept's file then leaves 0.
    - **Retirement:** the referenced concept must carry a `verified:` block. Parameterise
      `synced_with_referenced`, or write a sibling of it, so that it does. Assert at least
      1 before the drop and 0 after retirement.
  - `okf_version_follows_the_root_index`:
    - sync gives `okf-version "0.2"`;
    - changing only `index.md` to `"9.9"` and re-syncing gives `"9.9"` (Review Focus 4);
    - removing the key leaves no `okf-version`.
  - `a_v01_bundle_gains_no_provenance_attributes`: the existing `create_test_bundle`
    bundle stores 0 `trust-tier`, 0 `generated-by` and 0 `verification`.
  - **The guard is pinned by the existing
    `tightened_schema_applies_to_a_populated_database`**
    (`gecko-bin/tests/schema_init_test.rs`), which runs `sync_bundle` against `PRE_CORE`.
    - Its v0.1 bundle carries no v0.2 values, so the writes skip and only the guarded
      deletes would reach the old schema.
    - Its three `sync_bundle` calls are all inserts. Add an edited re-sync of the OKF
      bundle, still on the old schema, so that an update runs `clear_concept_content`
      there. The `okf-version` delete is already hit, because `upsert_bundle` runs on
      every sync.
    - The bundle stays v0.1. On an old schema the v0.2 writes fail loudly at TypeDB parse,
      as `timeout-ms` does (the spec bullet is amended to match).
    - Check the guard by removing it temporarily and watching the test fail with an
      unknown-type error. Then restore it.
- [ ] **Step 2: Run them to verify they fail.**
  Run: `cargo nextest run -p gecko-bin --no-default-features --test bundle_sync_test -j 1 provenance verification okf_version v01_bundle`
  Expected: FAIL. A schema label is missing, or the counts are 0.
- [ ] **Step 3: Implement the schema and sync changes.**
- [ ] **Step 4: Run `bundle_sync_test` and `schema_init_test` whole.**
  Run: `cargo nextest run -p gecko-bin --no-default-features --features cyber --test bundle_sync_test --test schema_init_test -j 1`.
  `tightened_schema_applies_to_a_populated_database` is `#[cfg(feature = "cyber")]`, so
  the guard check (remove it, watch the test fail, restore it) also runs with
  `--features cyber`.
  Expected: PASS.
- [ ] **Step 5: Commit** — `Sync writes OKF v0.2 provenance and the bundle's okf-version (O2, O4)`.

### Task 4: The CLI, the fixtures, the dogfood and the docs

**Files:**
- Modify: `gecko-bin/src/main.rs`: in `cmd_sync`, print each `manifest.warnings` entry
  as `  warning: {w}` **before** `sync_manifest`, so a sync that fails for another
  reason still shows them.
- Test: `gecko-bin/tests/run_args_cli_test.rs` (it has the `gecko()` / `text()` CLI
  helpers)
- Create: `core/gecko-engine/tests/okf_v02/`, a hand-written v0.2 bundle, and the test
  `core/gecko-engine/tests/okf_v02_test.rs`
- Modify: `docs/refactor/KNOWN-LIMITATIONS.md`, `docs/refactor/OWED-WORK-LEDGER.md`

- [ ] **Step 1: Write the failing tests.**
  - `sync_prints_provenance_warnings` (CLI): a bundle with one `stale_after: soon` concept
    syncs successfully, and stdout contains `warning:` and the file name and `soon`.
  - `okf_v02_test.rs` (serviceless):
    - `the_v02_fixture_bundle_reads_as_written` parses `tests/okf_v02/`:
      - a root `index.md` with `okf_version: "9.9"`;
      - `human.md`: a `human:` verifier, `status: deprecated` and `stale_after`;
      - `bare.md`: a bare-mapping `verified`;
      - `legacy.md`: a legacy `timestamp` beside `generated`;
      - `unknown.md`: `status: published`.
      It asserts each typed value and that there are no warnings.
    - `the_repos_own_openwiki_bundle_parses` parses
      `concat!(env!("CARGO_MANIFEST_DIR"), "/../../openwiki")`. The assertions are loose
      only:
      - the warnings are empty;
      - every concept has a `timestamp` and a `generated_by`;
      - every concept with verifications is `MachineConfirmed`, and every other concept
        has a `None` tier;
      - `okf_version == Some("0.2")`.
      It asserts no counts.
- [ ] **Step 2: Run them to verify they fail.**
  Run: `cargo nextest run -p gecko-engine --no-default-features --test okf_v02_test`, then
  `cargo nextest run -p gecko-bin --no-default-features --test run_args_cli_test -j 1 sync_prints_provenance_warnings`.
  Expected: the CLI test fails because no warning is printed. The fixture test fails
  until the fixture exists; write the fixture after watching it fail.
  `the_repos_own_openwiki_bundle_parses` passes as soon as it is written, since Tasks 1–2
  are done. It is a smoke test, not a RED.
- [ ] **Step 3: Implement the print, then write the fixture bundle.**
- [ ] **Step 4: Run both again.**
  Expected: PASS.
- [ ] **Step 5: Docs.**
  - **`KNOWN-LIMITATIONS.md`, under "Playbook frontmatter",** or a new "OKF v0.2
    frontmatter" section beside it:
    - an unchanged file synced before this release gains no v0.2 attributes until it is
      edited;
    - a malformed v0.2 provenance value is warned about and kept in `metadata-json`, not
      refused;
    - an unquoted `okf_version: 0.10` is stored as `0.1`, because YAML reads it as a
      number; quote the version.
  - **The ledger:**
    - an O2 + O4 row under Landed, naming the design doc and the pinning tests;
    - remove O2 and O4 from Deferred;
    - O3 moves to Next up, with the notes for its design listed in the spec's Docs
      section.
- [ ] **Step 6: Run the full suite.**
  Run: `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1`
  Expected: PASS, with nothing skipped that ran before.
- [ ] **Step 7: Commit** — `gecko sync prints provenance warnings; the v0.2 fixture and the openwiki dogfood test`.
