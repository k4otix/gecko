---
type: operations
title: CLI, Configuration and TypeDB Orchestration
description: Operating the gecko binary — its subcommands and global flags, gecko.toml (extensions, semantic_index, typedb, cache_dir) and runtime-asset path precedence, the orchestrated/compose/external TypeDB modes behind gecko up/down, the gecko doctor precondition checker, and release/install packaging.
tags: [cli, config, operations, typedb, doctor, install]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-28T02:42:19.865Z
sources:
  - id: openwiki-source-4d1d392666be6dfdd7a91a2e
    resource: repo://.github/workflows/release.yml
  - id: openwiki-source-0ce57d2c565c71dba9a43b7b
    resource: repo://gecko-bin/src/config.rs
  - id: openwiki-source-15da1c5b690b06412760f2fa
    resource: repo://gecko-bin/src/config/paths.rs
  - id: openwiki-source-357e2cb5f84f93d720f9aee9
    resource: repo://gecko-bin/src/doctor.rs
  - id: openwiki-source-c42ae122f8faf30c8246ac70
    resource: repo://gecko-bin/src/main.rs
  - id: openwiki-source-4f131df82629fe201181be1f
    resource: repo://gecko-bin/src/orchestrator.rs
generated: { by: "claude-code", at: "2026-11-28T02:42:19.865Z" }
---

# CLI, Configuration and TypeDB Orchestration

The `gecko` binary (`gecko-bin/src/main.rs`) is the Assembler: it parses CLI args, loads `gecko.toml`, and dispatches each command. User-facing install docs are `docs/INSTALL.md`.

## Commands

| Command | What it does |
| --- | --- |
| `init` | Writes `default_gecko_toml()` if no config exists; never clobbers. Runs before config load. |
| `up` / `down` | Start/stop TypeDB per `[typedb] mode` (below). |
| `schema-init [bundle] [--schema f]` | Applies core + every registered extension schema (with migrations). |
<!-- openwiki: broken internal link [/openwiki/okf/graph-sync.md] link "/openwiki/okf/graph-sync.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| `sync <bundle>` | Parse and sync an OKF bundle ([Bundle Sync](/openwiki/okf/graph-sync.md)). |
| `run <concept-id> [--args json]` | Fetch and execute a playbook; exits non-zero when it fails. |
| `query '<TypeQL>'` | Ad-hoc read query. |
| `status` | List synced bundles. |
| `databases` / `drop <name>` | List / delete databases. `drop` refuses `gecko` and, when the index is enabled, removes that database's index file best-effort. |
| `model fetch` | Stage the pinned embedding model into the cache. |
| `doctor [--check n] [--json] [--quiet]` | Precondition report. |
<!-- openwiki: broken internal link [/openwiki/extensions/cyber-gecko.md] link "/openwiki/extensions/cyber-gecko.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
| `sync-stix`, `sync-sensors` | `cyber` builds only ([cyber-gecko](/openwiki/extensions/cyber-gecko.md)). |

Global flags: `--address` (overrides `[typedb] endpoint`), `--database`, `--username`/`--password` (default `admin`/`password`), `--tls`, `--ca-cert`, `--config` (default `gecko.toml`).

**Database resolution** (`resolve_db`): explicit `--database`, else the bundle name for `schema-init`/`sync`/`sync-stix`, else `gecko`. `sync-sensors` requires `--database`. Running `gecko run` without `--database` targets the reserved `gecko` database.

<!-- openwiki: broken internal link [/openwiki/architecture/assembler-and-extensions.md] link "/openwiki/architecture/assembler-and-extensions.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Extensions are assembled only in the `SchemaInit`/`Sync`/`SyncStix`/`SyncSensors`/`Run` arms, so a bad `[extensions] enabled` never breaks `up`/`down`/`query`/`status`/`drop`/`databases`/`model` ([Assembler Pattern](/openwiki/architecture/assembler-and-extensions.md)).

## `gecko.toml`

`GeckoConfig::load` parses the file. A **missing** file is not an error: it yields the default, substrate-only config with a warning. A parse error fails.

