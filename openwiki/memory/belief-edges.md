---
type: concept
title: Belief Edges and the Belief-Tier Boundary
description: How extensions attach a belief to domain records through declared belief-edge relations written atomically by BeliefEdgeWriter, how mem's BeliefEdgeRegistry validates every extension fragment fail-closed, and how the belief-tier surface, fingerprint and monopoly lint keep belief writes on mem's path.
tags: [mem, beliefs, belief-edges, registry, invariants, security]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-28T02:42:19.865Z
sources:
  - id: openwiki-source-7df3e6f28374ff8b3ba29b34
    resource: repo://core/gecko-engine/src/db/surface.rs
  - id: openwiki-source-25eb5271b2078890188e98ad
    resource: repo://core/gecko-engine/src/syncer/bundle.rs
  - id: openwiki-source-ae158f56b3c60f02a606aa19
    resource: repo://core/gecko-extension-api/src/belief_edge.rs
  - id: openwiki-source-17619ca505037840d8f21168
    resource: repo://core/mem-gecko/src/belief_edge/registry.rs
  - id: openwiki-source-ed2ac54adc19a9dd3e60a9ea
    resource: repo://core/mem-gecko/src/belief_edge/scan.rs
  - id: openwiki-source-f0f7add287fdfc602f6b3615
    resource: repo://core/mem-gecko/src/writer/edges.rs
  - id: openwiki-source-fbdc30a071a57f3cf4cb7cd3
    resource: repo://core/mem-gecko/src/writer/mod.rs
  - id: openwiki-source-e85da85ddac07eb806ac54e0
    resource: repo://gecko-bin/tests/belief_tier_monopoly_test.rs
generated: { by: "claude-code", at: "2026-11-28T02:42:19.865Z" }
---

# Belief Edges and the Belief-Tier Boundary

Records (the syncer's tier) and beliefs (mem's tier) are disjoint (invariant 1), and every belief write goes through mem's writer (invariant 2). A **belief edge** is the sanctioned bridge between the two: an extension-owned relation in which a belief plays one role and domain records play the others, for example cyber's `technique-detection` linking a detection-claim belief to a detection concept and an ATT&CK technique.

## Declaring an edge

The types live in `core/gecko-extension-api/src/belief_edge.rs`:

- `BeliefEdgeDecl { relation, belief_role, roles: &[RoleDecl], owned_attrs: &[AttrDecl] }` — returned from `GeckoExtension::belief_edges()`.
- `RoleDecl { role, player, key }` — a non-belief role, the entity type that plays it, and the key attribute used to match that player.
- `AttrDecl { attr, value: AttrValueType, required }` — an attribute the relation owns.
- `BeliefEdge { relation, players: Vec<(role, GraphValue)>, attrs }` — one concrete edge to write.
- `BeliefEdgeWriter::assert_belief_with_edges(ctx, draft, evidence, method, edges)` — writes a belief and its edges **all or nothing**.

The relation itself and the `plays` clauses (`belief plays technique-detection:detection-claim;`) are defined in the extension's own schema fragment.

## The registry: `BeliefEdgeRegistry`

`core/mem-gecko/src/belief_edge/registry.rs` builds the set of edge relations each extension may write. `BeliefEdgeRegistry::from_extensions(core_schema, extensions)` takes mem's schema and migrations from the `mem-gecko` extension and scans every other extension's schema, migrations and declarations. `scan.rs` tokenizes fragments (stripping comments and string literals) without modelling the grammar, and every check **fails closed**: a form the position rules cannot prove safe is refused.

`RegistryError` variants describe what is refused, including:

- `NotOwned` — the declared relation is not defined by the declaring extension.
- `DuplicateRelation` / `DuplicateRole` / `SubtypedRelation` / `RoleAlias`.
- `MemoryItemPlayer` — a non-belief role played by `memory-item`, any subtype, or any supertype of those.
- `ReparentedType`, `ModifiesBase` (undefine/redefine of core or mem, or a function named like one of theirs), `RenameUnsupported`, `AliasUnsupported` (the token `alias` anywhere).
- `MemMissing` — mem is not registered.

