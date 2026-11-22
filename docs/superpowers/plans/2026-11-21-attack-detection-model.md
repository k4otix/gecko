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
- **Liveness:** every new function skips strategies, analytics, components and sensors with `retired-at`, `revoked true` or `deprecated true`.
  - **Exception:** a claim's technique is never skipped. A claim on a retired, revoked or deprecated technique yields `unassessed` on every declared platform.
  - **The id may be gone.** Such a technique may have lost its `attack-id`, so the output's `technique` is then `null`. The output always carries `technique_concept_id`.
- **Statuses and flags:** `claim-platforms` statuses are exactly `"blind"`, `"covered"`, `"unassessed"`. The `blinded` output always carries `no_log_sources`.
- **Sensor ids:** concept ids are `<bundle>/environment` and `<bundle>/sensor/<name>`, where `<bundle>` is the sync's bundle name (default `sensors`) and `<name>` matches `[A-Za-z0-9._-]+`. Two sensor files therefore need two bundle names, and do not collide.
- **The `missing` list groups by log-source key.** It displays the lexicographically smallest trimmed spelling of that key.
- **Shared fixture:** a current-shape ATT&CK fixture lives in `gecko-bin/tests/common/attack.rs` (`#[cfg(feature = "cyber")] pub mod attack;` in `common/mod.rs`). It provides `pub fn fixture_json() -> String` and `pub async fn sync_fixture(db: &TestDb)`. Every later task seeds from it.
- **The progress log** for rulings is `.superpowers/sdd/2026-11-21-attack-detection-model/progress.md`.

## Review Focus

- **Two sensor files in one database** (two bundle names) declare platforms. The declared set is the union, and a sensor from either covers. Task 5 test.
- **`sensors: []` with platforms declared** is accepted, and every claim is blind on every declared platform that has an analytic. Task 4 accepts it; Task 5 tests the report.
- **A platform written in the wrong case in the sensor file** (`windows`) is not matched, and it appears in the post-sync notice. Task 4 test.
- **A claim whose technique's only strategy is deprecated** reports every declared platform as unassessed. Task 5 test.
- **Re-syncing the unchanged v20.1 bundle after the upgrade** on a database already holding it under the old adapter retypes every strategy and analytic, and leaves counts stable on the next re-sync. Task 3 test with a two-object fixture, and S4 on the real bundle.

---

### Task 1: Probe the migration that removes the old detection types

This task commits a probe test and the migration text it proved. Nothing else changes.

**Files:**
- Create: `gecko-bin/tests/fixtures/cyber_schema_1cd621d.tql`, a frozen copy of `cyber_schema.tql` plus `cyber_functions.tql` exactly as at commit `1cd621d`, concatenated as `CyberGecko::new()` does.
- Create: `extensions/cyber-gecko/src/migrations.rs` with `pub static DETECTION_MODEL_MIGRATIONS: [SchemaMigration; N]`. It holds the ordered entries from spec §1, steps 1–4. The `@doc` redefines are Task 5's. It is not yet returned by `schema_migrations()`.
- Test: `gecko-bin/tests/schema_init_test.rs`, adding `old_cyber_detection_types_migrate_away`.

**Interfaces:**
- Produces: `cyber_gecko::migrations::DETECTION_MODEL_MIGRATIONS`. Each entry has:
  - an `id` of the form `cyber/<slug>`;
  - an `applies_when` string present in the exported schema only before that entry runs;
  - one TQL query.

- [ ] **Step 1: Write the probe.** On a throwaway database:
  - apply core, mem and the frozen fixture;
  - insert one `evidence-of`, one `requires-evidence` and one `sensor-coverage` instance (scoped to a `population`);
  - run `apply_schema_with_migrations(<frozen fixture>, &DETECTION_MODEL_MIGRATIONS)`.

  Also assert that `BeliefEdgeRegistry::from_extensions` accepts the migrations. `check_no_base_modification` refuses any `undefine` naming a base type such as `population`, so this assertion is how that refusal gets tested.

  Assert that the three relation types and `blinded-detections` are gone from the exported schema.
