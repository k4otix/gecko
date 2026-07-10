# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What GECKO is

A Rust workspace + TypeDB 3.12 engine that ingests human-readable "OKF" documentation bundles into a strongly-typed knowledge graph, then executes playbook code blocks in an all-WebAssembly sandbox. It also ships an always-on **epistemic memory substrate** (bitemporal beliefs, provenance, semantic recall). See `README.md` for the product-level overview and `docs/refactor/KNOWN-LIMITATIONS.md` for load-bearing caveats.

## Build / test / lint

The **fast, default path is `--no-default-features`** — it uses the deterministic *stub* embedder and never compiles `candle` (the ML lib). Prefer it for everything except real-model work.

```bash
# Build (fast dev; stub embedder, no candle)
cargo build --no-default-features

# Test tiers (via justfile — run `just` to list all recipes):
just test         # Tier 1: lib/bins, serviceless, stub embedder. No candle/model/network.
just test-it      # Tier 2: real TypeDB (starts it), still the stub embedder.
just test-model   # Tier 3: OPT-IN real bge-large model (first run downloads ~1.3GB) + real TypeDB.

# What CI actually runs (candle-free whole-suite + lint):
cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber
cargo clippy --workspace --exclude js-sandbox --no-default-features -- -D warnings
cargo fmt --all -- --check

# A single test (nextest substring filter, or plain cargo):
cargo test -p gecko-bin --test mem_recall_e2e_test recall_then_derive --no-default-features -- --test-threads=1
```

**Integration tests need a live TypeDB on `localhost:1729`** (admin/password). They create uniquely-named throwaway databases and self-delete. **Never touch the database named `gecko`**, and don't disrupt a TypeDB you didn't start — run integration tests with `--test-threads=1` to avoid contention.

`git commit`/`git push` run `cargo fmt --all -- --check` via `core.hooksPath=.githooks` (see `scripts/setup-hooks.sh`).

## The WASM guest is prebuilt and out of CI

