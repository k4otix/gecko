# ATT&CK detection model and platform-aware blindness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make cyber's `blinded` check work on current ATT&CK: report, per claimed detection, which of the user's declared platforms it is blind on and what the platform's analytic lacks, fed by a synced sensor file.

**Architecture:**
- **Sync engine.** It learns to retype a concept whose `type_hint` changed.
- **STIX adapter.** It models detection strategies, analytics and per-analytic log-source requirements, and replaces their relations on each re-sync.
- **Sensor file.** A YAML file converts to typed `sensor` / `monitored-environment` concepts.
- **The check.** TypeDB functions compute coverage, and the `blinded` import shapes their rows into the report.

**Tech Stack:** Rust 2024, TypeDB 3.15 / TypeQL, serde_yaml 0.9, tokio, cargo nextest.

**Spec:** `docs/superpowers/specs/2026-11-21-attack-detection-model-design.md`

## Global Constraints

- **Repo rules:**
  - All TQL is parameterized (values through `Params` / `GraphValue` / given rows), never string interpolation of values.
  - Every new type, role and function carries a one-sentence greenfield `@doc`.
  - A changed `@doc` on an existing type needs its own `redefine` `SchemaMigration`.
  - Comments are greenfield: no plan tags (`S2`, `D-L`), no "now"/"no longer".
  - Never `#[allow(dead_code)]`.
- **Integration tests:** TypeDB on localhost:1729, `--test-threads=1`, throwaway databases only, never the database `gecko`.
- **Before each commit:**
  - `cargo fmt --all`;
  - `cargo clippy --workspace --exclude js-sandbox --all-targets --no-default-features -- -D warnings`;
  - the same clippy with `--features cyber`.
- **Commit trailer:** commits end with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Name matching:** log-source and component names match on a key that is the name trimmed and lowercased. The display name is the name trimmed, as ATT&CK spells it. Platform names are compared exactly.
- **Liveness:** every new function skips concepts with `retired-at`, `revoked true` or `deprecated true`.
- **Statuses and flags:** `claim-platforms` statuses are exactly `"blind"`, `"covered"`, `"unassessed"`. The `blinded` output always carries `no_log_sources`.
- **Sensor names:** concept ids are `sensors/environment` and `sensors/sensor/<name>`, with `<name>` matching `[A-Za-z0-9._-]+`.

## Review Focus

- **Two sensor files in one database** (two bundle names) declare platforms. The declared set is the union, and a sensor from either covers. Task 4 test.
- **`sensors: []` with platforms declared** is accepted, and every claim is blind on every declared platform that has an analytic. Task 3 accepts it; Task 4 tests the report.
- **A platform written in the wrong case in the sensor file** (`windows`) is not matched, and it appears in the post-sync notice. Task 3 test.
- **A claim whose technique's only strategy is deprecated** reports every declared platform as unassessed. Task 4 test.
- **Re-syncing the unchanged v20.1 bundle after the upgrade** on a database already holding it under the old adapter retypes every strategy and analytic, and leaves counts stable on the next re-sync. Task 2 test with a two-object fixture, and S4 on the real bundle.

---

### Task 1: Sync retypes a concept whose type_hint changed

**Files:**
- Modify: `core/gecko-engine/src/syncer/bundle.rs` (`sync_bundle` step 3–6, using `concept_types`, `belief_edge_references` and `delete_concepts_gc`)
- Modify: `core/gecko-engine/src/db/router.rs` (`DbError`)
- Modify: `extensions/README.md` (one paragraph in the `type_hint` section)
- Test: `gecko-bin/tests/bundle_sync_test.rs`

**Interfaces:**
- Produces: `DbError::TypeChangeReferenced { ids: Vec<(String, String, String)> }`. Each entry is (concept id, stored type, new type), sorted. The message names every id and says nothing was committed.
- Produces: the sync behaviour. A concept in `to_update` whose stored entity type differs from `type_hint.unwrap_or("concept")`:
  - is deleted with its OKF relations (as GC does);
  - is then inserted as new;
  - is refused, before any write, if a belief edge references it.