The registry is built in `check_extension_schemas` (`gecko-bin/src/config.rs`) before any extension schema reaches the database (`schema-init`, `sync`, `sync-stix`) and again in `build_host_bridge` before a run.

## Writing an edge: `BoundEdgeWriter`

`MemWriter::edge_writer_for(extension)` (`core/mem-gecko/src/writer/mod.rs`) returns a `BeliefEdgeWriter` bound to one extension, and refuses an extension that declares no edges. The host calls it; extensions never receive mem's general `EpistemicWriter`.

`BoundEdgeWriter::assert_belief_with_edges` (`core/mem-gecko/src/writer/edges.rs`):

1. `validate_edge` — the relation must be owned by the bound extension; every declared role is supplied exactly once and no unknown role appears; attributes must be declared, unique, of the declared value type, and every `required` one present.
2. `validate_confidence` for the derivation method.
3. `live_check` — once per relation per writer, schema reads confirm the declaration matches the live database; the result is cached in `checked_relations`.
4. Build the ordinary belief ops (belief, provenance, derivation from `evidence`), then append one `tql::belief_edge_op` per edge, which matches each player by its key and expects one row.
5. Commit everything in **one** `GraphStore::write`. An edge whose players do not match is reported as `belief edge <rel> (<role> <key>=<value>, …) matched N rows; nothing was committed`.
6. Best-effort index upsert afterwards; failure only warns.

## Sync-side protection of the belief tier

Beliefs reference records, so the record syncer must neither write the belief tier nor pull records out from under an edge:

- **Surface and fingerprint** (`core/gecko-engine/src/db/surface.rs`). `belief_tier_surface(tx, root)` reads from the live schema the `sub` closure of the root (`memory-item`) plus every relation owning a role played only by that closure. `belief_tier_fingerprint` counts instances, owned attributes and role players for every surface label in at most three grouped reads. `sync_bundle` fingerprints at the start and end of its transaction and refuses with `DbError::BeliefTierWrite` on any change, committing nothing.
<!-- openwiki: broken internal link [/openwiki/okf/graph-sync.md] link "/openwiki/okf/graph-sync.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
- **Retirement instead of deletion.** A concept that left the bundle while a belief edge still references it is retired (`retired-at` set, ID and containment kept, required attributes kept), not deleted; `gecko run` refuses a retired concept. A type change of a referenced concept is refused (`TypeChangeReferenced`). See [Bundle Sync into TypeDB](/openwiki/okf/graph-sync.md).

## The monopoly lint

`gecko-bin/tests/belief_tier_monopoly_test.rs` (`no_raw_belief_tier_tql_outside_mem_tql`) enforces invariant 2 statically. It introspects the belief-tier surface from the composed core → mem → cyber schema on a live TypeDB, then scans every production Rust string literal and `.tql` file outside `core/mem-gecko/src/tql.rs` for a write verb (`insert`, `put`, `update`, `delete`) together with a surface label after `isa` or directly before `(`. A new belief-tier type is covered without editing the test.

`gecko_extension_api::relation_syntax::deprecated_relation_lines` is a related lint that flags the deprecated tuple-first relation pattern in schema and query text; extension unit tests run it over shipped TQL.

## Tests

- `gecko-bin/tests/belief_edge_registry_test.rs` — the shipped extensions pass `from_extensions`.
- `core/mem-gecko/src/belief_edge/registry.rs` unit tests — each refusal.
- `core/mem-gecko/tests/belief_edge_integration.rs` — atomic belief+edge writes against live TypeDB.
- `gecko-bin/tests/belief_tier_surface_test.rs` — surface and fingerprint against a live schema.
- `gecko-bin/tests/belief_tier_reconciliation_test.rs` — sync refusal and retirement.

<!-- openwiki: broken internal link [/openwiki/memory/epistemic-substrate.md] link "/openwiki/memory/epistemic-substrate.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
<!-- openwiki: broken internal link [/openwiki/extensions/cyber-gecko.md] link "/openwiki/extensions/cyber-gecko.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Related: [Epistemic Memory Substrate](/openwiki/memory/epistemic-substrate.md), [cyber-gecko](/openwiki/extensions/cyber-gecko.md).
