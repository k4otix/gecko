---
type: testing
title: Test Tiers, Integration Tests and CI
description: How GECKO is tested and gated — the three test tiers (serviceless stub, live TypeDB, opt-in real model), integration-test conventions with self-deleting databases, the candle-free per-push CI job and its guards, the manual Tier-3 model workflow and model-bump gate, and the rustfmt git hooks.
tags: [testing, ci, nextest, typedb, model, hooks]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-28T02:42:19.865Z
sources:
  - id: openwiki-source-bf5be0c9253ed1d07b502e10
    resource: repo://.githooks/pre-commit
  - id: openwiki-source-164e2da859b5277df81c7d94
    resource: repo://.github/workflows/ci.yml
  - id: openwiki-source-b7503e764849fcb04a9d7ad1
    resource: repo://.github/workflows/model-integration.yml
  - id: openwiki-source-a450e50015cfef6d6d9e8d56
    resource: repo://gecko-bin/tests/common/mod.rs
  - id: openwiki-source-c59fe4336a371ea1052a01dd
    resource: repo://justfile
  - id: openwiki-source-96cd2a2a68b5ccf6ba5c7b3e
    resource: repo://scripts/setup-hooks.sh
generated: { by: "claude-code", at: "2026-11-28T02:42:19.865Z" }
---

# Test Tiers, Integration Tests and CI

The default development path is `--no-default-features`: the deterministic stub embedder, no `candle`, no `cyber`. `gecko-bin`'s default features are `["cyber", "real-embedder"]`; `integration-model` (non-default, implies `real-embedder`) is the only flag that compiles the Tier-3 real-model tests.

## Tiers (`justfile`)

| Recipe | Command | Needs |
| --- | --- | --- |
| `just test` (Tier 1) | `cargo test --lib --bins --no-default-features` | Nothing: no TypeDB, model, or network. |
| `just test-it` (Tier 2) | `cargo test --test '*' --no-default-features --features cyber -- --test-threads=1` | Live TypeDB (the recipe runs `gecko up` first); still the stub embedder. |
| `just test-model` (Tier 3) | `cargo test --features integration-model -- --test-threads=1` | Real `bge-large-en-v1.5` (about 1.3 GB on first fetch) and TypeDB. |
| `just check` | `fmt` + `lint` + `test` | Pre-push gate. |

To run a single test:

```bash
cargo test -p gecko-bin --test mem_recall_e2e_test recall_then_derive --no-default-features -- --test-threads=1
```

Tier-3 tests read `GECKO_SMOKE_MODEL_DIR` (a staged model directory) and skip when it is unset.

## Integration-test conventions

- Live tests expect TypeDB at `localhost:1729` with `admin`/`password`.
- `gecko-bin/tests/common/mod.rs` provides `TestDb`: a database named `{prefix}_{nanos}_{pid}_{seq}` whose `Drop` deletes it on a fresh runtime in a separate thread, so cleanup happens even on panic and inside `#[tokio::test]`. Declare it before any router so it drops last. `core/mem-gecko/tests/common/mod.rs` has `drop_db_with_retry` for the same purpose.
- **Never touch the `gecko` database.** The router refuses to delete it anyway. Don't disrupt a TypeDB you didn't start, and use `--test-threads=1` to avoid contention.
- Shared fixtures live in `gecko-bin/tests/common/` (ATT&CK/STIX objects, beliefs, provenance, belief-tier surface, the cyber walkthrough) and `gecko-bin/tests/fixtures/`.

## Per-push CI (`.github/workflows/ci.yml`)

Runs on pull requests and on pushes to `main`.

**`build-and-test`** (Ubuntu, `typedb/typedb:3.15.0` service on 1729):

1. `cargo fmt --all -- --check`
2. `cargo audit`, then `cargo deny check licenses bans sources`
3. Clippy `-D warnings`, `--no-default-features`, both without and with `--features cyber`, excluding `js-sandbox`
4. **Guard:** `cargo tree --no-default-features` must contain no `candle`
5. **Guard:** `scripts/check-extension-deps.sh` (extensions depend on neither `gecko-engine` nor `typedb-driver`)
6. `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --locked` (Tiers 1 and 2)
7. **Guard:** `~/.cache/gecko/models` must be empty, so the job never materializes the model

**`real-embedder-check`**: `cargo check --workspace --exclude js-sandbox --features real-embedder`, which compiles candle without running it.

**`model-bump-gate`** (PRs only): diffs `gecko-bin/src/config.rs` (`default_model_id`, `model_id`, `bge-large-en-v1.5@` literals) and `model-integration.yml` (`MODEL_REVISION`, `MODEL_ID`). If the pinned model changed, the job fails unless the PR has the `tier3-validated` label.

<!-- openwiki: broken internal link [/openwiki/sandbox/js-guest.md] link "/openwiki/sandbox/js-guest.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
`js-sandbox` is excluded everywhere; the guest wasm is consumed prebuilt ([JavaScript Guest](/openwiki/sandbox/js-guest.md)).

## Tier-3 workflow (`.github/workflows/model-integration.yml`)

<!-- openwiki: broken internal link [/openwiki/semantic-index/index-and-embedders.md] link "/openwiki/semantic-index/index-and-embedders.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
This workflow is `workflow_dispatch` only. It pins `MODEL_ID`/`MODEL_REVISION` in lockstep with `PINNED_MODEL_REVISION`, caches `~/.cache/gecko/models` keyed on the immutable revision, optionally uses a `MODEL_MIRROR_URL` secret, stages the model with `gecko model fetch`, and runs the real-model tests against a TypeDB 3.15.0 service. A green run plus the `tier3-validated` label is required to merge a model bump; see [Semantic Index and Embedders](/openwiki/semantic-index/index-and-embedders.md).

## Other workflows

<!-- openwiki: broken internal link [/openwiki/operations/cli-config-and-orchestration.md] link "/openwiki/operations/cli-config-and-orchestration.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
- `release.yml` — per-platform release binaries ([CLI, Configuration and TypeDB Orchestration](/openwiki/operations/cli-config-and-orchestration.md)).
- `codeql.yml` — code scanning.
- `dependabot-auto-merge.yml` with `.github/dependabot.yml`.

## Git hooks

`scripts/setup-hooks.sh` sets `core.hooksPath=.githooks`. Both `pre-commit` and `pre-push` run `cargo fmt --all -- --check` and skip when `cargo` is not on `PATH`.

## Where the important tests are

| Area | Tests |
| --- | --- |
| Sandbox | `core/gecko-engine/tests/wasm_tests.rs`, `pipeline_tests.rs`, `gecko-bin/tests/sandbox_test.rs` |
| Sync | `bundle_sync_test.rs`, `multi_bundle_test.rs`, `belief_tier_reconciliation_test.rs`, `no_drag_test.rs` |
| Schema | `schema_init_test.rs`, `core/mem-gecko/tests/schema_*_test.rs` |
| Memory | `core/mem-gecko/tests/writer_reader_integration.rs`, `mem_recall_e2e_test.rs`, `epistemic_bridge_test.rs` |
| Belief tier | `belief_tier_monopoly_test.rs`, `belief_tier_surface_test.rs`, `belief_edge_registry_test.rs` |
| Cyber | `cyber_*_test.rs` |
| CLI | `config_init_test.rs`, `orchestrator_test.rs`, `run_args_cli_test.rs`, `index_per_database_cli_test.rs` |