- [ ] **Step 2: Find a working shape.**
  - **Instances.** Delete the instances first.
  - **The `population` plays.** Try removing `population plays sensor-coverage:coverage-scope` by `undefine relation sensor-coverage;` without naming `population`, then by any other route that names no base type.
  - **If no route passes both the database and the registry:** stop and report BLOCKED with each error. Do not loosen the registry.
- [ ] **Step 3: Run** `cargo nextest run -p gecko-bin --no-default-features --features cyber --test schema_init_test --test-threads 1`. Expected: PASS.
- [ ] **Step 4: Commit** `test(cyber): probe the migration that removes the pre-current detection types`.

### Task 2: Sync retypes a concept whose type_hint changed

**Files:**
- Modify: `core/gecko-engine/src/syncer/bundle.rs` (`sync_bundle` step 3–6, using `concept_types`, `belief_edge_references` and `delete_concepts_gc`)
- Modify: `core/gecko-engine/src/db/router.rs` (`DbError`)
- Modify: `extensions/README.md` (one paragraph in the `type_hint` section)
- Test: `gecko-bin/tests/bundle_sync_test.rs`

**Interfaces:**
- Produces: `DbError::TypeChangeReferenced { ids: Vec<(String, String, String)> }`. Each entry is (concept id, stored type, new type), sorted. The message names every id and says nothing was committed.
- Produces: the sync behaviour. **Any** manifest concept already in the graph whose stored entity type differs from `type_hint.unwrap_or("concept")` counts in `SyncResult` as updated:
  - this includes one whose hash is unchanged;
  - it is deleted with its OKF relations (as GC does);
  - it is then inserted as new;
  - it is refused, before any write, if a belief edge references it.

- [ ] **Step 1: Write the failing tests** in `bundle_sync_test.rs`:
  - `a_changed_type_hint_retypes_the_concept`: sync a bundle where concept `a` has `type_hint: None`, then re-sync it with `type_hint: Some("retype-probe")` and a changed `file_hash`. No concept subtype exists outside cyber, so the test first applies `define entity retype-probe sub concept @doc("A test-only concept subtype.");` to its throwaway database. Assert `a` is that subtype, not a bare `concept`, that its containment exists once, and that its attributes match the second manifest.
  - `a_changed_type_hint_with_an_unchanged_hash_still_retypes`: same as the first, but `file_hash` is unchanged.
  - `a_changed_type_hint_on_a_referenced_concept_is_refused`: same setup, but before the re-sync a belief edge references `a` (use `common::belief::claim_with_belief`). Assert `DbError::TypeChangeReferenced` naming `a`, and that `a` is still the original type with its original hash.
- [ ] **Step 2: Run, and expect both to fail.**
  - Run: `cargo nextest run -p gecko-bin --no-default-features --test bundle_sync_test --test-threads 1`
  - Expected: the first fails on type, and the second fails because the sync succeeds.
- [ ] **Step 3: Implement.**
  - After `plan_sync`, read `concept_types` for every manifest id already stored.
  - Partition out the retyped ones.
  - Run `belief_edge_references` on them and refuse with the new variant if any is referenced.
  - Otherwise `delete_concepts_gc` them in step 5 and move them to the insert set.
  - Keep `check_type_hints` before any write.
  - Add one `README.md` paragraph saying a changed `type_hint` replaces the concept, and is refused when a belief edge references it.
- [ ] **Step 4: Run the bundle, multi-bundle and reconciliation suites, and expect them to pass.**
  - Run: `cargo nextest run -p gecko-bin --no-default-features --test bundle_sync_test --test multi_bundle_test --test belief_tier_reconciliation_test --test-threads 1`
- [ ] **Step 5: Commit** `feat(sync): a concept whose type_hint changed is replaced, refused when a belief references it`.

### Task 3: Adapter and schema model strategies, analytics and requirements

