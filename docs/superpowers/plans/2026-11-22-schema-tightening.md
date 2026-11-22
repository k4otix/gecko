# Schema Tightening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make TypeDB refuse malformed data itself: relations missing a required player, beliefs missing a field recall depends on, and out-of-range scores.

**Architecture:** Constraints move from Rust into the schemas (mem, core, cyber). Additive annotations go through `define`; replacing an explicit annotation goes through a `MEM_MIGRATIONS` entry. A probe proves each form on TypeDB 3.15 first. Cyber's post-sync sweep removes one-player relations before its roles become required. A frozen-schema upgrade test proves a database populated by today's writers takes the new schema.

**Tech Stack:** Rust, TypeDB 3.15 / TypeQL, cargo nextest.

**Spec:** `docs/superpowers/specs/2026-11-22-typedb-review-fixes-design.md` (Pass 2). Start this plan from `main` after the query-correctness PR merges.

## Global Constraints

- **Schema rules:**
  - every annotation change keeps the capability's `@doc`;
  - never loosen the belief-edge registry;
  - ratified specs (SPEC-W2-W3, SPEC-W5, CONTRACT-W8) are never edited, and stale text goes to the D-G errata row in `docs/refactor/OWED-WORK-LEDGER.md`.
- **Cards on roles.** Only a bare `relates` (default `0..1`) gets `@card(1)`. A role with an explicit card is never changed, for example `source`, `evidence-item`, `origin`, `owner`, `assumption`, `claim @card(2..)` and `antecedent @card(0..)`.
- **Upgrades never fail on data today's code produces.** Today's GC leaves one-player relations behind, and `sync` and `sync-sensors` apply the schema before they sync.
  - Every relation that gains a required role therefore gets an upgrade cleanup.
  - The cleanup is an ordered `SchemaMigration` entry: `match $r isa <rel>; not { $r links (<role>: $x); }; delete $r;`.
  - It runs in the same schema transaction, before the `define` that adds the card.
  - Its `applies_when` is text the exported schema shows only before the card lands.
- **Shared test seeds.** Plan A added a seed helper. Every new required field or role is added to it, so raw seeds keep conforming.
- **Roles that keep their optional card:**
  - `prediction-resolution:resolving-episode`, which is linked after insert;
  - `divergence-precedence:exception-scope`;
  - `sensor-coverage`, which is gone;
  - every role whose current annotation is already explicit and deliberate.
- **TQL:** all TQL is parameterized; comments are greenfield; never `#[allow(dead_code)]`. The TypeDB guides are at `.claude/skills/typedb-modeling/SKILL.md` and `.claude/skills/typedb-typeql/SKILL.md`.
- **Integration tests:** TypeDB on localhost:1729, `--test-threads=1`, never the database `gecko`.
- **Before each commit:** `cargo fmt --all`; both clippy runs (`--no-default-features`, and `--no-default-features --features cyber`) with `-D warnings`.
- **Commit trailer:** commits end with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- **A bundle sync that garbage-collects a concept** another relation still points at. It must not fail at commit for any relation that gains `@card(1)`. Covered in Task 3 for core and Task 4 for cyber.
- **A belief written by every current writer path** satisfies the new required attributes and ranges. Covered in Task 6 by the reconciliation rig and the upgrade test.
- **The STIX sweep against the full v20.1 bundle.** A re-import must still fit well inside TypeDB's 300 s transaction limit. Covered in Task 4 by running the opt-in `stix_full_bundle_resync_is_stable`.
- **A schema-init on an existing database holding a violating row** fails with an error naming the constraint, not a panic or a partial apply. Covered in Task 2 as a probe assertion.
- **`@range` on `similarity-score`** must accept the real recall scores. Cosine similarity runs from -1 to 1; the stub embedder may produce exact ±1. Covered in Task 5.

---

### Task 1: Probe each schema form on TypeDB 3.15

**Files:**
- Test: `core/mem-gecko/tests/schema_migration_test.rs`, adding `tightening_forms_probe`. It stays as a regression test.
- Create: `gecko-bin/tests/fixtures/schemas_pre_tightening/`. It holds frozen copies of `core_schema.tql`, `mem_types.tql`, `mem_functions.tql`, `cyber_schema.tql` and `cyber_functions.tql`, taken with `git show $(git merge-base HEAD main):<path>` before any other edit. Task 6 uses them.

