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

**Interfaces:**
- Produces: a probe report in the task report file, naming the exact TQL form that works for each item below. Later tasks use those forms verbatim.

- [ ] **Step 1: Write the probe.** On a throwaway database with a minimal local schema, assert each of:
  1. **Additive `@card(1)`.** `define relation r, relates a @card(1);` on a bare `relates a` commits when data conforms, and fails at commit when one `r` lacks `a`.
  2. **Specializing an inherited `owns`.** `define b owns x @card(1);` where `b sub m` and `m owns x` is accepted, and refuses a `b` without `x`.
  3. **Tightening an explicit card.** Replacing an explicit `owns y @card(0..1)` with `@card(1)` by `redefine` works through `apply_schema_with_migrations`.
  4. **Ranges.** `@range(0.0..1.0)` on an `owns` of a `double` attribute is accepted, and refuses 1.5.
  5. **The registry.** It accepts a mem migration entry of form 3. Run this through `BeliefEdgeRegistry::from_extensions` the way `schema_init_test::old_cyber_detection_types_migrate_away` does.
  6. **Error on violating data.** Applying form 1 to a database already holding a violating row returns an error, and the message names the type.
- [ ] **Step 2: Run** `cargo nextest run -p mem-gecko --no-default-features --test schema_migration_test --test-threads 1`. If a form is refused, find the working one. If none works for an item, stop and report BLOCKED with the errors.
- [ ] **Step 3: Commit** `test(schema): probe the forms the schema tightening uses`.

### Task 2: mem's required roles

**Files:**
- Modify: `core/mem-gecko/schema/mem_types.tql`. These relations get `@card(1)` on each player role, or `@card(1..)` where a role holds several players, decided from the writers in `core/mem-gecko/src/tql.rs`:
  - `derivation`, `evidence`, `justification`, `support`, `attack`, `contradiction` (hub);
  - `supersession`, `source-link`, `ownership`, `rests-on`, `execution`, `consolidation-step`;
  - `scoped`, `population-member`, `resolves`, `surfaced`, `informs-synthesis`;
  - `divergence-precedence` (`prevailing`, `overridden`).
- Modify: `docs/refactor/OWED-WORK-LEDGER.md`, adding a D-G erratum: SPEC-W2-W3 §2.1's `divergence-precedence` now requires `prevailing` and `overridden` (`@card(1)`).
- Test: `core/mem-gecko/tests/schema_acceptance_test.rs`.

**Interfaces:**
- Consumes: Task 1's form 1.
- Produces: a comment-free list in the task report of each role and the card it got, which Task 6 checks.

- [ ] **Step 1: Write the failing tests.** `a_<relation>_missing_its_<role>_is_refused_at_commit`, one per relation, generated by a table-driven test. Each inserts the relation with one required role omitted and asserts the commit fails.
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
- Test: `gecko-bin/tests/bundle_sync_test.rs`, `gecko-bin/tests/multi_bundle_test.rs`.

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
  - `a_uses_missing_its_user_is_refused_at_commit` (raw insert).
- [ ] **Step 2: Run, and expect both to fail.**
- [ ] **Step 3: Implement the sweep, then the annotations.** `sighting-of` has no writer; leave it at its current card.
- [ ] **Step 4: Run all cyber tests, `belief_tier_reconciliation_test`, and the opt-in `GECKO_ATTACK_BUNDLE=<v20.1 path> … stix_full_bundle_resync_is_stable`.** All pass. Record the re-sync timing in the report; it must stay under 30 s of transaction time.
- [ ] **Step 5: Commit** `feat(cyber): STIX and detection relation roles are required; re-sync sweeps one-player relations`.

### Task 5: Required belief fields and value ranges

**Files:**
- Modify: `core/mem-gecko/schema/mem_types.tql`:
  - `belief owns valid-from @card(1)`, through a `MEM_MIGRATIONS` entry in `core/mem-gecko/src/lib.rs` (form 3);
  - `belief owns salience @card(1)`, `owns base-activation @card(1)`, `owns last-access @card(1)` (form 2);
  - `@range(0.0..1.0)` on `belief owns confidence`, `derivation owns confidence` and `prediction-resolution owns predicted-probability`;
  - `@range(-1.0..1.0)` on `surfaced owns similarity-score`;
  - `@range(0..)` on `access-count`, `corroboration-count` and `candidate-count`, each at its `owns`.
- Test: `core/mem-gecko/tests/schema_acceptance_test.rs`, `core/mem-gecko/tests/schema_migration_test.rs`.

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
- Create: `gecko-bin/tests/fixtures/schemas_<base7>/`, frozen copies of `core_schema.tql`, `mem_types.tql`, `mem_functions.tql` and the cyber schema plus functions at this plan's base commit.
- Test: `gecko-bin/tests/schema_init_test.rs`, adding `tightened_schema_applies_to_a_populated_database`.
- Modify: `docs/refactor/OWED-WORK-LEDGER.md` (the TypeDB review row: pass 2 ✅), `docs/refactor/KNOWN-LIMITATIONS.md`.

- [ ] **Step 1: Write the upgrade test.**
  - Apply the frozen schemas to a throwaway database.
  - Populate it through today's writers:
    - an OKF bundle sync with links and citations;
    - the shared ATT&CK fixture (`#[path = "common/attack.rs"] mod attack;`) and a sensor file;
    - every `MemWriter` write path the reconciliation rig drives: assert, derive, contest, supersede, rests_on, record and resolve a prediction, recall then derive, populations, and a daemon tick.
  - Apply the current schemas with every migration, and assert the commit succeeds.
  - Assert one constraint from each of Tasks 2–5 now holds on the populated data.
- [ ] **Step 2: Run it. Expect PASS.** If it fails, the failing constraint names a writer that produces violating data. Fix the writer, or narrow the constraint with a recorded ruling.
- [ ] **Step 3: Docs.**
  - KNOWN-LIMITATIONS: the schema enforces required roles, ownership and recall fields. An existing database holding violating rows fails `schema-init` with TypeDB's constraint error naming the type.
- [ ] **Step 4: Full verification.** Run fmt, both clippy runs, and `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --test-threads 1`. All pass.
- [ ] **Step 5: Commit** `test(schema): the tightened schema applies to a database populated by every writer; docs`.
