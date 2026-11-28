---
type: subsystem
title: TypeDB Access, Schemas and Migrations
description: How GECKO connects to TypeDB 3.15 through TypeDbRouter and GraphHandle, runs parameterized reads and atomic write batches via RouterGraphStore, applies the core and extension schemas re-appliably with self-detecting migrations, and what the core record schema contains.
tags: [typedb, database, schema, migrations, typeql]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-28T02:42:19.865Z
sources:
  - id: openwiki-source-1077e5ea2e7746667e674efb
    resource: repo://core/gecko-engine/src/db/graph_store.rs
  - id: openwiki-source-3fa592df9cb836d7cdce9329
    resource: repo://core/gecko-engine/src/db/label.rs
  - id: openwiki-source-0f462ec99ef6573573acd677
    resource: repo://core/gecko-engine/src/db/mod.rs
  - id: openwiki-source-7a7c44819969ff555cce5a8d
    resource: repo://core/gecko-engine/src/db/required.rs
  - id: openwiki-source-05ffb99a5b34dc14304c3e9a
    resource: repo://core/gecko-engine/src/db/router.rs
  - id: openwiki-source-0ba946525d5c7edef154504a
    resource: repo://core/gecko-engine/src/db/schema_apply.rs
  - id: openwiki-source-25eb5271b2078890188e98ad
    resource: repo://core/gecko-engine/src/syncer/bundle.rs
  - id: openwiki-source-85cce32ba4f46a46665d45fd
    resource: repo://core/gecko-extension-api/src/migration.rs
generated: { by: "claude-code", at: "2026-11-28T02:42:19.865Z" }
---

# TypeDB Access, Schemas and Migrations

All TypeDB access lives in `core/gecko-engine/src/db`. The engine pins `typedb-driver ~3.15` (workspace `Cargo.toml`) because the driver's wire protocol must match the server; driver and server are bumped together.

## Connections: `TypeDbRouter` and `GraphHandle`

`TypeDbRouter` (`db/router.rs`) owns the driver lifecycle for one `DbConfig` (`address`, `database`, `username`, `password`, `tls`; defaults `localhost:1729`, `gecko`, `admin`/`password`, TLS disabled). It connects lazily, and offers:

- `ensure_database`, `list_databases`, `database_schema` (exported `define` text), `server_version` (used by `gecko doctor`).
- `begin_read`, `begin_write`, `begin_schema` for callers that hold the router exclusively.
- `delete_database`, which **refuses the reserved `gecko` database**.
- `graph_handle()`, a cheap cloneable `GraphHandle` for the shared hot path: reads open lock-free; writes acquire a single-permit semaphore *before* opening the transaction, and the returned `WriteTransaction` holds the permit until it is committed or dropped, so belief-tier commits never overlap.

`DbError` carries typed refusals the syncer raises — `ConceptIdCollision`, `BeliefTierWrite`, `TypeChangeReferenced`, `TypeChangeShared`, `InvalidTimeout` — alongside `Connection`/`Transaction`/`Schema`/`Query`.

## The neutral graph seam

`gecko-extension-api/src/graph.rs` defines driver-free traits so mem and extensions never see `typedb-driver`:

- `GraphValue` — typed scalar params (string, datetime, double, integer, boolean).
- `GraphWrite` — one query with named `given` vars and value rows; `.expecting(n)` requires exactly `n × rows.len()` answer rows.
- `GraphReader::read` / `read_rows` — parameterized reads that must end in `fetch`.
- `GraphStore: GraphReader` — `write(&[GraphWrite])`.

`RouterGraphStore` (`db/graph_store.rs`) implements them over a `TypeDbRouter`. `write` runs the whole slice in **one transaction** through `run_graph_writes` and commits once: empty-row ops are skipped, parameterless ops run as text, others bind a `given` stage, and an `expect_rows` mismatch returns a `RowCountMismatch` error so the transaction is dropped uncommitted. This atomicity is what lets a belief, its provenance stamp, derivation and evidence land together or not at all. `read_rows` refuses mismatched row lengths and issues no query for empty rows.

### Parameterization rule

Values always travel through `given` rows. Only schema labels are interpolated into query text, and an extension-supplied label must first pass `validate_schema_label` (`db/label.rs`): non-empty ASCII alphanumerics, `-` or `_`, no leading digit.

