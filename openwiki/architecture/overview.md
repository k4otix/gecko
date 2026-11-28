---
type: architecture
title: Architecture Overview
description: Map of GECKO's Rust workspace crates and their dependency direction, the parse → sync → fetch → execute lifecycle of a playbook, and the record/belief tier split that the epistemic memory substrate relies on.
tags: [architecture, overview, lifecycle, workspace, invariants]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-28T02:42:19.865Z
sources:
  - id: openwiki-source-651d1fb6c9e49916a916ab51
    resource: repo://Cargo.toml
  - id: openwiki-source-97097834ee69a4cc1ab2ea9f
    resource: repo://core/gecko-engine/src/pipeline.rs
  - id: openwiki-source-2f16f013112a825ae84345f5
    resource: repo://core/mem-gecko/Cargo.toml
  - id: openwiki-source-6a428184b9be60031958fe01
    resource: repo://extensions/cyber-gecko/Cargo.toml
  - id: openwiki-source-7b17faa0130e72979ea16e9f
    resource: repo://gecko-bin/Cargo.toml
  - id: openwiki-source-c42ae122f8faf30c8246ac70
    resource: repo://gecko-bin/src/main.rs
generated: { by: "claude-code", at: "2026-11-28T02:42:19.865Z" }
---

# Architecture Overview

GECKO ingests human-readable OKF documentation bundles (Markdown + YAML frontmatter) into a strongly typed TypeDB 3.15 graph, then runs playbook code blocks from those documents inside an all-WebAssembly sandbox. Alongside the record graph it maintains an always-on **epistemic memory substrate**: bitemporal beliefs with provenance and semantic recall.

## Workspace crates

| Crate | Path | Responsibility |
| --- | --- | --- |
| `gecko-extension-api` | `core/gecko-extension-api` | The contract crate: `GeckoExtension`, `ExtensionCallback`, epistemic traits (`EpistemicWriter`/`EpistemicReader`), `GraphReader`/`GraphStore`, `BeliefEdgeWriter`, `RunContext`, OKF import types, ids. No engine or driver dependency. |
| `gecko-engine` | `core/gecko-engine` | Domain-agnostic core: OKF parser/linker (`okf`), TypeDB router, schema application and belief-tier surface (`db`), idempotent bundle syncer (`syncer`), Wasmtime sandbox and mem host bridge (`sandbox`), state handles (`state`), and the execution `pipeline`. |
| `mem-gecko` | `core/mem-gecko` | Epistemic substrate: `mem_types.tql`/`mem_functions.tql`, `MemWriter` (writer + reader), belief-edge registry. Always registered. |
| `gecko-semantic-index` | `core/gecko-semantic-index` | HNSW index, stub embedder, optional candle `bge-large-en-v1.5` embedder, pinned model fetch. |
| `js-sandbox` | `core/js-sandbox` | Source of the Boa JS guest compiled to `wasm32-wasip1`; checked in prebuilt as `core/gecko-engine/src/sandbox/quickjs.wasm`. Not a default member, excluded from CI. |
| `cyber-gecko` | `extensions/cyber-gecko` | Optional detection-engineering domain extension (STIX/ATT&CK import, sensors, detect host imports). |
| `gecko-bin` | `gecko-bin` | The Assembler binary `gecko`: CLI, config, TypeDB orchestration, doctor, extension registration and host-bridge composition. |

<!-- openwiki: broken internal link [/openwiki/architecture/assembler-and-extensions.md] link "/openwiki/architecture/assembler-and-extensions.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Dependency direction: extensions and `mem-gecko` depend only on `gecko-extension-api`; `gecko-engine` depends on the API crate plus `typedb-driver` and `wasmtime`; `gecko-bin` depends on all of them, with `cyber-gecko` optional behind the `cyber` feature. The engine never references `extensions/` (see [Assembler Pattern and Extension Contract](/openwiki/architecture/assembler-and-extensions.md)).

## Lifecycle of a playbook

```
OKF bundle dir ──parse──▶ BundleManifest ──sync──▶ TypeDB (records)
                                                      │
gecko run <concept-id> ──fetch_playbook──────────────┘
        │
        ├─ build_host_bridge (extensions, MemWriter, RunContext)
        └─ pipeline::execute_playbook ──▶ SandboxPool ──▶ Wasmtime + Boa guest
                                                           │ host_call_extension
                                                           ▼
                                          S3 scope gate → bridges → mem / cyber / call_import
```

