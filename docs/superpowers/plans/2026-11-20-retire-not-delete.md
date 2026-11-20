# Retire, Not Delete (O6) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A sync that drops a concept a belief edge references retires it (keeps the entity and the attributes its type requires, strips the rest, stamps `retired-at`) instead of refusing.

**Architecture:** `sync_bundle` step 5 partitions its GC-delete candidates by `belief_edge_references`: referenced ones go to a new `retire_concepts`, the rest to `delete_concepts_gc` as today. Which attributes a type requires comes from a pure parser over the server's canonical schema text, because TypeDB 3.15 cannot query annotations. Retirement touches no belief-tier relation, so the fingerprint guard sees nothing. `gecko run` reads `retired-at` and refuses.

**Tech Stack:** Rust, TypeDB 3.15 (typedb-driver 3.15.0), TypeQL `given` parameterized queries.

**Spec:** `docs/superpowers/specs/2026-11-20-retire-not-delete-design.md`

## Global Constraints

- Branch `feat/retire-not-delete`, off `main` (which has #63, #65 and #66). Line numbers below are from before the rebase; locate code by name.
- All TQL values go through `given` rows; any interpolated type label must first pass `crate::db::validate_schema_label`.
- Comments and `@doc` strings are greenfield (describe what the code does now; no "used to", no plan tags like O6).
- Never `#[allow(dead_code)]`. Never default a missing/invalid field to success.
- Integration tests need TypeDB on `localhost:1729`; run them with `-- --test-threads=1`. Never touch the database named `gecko`.
- Before each commit: `cargo fmt --all`, `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`, and the same with `--features cyber`.
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Exact names: attribute `retired-at` (datetime); field `SyncResult.concepts_retired: usize`; field `StoredConcept.retired: bool`; run error text ``concept `<id>` is retired (since <retired-at>)``; CLI line `  Concepts retired:  <n>`.

## Review Focus

1. A live file that still links (markdown link or hierarchy) to the retired concept's old path must not recreate an incoming `concept-link`/`hierarchy` to it — `linker.rs` emits links to any resolved path, and a retired concept keeps its containment. `create_links`/`create_hierarchy` already skip rows whose ends are not both manifest IDs (`bundle_ids`, from #65); the test pins that this keeps holding. Pinned in Task 2 (`retired_concept_gets_no_new_links_from_live_files`) and by the cyber test dropping the object from the STIX JSON.
2. Re-syncing an unchanged bundle with a still-referenced retired concept must report `concepts_retired == 0`, `concepts_deleted == 0`, and keep the first `retired-at`. Pinned in Task 2.
3. A concept dropped by bundle A but still contained by bundle B is released, not retired, even if a belief references it. Pinned in Task 2.
4. Retiring a concept whose type requires attributes (cyber `observable`: `ioc-value`/`ioc-type @card(1)`) commits and keeps them. Pinned in Task 3.
5. `gecko run` on a retired concept names it and its `retired-at`, and fails before the "no executable program" message. Pinned in Task 3.

---

### Task 1: Required attributes from the schema text

**Files:**
- Create: `core/gecko-engine/src/db/required.rs` (declare `pub mod required;` in `core/gecko-engine/src/db/mod.rs`)
- Modify: `core/gecko-engine/src/db/router.rs` (new method)

**Interfaces:**
- Produces: `pub fn required_attributes(schema: &str) -> Result<HashMap<String, BTreeSet<String>>, DbError>` — for every entity type in the text, the attribute labels it must own (min cardinality ≥ 1, or `@key`), including those inherited through `sub`, with a subtype's own `owns X @card(..)` overriding its supertype's for `X`; `pub async fn TypeDbRouter::schema_text(&self) -> Result<String, DbError>` (wraps the driver's `Database::schema()` for the router's database).

- [ ] **Step 1: Write the failing unit tests** in `required.rs`, over the server's canonical form (captured from TypeDB 3.15 `database schema`):

```text
define
attribute a1,
 value string;
attribute a2 @doc("x"),
 value string;
entity c @doc("y"),
  owns k @key,
  owns a1 @card(1..1),
  owns a2 @card(0..1);
entity o,
  sub c,
  owns a2 @card(1..3);
entity p,
  sub o,
  owns a1 @card(0..2);
```

  `required_attributes_reads_cards_keys_and_inheritance`: `c → {k, a1}`, `o → {k, a1, a2}`, `p → {k, a2}` (p overrides a1 to optional). `required_attributes_ignores_relations_and_functions`: a `relation r, relates x, owns a1 @card(1..1);` and a `fun` block add no entry and do not error. `required_attributes_rejects_an_unknown_supertype`: `entity q, sub missing;` → `Err(DbError::Schema(_))` naming `missing`.

- [ ] **Step 2: Run** `cargo test -p gecko-engine --no-default-features required` — expected FAIL (missing function).
- [ ] **Step 3: Implement** `required_attributes` (split on `;` at statement level outside quoted strings, take statements starting `entity `, read the label, `sub`, and each `owns <attr> <annotations>`; `@key` or `@card(n..)`/`@card(n)` with n ≥ 1 is required; resolve inheritance top-down with overrides) and `schema_text`.
- [ ] **Step 4: Run** the same command — expected PASS. Add one live test in `gecko-bin/tests/bundle_sync_test.rs`: `required_attributes_of_the_live_cyber_schema` (core+mem+cyber applied, `required_attributes(&db.schema_text().await?)`) gives `observable ⊇ {concept-id, ioc-value, ioc-type}` and `attack-pattern ⊇ {concept-id}` — run under `--features cyber`, or put it in `cyber_stix_sync_test.rs` if `bundle_sync_test.rs` is not cyber-gated.
- [ ] **Step 5: Commit** — `feat(db): read the attributes each entity type requires from the schema text`

### Task 2: Retire belief-referenced concepts in `sync_bundle`

**Files:**
- Modify: `core/gecko-engine/schema/core_schema.tql` (attribute block ~line 18; `entity concept` ~line 63)
- Modify: `core/gecko-engine/src/syncer/bundle.rs` (module doc :28-32, `SyncResult` :54, `StoredConcept` :69, `sync_bundle` :188-336 incl. step comment :261, `fetch_stored_concepts` :420, `delete_concepts_gc` doc :561-567, `create_hierarchy`, `create_links`, unit helper `stored()` :1093; new `retire_concepts` next to `delete_concepts_gc`)
- Modify: `core/gecko-engine/src/db/router.rs` (remove `BeliefEdgeReferencesRemovedConcept` :56 and its message)
- Modify: `gecko-bin/src/main.rs` (`print_sync_result` :709-718; `sync-stix --bundle-name` help :120-126)
- Test: `gecko-bin/tests/bundle_sync_test.rs` (replace `gc_of_a_concept_a_belief_edge_references_is_refused_before_any_delete` :362)

**Interfaces:**
- Consumes: Task 1's `required_attributes`, `TypeDbRouter::schema_text`.
- Produces: `SyncResult.concepts_retired: usize` (newly retired this sync); `StoredConcept.retired: bool`; `async fn retire_concepts(tx: &Transaction, ids: &[&str], surface: &BeliefTierSurface, required: &HashMap<String, BTreeSet<String>>, at: DateTime<Utc>) -> Result<(), DbError>`; schema attribute `retired-at`.

- [ ] **Step 1: Schema.** Add `attribute retired-at @doc("When a sync retired this concept: its file left the bundle while a belief edge still referenced it."), value datetime;` and `owns retired-at @card(0..1)` on `entity concept`.

- [ ] **Step 2: Write the failing tests** in `bundle_sync_test.rs`. Add a helper `async fn synced_with_referenced(db: &mut TypeDbRouter, dir: &Path) -> (OkfBundle /*full*/, String /*referenced id*/, OkfBundle /*dropped*/)`: a bundle of **two** concepts (write a second markdown file next to `create_test_bundle`'s, linking to the first), synced; insert the `mem/bel/ref` belief + `source-link` to the first concept (existing setup); `dropped` removes the first concept and its links/hierarchy/citations. The surviving concept is `dropped.concepts[0]`.

  `gc_of_a_concept_a_belief_edge_references_retires_it`:
  - `sync_bundle(dropped)` is `Ok(r)` with `r.concepts_retired == 1`, `r.concepts_deleted == 0`.
  - the referenced concept exists, has exactly one `retired-at`, owns no attribute besides `concept-id` and `retired-at`, is still in `containment` with the bundle, plays no `hierarchy`/`concept-link`/`citation`.
  - `match $l isa source-link;` count is 1 and its `origin` is the referenced concept.
  - re-sync `dropped`: `concepts_retired == 0`, `concepts_deleted == 0`, `retired-at` value equal to the first.

  `restoring_a_retired_concepts_file_unretires_it`: after retiring, `sync_bundle(full)` is `Ok` with `concepts_updated == 1`; no `retired-at`; `file-hash` equals the full manifest's; the surviving concept's link to it exists again.

  `a_retired_concept_no_longer_referenced_is_deleted`: after retiring, delete the `source-link` and the belief in a raw write tx; `sync_bundle(dropped)` gives `concepts_deleted == 1`, `concepts_retired == 0`, and the id is gone.

  `retired_concept_gets_no_new_links_from_live_files`: after retiring, sync a copy of `dropped` with a `links` entry from `dropped.concepts[0]` to the referenced id and a `hierarchy` entry with the referenced id as child; afterwards no `concept-link` or `hierarchy` links the retired concept.

  `a_dropped_concept_another_bundle_contains_is_released_not_retired`: after the helper, raw-insert a second `bundle` (`has bundle-name "beta"`) and a `containment` of the referenced concept in it (pattern: `multi_bundle_test.rs:366-379`); `sync_bundle(dropped)` gives `concepts_released == 1`, `concepts_retired == 0`, no `retired-at`.

- [ ] **Step 3: Run** `cargo test -p gecko-bin --test bundle_sync_test --no-default-features -- --test-threads=1` — expected: compile error on `concepts_retired`.

- [ ] **Step 4: Implement.**
  - `fetch_stored_concepts`: fetch `"retired": $c.retired-at`; `entry.retired = true` only when the value is non-null (as `hash` is read at :450). `stored()` unit helper sets `retired: false`.
  - `sync_bundle`: read `required_attributes(&db.schema_text().await?)` before `begin_write`. Step 5: partition `gc_delete` by membership in `belief_edge_references`; `retire_concepts(&tx, <referenced ids not stored.retired>, &surface, &required, Utc::now())`; `delete_concepts_gc(&tx, <unreferenced ids>)`. Already-retired referenced concepts get no writes. `concepts_retired` = newly retired; add `retired` to the completion `info!`.
  - `retire_concepts`: read each concept's exact type (`isa! $t`, `label($t)`); delete every attribute except `concept-id` and the labels `required[type]` names (validate each label; one parameterized delete per concept type, excluding those labels with `not { $a isa <label>; }` clauses); read every relation each concept plays (the `isa! $rt, links ($c); let $rl = label($rt)` shape of `belief_edge_references`) and delete each whose label is neither `containment` nor in `surface.relations`, one delete per distinct label (`$r isa! <label>, links ($c); delete $r;`); then insert `has retired-at == $at` (`dt_entry`). A concept type missing from `required` is a `DbError::Schema` naming it.
  - Remove `DbError::BeliefEdgeReferencesRemovedConcept` and its message. Rewrite for the retire behaviour: the module doc's last sentence (:31-32), `sync_bundle`'s doc (:196-205), the step-5 comment (:261), `delete_concepts_gc`'s doc where it mentions belief edges, and the `sync-stix --bundle-name` help ("…a record a belief edge references is retired rather than deleted").
  - `print_sync_result`: add `  Concepts retired:  {}` after the released line.

- [ ] **Step 5: Run** `cargo test -p gecko-bin --test bundle_sync_test --test multi_bundle_test --test no_drag_test --no-default-features -- --test-threads=1` and `cargo test -p gecko-engine --no-default-features` — expected all pass.

- [ ] **Step 6: Commit** — `feat(sync): retire a belief-referenced concept instead of refusing the sync`

### Task 3: Cyber retirement, `gecko run` refusal, reconciliation test

**Files:**
- Modify: `gecko-bin/src/main.rs:908-993` (`fetch_playbook`)
- Test: `gecko-bin/tests/cyber_stix_sync_test.rs` (two new tests)
- Test: `gecko-bin/tests/belief_tier_reconciliation_test.rs:1069-1122` (convert `gc_of_a_concept_a_belief_edge_references_is_refused`)

**Interfaces:**
- Consumes: Task 2's `retired-at`, `SyncResult.concepts_retired`, CLI line `  Concepts retired:  <n>`.

- [ ] **Step 1: Write the failing tests.**

  `cyber_stix_sync_test.rs::dropping_a_claimed_technique_retires_it_and_its_typed_relations`: apply core+mem+cyber (`apply_cyber_schema`), sync `to_okf(BUNDLE)` with `cyber_post_sync`. Find the live T1059 attack-pattern's concept-id. Insert a `mem/bel/t1059` belief + `source-link` (as in `bundle_sync_test`). Assert it plays ≥ 1 relation besides `containment`/`source-link`. Remove the T1059 object from the STIX JSON (parse `BUNDLE` with `serde_json`, drop the object by its STIX id, keep the SROs that reference it) and re-run `to_okf` + `cyber_post_sync` + `sync_bundle`. Assert `concepts_retired == 1`; the concept has `retired-at` and no `attack-id`; the only relations it plays are one `containment` and one `source-link`.

  `retiring_a_claimed_observable_keeps_its_required_attributes`: same shape with the `1.1.1.1` observable; the re-sync is `Ok` with `concepts_retired == 1`, and the concept still has `ioc-value "1.1.1.1"` and `ioc-type "ipv4"` plus `retired-at`.

  `belief_tier_reconciliation_test.rs`: rename to `gc_of_a_concept_a_belief_edge_references_retires_it`. Library sync of `without_cyber` is `Ok(r)` with `r.concepts_retired == 1`; fingerprint equals `f0`; `CYBER_PLAYBOOK` still counts 1 and has `retired-at`. Re-sync via the binary (`gecko(&rig, &db, &["sync", …])`) succeeds and its stdout contains `Concepts retired:  0`. `gecko_expect_failure(&rig, &db, &["run", CYBER_PLAYBOOK])` output contains ``concept `<CYBER_PLAYBOOK>` is retired (since`` and not `no executable program`. Fingerprint still `f0`.

- [ ] **Step 2: Run** `cargo test -p gecko-bin --test cyber_stix_sync_test --test belief_tier_reconciliation_test --no-default-features --features cyber -- --test-threads=1` — expected: the run-refusal assertion fails (`no executable program`). The cyber tests may already pass on Task 2's code; keep them.

- [ ] **Step 3: Implement** — `fetch_playbook` fetches `"retired": [ $c.retired-at ]`; when non-empty, `bail!` with ``concept `{concept_id}` is retired (since {value})`` before the code-block check.

- [ ] **Step 4: Run** the same command — expected all pass.

- [ ] **Step 5: Commit** — `feat(run): refuse a retired concept; cover cyber retirement`

### Task 4: Docs and full verification

**Files:**
- Modify: `docs/refactor/KNOWN-LIMITATIONS.md` ("Pre-GC refusal" bullet under "Sync guard")
- Modify: `docs/refactor/SPEC-W5.md` (§3.5, the implementation note's pre-GC bullet, and :631)
- Modify: `docs/refactor/OWED-WORK-LEDGER.md` (O6 row, :48, :164)
- Modify: `docs/superpowers/specs/2026-11-20-multi-bundle-databases-design.md:44`
- Modify: `CLAUDE.md:51`, `extensions/README.md:166`

- [ ] **Step 1:** KNOWN-LIMITATIONS: replace the "Pre-GC refusal" bullet with **Retired concepts.** — a sync that drops a concept a belief-tier relation references retires it (attributes beyond its ID and those its type requires removed, non-belief relations removed, containment kept, `retired-at` set); `gecko run` refuses it; restoring its file revives it; it is deleted once unreferenced. Readers learn of a retirement only from the concept itself; recall does not flag beliefs about it. A retired ID stays contained by its bundle, so another bundle syncing that ID is refused as a collision while the belief exists. An existing database needs `schema-init` before its first sync.
- [ ] **Step 2:** SPEC-W5 §3.5: add "Retirement — decided by the user on 2026-11-20: a referenced GC candidate is retired instead of refusing the sync (design `docs/superpowers/specs/2026-11-20-retire-not-delete-design.md`)"; rewrite the implementation note's pre-GC bullet and :631 to match. Ledger: O6 ✅ with the date and design path; fix :48 and :164. Multi-bundle design :44, CLAUDE.md:51 and extensions/README.md:166: describe retirement, not a refusal. In `extensions/README.md`, add one line: a retired concept keeps the attributes its type requires, so a required attribute on a concept subtype is kept across retirement.
- [ ] **Step 3:** Verify: `grep -rnE "BeliefEdgeReferencesRemovedConcept|refuses to GC|is refused instead|import is refused" --exclude-dir=target --exclude-dir=.git --exclude-dir=.worktrees .` prints only the dated history in `docs/superpowers/specs/2026-11-20-belief-tier-guard-and-api-boundary-design.md` and this plan/spec.
- [ ] **Step 4:** Full suite: `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --test-threads 1`, both clippy runs, `cargo fmt --all -- --check`. Expected: green.
- [ ] **Step 5: Commit** — `docs: retirement replaces the pre-GC refusal`