```toml
[extensions]
enabled = ["cyber"]        # mem is always on; unknown/not-compiled names fail

cache_dir = "/path"        # optional

[semantic_index]
enabled  = true            # default false
embedder = "stub"          # or "candle" (real-embedder builds)
backend  = "hnsw"
record_retrieval_provenance = true   # defaults to `enabled`
model_id = "BAAI/bge-large-en-v1.5@<sha>"
auto_fetch = false
# dir, model_path, model_source optional

[typedb]
endpoint = "localhost:1729"
mode = "orchestrated"      # | "compose" | "external"
```

The repository's own `gecko.toml` enables `cyber` and the index with the stub embedder.

### Runtime-asset paths (`gecko-bin/src/config/paths.rs`)

- **Cache root** (`cache_dir`): `GECKO_CACHE_DIR` env > `cache_dir` key > `$XDG_CACHE_HOME/gecko` > `~/.cache/gecko`. It is created and checked writable. The model, index files and downloaded TypeDB all live here, outside the build tree.
- **Model dir** (`model_path`): `model_path` override, else `cache_dir/models/<model_id>/`, content-addressed by the pinned id.
- **Index** (`index_dir`/`index_file`): `[semantic_index] dir`, else `cache_dir/index/`, one `<database>.hnsw` per database. A legacy `path` key is refused with guidance.

<!-- openwiki: broken internal link [/openwiki/semantic-index/index-and-embedders.md] link "/openwiki/semantic-index/index-and-embedders.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
The model id is single-sourced from `PINNED_MODEL_REVISION`; the `BAAI/` org prefix is required. See [Semantic Index and Embedders](/openwiki/semantic-index/index-and-embedders.md).

## TypeDB run modes

`cmd_up`/`cmd_down` switch on `[typedb] mode`:

- **orchestrated** (default) — `orchestrator::up` downloads the hard-pinned TypeDB `PINNED_TYPEDB = "3.15.0"` once into the cache, then spawns `typedb_server_bin` as a managed child on the endpoint's port. The data dir is under the cache, and HTTP and diagnostics are disabled so ports 8000/4104 are not taken. It waits up to 60 s for readiness (a driver connect plus `databases().all()`) and records a pidfile. If a server is already running it is a no-op. `down` reads the pidfile and sends SIGTERM, waits 15 s, then SIGKILL.
- **compose** — prints `docker compose up -d` (see `docker-compose.yml`); gecko manages nothing.
- **external** — connects to the configured endpoint and never spawns.

The pinned server version must match the `typedb-driver ~3.15` line; bump both together.

## `gecko doctor`

`gecko-bin/src/doctor.rs` runs independent checks, each `Ok | Warn | Fail` with a remediation:

- `config` — present and parseable; loaded leniently so doctor runs even on a broken config.
- `cache_dir` — writable.
- `typedb` — endpoint answers and server version matches the pin. A dead endpoint is a Fail in orchestrated mode and a Warn in external mode.
- `model` — staged when the index needs the real embedder.
- `index` — directory state, read without side effects.
- `embedder` — matches the build flavor.

The exit code is non-zero **iff** a check is `Fail`; warnings pass. `scripts/setup.sh` relies on this for idempotency.

## Packaging

- `.github/workflows/release.yml` builds `cargo build --release --features real-embedder --bin gecko` for `aarch64-apple-darwin`, `x86_64-apple-darwin` and `x86_64-unknown-linux-gnu` on tags/releases. Archives contain only the binary: no model, no TypeDB.
- `scripts/install.sh` installs the latest release binary (`GECKO_INSTALL_DIR`, `GECKO_VERSION`, `GECKO_REPO` overrides).
- `HomebrewFormula/gecko.rb` is a tap template. `Dockerfile` and `docker-compose.yml` cover container use.

## Tests

- `gecko-bin/tests/config_init_test.rs` — `gecko init` idempotency.
- `gecko-bin/tests/orchestrator_test.rs` — orchestrated lifecycle.
- `gecko-bin/tests/run_args_cli_test.rs`, `run_missing_database_test.rs`, `index_per_database_cli_test.rs` — CLI behavior.
- Unit tests in `config.rs`, `config/paths.rs`, `doctor.rs`.