<!-- openwiki: broken internal link [/openwiki/okf/bundle-format-and-parser.md] link "/openwiki/okf/bundle-format-and-parser.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
1. **Parse** — `gecko sync <bundle>` parses the bundle into concepts with ids that are bundle-relative paths (e.g. `playbooks/hello_js`). See [OKF Bundles and the Parser](/openwiki/okf/bundle-format-and-parser.md).
<!-- openwiki: broken internal link [/openwiki/okf/graph-sync.md] link "/openwiki/okf/graph-sync.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
2. **Sync** — the syncer writes concepts, hierarchy, links and code blocks idempotently with parameterized queries, into a database named from `bundle.json` unless `--database` is given. See [Bundle Sync into TypeDB](/openwiki/okf/graph-sync.md).
3. **Fetch** — `fetch_playbook` (`gecko-bin/src/main.rs`) reads a concept's `engine`, `code-block`, `scope`s, and (when the schema defines them) `retired-at` and `timeout-ms`; a retired concept is refused.
<!-- openwiki: broken internal link [/openwiki/sandbox/wasm-executor.md] link "/openwiki/sandbox/wasm-executor.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
4. **Execute** — `cmd_run` builds the host bridge, then calls `gecko_engine::pipeline::execute_playbook`, which allocates an RAII state handle, runs the program in the sandbox pool with the run's scopes, timeout and `--args`, and logs the execution on success. A failed playbook makes `gecko run` exit non-zero. See [WASM Sandbox Executor and Host ABI](/openwiki/sandbox/wasm-executor.md).

`cmd_run` opens two connections to the same database: one the pipeline borrows mutably, and one owned by the epistemic writer for belief-tier commits.

`record_execution` in `pipeline.rs` only logs; playbooks persist memory themselves through `mem.*`.

## Two tiers: records and beliefs

The ratified invariants in `docs/refactor/INVARIANTS.md` shape the design. The load-bearing ones:

- **Invariant 1 — belief/record boundary.** Records (observed, authoritative facts) live in `core_schema.tql` and are written directly by the syncer. Beliefs (contested, derived, revisable claims) live in `mem_types.tql`.
- **Invariant 2 — `EpistemicWriter` is the only belief-write path.** Every belief write is host-mediated and stamped with a host-minted `RunContext`. This is a procedural guarantee, guarded by `gecko-bin/tests/belief_tier_monopoly_test.rs`; the syncer additionally fingerprints the belief tier and refuses any sync that would change it.
- **Invariant 3 — registration is the activation gate** (scoped to runtime activation, not schema reversibility).
- **Invariant 6 — bitemporal is substrate**: event/ingest time and validity live on mem's belief types.
- **Invariant 8 — the semantic index is a rebuildable accelerator**, never a source of truth; recall works without it.

<!-- openwiki: broken internal link [/openwiki/memory/epistemic-substrate.md] link "/openwiki/memory/epistemic-substrate.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
<!-- openwiki: broken internal link [/openwiki/memory/belief-edges.md] link "/openwiki/memory/belief-edges.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
See [Epistemic Memory Substrate](/openwiki/memory/epistemic-substrate.md) and [Belief Edges and the Belief-Tier Boundary](/openwiki/memory/belief-edges.md).

## Where to go next

<!-- openwiki: broken internal link [/openwiki/sandbox/wasm-executor.md] link "/openwiki/sandbox/wasm-executor.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
<!-- openwiki: broken internal link [/openwiki/sandbox/js-guest.md] link "/openwiki/sandbox/js-guest.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
- Security boundary and host ABI: [WASM Sandbox Executor](/openwiki/sandbox/wasm-executor.md), [JavaScript Guest](/openwiki/sandbox/js-guest.md)
<!-- openwiki: broken internal link [/openwiki/database/typedb-and-schemas.md] link "/openwiki/database/typedb-and-schemas.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
- Schemas and TypeDB access: [TypeDB Access, Schemas and Migrations](/openwiki/database/typedb-and-schemas.md)
<!-- openwiki: broken internal link [/openwiki/operations/cli-config-and-orchestration.md] link "/openwiki/operations/cli-config-and-orchestration.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
- Operating the binary: [CLI, Configuration and TypeDB Orchestration](/openwiki/operations/cli-config-and-orchestration.md)
- Known caveats: `docs/refactor/KNOWN-LIMITATIONS.md`
