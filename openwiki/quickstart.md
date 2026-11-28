---
type: guide
title: GECKO Quickstart
description: Orientation for working in the GECKO repository — what the system does, the fast build/test commands, the first-run CLI flow, the rules that most often trip contributors, and a task-to-page routing map for the rest of this wiki.
tags: [quickstart, orientation, build, testing, routing]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-28T02:42:19.865Z
sources:
  - id: openwiki-source-c42ae122f8faf30c8246ac70
    resource: repo://gecko-bin/src/main.rs
  - id: openwiki-source-76f0bf90f43c9d32df797b11
    resource: repo://gecko.toml
  - id: openwiki-source-c59fe4336a371ea1052a01dd
    resource: repo://justfile
  - id: openwiki-source-5f3e739e296777bc21a9a199
    resource: repo://sample-bundle/bundle.json
generated: { by: "claude-code", at: "2026-11-28T02:42:19.865Z" }
---

# GECKO Quickstart

<!-- openwiki: broken internal link [/openwiki/architecture/overview.md] link "/openwiki/architecture/overview.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
GECKO is a Rust workspace with a TypeDB 3.15 backend. It ingests OKF documentation bundles (Markdown + YAML frontmatter) into a typed knowledge graph, runs playbook code blocks in an all-WebAssembly sandbox (a Boa JS guest under Wasmtime), and maintains an always-on epistemic memory substrate: bitemporal beliefs, provenance, and semantic recall. Start with the [Architecture Overview](/openwiki/architecture/overview.md).

## Build and test fast

The default development path is **`--no-default-features`**: stub embedder, no `candle`, no `cyber`.

```bash
cargo build --no-default-features
just test        # Tier 1: serviceless
just test-it     # Tier 2: starts TypeDB; stub embedder; --test-threads=1
just test-model  # Tier 3: opt-in real model (~1.3 GB)

# What CI runs
cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber
cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings
cargo fmt --all -- --check
```

<!-- openwiki: broken internal link [/openwiki/testing/test-tiers-and-ci.md] link "/openwiki/testing/test-tiers-and-ci.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Details: [Test Tiers, Integration Tests and CI](/openwiki/testing/test-tiers-and-ci.md).

## First run

```bash
gecko up                          # orchestrated TypeDB 3.15.0 child (or compose/external)
gecko schema-init
gecko sync sample-bundle          # database "gecko-sample" from bundle.json
gecko run playbooks/hello_js --database gecko-sample
gecko doctor                      # ✓/⚠/✗ preconditions
```

<!-- openwiki: broken internal link [/openwiki/operations/cli-config-and-orchestration.md] link "/openwiki/operations/cli-config-and-orchestration.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
From source, prefix commands with `cargo run -p gecko-bin --no-default-features --`. Without `--database`, `gecko run` targets the reserved `gecko` database. See [CLI, Configuration and TypeDB Orchestration](/openwiki/operations/cli-config-and-orchestration.md).

## Rules that bite

- **Never touch the `gecko` database** in tests; use self-deleting `TestDb`s and `--test-threads=1`.
- **The guest wasm is prebuilt.** After editing `core/js-sandbox/src/main.rs`, run `just build-guest` and commit `quickjs.wasm`. CI never rebuilds it.
- **All TQL is parameterized.** Only validated schema labels are interpolated.
- **Belief writes go only through mem's writer** (invariant 2). The syncer refuses any change to the belief tier, and a lint enforces this.
- **The semantic index is a rebuildable bolt-on** (invariant 8). Index failures only warn, and bulk loads use `upsert_many`.
- **Never fabricate success over bad input** in host imports or bridges; return an error.
- **Extensions depend only on `gecko-extension-api`**, enforced by `scripts/check-extension-deps.sh`.
- The embedding model is a runtime asset under the cache dir, pinned as `BAAI/bge-large-en-v1.5@<sha>`. A bump needs a Tier-3 run and the `tier3-validated` label.

Repo conventions are in `CLAUDE.md`, the extension contract is in `extensions/README.md`, and caveats are in `docs/refactor/KNOWN-LIMITATIONS.md`.

