---
type: subsystem
title: Epistemic Memory Substrate (mem-gecko)
description: The always-on mem-gecko substrate — its bitemporal memory-item/episode/belief schema and persisted functions, the EpistemicWriter/EpistemicReader contract, MemWriter's authoritative-then-best-effort write discipline, supersede/retract/contest/rests-on semantics, visibility gating and the invariant-7 entrenchment guard.
tags: [mem, epistemic, beliefs, bitemporal, provenance, invariants]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-28T02:42:19.865Z
sources:
  - id: openwiki-source-8d801566a819b445226b1496
    resource: repo://core/gecko-extension-api/src/epistemic.rs
  - id: openwiki-source-9e41dc5152e3cb4d5df3f47d
    resource: repo://core/mem-gecko/schema/mem_types.tql
  - id: openwiki-source-302e2f40793f11e9ecb6c33a
    resource: repo://core/mem-gecko/src/lib.rs
  - id: openwiki-source-dfa481e7c5a630b55f795ea2
    resource: repo://core/mem-gecko/src/tql.rs
  - id: openwiki-source-71302efdf2c88746c3bbc2a8
    resource: repo://core/mem-gecko/src/writer/beliefs.rs
  - id: openwiki-source-fbdc30a071a57f3cf4cb7cd3
    resource: repo://core/mem-gecko/src/writer/mod.rs
  - id: openwiki-source-c5b3ee92d417d747f07a3fa1
    resource: repo://core/mem-gecko/src/writer/reader.rs
  - id: openwiki-source-4dc8a7ddb7ec8833bbee8c2d
    resource: repo://docs/refactor/KNOWN-LIMITATIONS.md
generated: { by: "claude-code", at: "2026-11-28T02:42:19.865Z" }
---

# Epistemic Memory Substrate (mem-gecko)

`core/mem-gecko` is the belief tier. It is always registered first by `build_extensions`, needs no feature flag, and its writer is the only sanctioned belief-write path (invariant 2). Records synced from bundles live in the core schema; beliefs, episodes and their provenance live here.

## Contract (`gecko-extension-api`)

`core/gecko-extension-api/src/epistemic.rs` defines the stable traits; every write method takes a host-minted `RunContext`.