**Interfaces:**
- Produces: a probe report in the task report file, naming the exact TQL form that works for each item below. Later tasks use those forms verbatim.

- [ ] **Step 1: Write the probe.** On a throwaway database with a minimal local schema, assert each of:
  1. **Additive `@card(1)`.** `define relation r, relates a @card(1);` on a bare `relates a` commits when data conforms, and fails at commit when one `r` lacks `a`.
  2. **Specializing an inherited `owns`.** `define b owns x @card(1);` where `b sub m` and `m owns x` is accepted, and refuses a `b` without `x`.
  3. **Tightening an explicit card.** Replacing an explicit `owns y @card(0..1)` with `@card(1)` by `redefine` works through `apply_schema_with_migrations`.
  4. **Ranges.** `@range(0.0..1.0)` on an `owns` of a `double` attribute is accepted, and refuses 1.5.
  5. **The registry.** It accepts a mem migration entry of form 3. Run this through `BeliefEdgeRegistry::from_extensions` the way `schema_init_test::old_cyber_detection_types_migrate_away` does.
  6. **Error on violating data.** Applying form 1 to a database already holding a violating row returns an error, and the message names the type.
  7. **Cleanup, then define.** A cleanup entry (`match … not { links }; delete $r;`) followed by the `define` commits in one schema transaction on a database holding a one-player row.
     - Check this through mem's migrations (`MEM_MIGRATIONS`) and the registry, through cyber's (`CYBER_MIGRATIONS`), and through core's path.
     - Core has no migration list today. Find how `apply_all_schemas` applies the core schema, and add the smallest equivalent: a core migration list applied the same way.
  8. **Apply twice.** Every migration form applies once, and a second apply is a no-op.
     - `applies_when` must be unique to its target. The export prints `owns valid-from @card(0..1)` for both `belief` and `population-member`, and a repeated `redefine` errors `[REX27]`.
     - Copy each trigger text from real `database schema` output.
     - mem's check that the shipped schema contains the migration text (`core/mem-gecko/src/lib.rs:118-135`) means the shipped schema needs a standalone statement such as `belief owns valid-from @card(1);`.
- [ ] **Step 2: Run** `cargo nextest run -p mem-gecko --no-default-features --test schema_migration_test --test-threads 1`. If a form is refused, find the working one. If none works for an item, stop and report BLOCKED with the errors.
- [ ] **Step 3: Commit** `test(schema): probe the forms the schema tightening uses`.

### Task 2: mem's required roles

**Files:**
- Modify: `core/mem-gecko/src/lib.rs`, adding one cleanup entry per relation below to `MEM_MIGRATIONS` (form 7).
- Modify: `core/gecko-engine/src/syncer/bundle.rs`. `delete_concepts_gc` also deletes `population-member` rows whose member is a deleted concept. A deleted or retyped member concept otherwise leaves a one-player row that fails the new card at commit.
- Modify: the shared seed helpers (Global Constraints).
- Modify: `core/mem-gecko/schema/mem_types.tql`. Each bare player role of these relations gets `@card(1)` (Global Constraints):
  - `derivation`, `evidence`, `justification`, `support`, `attack`, `contradiction` (hub);
  - `supersession`, `source-link`, `ownership`, `rests-on`, `execution`, `consolidation-step`;
  - `scoped`, `population-member`, `resolves`, `surfaced`, `informs-synthesis`;
  - `divergence-precedence` (`prevailing`, `overridden`).
- Modify: `docs/refactor/OWED-WORK-LEDGER.md`, adding a D-G erratum: SPEC-W2-W3 §2.1's `divergence-precedence` now requires `prevailing` and `overridden` (`@card(1)`).
- Test: `core/mem-gecko/tests/schema_acceptance_test.rs`.

**Interfaces:**
- Consumes: Task 1's form 1.
- Produces: a comment-free list in the task report of each role and the card it got, which Task 6 checks.

- [ ] **Step 1: Write the failing tests:**
  - `a_<relation>_missing_its_<role>_is_refused_at_commit`, one per relation, generated by a table-driven test. Each inserts the relation with one required role omitted and asserts the commit fails.
  - `a_database_holding_a_one_player_row_upgrades`: seed a one-player `derivation` and a one-player `population-member` on the pre-change schema, then apply the current schema. It commits and the rows are gone.
  - `garbage_collecting_a_population_member_concept_commits`.
