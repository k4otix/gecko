---
type: architecture
title: Assembler Pattern and Extension Contract
description: How GECKO composes the core engine with domain extensions at compile time, activates them at runtime from gecko.toml, applies their schema fragments, and services sandbox host calls through call_import or a composed host bridge.
tags: [architecture, extensions, assembler, host-bridge, schema]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-28T02:42:19.865Z
sources:
  - id: openwiki-source-970e511107776faf68c7ab0f
    resource: repo://core/gecko-extension-api/src/lib.rs
  - id: openwiki-source-0ce57d2c565c71dba9a43b7b
    resource: repo://gecko-bin/src/config.rs
  - id: openwiki-source-c42ae122f8faf30c8246ac70
    resource: repo://gecko-bin/src/main.rs
  - id: openwiki-source-a406fa03fc281d8b70222054
    resource: repo://scripts/check-extension-deps.sh
generated: { by: "claude-code", at: "2026-11-28T02:42:19.865Z" }
---

# Assembler Pattern and Extension Contract

GECKO ships as one binary (`gecko-bin`) that statically links the core engine with every compiled-in domain extension. Which extensions are *activated* is decided at runtime by `gecko.toml`. The extension contract itself lives in the thin `gecko-extension-api` crate so extensions never depend on the engine.

## Crate boundary

- `core/gecko-extension-api` holds the `GeckoExtension` trait, `HostImportDef`, the `ExtensionCallback` type, `SchemaMigration`, and the epistemic/graph/belief-edge contracts (`EpistemicWriter`, `GraphReader`, `BeliefEdgeWriter`, `RunContext`, …).
- `core/gecko-engine` re-exports these for its internal plumbing (`core/gecko-engine/src/extension.rs`). The engine's golden rule, stated on the trait, is that it never imports anything under `extensions/`.
- Extension crates under `extensions/` must depend on neither `gecko-engine` nor `typedb-driver` in normal, build, or dev dependency trees. `scripts/check-extension-deps.sh` walks `cargo metadata`/`cargo tree` for every crate under `extensions/` and fails CI on a match.

## The `GeckoExtension` trait

| Method | Role |
| --- | --- |
| `name()` | Stable id (`"mem-gecko"`, `"cyber-gecko"`); used for host-call routing and as the S3 scope prefix (`"{ext}:{func}"`). |
| `schema()` | TypeQL fragment applied after core (and after mem, for domain extensions). |
| `schema_migrations()` | Self-detecting `SchemaMigration`s run before `schema()` in the same schema transaction (default empty). |
| `belief_edges()` | Relations this extension attaches to a belief, written atomically through `BeliefEdgeWriter` (default empty). |
| `host_imports()` | Declared callable host functions. |
| `on_init()` | Hook run after the extension's schema is applied. |
| `call_import(name, args)` | Synchronous dispatch; the default errors with `Import '<name>' not implemented`. |

<!-- openwiki: broken internal link [/openwiki/database/typedb-and-schemas.md] link "/openwiki/database/typedb-and-schemas.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
A `SchemaMigration` runs only while the database's exported schema text still contains its `applies_when` marker, so it is skipped on fresh databases and on ones already migrated. See [TypeDB Access, Schemas and Migrations](/openwiki/database/typedb-and-schemas.md).

## Registration and activation

`build_extensions` (`gecko-bin/src/config.rs`) is the activation gate:

1. `mem_gecko::extension()` is always pushed first, regardless of config. Order matters: registration order is schema-application order, and domain extensions subtype mem types.
2. Each name in `[extensions] enabled` is matched: `"mem"` is a no-op; `"cyber"` pushes `cyber_gecko::extension()` when compiled with the `cyber` feature, and is a hard error naming the missing feature otherwise; any other name is `unknown or not-compiled extension`.

Only registered extensions get host functions dispatched and write paths opened. Extensions are built *per command* inside the `SchemaInit`, `Sync`, `SyncStix`, `SyncSensors`, and `Run` arms of `gecko-bin/src/main.rs`, so a typo in `enabled` cannot break `up`, `down`, `query`, `status`, `drop`, `databases`, or `model`.

Registration gates runtime activation, not schema reversibility: a `define` an extension once applied (for instance a `plays` added onto a mem type) persists in that database after the extension is disabled (invariant 3 as scoped in `docs/refactor/INVARIANTS.md`).

## Schema application