JavaScript playbooks run in a **Boa** JS engine compiled to `wasm32-wasip1`, checked in **prebuilt** as `core/gecko-engine/src/sandbox/quickjs.wasm` (a Boa build despite the historical name) from source at `core/js-sandbox/src/main.rs`. CI never rebuilds it (`--exclude js-sandbox` everywhere; `js-sandbox` is a workspace member but **not** a default-member, and can't be built/tested on the host because of its wasm import — use `cargo check/clippy -p js-sandbox` only).

**After editing `core/js-sandbox/src/main.rs` you must `just build-guest` and commit the regenerated `quickjs.wasm`**, or the change never reaches a run. The rebuild targets *behavioral* equivalence (existing guest tests green), not a byte-identical binary. See `docs/refactor/guest-wasm.md`.

## Architecture (the parts that span files)

**Assembler pattern.** One binary statically composes the core engine with domain extensions at *compile time* (`GeckoExtension` trait in `core/gecko-extension-api`). `gecko.toml`'s `[extensions] enabled` selects which are *activated* at runtime — registration is the gate for loading host functions and opening write-paths. The `mem` substrate is forced-on and not listed.

**Lifecycle** (`gecko-bin/src/main.rs` → `core/gecko-engine/src/pipeline.rs`): parse OKF bundle → idempotent TypeDB sync (content-hashed, parameterized writes) → retrieve a concept's code block → execute in the sandbox. GECKO is **single-bundle-scoped** (one bundle per database); concept IDs are bundle-relative (e.g. `playbooks/hello_js`).

**Sandbox host boundary** (`core/gecko-engine/src/sandbox/wasm_executor.rs`): a single wasm import `host_call_extension` (JSON string in/out). Every host call is **S3 default-deny gated** on an `"ext:func"` scope the concept must grant in frontmatter (`scopes: [mem:recall, mem:derive]`) — ungranted calls fail before the callback runs. The guest exposes `console.log`, `gecko.callExtension(ext, func, args)`, and the `mem.*` namespace.

**Epistemic memory** (`core/gecko-extension-api/src/epistemic.rs` traits; `core/mem-gecko`): `EpistemicWriter`/`EpistemicReader` over a bitemporal belief/provenance schema. The sandbox→mem bridge is `core/gecko-engine/src/sandbox/mem_host.rs` (`epistemic_extension_callback`, `dispatch_mem_call`/`dispatch_mem_recall`). The host stamps run identity (`run_id`/`actor`/`source`) — the sandbox cannot forge it. A `recall` and a later `derive` in the same run share one `RunContext` scratch, auto-linking the synthesized belief to the retrieval (`retrieval-provenance`, `informs-synthesis`, `surfaced was-used`). End-to-end proof: `gecko-bin/tests/mem_recall_e2e_test.rs`.

**Semantic index** (`core/gecko-semantic-index`): an in-process HNSW accelerator for recall. It is a **pure, rebuildable bolt-on (invariant 8) — never a source of truth**; recall works without it (non-vector fallback). Embedder is the deterministic stub by default; the candle-backed `bge-large-en-v1.5` compiles only under the `real-embedder` feature.

## Feature flags and the model-as-runtime-asset rule

- `real-embedder` (default-ON for `gecko-bin`) → compiles the candle `CandleEmbedder`. `--no-default-features` drops it (and `cyber`) for the fast, candle-free build.
- `integration-model` (NON-default) → the ONLY flag that compiles the Tier-3 real-model tests. Implies `real-embedder`.
- `cyber` → the optional cyber-gecko domain extension.

**The embedding model is a runtime asset, never in the build tree.** `auto_fetch` defaults to `false` (no silent 1.3GB pull). The model id is pinned by immutable HuggingFace commit SHA as `BAAI/bge-large-en-v1.5@<sha>` (`default_model_id()` in `gecko-bin/src/config.rs`, single-sourced from `PINNED_MODEL_REVISION` in `core/gecko-semantic-index/src/fetch.rs`) — the **`BAAI/` org prefix is required** or the HF fetch 401s. Downloads are checksum-verified.

**CI structure** (`.github/workflows/`): `ci.yml` is per-push, **must stay candle-free** (a `cargo tree | grep candle` guard enforces this) and runs against a TypeDB 3.12 service. `model-integration.yml` is the Tier-3 real-model job — **manual `workflow_dispatch` only**; validate the model locally otherwise. Changing the pinned model id/revision trips the `model-bump-gate` job, which requires a green Tier-3 run and the `tier3-validated` PR label. Tier-3 tests read `GECKO_SMOKE_MODEL_DIR` (a staged model dir) or they skip.

## Code-quality conventions

Hold new code to these — they encode recurring pitfalls specific to this codebase.

**Comments are greenfield.** Describe what the code does *now*. No plan/epic tags (`A5.7`, `P2`, `C1`, `WS3`), no changelog phrasing (`FIX 2:`, `Carry-forward fix:`, "used to", "no longer", "now ships"). Keep permanent design vocabulary — `invariant N`, the `S1`–`S5` security tiers, and `design §X.Y` references are load-bearing, not history. A doc comment whose second sentence just restates the first is noise — write one accurate sentence.

**Never fabricate success over bad input.** A host import, guest arg-marshaling path, or extension handler that receives a missing/invalid field must return an error, not default it (`""`, `"unknown"`) and report success — that masks a caller bug as a confusing failure far downstream. If a `GeckoExtension` exposes no plain host imports, `host_imports()` returns `vec![]` and `call_import` errors; mem's real surface is the `mem.*` sandbox bridge (`inject_epistemic_host_fns`), not `call_import`.

**The guest is untrusted.** At the `host_call_extension` boundary, cap every guest-supplied length against a constant *before* allocating a host buffer (see `MAX_IMPORT_NAME_LEN`/`MAX_ARGS_JSON_LEN` in `wasm_executor.rs`). Scope-deny (S3) is checked before anything else, so an ungranted call never leaks whether its payload was even well-formed.

**The semantic index is a rebuildable bolt-on (invariant 8).** Bulk load/rebuild paths must call `SemanticIndex::upsert_many` (one persist for the batch) — never `upsert` in a loop, which re-serializes the whole snapshot per item (O(n²)). Index mutations are best-effort and post-commit; a failure marks the id dirty for background repair and never fails the authoritative graph write.

**Don't block the async runtime.** CPU-heavy embedding, `ureq` downloads, and `std::thread::sleep`/`std::fs` reached from an `async fn` must go through `tokio::task::spawn_blocking` (or the `tokio::*` async equivalent). The real embedder and `gecko up`/`down` orchestration are the usual offenders.

**Lock poisoning on best-effort or `Drop` paths** uses `.unwrap_or_else(|e| e.into_inner())`, not `.expect()`/`.unwrap()` — a panic in one task must not cascade into every later access (or abort the process from a `Drop`).

**Assembler wiring.** Build extensions only inside the CLI arms that use them (`SchemaInit`/`Sync`/`Run`); a typo in `[extensions] enabled` must not break unrelated commands like `up`/`drop`/`databases`. The engine refuses to delete the reserved `gecko` database.

**Observability tells the truth.** Idempotent syncs use `not { … }` guards, so counts are *attempted*, not *created* (`SyncResult.links_attempted`/`citations_attempted`); don't relabel an attempt count as a mutation count. Stream loops (`while let Some(Ok(row)) = …`) must handle `Some(Err(e))` explicitly — silently ending on error truncates results or masks a real DB failure as "not found".

**All TQL is parameterized** — values go through `Params`, never string interpolation (injection-safe). `tql.rs` owns the schema-specific query text.