- [ ] **Step 2: Run, and expect them to fail.**
- [ ] **Step 3: Add the annotations.**
- [ ] **Step 4: Run the mem suites, `mem_recall_e2e_test`, and `belief_tier_reconciliation_test` (`--features cyber`). All pass.**
- [ ] **Step 5: Commit** `feat(mem): required relation roles are enforced by the schema`.

### Task 3: core's required roles

**Files:**
- Modify: `core/gecko-engine/schema/core_schema.tql`:
  - `containment` (container, member);
  - `hierarchy` (parent, child);
  - `okf-link` (source, target);
  - `citation`'s `citing-concept` and `cited-resource`, which need their own card.
- Modify: the core migration list from Task 1, form 7, adding one cleanup entry per relation above.
- Test: `gecko-bin/tests/bundle_sync_test.rs`, `gecko-bin/tests/multi_bundle_test.rs`, and `gecko-bin/tests/schema_init_test.rs` (`a_database_holding_a_one_player_core_relation_upgrades`).

**Interfaces:** consumes Task 1's form 1.

- [ ] **Step 1: Write the failing tests:**
  - `a_concept_link_missing_its_target_is_refused_at_commit` (raw insert);
  - `garbage_collecting_a_linked_concept_commits`: bundle B links to a concept that a re-sync of B removes, and the sync commits with no dangling link.
- [ ] **Step 2: Run, and expect the first to fail.** The second should pass before and after the change; if it fails after, GC must delete the relation first.
- [ ] **Step 3: Add the annotations.**
- [ ] **Step 4: Run `bundle_sync_test`, `multi_bundle_test`, `belief_tier_reconciliation_test` and `schema_init_test`. All pass.**
- [ ] **Step 5: Commit** `feat(core): OKF relation roles are required`.

### Task 4: cyber's required roles, after an orphan sweep

**Files:**
- Modify: `extensions/cyber-gecko/src/stix/mod.rs` (`cyber_post_sync` orphan cleanup).
- Modify: `extensions/cyber-gecko/src/migrations.rs`, adding one upgrade cleanup entry per relation below (form 7). The post-sync sweep runs after the schema is applied, so it cannot rescue an upgrade on its own.
- Modify: `extensions/cyber-gecko/schema/cyber_schema.tql`:
  - `indicates`, `uses`, `attributed-to`, `targets`, `mitigates`, `observed-as`;
  - `strategy-detects`, `strategy-analytic`, `analytic-requirement`;
  - `technique-detection` (`detecting-logic`, `detected-technique`, `detection-claim`);
  - `alert-disposition` (`dispositioned-logic`, `disposition-belief`).
- Test: `gecko-bin/tests/cyber_stix_sync_test.rs`.

**Interfaces:**
- Consumes: Task 1's form 1.
- Produces: the orphan sweep covers every relation in `typed_relation_players()`, deleting a relation that lacks either player. It runs before the inserts, as the existing strategy and analytic sweep does.

- [ ] **Step 1: Write the failing tests:**
  - `a_reimport_that_drops_a_malware_keeps_no_one_player_uses`: the `uses` count after equals the count of `uses` whose two ends are both in the new bundle;
  - `a_uses_missing_its_user_is_refused_at_commit` (raw insert);
  - `a_database_holding_a_one_player_uses_upgrades` (pre-change schema, then the current one).
- [ ] **Step 2: Run, and expect both to fail.**
- [ ] **Step 3: Implement the sweep, then the annotations.** `sighting-of` has no writer; leave it at its current card.
- [ ] **Step 4: Run all cyber tests, `belief_tier_reconciliation_test`, and the opt-in `GECKO_ATTACK_BUNDLE=<v20.1 path> … stix_full_bundle_resync_is_stable`.** All pass. Record the re-sync timing in the report; it must stay under 30 s of transaction time.
- [ ] **Step 5: Commit** `feat(cyber): STIX and detection relation roles are required; re-sync sweeps one-player relations`.

### Task 5: Required belief fields and value ranges