- [ ] **Step 1: Write the failing tests** in `bundle_sync_test.rs`:
  - `a_changed_type_hint_retypes_the_concept`: sync a bundle where concept `a` has `type_hint: None`, then re-sync it with `type_hint: Some("retype-probe")` and a changed `file_hash`. No concept subtype exists outside cyber, so the test first applies `define entity retype-probe sub concept @doc("A test-only concept subtype.");` to its throwaway database. Assert `a` is that subtype, not a bare `concept`, that its containment exists once, and that its attributes match the second manifest.
  - `a_changed_type_hint_on_a_referenced_concept_is_refused`: same setup, but before the re-sync a belief edge references `a` (use `common::belief::claim_with_belief`). Assert `DbError::TypeChangeReferenced` naming `a`, and that `a` is still the original type with its original hash.
- [ ] **Step 2: Run, and expect both to fail.**
  - Run: `cargo nextest run -p gecko-bin --no-default-features --test bundle_sync_test --test-threads 1`
  - Expected: the first fails on type, and the second fails because the sync succeeds.
- [ ] **Step 3: Implement.**
  - After `plan_sync`, read `concept_types` for the update ids.
  - Partition out the retyped ones.
  - Run `belief_edge_references` on them and refuse with the new variant if any is referenced.
  - Otherwise `delete_concepts_gc` them in step 5 and move them to the insert set.
  - Keep `check_type_hints` before any write.
  - Add one `README.md` paragraph saying a changed `type_hint` replaces the concept, and is refused when a belief edge references it.
- [ ] **Step 4: Run the bundle, multi-bundle and reconciliation suites, and expect them to pass.**
  - Run: `cargo nextest run -p gecko-bin --no-default-features --test bundle_sync_test --test multi_bundle_test --test belief_tier_reconciliation_test --test-threads 1`
- [ ] **Step 5: Commit** `feat(sync): a concept whose type_hint changed is replaced, refused when a belief references it`.

### Task 2: Adapter and schema model strategies, analytics and requirements