`apply_all_schemas` (`gecko-bin/src/main.rs`) applies schemas in this order:

1. `check_extension_schemas` builds mem's `BeliefEdgeRegistry` from the core schema and every extension, refusing — before any database contact — a fragment or migration that `undefine`s/`redefine`s core or mem types, renames a type or role, reuses a core/mem function name, or contains the reserved token `alias`.
2. The core schema (embedded `CORE_SCHEMA` with `CORE_MIGRATIONS`, or a custom `--schema` file, which never migrates).
3. Each extension's migrations then `schema()`, then `on_init()`.

Fragment authoring rules (subtype the concrete `concept` for records, persist queryable fields as typed `owns` via `OkfConcept.typed_attributes`, put a one-sentence `@doc` on every type/role/function) are documented in `extensions/README.md`.

## Host-import seams

<!-- openwiki: broken internal link [/openwiki/sandbox/wasm-executor.md] link "/openwiki/sandbox/wasm-executor.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
A playbook calls `gecko.callExtension(ext, func, args)`. The sandbox checks the S3 scope before anything else (see [WASM Sandbox Executor and Host ABI](/openwiki/sandbox/wasm-executor.md)), then invokes a single `ExtensionCallback` (`Arc<dyn Fn(&str, &str, Value) -> Result<Value, String>>`). Two seams service that callback:

- **`call_import`** — synchronous, receives only `name` and `args`; no graph, no writer, no run identity. Missing or invalid fields must error, never default.
- **Host bridge** — a wrapper callback that captures host-owned capabilities, intercepts its own extension's functions, and delegates every other call to the callback it wraps. Bridges drive async work with `tokio::task::block_in_place` + `Handle::block_on`, which requires the multi-thread runtime.

### `build_host_bridge`

`build_host_bridge` (`gecko-bin/src/main.rs`) composes the chain for one `gecko run`:

```
cyber_extension_callback (if cyber-gecko registered)
  └─ epistemic_extension_callback (mem.*)
       └─ base_cb: match ext.name() == ext_name → ext.call_import(func, args)
```

- It re-runs `check_extension_schemas` so a bad declaration fails the run before anything executes.
- It builds one `MemWriter` over a `RouterGraphStore`; the same object is the `EpistemicWriter` and, via `as_epistemic_reader`, the reader for `mem.recall`, so recall and derive share per-run scratch.
- It mints exactly one `RunContext` per run (actor `system`, source `ExecutableDoc { concept_id }`). The sandbox can never construct one.
- The cyber bridge gets the store as a read-only `GraphReader` and a `BeliefEdgeWriter` bound to cyber-gecko's declared edges (`MemWriter::edge_writer_for("cyber-gecko")`), which fails if the registry has no cyber edges.

<!-- openwiki: broken internal link [/openwiki/memory/sandbox-mem-bridge.md] link "/openwiki/memory/sandbox-mem-bridge.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
<!-- openwiki: broken internal link [/openwiki/extensions/cyber-gecko.md] link "/openwiki/extensions/cyber-gecko.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
No extension is ever handed mem's `EpistemicWriter`; the binary binds it to mem's own bridge only. The bridge type lives in the API crate so an extension can define its bridge without the engine, but the fold happens in the binary because the host owns everything a bridge captures. Details of each bridge: [Sandbox → mem Bridge](/openwiki/memory/sandbox-mem-bridge.md) and [cyber-gecko](/openwiki/extensions/cyber-gecko.md).

## Adding an extension

1. Crate under `extensions/<name>/` depending only on `gecko-extension-api`.
2. Implement `GeckoExtension`; expose an `extension()` constructor.
3. Add a feature-gated arm in `build_extensions` and, if it needs graph/belief/identity access, a fold in `build_host_bridge`.
4. Grant scopes in playbook frontmatter (`scopes: [<name>:<func>]`).
5. Test schema application on a throwaway database, bad-input rejection in imports, and an end-to-end sync/run.

## Representative tests

- `gecko-bin/src/main.rs` unit test `apply_all_schemas_refuses_before_touching_the_database` — a modifying extension fragment is refused with `RegistryError::ModifiesBase` against an unreachable address.
- `gecko-bin/src/config.rs` tests — `enabled` parsing and semantic-index defaults.
- `gecko-bin/tests/schema_init_test.rs` — core + extension schemas apply against live TypeDB.
- `gecko-bin/tests/belief_edge_registry_test.rs` — registry refusals for extension fragments.