**Files:**
- Modify: `extensions/cyber-gecko/schema/cyber_schema.tql`
- Modify: `extensions/cyber-gecko/src/stix/mod.rs` (`sdo_type_hint`, `owns_platform`, `REL_SHAPES`, the embedded-ref block, content hash, `cyber_post_sync`)
- Modify: `gecko-bin/src/main.rs` (the `SyncStix` arm, for the new `to_okf` / `cyber_post_sync` types)
- Create: `gecko-bin/tests/common/attack.rs` (the shared fixture, see Global Constraints)
- Test:
  - `stix/mod.rs` unit tests;
  - `typed_relation_players_match_the_schema` (live, `cyber_stix_sync_test.rs:343`);
  - `gecko-bin/tests/cyber_stix_sync_test.rs` (its `requires-evidence` / `evidence-of` assertions move to the new shape);
  - `gecko-bin/tests/belief_tier_reconciliation_test.rs` STIX fixture (lines ~93, ~129);
  - `gecko-bin/tests/cyber_detect_test.rs` seed.

**Interfaces:**
- Consumes: Task 2's retyping, which is how an existing database's bare `cyber-object` strategies become typed.
- Produces the schema types and attributes:
  - `detection-strategy`, `analytic` (owns `platform`);
  - `strategy-detects (strategy, technique)`, `strategy-analytic (strategy, analytic)`;
  - `analytic-requirement (analytic, component)` owning `log-source-name @card(1)`, `log-source-key @card(1)` and `log-source-channel @card(0..)`;
  - `attribute deprecated value boolean`, owned by `cyber-object`;
  - `attribute component-key value string`, owned by `data-component`, written by the adapter as the trimmed, lowercased name.
- Produces: `pub const STIX_MAPPING_VERSION: &str = "2";` and `fn concept_hash(version: &str, obj: &Value) -> String`. The hash is SHA-256 over the version, then the object JSON, and `to_okf` calls it with the constant.
- Produces these exact types:
  - `pub struct AnalyticRequirement { pub analytic: String, pub component: String, pub name: String, pub key: String, pub channels: Vec<String> }`. The ids are concept ids. `name` is trimmed, `key` is trimmed and lowercased, and `channels` is sorted and distinct, without `"None"`.
  - `pub struct StixRelations { pub typed: Vec<TypedRelation>, pub requirements: Vec<AnalyticRequirement> }`.
  - `pub fn to_okf(bundle_json: &str) -> Result<(OkfBundle, StixRelations), String>`.
  - `pub fn cyber_post_sync(rels: &StixRelations) -> Vec<GraphWrite>`.

  Update every caller: `main.rs`, `cyber_detect_test.rs`, `cyber_stix_sync_test.rs` and `belief_tier_reconciliation_test.rs`.

**Kept in this task.** `evidence-of`, `requires-evidence`, `sensor-coverage` and `blinded-detections()` stay in the schema; Task 5 removes them. The adapter simply stops writing the first two.

- [ ] **Step 1: Unit tests in `stix/mod.rs`:**
  - `detection_strategy_and_analytic_get_their_types`;
  - `analytic_platform_is_written`;
  - `strategy_analytic_is_synthesized_from_analytic_refs`;
  - `requirements_group_by_analytic_component_and_trimmed_name`: two refs `"saas:audit "` / `"saas:audit"` on one component give one row, with name `saas:audit`, key `saas:audit`, and both channels;
  - `a_none_channel_writes_no_channel`;
  - `a_data_component_detects_falls_to_a_concept_link`;
  - `deprecated_is_a_typed_attribute`;
  - `the_mapping_version_changes_the_hash`: `concept_hash("1", obj) != concept_hash("2", obj)`;
  - `a_requirement_on_an_out_of_bundle_component_is_dropped`;
  - `typed_relation_players_match_the_schema` (live) stays green with the new shapes.
- [ ] **Step 2: Live tests in `cyber_stix_sync_test.rs`** (a fixture of a technique, a strategy with a `detects` SRO, two analytics on Windows and Linux, two components and log-source refs):
  - `stix_sync_models_strategies_analytics_and_requirements`: counts of each new relation, and the attribute values;
  - `a_reimport_replaces_requirements`: re-sync with one log-source ref removed from an analytic; that requirement is gone, and the others are unchanged;
  - `a_reimport_without_an_analytic_leaves_no_requirement`: re-sync with one analytic removed from the bundle and its strategy's refs. No `analytic-requirement` or `strategy-analytic` is left without its analytic player;
  - `stix_resync_is_stable`: a second identical sync leaves every new relation count unchanged;
  - `a_database_synced_by_the_previous_adapter_is_retyped`: seed by syncing the fixture with strategy and analytic forced to `cyber-object` (build the manifest, then override `type_hint` and `file_hash`), then sync with the real adapter. Every strategy and analytic is retyped, and the new relations exist.

  Update the existing `requires-evidence` / `evidence-of` assertions to the new shape, and the reconciliation rig's STIX fixture to the current shape.
