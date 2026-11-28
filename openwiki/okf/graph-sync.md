---
type: workflow
title: Bundle Sync into TypeDB
description: How sync_bundle writes a parsed OKF bundle into TypeDB in one guarded, parameterized write transaction — content-hash planning, in-place updates, collision and retype refusals, garbage collection with release and retirement, extension writes in the same transaction, and the belief-tier fingerprint that refuses any belief change.
tags: [sync, okf, typedb, idempotency, records, belief-tier]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-28T02:42:19.865Z
sources:
  - id: openwiki-source-25eb5271b2078890188e98ad
    resource: repo://core/gecko-engine/src/syncer/bundle.rs
  - id: openwiki-source-c42ae122f8faf30c8246ac70
    resource: repo://gecko-bin/src/main.rs
generated: { by: "claude-code", at: "2026-11-28T02:42:19.865Z" }
---

# Bundle Sync into TypeDB

<!-- openwiki: broken internal link [/openwiki/okf/bundle-format-and-parser.md] link "/openwiki/okf/bundle-format-and-parser.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
`gecko sync <bundle>` parses the bundle (see [OKF Bundles and the Parser](/openwiki/okf/bundle-format-and-parser.md)) and hands it to `sync_manifest` (`gecko-bin/src/main.rs`). That function applies every schema (`apply_all_schemas`), then calls `gecko_engine::syncer::bundle::sync_bundle(db, manifest, extension_writes, mem_gecko::BELIEF_TIER_ROOT)`. The database is `--database` if given, else the bundle name. `sync-stix` and `sync-sensors` reach the same function with extension-built manifests and post-sync writes.

## Record-path guardrail

The syncer writes **records** directly through the router. It never touches the belief-write path or the semantic index, so a documentation sync is drag-free (invariants 1, 2, 8). The unit test `syncer_stays_on_the_record_path` asserts that the module names no belief or accelerator type, and `gecko-bin/tests/no_drag_test.rs` checks it live.

## The transaction

A single write transaction runs these steps; returning early drops it uncommitted.

0. Refuse an out-of-range `timeout_ms` (`InvalidTimeout`) before opening. Read the belief-tier surface rooted at `memory-item` and **fingerprint** it.
1. Read every stored concept with its hash, containing bundles and retired flag (`StoredConcept`).
2. **Collisions.** `find_collisions` refuses manifest ids held by another bundle or by no bundle (`ConceptIdCollision`).
3. **Plan.** `plan_sync` diffs manifest hashes against the concepts this bundle contains: unchanged are skipped, changed are updated, new are inserted. Orphans are split into ones this bundle owns alone and ones it shares. A stored concept whose exact type differs from its `type_hint` is *retyped*. Refusals: a `type_hint` outside `concept`'s closure (`Schema`), retyping a concept another bundle contains (`TypeChangeShared`), retyping one a belief edge references (`TypeChangeReferenced`).
4. Ensure the `bundle` entity and its metadata.
5. **Garbage collection.**
   - Orphans owned alone and referenced by a belief-tier relation are **retired** (`retire_concepts`): the entity, its id, required attributes (via `required_attributes` from the live schema), containment and belief edges are kept; other attributes and relations are removed; `retired-at` is set. An already-retired concept gets no further writes, and restoring its file revives it.
   - Other sole-owned orphans are deleted with every relation they play.
   - Shared orphans are *released*: only this bundle's containment is removed.
   - Retyped concepts are deleted here too.
6. **Reconcile.** New and retyped concepts are inserted. Changed concepts are updated **in place**: content is cleared and rewritten, preserving the entity and therefore its containment, hierarchy, incoming links and extension relations.
7. Containment for written concepts (a not-exists guard makes it a no-op for updates).
8. Hierarchy, links and citations, idempotent through `not { … }` guards.
9. `extension_writes` (`GraphWrite`s from importers such as `cyber_post_sync`) run in the same transaction, so a failing extension write rolls back the whole sync.
10. Fingerprint again; any change in a belief-tier label's instance, attribute or role-player count → `BeliefTierWrite`, nothing committed. The guard cannot see a same-count swap.

All values go through parameterized `given` rows, batched as one multi-row query per statement shape. Labels (subtypes, typed-attribute names) are validated before interpolation.

## Result counts

`SyncResult` reports `concepts_inserted/updated/skipped/deleted/released/retired` and `links_attempted` / `citations_attempted`. The last two are **attempts**: guarded inserts do not reveal whether a row was created, so they must not be relabelled as created counts.

## Multiple bundles per database

A database holds one bundle by default. Several may share one with `--database`, each keyed by its `bundle-name`. Each sync acts only on the concepts its own bundle contains; shared concepts are released rather than deleted; colliding ids are refused. `cyber-gecko` uses this to place sensors and detections beside an ATT&CK import.

## Runtime consequences

- `gecko run` refuses a retired concept (`fetch_playbook`).
- A retype loses incoming links from other bundles and extension relations that this sync does not recreate.

## Tests

- `gecko-bin/tests/bundle_sync_test.rs` — idempotency, in-place updates, GC, retyping.
- `gecko-bin/tests/multi_bundle_test.rs` — shared databases, collisions, release.
- `gecko-bin/tests/belief_tier_reconciliation_test.rs` — retirement of belief-referenced concepts and `BeliefTierWrite` refusal.
- `gecko-bin/tests/no_drag_test.rs` — the record path mints no belief or index entries.
- Unit tests in `syncer/bundle.rs` (`plan_sync`, `find_collisions`, the record-path guard).

<!-- openwiki: broken internal link [/openwiki/memory/belief-edges.md] link "/openwiki/memory/belief-edges.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
<!-- openwiki: broken internal link [/openwiki/database/typedb-and-schemas.md] link "/openwiki/database/typedb-and-schemas.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Related: [Belief Edges and the Belief-Tier Boundary](/openwiki/memory/belief-edges.md), [TypeDB Access, Schemas and Migrations](/openwiki/database/typedb-and-schemas.md).