**Files:**
- Modify: `extensions/cyber-gecko/schema/cyber_schema.tql`
- Modify: `extensions/cyber-gecko/src/stix/mod.rs` (`sdo_type_hint`, `owns_platform`, `REL_SHAPES`, the embedded-ref block, content hash, `cyber_post_sync`)
- Modify: `gecko-bin/src/main.rs` (only if `cyber_post_sync`'s signature changes)
- Test:
  - `stix/mod.rs` unit tests;
  - `gecko-bin/tests/cyber_stix_sync_test.rs` (its `requires-evidence` / `evidence-of` assertions move to the new shape);
  - `gecko-bin/tests/belief_tier_reconciliation_test.rs` STIX fixture (lines ~93, ~129);
  - `gecko-bin/tests/cyber_detect_test.rs` seed.

**Interfaces:**
- Consumes: Task 1's retyping, which is how an existing database's bare `cyber-object` strategies become typed.
- Produces the schema types and attributes:
  - `detection-strategy`, `analytic` (owns `platform`);
  - `strategy-detects (strategy, technique)`, `strategy-analytic (strategy, analytic)`;
  - `analytic-requirement (analytic, component)` owning `log-source-name @card(1)`, `log-source-key @card(1)` and `log-source-channel @card(0..)`;
  - `attribute deprecated value boolean`, owned by `cyber-object`;
  - `attribute component-key value string`, owned by `data-component`, written by the adapter as the trimmed, lowercased name.
- Produces: `pub const STIX_MAPPING_VERSION: &str = "2";`, prefixed into every concept's hash input.
- Unchanged: `cyber_post_sync(&[TypedRelation]) -> Vec<GraphWrite>` keeps its signature, if requirement rows travel in `TypedRelation` or a sibling field of `to_okf`'s return. The implementer picks one and states it in the report; Task 4 consumes only the graph.

**Kept in this task.** `evidence-of`, `requires-evidence`, `sensor-coverage` and `blinded-detections()` stay in the schema; Task 4 removes them. The adapter simply stops writing the first two.

- [ ] **Step 1: Unit tests in `stix/mod.rs`:**
  - `detection_strategy_and_analytic_get_their_types`;
  - `analytic_platform_is_written`;
  - `strategy_analytic_is_synthesized_from_analytic_refs`;
  - `requirements_group_by_analytic_component_and_trimmed_name`: two refs `"saas:audit "` / `"saas:audit"` on one component give one row, with name `saas:audit`, key `saas:audit`, and both channels;
  - `a_none_channel_writes_no_channel`;
  - `a_data_component_detects_falls_to_a_concept_link`;
  - `deprecated_is_a_typed_attribute`;
  - `the_mapping_version_changes_the_hash` (same object, different constant, different hash);
  - `typed_relation_players_match_the_schema` stays green with the new shapes.
- [ ] **Step 2: Live tests in `cyber_stix_sync_test.rs`** (a fixture of a technique, a strategy with a `detects` SRO, two analytics on Windows and Linux, two components and log-source refs):
  - `stix_sync_models_strategies_analytics_and_requirements`: counts of each new relation, and the attribute values;
  - `a_reimport_replaces_requirements`: re-sync with one log-source ref removed from an analytic; that requirement is gone, and the others are unchanged;
  - `stix_resync_is_stable`: a second identical sync leaves every new relation count unchanged;
  - `a_database_synced_by_the_previous_adapter_is_retyped`: seed by syncing the fixture with strategy and analytic forced to `cyber-object` (build the manifest, then override `type_hint` and `file_hash`), then sync with the real adapter. Every strategy and analytic is retyped, and the new relations exist.

  Update the existing `requires-evidence` / `evidence-of` assertions to the new shape, and the reconciliation rig's STIX fixture to the current shape.
- [ ] **Step 3: `cyber_detect_test.rs`.** Move its seed to the current shape. Remove the `blinded` assertion from `detect_bridge_claims_disposes_and_reports`, because Task 4 owns the new `blinded` suite. Ledger this as a ruling.
- [ ] **Step 4: Run and expect the new tests to fail.**
  - Run: `cargo nextest run -p cyber-gecko --no-default-features` and `cargo nextest run -p gecko-bin --no-default-features --features cyber --test cyber_stix_sync_test --test-threads 1`
- [ ] **Step 5: Implement the schema, adapter and replace-then-insert post-sync writes.**
  - **Replace, then insert.** For `strategy-analytic` and `analytic-requirement`, one op deletes every such relation whose strategy or analytic id is in this manifest's set, then the insert ops run. `strategy-detects` stays insert-only, under `not {}`.
  - **Docs.** Every new type and role gets an `@doc`.
- [ ] **Step 6: Run cyber unit tests, `cyber_stix_sync_test`, `cyber_detect_test` and `belief_tier_reconciliation_test`; all pass.**
- [ ] **Step 7: Commit** `feat(cyber): model ATT&CK detection strategies, analytics and log-source requirements`.

### Task 3: The sensor file and `gecko sync-sensors`

**Files:**
- Create: `extensions/cyber-gecko/src/sensors.rs` (`pub mod sensors` in `lib.rs`; `serde_yaml` added to cyber's `Cargo.toml` from the workspace)
- Modify: `extensions/cyber-gecko/schema/cyber_schema.tql`:
  - `monitored-environment` owns `platform @card(1..)`;
  - `sensor` owns `platform @card(1..)`, `log-source-name @card(0..)`, `log-source-key @card(0..)`, `covered-component-name @card(0..)` and `covered-component-key @card(0..)`;
  - new string attributes `covered-component-name` and `covered-component-key`.
- Modify: `gecko-bin/src/main.rs`: a `#[cfg(feature = "cyber")] SyncSensors { file: PathBuf, #[arg(long)] bundle_name: Option<String> }` arm. It refuses unless the global `--database` is given.
- Test: unit tests in `sensors.rs`; create `gecko-bin/tests/cyber_sensors_sync_test.rs`.

**Interfaces:**
- Consumes: Task 2's `strategy-detects`, `analytic`, `analytic-requirement`, `log-source-key` and `component-key`.
- Produces:
  - `pub struct SensorFile`, the parsed and validated file;
  - `pub fn parse(yaml: &str) -> Result<SensorFile, String>`;
  - `pub fn to_okf(file: &SensorFile, bundle_name: &str) -> OkfBundle`;
  - `pub async fn check_against_graph(graph: &Arc<dyn GraphReader>, file: &SensorFile) -> Result<(), String>`;
  - `pub async fn notices(graph: &Arc<dyn GraphReader>, file: &SensorFile) -> Result<Vec<String>, String>`.

- [ ] **Step 1: Unit tests in `sensors.rs`:**
  - `parse` refuses, and the message names the problem, for each of:
    - `missing_platforms`, `empty_platforms`;
    - `sensor_without_name`, `bad_name` (`"a/b"`), `duplicate_name`;
    - `sensor_without_platforms`, `sensor_with_neither_log_sources_nor_components`;
    - `non_string_entry`.
  - `accepts_an_empty_sensor_list`.
  - `to_okf_writes_every_field`: ids `sensors/environment` and `sensors/sensor/windows-edr`, type hints `monitored-environment` and `sensor`, and the platform, log-source-name, key (`" WinEventLog:Security"` gives name `WinEventLog:Security`, key `wineventlog:security`) and component name and key attributes.
- [ ] **Step 2: Live tests in `cyber_sensors_sync_test.rs`** (sync Task 2's fixture first):
  - `refuses_a_database_without_attack`: the error contains `sync the ATT&CK bundle into this database first`;
  - `refuses_every_unmatched_or_ambiguous_component_at_once`: two bad names are both named, and no `sensor` exists after;
  - `sync_twice_changes_nothing`;
  - `notices_name_unused_log_sources_and_platforms`: `windows` and an unknown log source both appear;
  - `a_removed_component_leaves_the_sensor`: a re-sync without a component leaves no `covered-component-key` for it.
- [ ] **Step 3: Run, and expect failure.**
  - Run: `cargo nextest run -p cyber-gecko --no-default-features sensors` and `cargo nextest run -p gecko-bin --no-default-features --features cyber --test cyber_sensors_sync_test --test-threads 1`
- [ ] **Step 4: Implement.**
  - **The CLI arm** runs: `parse` → `check_against_graph` → `sync_manifest` with no post-sync writes → print `notices`.
  - **Graph handle.** It uses the same read-only graph handle construction the cyber bridge uses.
  - **Tests.** The tests drive the library functions and `sync_manifest`, not the binary.
- [ ] **Step 5: Run; pass.**
- [ ] **Step 6: Commit** `feat(cyber): a sensor file declares platforms and sensors, synced by gecko sync-sensors`.

### Task 4: Coverage functions, the `blinded` report, and removal of the old model

**Files:**
- Modify: `extensions/cyber-gecko/schema/cyber_functions.tql`:
  - add `analytic-gaps($a: analytic) -> { analytic-requirement }`;
  - add `claim-platforms() -> { concept, attack-pattern, string, string }`, giving (logic, technique, platform, status);
  - add `requirement-covered` if kept as a function;
  - remove `blinded-detections()`.
- Modify: `extensions/cyber-gecko/schema/cyber_schema.tql`: remove `evidence-of`, `requires-evidence`, `sensor-coverage` and their `plays`, and rewrite the `@doc`s of `data-source`, `data-component` and `sensor`.
- Modify: `extensions/cyber-gecko/src/lib.rs`: `schema_migrations()` returns the ordered entries from spec §1.
- Modify: `extensions/cyber-gecko/src/detect/mod.rs`: `blinded` and the module doc.
- Test: `gecko-bin/tests/cyber_detect_test.rs`, `gecko-bin/tests/schema_init_test.rs`.

**Interfaces:**
- Consumes:
  - Task 2's types;
  - Task 3's `monitored-environment` / `sensor` attributes;
  - `covered-component-key` matched against `data-component`'s `component-key`.
- Produces: the `blinded` JSON exactly as spec §6, with these fields:
  - `blinded_detections[].concept_id`, `.technique` (the attack-id), `.unassessed_platforms` (sorted);
  - `.blind_platforms[]` with `platform`, `analytic` (the attack-id), `no_log_sources`, and `missing[]`;
  - each `missing[]` entry has `log_source` (the display name) and `data_components` (sorted names);
  - a top-level `stale_sensor_components[]` with `sensor` and `component`.

  Errors: `no sensor file synced` and `no current ATT&CK detection data`.

- [ ] **Step 1: Probe the migration first.** On a throwaway database with the old cyber schema plus one `evidence-of`, one `requires-evidence` and one `sensor-coverage` instance, run the ordered migration entries through `apply_schema_with_migrations`.
  - **If a `match … delete` entry fails in the schema transaction, or the registry refuses it:** stop, and report BLOCKED with the error. Do not loosen the registry.
  - **If it passes:** keep the probe as `schema_init_test::old_cyber_detection_types_migrate_away`, asserting the three types are gone and the function is gone.
- [ ] **Step 2: Live tests in `cyber_detect_test.rs`**, one each, named for the cases in spec §7's "`blinded` cases" list plus Review Focus. All share one fixture builder (technique, strategy, analytics for Windows / Linux / one zero-requirement platform, components, a claim, and a sensor file built through `sensors::to_okf`). Required names:
  - `blind_with_no_sensor_lists_every_requirement_by_log_source`
  - `covered_by_log_source_including_a_whitespace_or_case_variant`
  - `covered_by_component_fallback`
  - `partial_coverage_is_blind_listing_only_gaps`
  - `undeclared_platform_is_never_reported`
  - `declared_platform_without_analytic_is_unassessed`
  - `zero_requirement_analytic_reports_no_log_sources`
  - `sensor_on_wrong_platform_does_not_cover`
  - `removed_sensor_stops_covering`
  - `removed_component_stops_covering`
  - `deprecated_analytic_is_ignored`
  - `claim_on_retired_technique_is_unassessed`
  - `claim_on_technique_with_only_deprecated_strategy_is_unassessed`
  - `stale_fallback_component_is_reported`
  - `two_sensor_files_union_platforms_and_coverage`
  - `empty_sensor_list_is_blind_everywhere_assessed`
  - `blinded_errors_without_a_sensor_file`
  - `blinded_errors_on_a_pre_current_graph`
- [ ] **Step 3: Run, and expect failure.**
  - Run: `cargo nextest run -p gecko-bin --no-default-features --features cyber --test cyber_detect_test --test schema_init_test --test-threads 1`
- [ ] **Step 4: Implement** the functions, the removals and migrations, and `blinded`.
  - `blinded` reads `claim-platforms()`.
  - For each blind row it reads that platform's analytic and its `analytic-gaps`.
  - It reads stale fallback components, and returns the output sorted by concept id, platform and log source.
- [ ] **Step 5: Run all cyber tests, `schema_init_test` and `belief_tier_reconciliation_test`; pass.**
- [ ] **Step 6: Commit** `feat(cyber): blinded reports blind and unassessed platforms per claim from the current ATT&CK model`.

### Task 5: Real-bundle check, docs, full verification

**Files:**
- Test: `gecko-bin/tests/cyber_stix_sync_test.rs`, adding `stix_full_bundle_resync_is_stable`.
- Modify: `docs/refactor/OWED-WORK-LEDGER.md`:
  - D-L decided, citing the spec;
  - S2 ✅ and S4 ✅, with test names and the release;
  - the status bullets and sequence lines;
  - S3's row noting that type-change retyping exists.
- Modify: `docs/refactor/KNOWN-LIMITATIONS.md`, adding the four points in spec §8.

**Interfaces:** consumes everything above.

- [ ] **Step 1: `stix_full_bundle_resync_is_stable`.**
  - **Skip rule.** If `GECKO_ATTACK_BUNDLE` is unset, it prints a skip line and returns.
  - **Body.** It imports the file twice into one throwaway database, and asserts every typed relation count (the new three included) and the concept count are equal across the two syncs. It prints both timings.
- [ ] **Step 2: Run the real-bundle check against v20.1** with `GECKO_ATTACK_BUNDLE=<path to enterprise-attack.json v20.1>`, `cargo nextest run -p gecko-bin --no-default-features --features cyber --test cyber_stix_sync_test stix_full_bundle_resync_is_stable --test-threads 1`.
  - Expected: PASS.
  - Record the counts and timings in the S4 row.
- [ ] **Step 3: Docs as listed.**
- [ ] **Step 4: Full verification.** Run fmt and both clippy runs, then `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --test-threads 1`.
  - Expected: all pass.
- [ ] **Step 5: Commit** `docs: record D-L, S2 and S4; real-bundle re-sync check`.
