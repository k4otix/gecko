---
type: subsystem
title: Semantic Index and Embedders
description: GECKO's optional, rebuildable recall accelerator — the SemanticIndex/Embedder contracts, the HNSW index with its persisted per-database snapshot, the deterministic stub and the candle bge-large-en-v1.5 embedder, pinned model fetching with checksums, and the startup reconciliation that keeps the index in step with the graph.
tags: [semantic-index, hnsw, embedder, candle, model, invariants]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-28T02:42:19.865Z
sources:
  - id: openwiki-source-74f8cb8605f4fd293950b0cc
    resource: repo://core/gecko-extension-api/src/semantic.rs
  - id: openwiki-source-9fa653caed4a4d36e691981e
    resource: repo://core/gecko-semantic-index/src/candle_embedder.rs
  - id: openwiki-source-5dbb613090817fd64685875d
    resource: repo://core/gecko-semantic-index/src/fetch.rs
  - id: openwiki-source-f639f31d7496535b7b8699ec
    resource: repo://core/gecko-semantic-index/src/index.rs
  - id: openwiki-source-50387b98809acb73db37a253
    resource: repo://core/gecko-semantic-index/src/lib.rs
  - id: openwiki-source-bf1d5b2f97f2ef403b795f04
    resource: repo://core/mem-gecko/src/writer/index.rs
  - id: openwiki-source-c42ae122f8faf30c8246ac70
    resource: repo://gecko-bin/src/main.rs
generated: { by: "claude-code", at: "2026-11-28T02:42:19.865Z" }
---

# Semantic Index and Embedders

<!-- openwiki: broken internal link [/openwiki/memory/epistemic-substrate.md] link "/openwiki/memory/epistemic-substrate.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
`core/gecko-semantic-index` supplies the concrete accelerator behind `mem.recall`. Invariant 8 is load-bearing: the index is **a pure, rebuildable accelerator and never a source of truth**. It stores only `(embedding, concept_id, coarse FilterMeta)` and answers which ids are near a query. mem then fetches and gates every hit from TypeDB ([Epistemic Memory Substrate](/openwiki/memory/epistemic-substrate.md)). Recall works without it, through the non-vector `select-for-context` fallback.

## Contracts (`gecko-extension-api/src/semantic.rs`)

- `Embedder`: `model_id`, `dim`, `embed_query` (adds the BGE search prefix), `embed_document` (raw text), and `embed` (defaults to the document side).
- `SemanticIndex`: `upsert`, `upsert_many` (persists at most once per batch), `remove`, `remove_many`, `ids()` (for reconciliation; `None` means self-managed), `query(v, k, pre)`, `model_id`.

Bulk paths must use `upsert_many`. Calling `upsert` in a loop re-serializes the snapshot for each item, which is O(n²).

## `HnswIndex` (`src/index.rs`)

- Wraps `hnsw_rs` over L2-normalized vectors with cosine distance, and keeps a `FilterMeta` per id for pre-filtering.
- `hnsw_rs` has no update or delete, so a live-id map handles both: upsert inserts a new internal id and tombstones the old one, remove drops the id from the live set, queries filter out stale ids, and `reset`/`remove_many` rebuild a clean graph.
- Persists a snapshot file with header `{model_id, dim}` plus live entries, so a warm start skips the rebuild. A missing or corrupt file, or a model-id mismatch, opens empty.
- `NoopIndex` stands in when the accelerator is disabled.

**One file per database**: `config::index_file(cfg, database)` resolves to `<[semantic_index] dir or cache_dir/index>/<database>.hnsw`. `gecko drop` removes it best-effort.

## Embedders

- **`StubEmbedder`** (`src/embedder.rs`, always compiled): deterministic token hashing into a fixed-dimension normalized vector. The binary builds it at 384 dimensions. It keeps the query/document asymmetry, so the two paths differ. It is suitable for correctness and gating tests, not retrieval quality.
- **`CandleEmbedder`** (`src/candle_embedder.rs`, only with the `real-embedder` feature): `bge-large-en-v1.5`, BERT, 1024 dimensions, CPU, CLS pooling then L2-normalization. Construction touches no files; the model and tokenizer load lazily on the first embed and are cached. If the model directory is absent, `auto_fetch = false` (the default) returns an error pointing at `gecko model fetch`, never a silent 1.3 GB download. CPU-heavy embedding runs on the blocking pool.

`gecko-bin`'s `build_epistemic_writer` picks the embedder from `[semantic_index] embedder`. `"stub"` and `"candle"` are the only accepted values; `"candle"` in a build without `real-embedder` is a hard error, and the only `backend` is `"hnsw"`. With `enabled = false`, `MemWriter::without_index` is used and nothing is embedded.

## Model acquisition (`src/fetch.rs`)

- The model id has the form `BAAI/bge-large-en-v1.5@<revision>`. The revision is single-sourced as `PINNED_MODEL_REVISION`, and `default_model_id()` in `gecko-bin/src/config.rs` is built from it. The `BAAI/` org prefix is required, or the HuggingFace fetch returns 401.
- The fetcher downloads `config.json`, `tokenizer.json` and `model.safetensors` from HuggingFace (or `model_source`) into `cache_dir/models/<model_id>/`, writing atomically. It verifies `known_checksums(model_id)` for the pinned revision; a mismatch is an error.
- It is candle-free and shared by `gecko model fetch` and `auto_fetch`.
<!-- openwiki: broken internal link [/openwiki/testing/test-tiers-and-ci.md] link "/openwiki/testing/test-tiers-and-ci.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
- Changing the pinned revision trips CI's `model-bump-gate`, which requires a green Tier-3 run and the `tier3-validated` label ([Test Tiers and CI](/openwiki/testing/test-tiers-and-ci.md)).

## Write and reconcile lifecycle

- **Post-commit hooks.** After an authoritative belief write, `MemWriter` embeds and upserts best-effort. A supersede also removes the old vector. Failures only warn.
- **Startup reconciliation** (`MemWriter::reconcile_index`, `core/mem-gecko/src/writer/index.rs`) runs in `build_epistemic_writer` before any recall. It enumerates the graph's current-state embeddables (asserted beliefs, and episodes other than retrieval events).
  - Same model with known ids: remove stale ids first, then embed and `upsert_many` the missing ones. Matching sets embed nothing, so the real model is never loaded.
  - Model changed or index empty: reset and rebuild from the graph.
  - Only the graph read can fail reconciliation; index errors warn.
- **As-of recall never touches the index**, because the index holds present state only.

## Tests

- Unit tests in `index.rs`, `embedder.rs`, `fetch.rs`.
- `core/mem-gecko/tests/semantic_index_integration.rs` — index-seeded recall, gating, reconciliation against live TypeDB.
- `gecko-bin/tests/index_per_database_cli_test.rs` — one index file per database through the CLI.
- Tier 3 (`--features integration-model`, `GECKO_SMOKE_MODEL_DIR`) exercises the real model.