## Applying schemas

`TypeDbRouter::apply_schema_with_migrations(schema, migrations)`:

1. `ensure_database`, then export the current schema text.
2. Select pending migrations: those whose `applies_when` text is still present.
3. `schema_apply::plan_queries` splits the fragment into type statements and individual `fun` blocks, compares each function with the stored one, and orders queries: types first, then new functions `define`d and changed ones `redefine`d **callee-first**; unchanged functions are skipped (TypeDB refuses re-`define` of an existing function, `[FUN5]`). Functions the database holds but the fragment omits are left in place.
4. Run pending migrations, then the planned queries, all in **one schema transaction**, then commit.

<!-- openwiki: broken internal link [/openwiki/architecture/assembler-and-extensions.md] link "/openwiki/architecture/assembler-and-extensions.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
The binary's `apply_all_schemas` drives this for core, then each registered extension; see [Assembler Pattern and Extension Contract](/openwiki/architecture/assembler-and-extensions.md).

### Migrations

`SchemaMigration { id, applies_when, tql }` covers changes `define` cannot re-apply — chiefly changed annotations (`@doc`, `@card`, `@values`, `[DEX15]`). Because a migration runs only while its marker text is present, it is skipped on fresh databases and on already-migrated ones (where an identical `redefine` would fail, `[REX25]`).

`CORE_MIGRATIONS` (`db/mod.rs`) currently redefines the `bundle` `@doc` for shared databases and adds cleanup migrations that delete `containment`, `hierarchy`, `concept-link` and `citation` rows missing a role, so the `define` adding `@card(1)` commits.

`gecko_extension_api::migration::check_migrations` is a test-time shape check every fragment runs over its migration list. It accepts only: role-cleanup deletes for roles declared `@card(1)`/`@card(1..)`, type-removal deletes followed by an `undefine`, `@range`-bounded `update`s, `undefine @doc from X`, `undefine` of types/functions the shipped schema no longer declares, and `redefine` of text the shipped schema contains. It also requires that no `applies_when` text remains in the shipped schema.

### Reading annotations from schema text

TypeDB 3.15 cannot query annotations, so `db/required.rs` parses the exported schema text: `required_attributes` returns, per entity type, the attributes owned with `@key` or a minimum cardinality ≥ 1 (inherited through `sub`, overridden by a subtype's own `owns`), and `defines_attribute` tests for an attribute label. The syncer uses the former to keep required attributes when retiring a concept; `fetch_playbook` uses the latter to project `retired-at`/`timeout-ms` only where defined.

## The core record schema

`core/gecko-engine/schema/core_schema.tql` (embedded as `CORE_SCHEMA`) defines the record tier:

- `okf-concept` (abstract; `concept-id @key`) and its concrete subtype `concept` (body, code blocks, tags, `engine`, `scope`s, `metadata-json`, `file-hash`, `synced-at`, `retired-at`, `timeout-ms @range(1..)`).
- `bundle` keyed by `bundle-name`, playing `containment:container`.
- `external-resource` keyed by `target-url`.
- Relations `containment`, `hierarchy`, abstract `okf-link` with subtypes `concept-link` and `citation`; every role is `@card(1)`.

<!-- openwiki: broken internal link [/openwiki/memory/epistemic-substrate.md] link "/openwiki/memory/epistemic-substrate.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
<!-- openwiki: broken internal link [/openwiki/okf/graph-sync.md] link "/openwiki/okf/graph-sync.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Every type, role and function carries a one-sentence `@doc` that clients can read at runtime. The belief tier lives in mem's schema ([Epistemic Memory Substrate](/openwiki/memory/epistemic-substrate.md)); `db/surface.rs` introspects it for the sync guard ([Bundle Sync into TypeDB](/openwiki/okf/graph-sync.md)).

## Tests

- `core/gecko-engine/src/db/mod.rs` `core_migrations_lead_to_the_shipped_schema` and the `schema_apply`/`required` unit tests (serviceless).
- `gecko-bin/tests/schema_init_test.rs` — schema init against live TypeDB, including migrations of older cyber/mem schemas.
- `core/mem-gecko/tests/schema_migration_test.rs` — mem migrations apply once to an existing database.