- [ ] **Step 3: `cyber_detect_test.rs`.** Move its seed to the current shape. Remove the `blinded` assertion from `detect_bridge_claims_disposes_and_reports`, because Task 5 owns the new `blinded` suite. Put the seed in `common/attack.rs`; `cyber_detect_test` uses it.
- [ ] **Step 4: Run and expect the new tests to fail.**
  - Run: `cargo nextest run -p cyber-gecko --no-default-features` and `cargo nextest run -p gecko-bin --no-default-features --features cyber --test cyber_stix_sync_test --test-threads 1`
- [ ] **Step 5: Implement the schema, adapter and replace-then-insert post-sync writes.**
  - **Replace, then insert.** For `strategy-analytic` and `analytic-requirement`:
    - one op deletes every such relation whose strategy or analytic id is in this manifest's set;
    - one op deletes every such relation left without its `analytic` player, or its `strategy` player for `strategy-analytic`, because GC of a removed analytic leaves those behind;
    - then the insert ops run.

    `strategy-detects` stays insert-only, under `not {}`.
  - **Docs.** Every new type and role gets an `@doc`.
- [ ] **Step 6: Run cyber unit tests, `cyber_stix_sync_test`, `cyber_detect_test` and `belief_tier_reconciliation_test`; all pass.**
- [ ] **Step 7: Commit** `feat(cyber): model ATT&CK detection strategies, analytics and log-source requirements`.

### Task 4: The sensor file and `gecko sync-sensors`