**`EpistemicWriter`**: `observe` (episode), `assert_belief(ctx, draft, evidence, method)`, `supersede(ctx, old, new, reason)`, `retract(ctx, id, reason)` → visible current blast radius, `contest(ctx, claims)` → anomaly, `rests_on(ctx, resting, assumptions)`, `record_prediction`, `resolve_prediction`, and `as_epistemic_reader` (mem's writer returns itself, so recall and derive share one run scratch).

**`EpistemicReader`**: `recall(ctx, RecallQuery, ContextBudget)` → `Vec<Chunk>`, `derivation_chain`, `believed_at(ctx, t)`, `blast_radius`.

**`DerivationMethod`** has seven kebab-case values mirroring the schema: `type-join`, `cardinality`, `functional-dependency` (provable; may carry confidence 1.0), `type-db-function`, `external-tool`, `human-assertion`, and `llm-synthesis` (probabilistic; confidence must be < 1.0).

**`RunContext`** (`provenance.rs`) holds `run_id`, `actor`, `source` (`ExecutableDoc`/`Sync`/`Tool`/`Manual`, stored as the open-set `origin-kind`), `occurred_at`, and a non-serialized shared `RunScratch` of this run's retrievals. `matched_retrievals(evidence)` correlates them with a belief's evidence. Equality ignores the scratch.

## Schema

The schema has two fragments, `schema/mem_types.tql` and `schema/mem_functions.tql`, merged into one `define` (`MEM_SCHEMA` in `src/lib.rs`, types before functions). `BELIEF_TIER_ROOT` is `memory-item`.

- `memory-item` (abstract, `sub okf-concept`) → `episode` (→ `execution-episode` → `retrieval-event`), `belief` (→ `resolution`), `anomaly`.
- `belief` requires `belief-state` (`asserted`/`retracted`/`superseded`/`contested`), `valid-from`, `salience`, `base-activation`, `last-access`; optionally `confidence @range(0.0..1.0)`, `entrenchment` (`axiom` > `user-stated` > `tool-derived` > `inferred` > `llm`), `valid-to`; it plays `retraction:retracted @card(0..1)`.
- Relations: `derivation`, `supersession`, `retraction`, `contradiction`, `source-link`, `ownership` (with `visibility` `private`/`team`/`shared`), `rests-on`, `pivot`, `prediction-resolution`, `resolves`, and the retrieval ledger `surfaced` and `informs-synthesis`. Also `agent` and `doc-run` entities.
- Persisted functions include `is-superseded`, `believed-at`, `derivation-chain`, `blast-radius`/`blast-step`, `visible-to`, `gate`, `select-for-context`, `retrieval-score`, `retrieval-provenance-of`, `contradicts`, and the provable-only `canonical-entity`/`canonical-entity-at` closure over `resolution` beliefs (invariant 9: cross-source identity is a belief, never a merge).

<!-- openwiki: broken internal link [/openwiki/database/typedb-and-schemas.md] link "/openwiki/database/typedb-and-schemas.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Schema upgrades on existing databases go through `MEM_MIGRATIONS` (see [TypeDB Access, Schemas and Migrations](/openwiki/database/typedb-and-schemas.md)).

## `MemWriter`

`core/mem-gecko/src/writer/` implements both traits over a `GraphStore` (from the engine's `RouterGraphStore`), with an optional `Embedder` + `SemanticIndex`, a `record_retrieval_provenance` flag, and the `BeliefEdgeRegistry`. Split: `beliefs.rs` (writer trait and invariant-7 guards), `reader.rs` (function reads and recall), `index.rs` (reconciliation and index hooks), `edges.rs` (per-extension belief-edge writer). All query text lives in `src/tql.rs`.

**Write discipline.** The graph write is authoritative and commits first as one atomic `GraphStore::write`. Embedding and index upsert/remove happen afterwards, best-effort: a failure warns and never fails or rolls back the write, and startup reconciliation repairs the index (invariant 8).

**Entrenchment.** `assert_belief` derives entrenchment from the method unless the draft sets one: `human-assertion` → `user-stated`, `llm-synthesis` → `llm`, others → `tool-derived`. `supersede` reads the old belief's entrenchment (only if visible); the new one inherits it unless set explicitly, and a lower-ranked replacement is rejected with `EntrenchmentViolation` before anything is written.

**Supersede and retract.** Both require a currently asserted belief visible to the caller, and both close the old belief's `valid-to` at the successor's `valid-from` or the run's `occurred_at`. Superseding or retracting a retracted or superseded belief is refused; a second retraction is refused at commit by the `@card(0..1)`. `retract` returns the blast radius — the beliefs resting on it, or derived from it, that the caller can see and that are still current — read after commit. Dependents are reported, never retracted.

**Visibility.** Every write acts only on what the `RunContext` actor can see (`visible-to`: team/shared, or owned). This covers `supersede`, `retract`, `contest`, `rests_on` (resting item and every assumption), and `derive`'s evidence, including cyber's belief-edge path. Another agent's private belief is refused with the same message as a missing one, so refusals leak nothing.

## Recall

`recall` routes three ways (`reader.rs`):

- **As-of** (`RecallQuery.as_of` set) → `believed-at` on the temporal path; never touches the index.
- **Indexed** (embedder and index present) → embed the query, over-fetch `k×4` ANN candidates, then fetch and **gate** each candidate authoritatively (concurrently, bounded), dropping superseded, invalid or out-of-scope ids. Results are sorted by score and truncated to budget. With retrieval provenance on, it stamps a `retrieval-event` with `surfaced` edges and pushes the candidates into the run scratch.
- **Fallback** → `select-for-context`: recent, salient, scoped beliefs, with Rust-side recency/ACT-R decay.

`derivation_chain` follows `derivation` only, never the retrieval ledger, so similarity scores never become derivation evidence (the two-ledger firewall of invariant 8).

## Caveats

`docs/refactor/KNOWN-LIMITATIONS.md` holds the precise as-of semantics (empty and inverted windows, legacy databases), the race note (concurrent supersedes are not guarded), and that `record_prediction`/`resolve_prediction` are not guest-reachable and skip visibility checks.

## Tests

- `core/mem-gecko/src/writer/tests.rs` — serviceless writer logic over fake stores.
- `core/mem-gecko/tests/writer_reader_integration.rs` — the large live-TypeDB suite named in KNOWN-LIMITATIONS (supersede/retract windows, visibility refusals).
- `core/mem-gecko/tests/closure_functions_test.rs`, `schema_acceptance_test.rs`, `schema_migration_test.rs`.
- `gecko-bin/tests/no_drag_test.rs` — the record path stays off the belief tier.

<!-- openwiki: broken internal link [/openwiki/memory/sandbox-mem-bridge.md] link "/openwiki/memory/sandbox-mem-bridge.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
<!-- openwiki: broken internal link [/openwiki/memory/belief-edges.md] link "/openwiki/memory/belief-edges.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
<!-- openwiki: broken internal link [/openwiki/semantic-index/index-and-embedders.md] link "/openwiki/semantic-index/index-and-embedders.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Related: [Sandbox → mem Bridge](/openwiki/memory/sandbox-mem-bridge.md), [Belief Edges](/openwiki/memory/belief-edges.md), [Semantic Index and Embedders](/openwiki/semantic-index/index-and-embedders.md).