## Where to look

| Task | Page |
| --- | --- |
<!-- openwiki: broken internal link [/openwiki/architecture/overview.md] link "/openwiki/architecture/overview.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| Understand the crates and the run lifecycle | [Architecture Overview](/openwiki/architecture/overview.md) |
<!-- openwiki: broken internal link [/openwiki/architecture/assembler-and-extensions.md] link "/openwiki/architecture/assembler-and-extensions.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| Add or change an extension, host import or bridge | [Assembler Pattern and Extension Contract](/openwiki/architecture/assembler-and-extensions.md) |
<!-- openwiki: broken internal link [/openwiki/okf/bundle-format-and-parser.md] link "/openwiki/okf/bundle-format-and-parser.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| Change bundle parsing or frontmatter (`engine`, `scopes`, `timeout-ms`) | [OKF Bundles and the Parser](/openwiki/okf/bundle-format-and-parser.md) |
<!-- openwiki: broken internal link [/openwiki/okf/graph-sync.md] link "/openwiki/okf/graph-sync.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| Change how bundles land in TypeDB, GC or retirement | [Bundle Sync into TypeDB](/openwiki/okf/graph-sync.md) |
<!-- openwiki: broken internal link [/openwiki/database/typedb-and-schemas.md] link "/openwiki/database/typedb-and-schemas.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| Touch the router, schema application or migrations | [TypeDB Access, Schemas and Migrations](/openwiki/database/typedb-and-schemas.md) |
<!-- openwiki: broken internal link [/openwiki/sandbox/wasm-executor.md] link "/openwiki/sandbox/wasm-executor.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| Sandbox limits, the host ABI or S3 scopes | [WASM Sandbox Executor and Host ABI](/openwiki/sandbox/wasm-executor.md) |
<!-- openwiki: broken internal link [/openwiki/sandbox/js-guest.md] link "/openwiki/sandbox/js-guest.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| Playbook-facing JS API, or rebuilding the guest | [JavaScript Guest and Playbook API](/openwiki/sandbox/js-guest.md) |
<!-- openwiki: broken internal link [/openwiki/memory/epistemic-substrate.md] link "/openwiki/memory/epistemic-substrate.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| Belief schema, supersede/retract and visibility | [Epistemic Memory Substrate](/openwiki/memory/epistemic-substrate.md) |
<!-- openwiki: broken internal link [/openwiki/memory/sandbox-mem-bridge.md] link "/openwiki/memory/sandbox-mem-bridge.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| `mem.*` calls and recall → derive provenance | [Sandbox → mem Bridge](/openwiki/memory/sandbox-mem-bridge.md) |
<!-- openwiki: broken internal link [/openwiki/memory/belief-edges.md] link "/openwiki/memory/belief-edges.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| Linking beliefs to records, and registry refusals | [Belief Edges and the Belief-Tier Boundary](/openwiki/memory/belief-edges.md) |
<!-- openwiki: broken internal link [/openwiki/semantic-index/index-and-embedders.md] link "/openwiki/semantic-index/index-and-embedders.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| Embeddings, HNSW and model fetching | [Semantic Index and Embedders](/openwiki/semantic-index/index-and-embedders.md) |
<!-- openwiki: broken internal link [/openwiki/extensions/cyber-gecko.md] link "/openwiki/extensions/cyber-gecko.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| ATT&CK, sensors and detection coverage | [cyber-gecko](/openwiki/extensions/cyber-gecko.md) |
<!-- openwiki: broken internal link [/openwiki/operations/cli-config-and-orchestration.md] link "/openwiki/operations/cli-config-and-orchestration.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| Config, `gecko up`/`doctor` and releases | [CLI, Configuration and TypeDB Orchestration](/openwiki/operations/cli-config-and-orchestration.md) |
<!-- openwiki: broken internal link [/openwiki/testing/test-tiers-and-ci.md] link "/openwiki/testing/test-tiers-and-ci.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| Tests and CI gates | [Test Tiers, Integration Tests and CI](/openwiki/testing/test-tiers-and-ci.md) |
