# Retire, Not Delete (O6) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A sync that drops a concept a belief edge references retires it (keeps the entity, strips its content, stamps `retired-at`) instead of refusing.

**Architecture:** `sync_bundle` step 5 partitions its GC-delete candidates by `belief_edge_references`: referenced ones go to a new `retire_concepts`, the rest to `delete_concepts_gc` as today. Retirement touches no belief-tier relation, so the fingerprint guard sees nothing. `gecko run` reads `retired-at` and refuses.

**Tech Stack:** Rust, TypeDB 3.15 (typedb-driver 3.15.0), TypeQL `given` parameterized queries.

**Spec:** `docs/superpowers/specs/2026-11-20-retire-not-delete-design.md`

## Global Constraints

- Branch `feat/retire-not-delete`, stacked on `feat/belief-tier-guard` (PR #63).
- All TQL values go through `given` rows; any interpolated type label must first pass `crate::db::validate_schema_label`.
- Comments and `@doc` strings are greenfield (describe what the code does now; no "used to", no plan tags like O6).
- Never `#[allow(dead_code)]`. Never default a missing/invalid field to success.
- Integration tests need TypeDB on `localhost:1729`; run them with `-- --test-threads=1`. Never touch the database named `gecko`.
- Before each commit: `cargo fmt --all`, `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`, and the same with `--features cyber`.
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Exact names: attribute `retired-at` (datetime); field `SyncResult.concepts_retired: usize`; field `StoredConcept.retired: bool`; run error text ``concept `<id>` is retired (since <retired-at>)``; CLI line `  Concepts retired:  <n>`.

## Review Focus

1. A live file that still links (markdown link or hierarchy) to the retired concept's old path must not recreate an incoming `concept-link`/`hierarchy` to it — `linker.rs` emits links to any resolved path, and on this branch `create_links`/`create_hierarchy` match ends by containment, which a retired concept keeps. Pinned in Task 1 (`retired_concept_gets_no_new_links_from_live_files`).
2. Re-syncing an unchanged bundle with a still-referenced retired concept must report `concepts_retired == 0`, `concepts_deleted == 0`, and keep the first `retired-at`. Pinned in Task 1.
3. A concept dropped by bundle A but still contained by bundle B is released, not retired, even if a belief references it. Pinned in Task 1.
4. A retired concept's cyber typed relations (non-belief-tier, written by `cyber_post_sync`) are removed, while its belief-tier relations stay. Pinned in Task 2.
5. `gecko run` on a retired concept names it and its `retired-at`, and fails before the "no executable program" message. Pinned in Task 2.

---

### Task 1: Retire belief-referenced concepts in `sync_bundle`

**Files:**
- Modify: `core/gecko-engine/schema/core_schema.tql` (attribute block ~line 18; `entity concept` ~line 63)
- Modify: `core/gecko-engine/src/syncer/bundle.rs` (`SyncResult` :54, `StoredConcept` :69, `sync_bundle` :188-336, `fetch_stored_concepts` :420, `create_hierarchy`, `create_links`; new `retire_concepts` next to `delete_concepts_gc` :568)
- Modify: `core/gecko-engine/src/db/router.rs` (remove `BeliefEdgeReferencesRemovedConcept` :56 and its `Display` arm)
- Modify: `gecko-bin/src/main.rs:709-718` (`print_sync_result`)
- Test: `gecko-bin/tests/bundle_sync_test.rs` (replace `gc_of_a_concept_a_belief_edge_references_is_refused_before_any_delete` :362)

**Interfaces:**
- Produces: `SyncResult.concepts_retired: usize` (newly retired this sync); `StoredConcept.retired: bool` (concept has `retired-at`); `async fn retire_concepts(tx: &Transaction, ids: &[&str], surface: &BeliefTierSurface, at: DateTime<Utc>) -> Result<(), DbError>`; schema attribute `retired-at`.

- [ ] **Step 1: Schema.** Add `attribute retired-at value datetime @doc("When a sync retired this concept: its file left the bundle while a belief edge still referenced it.");` (match the file's `@doc` placement for attributes) and `owns retired-at @card(0..1)` on `entity concept`.

- [ ] **Step 2: Write the failing tests** in `bundle_sync_test.rs`, reusing the existing test's setup (sync `create_test_bundle`, insert `mem/bel/ref` belief + `source-link` to `bundle.concepts[0]`, build `dropped`). Extract that setup into a helper `async fn synced_with_referenced(db) -> (OkfBundle, String /*referenced id*/, OkfBundle /*dropped*/)` so the tests share it.

  `gc_of_a_concept_a_belief_edge_references_retires_it`:
  - `sync_bundle(dropped)` is `Ok(r)` with `r.concepts_retired == 1`, `r.concepts_deleted == 0`.
  - the referenced concept exists, has exactly one `retired-at`, has no `file-hash`/`body`/`code-block` (any attribute besides `concept-id` and `retired-at` counts 0), is still in `containment` with the bundle, plays no `hierarchy`/`concept-link`/`citation`.
  - `match $l isa source-link;` count is 1 and its `origin` is the referenced concept.
  - re-sync `dropped`: `concepts_retired == 0`, `concepts_deleted == 0`, `retired-at` value equal to the first one.

  `restoring_a_retired_concepts_file_unretires_it`: after retiring, `sync_bundle(bundle)` (the original) is `Ok` with `concepts_updated == 1`; the concept has no `retired-at` and its `file-hash` equals `bundle.concepts[0].file_hash`; its outgoing links from the manifest exist again.

  `a_retired_concept_no_longer_referenced_is_deleted`: after retiring, delete the `source-link` and the belief in a raw write tx; `sync_bundle(dropped)` gives `concepts_deleted == 1`, `concepts_retired == 0`, and the concept id is gone.

  `retired_concept_gets_no_new_links_from_live_files`: after retiring, sync a copy of `dropped` with one `links` entry added from `dropped.concepts[0]` to the referenced id and one `hierarchy` entry with the referenced id as child; afterwards no `concept-link` or `hierarchy` links the retired concept.

  `a_dropped_concept_another_bundle_contains_is_released_not_retired`: put the referenced concept in a second bundle too (use `multi_bundle_test.rs`'s pattern: a second manifest with a different `bundle_name` containing the same concept, synced with the same database), then `sync_bundle(dropped)` gives `concepts_released == 1`, `concepts_retired == 0`, and no `retired-at`.

- [ ] **Step 3: Run them to see them fail**

  Run: `cargo test -p gecko-bin --test bundle_sync_test --no-default-features -- --test-threads=1`
  Expected: compile error on `concepts_retired` (or, once stubbed, the sync still returning the refusal).

- [ ] **Step 4: Implement.**
  - `fetch_stored_concepts`: add `"retired": $c.retired-at` to the fetch; set `entry.retired = true` when present.
  - `sync_bundle` step 5: `let references = belief_edge_references(...)`; partition `gc_delete` into `retire` (id appears in `references`) and `delete`. `retire_concepts(&tx, <retire ids not already stored.retired>, &surface, Utc::now())`; `delete_concepts_gc(&tx, <delete ids>)`. Already-retired referenced concepts get no writes. `concepts_retired` = count of newly retired; `concepts_deleted` = the delete partition. Add `retired` to the completion `info!`.
  - `retire_concepts`: remove every attribute except `concept-id` (same query as `clear_concept_content`'s first); then read every relation each concept plays (the `isa! $rt, links ($c); let $rl = label($rt)` shape of `belief_edge_references`) and delete each relation whose label is neither `containment` nor in `surface.relations`, one parameterized delete per distinct label (`validate_schema_label` on it first; `$r isa! {label}, links ($c); delete $r;`); then `insert`/`put` `has retired-at == $at` (use `dt_entry`).
  - `create_hierarchy` / `create_links`: skip rows whose two ends are not both manifest concept IDs (a `HashSet<&str>` of `manifest.concepts` IDs), so a live file's link to a retired path writes nothing. Keep the `attempted` count as the number of rows sent.
  - Remove `DbError::BeliefEdgeReferencesRemovedConcept` and its message; rewrite `sync_bundle`'s doc comment for the retire behaviour (one sentence: a GC candidate a belief-tier relation references is retired — content and non-belief relations removed, containment kept, `retired-at` set — instead of deleted).
  - `print_sync_result`: add `  Concepts retired:  {}` after the released line.

- [ ] **Step 5: Run tests to verify they pass**

  Run: `cargo test -p gecko-bin --test bundle_sync_test --test multi_bundle_test --no-default-features -- --test-threads=1`
  Expected: all pass.

- [ ] **Step 6: Commit** — `feat(sync): retire a belief-referenced concept instead of refusing the sync`

### Task 2: Cyber retirement, `gecko run` refusal, reconciliation test

**Files:**
- Modify: `gecko-bin/src/main.rs:908-993` (`fetch_playbook`)
- Test: `gecko-bin/tests/cyber_stix_sync_test.rs` (new test)
- Test: `gecko-bin/tests/belief_tier_reconciliation_test.rs:1069-1122` (convert `gc_of_a_concept_a_belief_edge_references_is_refused`)

**Interfaces:**
- Consumes: Task 1's `retired-at`, `SyncResult.concepts_retired`, CLI line `  Concepts retired:  <n>`.

- [ ] **Step 1: Write the failing tests.**

  `cyber_stix_sync_test.rs::dropping_a_claimed_technique_retires_it_and_its_typed_relations`: apply core+mem+cyber schema (`apply_cyber_schema`), sync `to_okf(BUNDLE)` with `cyber_post_sync`. Pick the T1059 attack-pattern's concept-id from the graph. Insert a `mem/bel/t1059` belief + `source-link` (as in `bundle_sync_test`). Record that it plays ≥1 relation besides `containment`/`source-link`. Re-sync with that concept removed from `manifest.concepts`/`links`/`hierarchy`/`citations` and from `typed_rels` (both ends). Assert `concepts_retired == 1`; the concept has `retired-at`, no `attack-id`; the only relations it plays are one `containment` and one `source-link`.

  `belief_tier_reconciliation_test.rs`: rename the test to `gc_of_a_concept_a_belief_edge_references_retires_it`. Library sync of `without_cyber` is `Ok(r)` with `r.concepts_retired == 1`; fingerprint equals `f0`; `CYBER_PLAYBOOK` still counts 1 and has `retired-at`. Re-sync via the binary (`gecko(&rig, &db, &["sync", …])`) succeeds and its stdout contains `Concepts retired:  0`. `gecko_expect_failure(&rig, &db, &["run", CYBER_PLAYBOOK])` output contains ``concept `<CYBER_PLAYBOOK>` is retired (since`` and not `no executable program`. Fingerprint still `f0`.

- [ ] **Step 2: Run them to see them fail**

  Run: `cargo test -p gecko-bin --test cyber_stix_sync_test --test belief_tier_reconciliation_test --no-default-features --features cyber -- --test-threads=1`
  Expected: the run-refusal assertion fails (`no executable program` instead); the cyber test passes or fails depending on Task 1 — if it passes, that is fine, keep it.

- [ ] **Step 3: Implement** — in `fetch_playbook`, add `"retired": [ $c.retired-at ]` to the fetch; when non-empty, `bail!("concept `{concept_id}` is retired (since {value})")` with the value as fetched, before the code-block check.

- [ ] **Step 4: Run tests to verify they pass** — same command; expected all pass.

- [ ] **Step 5: Commit** — `feat(run): refuse a retired concept; cover cyber retirement`

### Task 3: Docs

**Files:**
- Modify: `docs/refactor/KNOWN-LIMITATIONS.md` ("Pre-GC refusal" bullet under "Sync guard")
- Modify: `docs/refactor/SPEC-W5.md` (§3.5 and the implementation note's pre-GC bullet)
- Modify: `docs/refactor/OWED-WORK-LEDGER.md` (O6 row)
- Modify: `CLAUDE.md:51`, `extensions/README.md:166`

- [ ] **Step 1:** KNOWN-LIMITATIONS: replace the "Pre-GC refusal" bullet with **Retired concepts.** — a sync that drops a concept a belief-tier relation references retires it (content and non-belief relations removed, containment kept, `retired-at` set); `gecko run` refuses it; restoring its file revives it; it is deleted once unreferenced. Readers learn of a retirement only from the concept itself; recall does not flag beliefs about it.
- [ ] **Step 2:** SPEC-W5 §3.5: add a paragraph "Retirement — decided by the user on 2026-11-20: a referenced GC candidate is retired instead of refusing the sync (design `docs/superpowers/specs/2026-11-20-retire-not-delete-design.md`)"; update the implementation note's pre-GC bullet to match. Ledger: mark O6 ✅ with the date and the design path. CLAUDE.md:51 and extensions/README.md:166: replace "refuses to GC a concept a belief edge references" / the error name with "retires a concept a belief edge references instead of deleting it".
- [ ] **Step 3:** Verify: `grep -rn "BeliefEdgeReferencesRemovedConcept\|refuses to GC" --exclude-dir=target --exclude-dir=.git .` prints only spec/plan history files under `docs/superpowers/`.
- [ ] **Step 4:** Full suite: `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --test-threads 1`, both clippy runs, `cargo fmt --all -- --check`. Expected: green.
- [ ] **Step 5: Commit** — `docs: retirement replaces the pre-GC refusal`