**Files:**
- Create: `extensions/cyber-gecko/src/sensors.rs` (`pub mod sensors` in `lib.rs`; `serde_yaml` added to cyber's `Cargo.toml` from the workspace)
- Modify: `extensions/cyber-gecko/schema/cyber_schema.tql`:
  - `monitored-environment` owns `platform @card(1..)`;
  - `sensor` owns `platform @card(1..)`, `log-source-name @card(0..)`, `log-source-key @card(0..)`, `covered-component-name @card(0..)` and `covered-component-key @card(0..)`;
  - new string attributes `covered-component-name` and `covered-component-key`.
- Modify: `gecko-bin/src/main.rs`: a `#[cfg(feature = "cyber")] SyncSensors { file: PathBuf, #[arg(long)] bundle_name: Option<String> }` arm. It refuses unless the global `--database` is given.
- Test: unit tests in `sensors.rs`; create `gecko-bin/tests/cyber_sensors_sync_test.rs`.

**Interfaces:**
- Consumes: Task 3's `strategy-detects`, `analytic`, `analytic-requirement`, `log-source-key` and `component-key`.
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
  - `to_okf_writes_every_field`: with bundle name `sensors`, ids `sensors/environment` and `sensors/sensor/windows-edr` (with bundle `lab`, `lab/environment`), type hints `monitored-environment` and `sensor`, and the platform, log-source-name, key (`" WinEventLog:Security"` gives name `WinEventLog:Security`, key `wineventlog:security`) and component name and key attributes.
- [ ] **Step 2: Live tests in `cyber_sensors_sync_test.rs`**:
  - **Setup.** Seed with `common::attack::sync_fixture`; apply schemas with `apply_schema`, and sync with `sync_bundle`. `sync_manifest` is private to the binary.
  - `refuses_a_database_without_attack`: the current schema is applied with no ATT&CK data, and `check_against_graph` errors with `sync the ATT&CK bundle into this database first`;
  - `refuses_every_unmatched_or_ambiguous_component_at_once`: `check_against_graph` errors, and both bad names appear in the one message;
  - `sync_twice_changes_nothing`;
  - `notices_name_unused_log_sources_and_platforms`: `windows`, an unknown log source, and a declared platform no analytic uses all appear;
  - `a_removed_component_leaves_the_sensor`: a re-sync without a component leaves no `covered-component-key` for it.
- [ ] **Step 3: Run, and expect failure.**
  - Run: `cargo nextest run -p cyber-gecko --no-default-features sensors` and `cargo nextest run -p gecko-bin --no-default-features --features cyber --test cyber_sensors_sync_test --test-threads 1`
- [ ] **Step 4: Implement.**
  - **The CLI arm** runs, in order:
    1. refuse if the database does not exist (`database <db> does not exist; sync the ATT&CK bundle into it first`), and do not create it;
    2. `parse`;
    3. apply all schemas, exactly as `sync_manifest` does before syncing, so the new types exist on an upgraded database;
    4. `check_against_graph`;
    5. `sync_manifest` with no post-sync writes;
    6. print `notices`.
  - **Graph handle.** It uses the same read-only graph handle construction the cyber bridge uses.
  - **Tests.** The tests drive the library functions and `sync_bundle`, not the binary.
- [ ] **Step 5: Run; pass.**
- [ ] **Step 6: Commit** `feat(cyber): a sensor file declares platforms and sensors, synced by gecko sync-sensors`.

### Task 5: Coverage functions, the `blinded` report, and removal of the old model

**Files:**
- Modify: `extensions/cyber-gecko/schema/cyber_functions.tql`:
  - add `analytic-gaps($a: analytic) -> { analytic-requirement }`;
  - add `claim-platforms() -> { concept, attack-pattern, string, string }`, giving (logic, technique, platform, status).
    - Returning an attribute variable under a declared `string` fails TypeDB inference with `[FIN4]`. Bind values first, for example `let $pv = $p;`, and assign the status with `let $s = "blind";` in each `or` branch. Both forms were checked against TypeDB 3.15.
  - add `requirement-covered` if kept as a function;
  - remove `blinded-detections()`.
- Modify: `extensions/cyber-gecko/schema/cyber_schema.tql`: remove `evidence-of`, `requires-evidence`, `sensor-coverage` and their `plays`, and rewrite the `@doc`s of `data-source`, `data-component` and `sensor`.
- Modify: `extensions/cyber-gecko/src/lib.rs` and `migrations.rs`:
  - `schema_migrations()` returns Task 1's `DETECTION_MODEL_MIGRATIONS` plus one `redefine` entry per rewritten `@doc`;
  - Task 1's probe test then runs against the full list.
- Modify: `extensions/cyber-gecko/src/detect/mod.rs`: `blinded` and the module doc.
- Test: `gecko-bin/tests/cyber_detect_test.rs`, `gecko-bin/tests/schema_init_test.rs`.

**Interfaces:**
- Consumes:
  - Task 3's types;
  - Task 4's `monitored-environment` / `sensor` attributes;
  - `covered-component-key` matched against `data-component`'s `component-key`.
- Produces: the `blinded` JSON exactly as spec §6, with these fields:
  - `blinded_detections[].concept_id`, `.technique` (the attack-id, or `null`), `.technique_concept_id`, `.unassessed_platforms` (sorted);
  - `.blind_platforms[]` with `platform`, `analytic` (the attack-id), `no_log_sources`, and `missing[]`;
  - each `missing[]` entry has `log_source` (the display name) and `data_components` (sorted names);
  - a top-level `stale_sensor_components[]` with `sensor` and `component`.

  Errors: `no sensor file synced` and `no current ATT&CK detection data`.

- [ ] **Step 1: Wire the migrations** (Task 1) into `schema_migrations()` and rerun `old_cyber_detection_types_migrate_away`. It must stay green with the `@doc` redefines added.
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
  - `claim_on_retired_technique_is_unassessed` (also asserts `technique: null` and `technique_concept_id`)
  - `claim_on_deprecated_technique_is_unassessed`
  - `output_order_is_deterministic` (two claims, two platforms, several log sources)
  - `missing_groups_log_source_spellings_by_key`
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

### Task 6: Real-bundle check, docs, full verification

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