**Files:**
- Modify: `core/mem-gecko/schema/mem_types.tql`:
  - `belief owns valid-from @card(1)`, through a `MEM_MIGRATIONS` entry in `core/mem-gecko/src/lib.rs` (form 3). Its `applies_when` is unique to `belief`, and the shipped schema has the standalone statement (form 8);
  - `belief owns salience @card(1)`, `owns base-activation @card(1)`, `owns last-access @card(1)` (form 2);
  - `@range(0.0..1.0)` on `belief owns confidence`, `derivation owns confidence` and `prediction-resolution owns predicted-probability`;
  - `@range(-1.0..1.0)` on `surfaced owns similarity-score`;
  - `@range(0..)` on `access-count`, `corroboration-count` and `candidate-count`, each at its `owns`.
- Modify: `core/mem-gecko/src/writer/reader.rs` (~224). The similarity score `record_retrieval` writes is clamped to [-1, 1]. It is an unclamped f32 `1.0 - distance` (`core/gecko-semantic-index/src/index.rs:298`), and a value just outside the range would otherwise fail the recall.
- Modify: the shared seed helpers, adding `valid-from`, `salience`, `base-activation` and `last-access` to raw beliefs.
- Test: `core/mem-gecko/tests/schema_acceptance_test.rs`, `core/mem-gecko/tests/schema_migration_test.rs`, plus a unit test `similarity_score_is_clamped`.

**Interfaces:** consumes Task 1's forms 2, 3 and 4.

- [ ] **Step 1: Write the failing tests:**
  - `a_belief_without_valid_from_is_refused`;
  - `a_belief_without_salience_is_refused`, and the same for `base-activation` and `last-access`;
  - `a_confidence_of_1_5_is_refused`;
  - `a_negative_candidate_count_is_refused`;
  - `the_valid_from_migration_applies_once_to_an_existing_database`, using the existing frozen-fixture pattern in `schema_migration_test.rs`.
- [ ] **Step 2: Run, and expect them to fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the mem suites, `semantic_index_integration` (real similarity scores), `mem_recall_e2e_test` and `belief_tier_reconciliation_test`. All pass.**
- [ ] **Step 5: Commit** `feat(mem): beliefs require the fields recall reads, and scores stay in range`.

### Task 6: Upgrade check, docs, full verification

**Files:**
- Uses: `gecko-bin/tests/fixtures/schemas_pre_tightening/` (frozen in Task 1).
- Test: `gecko-bin/tests/schema_init_test.rs`, adding `tightened_schema_applies_to_a_populated_database`. Populate through library calls (`apply_schema` with the frozen fixtures, then `sync_bundle`, `cyber_post_sync`, `MemWriter`), not the binary, whose `sync` upgrades the schema first. The reconciliation rig's drive helpers are private to its test file; use them as the list of paths to cover, not as code to call.
- Modify: `docs/refactor/OWED-WORK-LEDGER.md` (the TypeDB review row: pass 2 ✅), `docs/refactor/KNOWN-LIMITATIONS.md`.

- [ ] **Step 1: Write the upgrade test.**
  - Apply the frozen schemas to a throwaway database.
  - Populate it through today's writers:
    - an OKF bundle sync with links and citations;
    - the shared ATT&CK fixture (`#[path = "common/attack.rs"] mod attack;`) and a sensor file;
    - every `MemWriter` write path the reconciliation rig drives: assert, derive, contest, supersede, rests_on, record and resolve a prediction, recall then derive, populations, and a daemon tick.
  - Also seed what today's GC leaves behind: a one-player `concept-link` and a one-player `uses`, created by syncing then removing a linked concept, and a one-player `population-member`.
  - Apply the current schemas with every migration, and assert the commit succeeds.
  - Assert one constraint from each of Tasks 2–5 now holds on the populated data.
- [ ] **Step 2: Run it. Expect PASS.** If it fails, the failing constraint names a writer that produces violating data. Fix the writer, or narrow the constraint with a recorded ruling.
- [ ] **Step 3: Docs.**
  - KNOWN-LIMITATIONS: the schema enforces required roles, ownership and recall fields. An existing database holding violating rows fails `schema-init` with TypeDB's constraint error naming the type.
- [ ] **Step 4: Full verification.** Run fmt, both clippy runs, and `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --test-threads 1`. All pass.
- [ ] **Step 5: Commit** `test(schema): the tightened schema applies to a database populated by every writer; docs`.
