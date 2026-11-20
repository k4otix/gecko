# Several Bundles in One Database (O1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let several bundles share one TypeDB database:
- a sync acts only on the concepts its own bundle contains;
- a concept ID held by another bundle (or by none) is refused;
- links and hierarchy resolve within the bundle;
- `sync-stix` gains `--bundle-name`;
- the core schema's `bundle` `@doc` states the new rule, reaching existing databases through a core-schema migration.

**Architecture:**
- **gecko-engine `syncer/bundle.rs`:** one membership read (concept-id, file-hash, containing `bundle-name`s) replaces `fetch_existing_hashes`. Pure helpers split it into this bundle's owned hashes (fed to the unchanged `plan_sync`) and collisions, which are checked before any write. GC deletes only concepts this bundle owns alone, and releases this bundle's containment from a concept another bundle also contains. Containment, hierarchy and link queries bind the bundle by name.
- **gecko-engine `db/router.rs`:** gains `DbError::ConceptIdCollision`.
- **gecko-engine `db/mod.rs`:** gains `CORE_MIGRATIONS`, applied by `apply_all_schemas` through the existing `apply_schema_with_migrations`, as mem's `MEM_MIGRATIONS` are.
- **gecko-bin:** `sync-stix` takes `--bundle-name` and records the real file path as `bundle-path`.
- **Tests:** a new `multi_bundle_test.rs` holds the library tests (un-gated) and the CLI tests (`cyber`), which drive the `gecko` binary.

**Tech Stack:** Rust 2024 workspace, TypeDB 3.15 (`typedb-driver ~3.15`), tokio, clap, thiserror, tempfile, nextest.

**Spec:** `docs/superpowers/specs/2026-11-20-multi-bundle-databases-design.md` (ledger item O1). This is PR 1 of 2 on branch `feat/multi-bundle-databases` from `main`. The A4 + A10 belief-tier-guard plan stacks on it.

## Global Constraints

- **Parameterized TQL.** All values, including the bundle name, go through `given` rows. Only fixed schema labels are interpolated.
- **Greenfield comments.** No plan or epic tags (`O1`, `S5`, …) and no changelog phrasing in code comments ("no longer", "now"). The CLAUDE.md "Comments are greenfield" rule applies.
- **Never fabricate success on bad input.** A membership row without a string `id` is an error, not a skipped row. A stream `Some(Err(e))` is an error, not the end of the stream. An empty `--bundle-name` is refused.
- **Integration tests use throwaway databases** via `common::TestDb`. Never touch the database named `gecko`. Run them with `--test-threads=1`. CLI tests pass `--database <TestDb name>` on every call, because `gecko run` defaults to `gecko`.
- **Lint and format must pass, both clippy runs, as CI runs them (`.github/workflows/ci.yml:137,140`):**
  - `cargo clippy --locked --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`
  - `cargo clippy --locked --workspace --exclude js-sandbox --no-default-features --features cyber --all-targets -- -D warnings`
  - `cargo fmt --all -- --check`
  - Every task runs all three before its commit. Code reachable only under `cyber` is `#[cfg(feature = "cyber")]`-gated. Never use `#[allow(dead_code)]`.
- **`Cargo.lock` is tracked and CI uses `--locked`.** No task should need a new dependency. If one does, commit the lockfile.
- **Commits:** end every message with the co-author attribution line your session specifies. Commit on `feat/multi-bundle-databases` only, and never push.
- **Unchanged behaviour for one bundle per database:**
  - Every existing assertion in `bundle_sync_test`, `no_drag_test`, `sandbox_test` and `cyber_stix_sync_test` passes unchanged. The three `plan_sync` unit tests pass unchanged.
  - `cyber_detect_test` keeps every assertion. Only its setup changes (Task 5).
  - `SyncResult` gains one field, `concepts_released`. No existing field changes meaning.
- **Signatures held for the stacked plan:**
  - `sync_bundle(db: &mut TypeDbRouter, manifest: &OkfBundle, extension_writes: &[GraphWrite]) -> Result<SyncResult, DbError>` keeps exactly these parameters. PR 2 adds `belief_tier_root: &str`.
  - `plan_sync(concepts: &[OkfConcept], existing: &HashMap<String, Option<String>>) -> SyncPlan` keeps its signature.
- **Concept IDs stay plain OKF paths.** No bundle prefix, no linker namespacing. `to_okf` keeps `bundle_name = "stix-sync"` and `bundle_path = "stix-sync"`.
- **A changed `@doc` ships with its migration.** TypeDB refuses a changed type annotation under `define` (`[DEX15]`). The only `@doc` this plan changes is `bundle`'s, and Task 3 ships it together with its `CORE_MIGRATIONS` entry. Every other `@doc` stays as it is.

## Review Focus

1. **A bundle renamed in `bundle.json`** on an existing database must be refused. The refusal names the old bundle, says to sync under the old name or into a fresh database, and writes nothing. Task 1 pins this with `renamed_bundle_is_refused_with_guidance`.
2. **A bundle with no concepts** synced into a shared database must delete nothing that belongs to the other bundles. Task 1 pins this with `empty_bundle_deletes_nothing_from_others`.
3. **An edited concept in a shared database** goes through the in-place update, which clears its outgoing links. It must re-derive them within its own bundle, exactly once. Task 2 pins this with `updated_concept_relinks_within_its_bundle`.
4. **A collision with hundreds of IDs** (a second ATT&CK domain) must give a readable error: at most 20 IDs listed plus a count of the rest, while the variant still carries every ID. Task 1 pins this with `collision_message_lists_at_most_twenty_ids`.
5. **`sync-stix --bundle-name` without `--database`** must sync into the database named after the overridden bundle, as `sync` does. An empty name must be refused before any database contact. Task 4 pins this with `bundle_name_override_selects_the_default_database` and `stix_naming_refuses_an_empty_bundle_name`.

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `core/gecko-engine/src/db/router.rs` | modify | `DbError::ConceptIdCollision`, `collision_message`, unit test |
| `core/gecko-engine/src/syncer/bundle.rs` | modify | membership read, `StoredConcept`, `owned_hashes`, `find_collisions`, `split_gc`, `release_containment`, `SyncResult.concepts_released`, bundle-scoped containment/hierarchy/links, module docs |
| `core/gecko-engine/src/db/mod.rs`, `core/gecko-engine/schema/core_schema.tql:52` | modify | `CORE_MIGRATIONS`; the `bundle` `@doc` |
| `gecko-bin/tests/schema_init_test.rs` | modify | the live core-migration upgrade test |
| `gecko-bin/tests/multi_bundle_test.rs` | create | library multi-bundle tests; `cfg(feature = "cyber")` CLI tests |
| `gecko-bin/src/main.rs` | modify | `concepts_released` line in `print_sync_result`; core migrations in `apply_all_schemas`; `SyncStix { bundle_name }`, `apply_stix_naming` + unit tests; doc comments |
| `gecko-bin/tests/cyber_detect_test.rs` | modify | seed the logic concept through a synced playbook bundle |
| `core/gecko-engine/src/okf/{parser,types,linker}.rs`, `core/gecko-engine/schema/core_schema.tql:50-51` (`#` comment), `gecko-bin/tests/sandbox_test.rs:52` (comment) | modify | the new rule in comments |
| `CLAUDE.md`, `README.md` | modify | the new rule |
| `docs/refactor/OWED-WORK-LEDGER.md` | modify | Track O, S5, D-N, A4 blocker, sequence |

---

### Task 1: Bundle ownership, collision refusal, scoped containment and GC

**Files:**
- Modify: `core/gecko-engine/src/db/router.rs:21-34` (the `DbError` enum); add a `#[cfg(test)] mod tests` at the end
- Modify: `core/gecko-engine/src/syncer/bundle.rs`:
  - module doc `:1-5`;
  - `sync_bundle` `:129-223`;
  - `upsert_bundle` doc `:262-264`;
  - `SyncResult` `:42-51` (add `pub concepts_released: usize`);
  - `fetch_existing_hashes` `:305-330` (replace it);
  - `create_containment` `:602-616`;
  - the tests module `:714-816`.
- Modify: `gecko-bin/src/main.rs:666-674` (`print_sync_result` prints `Concepts released:`)
- Create: `gecko-bin/tests/multi_bundle_test.rs`

**Interfaces:**
- Produces in `router.rs`:
  - `DbError::ConceptIdCollision { bundle: String, ids: Vec<(String, Option<String>)> }`. `bundle` is the incoming bundle's name. `ids` holds **every** colliding ID, sorted by ID, each paired with the bundle that holds it (`None` for a concept no bundle contains).
  - `fn collision_message(bundle: &str, ids: &[(String, Option<String>)]) -> String`, used as the variant's `#[error]` text (`#[error("{}", collision_message(.bundle, .ids))]`, or a manual `Display` if thiserror rejects that).
  - The message lists at most the first 20 entries, each as `` `id` (bundle `x`) `` or `` `id` (no bundle) ``, then `and N more` when there are more than 20. It ends with the guidance: concept IDs must be unique per database, so sync this bundle under the name that holds them, or into a fresh database with `--database`. Exact wording is the builder's.
- Produces in `bundle.rs`:
  - `#[derive(Debug, Clone, Default, PartialEq, Eq)] pub struct StoredConcept { pub hash: Option<String>, pub bundles: Vec<String> }`. `bundles` holds the names of every bundle containing the concept, sorted and deduplicated. It is empty for an uncontained concept.
  - `pub fn owned_hashes(bundle_name: &str, stored: &HashMap<String, StoredConcept>) -> HashMap<String, Option<String>>` returns the concepts whose `bundles` contains `bundle_name`. This is what `plan_sync` receives.
  - `pub fn find_collisions(bundle_name: &str, concepts: &[OkfConcept], stored: &HashMap<String, StoredConcept>) -> Vec<(String, Option<String>)>` returns every manifest ID that is stored but whose `bundles` lacks `bundle_name`. The holder is `bundles.first().cloned()`. The result is sorted by ID.
  - `async fn fetch_stored_concepts(tx: &Transaction) -> Result<HashMap<String, StoredConcept>, DbError>` replaces `fetch_existing_hashes`. The starting query shape (the spec leaves the exact shape to the builder):
    ```
    match $c isa concept, has concept-id $id;
          try { $ct isa containment, links (container: $b, member: $c); $b has bundle-name $bn; };
    fetch { "id": $id, "hash": $c.file-hash, "bundle": $bn? };
    ```
    - A concept appears once per containment, so fold rows by ID.
    - A `Some(Err(e))` from the stream returns `DbError::Query`.
    - A document without a string `id` returns `DbError::Query`.
    - Convert documents as `fetch_playbook` does (`serde_json::to_value(doc.into_json())`), not with `unwrap_or_default`.
  - `async fn create_containment(tx: &Transaction, bundle_name: &str, ids: &[&str]) -> Result<(), DbError>`. The match binds `$b isa bundle, has bundle-name $bn; $bn == $name;` with `$name` as a second `given` column.
  - `pub fn split_gc(bundle_name: &str, to_delete: &[String], stored: &HashMap<String, StoredConcept>) -> (Vec<String>, Vec<String>)` returns `(delete, release)`, both sorted. An ID whose `bundles == [bundle_name]` goes to `delete`. An ID that another bundle also contains goes to `release`.
  - `async fn release_containment(tx: &Transaction, bundle_name: &str, ids: &[&str]) -> Result<(), DbError>` deletes only the `containment` between this bundle and each concept. The concept, its other containments and its links stay.
  - The read covers `concept` only. A manifest ID equal to a memory item's ID (mem's `mem/…` namespace) is not reported as a collision. It fails at insert as TypeDB's key violation, and the sync still commits nothing. The `find_collisions` doc says so.
  - `SyncResult.concepts_deleted` counts `delete`. `concepts_released` counts `release`, and the `info!` line logs it.
- Consumes: `plan_sync`, `delete_concepts_gc`, `OkfBundle.bundle_name`, `common::TestDb`.

**`sync_bundle` order:**
1. Fetch stored concepts.
2. Compute `find_collisions`. If it is non-empty, return `Err(ConceptIdCollision)` before `upsert_bundle`. The transaction drops uncommitted.
3. Call `upsert_bundle`.
4. `plan_sync(&manifest.concepts, &owned_hashes(..))`, then `split_gc` over `plan.to_delete`.
5. `delete_concepts_gc` on `delete`, and `release_containment` on `release`.
6. Continue as today: update, insert, then containment (now by name).

A concept with several containments exists in legacy databases: the old `create_containment` contained each written concept in every `bundle` in the database. Each of its bundles owns it for change detection and updates. GC by one of them releases only that bundle's containment. It is a collision for every other bundle.

- [ ] **Step 1: Write the failing unit tests.**
  - In `bundle.rs` tests (reuse the `concept(id, hash)` helper; `stored(hash, &[bundles])` builds a `StoredConcept`):
    - `owned_hashes_keeps_only_this_bundles_concepts`: stored `a` held by `["alpha"]`, `b` by `["beta"]`, `c` by `[]`. `owned_hashes("alpha", ..)` has exactly the key `a`.
    - `find_collisions_reports_other_and_uncontained_holders`: manifest `[b, c, d]` for bundle `alpha` over the same stored map gives exactly `[("b", Some("beta")), ("c", None)]`.
    - `concept_contained_by_two_bundles_is_owned_by_both`: stored `s` held by `["alpha", "beta"]`. `owned_hashes` for `alpha` and for `beta` each contain `s`. `find_collisions` for `alpha` and for `beta` is empty. For `gamma` it is `[("s", Some("alpha"))]`.
    - `gc_releases_a_shared_concept_and_deletes_an_exclusive_one`: stored `e` held by `["alpha"]` and `s` by `["alpha", "beta"]`. `split_gc("alpha", &["e", "s"], ..) == (vec!["e"], vec!["s"])`.
  - In `router.rs` tests:
    - `collision_message_lists_at_most_twenty_ids`: 25 IDs `id00`..`id24`, each held by `Some("alpha")`. The message contains `` `beta` `` (the incoming bundle), `id00` and `id19`. It does not contain `id20`. It contains `and 5 more` and `fresh database`.
    - `collision_message_names_an_uncontained_holder`: `[("x", None)]` gives a message containing `no bundle`.
- [ ] **Step 2: Write the failing integration tests** in `gecko-bin/tests/multi_bundle_test.rs` (un-gated; core + mem schema only).
  - **Helpers:**
    - `fn write_bundle(dir: &Path, name: &str, files: &[(&str, &str, &str)])` writes `bundle.json` `{"name": name}` and, for each `(path, title, body)`, the file `path` with `note(title, body)`.
    - `fn note(title: &str, body: &str) -> String` returns frontmatter `type: Note` and `title`, then the body. `write_bundle` renders each file through it, so it is used from Task 1 on.
    - `async fn apply_schemas(db: &TestDb)` applies `CORE_SCHEMA` and `MemGecko::new().schema()`.
    - `async fn sync_dir(db: &TestDb, dir: &Path) -> Result<SyncResult, DbError>` calls `parse_bundle(dir).expect("parse bundle")`, then `sync_bundle(.., &[])`.
    - `async fn count(db: &TestDb, query: &str) -> usize` counts the rows a match query returns.
    - Read helpers, each opening a fresh read transaction:
      - `concept_ids(db) -> BTreeSet<String>`;
      - `membership(db) -> BTreeSet<(String, String)>`, as (concept-id, bundle-name) over `containment`;
      - `bundle_names(db) -> BTreeSet<String>`;
      - `links(db) -> BTreeSet<(String, String)>`, as `concept-link` source and target IDs;
      - `hierarchy(db) -> BTreeSet<(String, String)>`, as parent and child IDs.
    - Every helper must be used by an un-gated test, so the cyber-less build has no dead code.
  - **Fixtures**, as `const` arrays of `(path, title, body)`:
    - `ALPHA` (name `alpha`):
      - `tables.md`;
      - `tables/x.md`;
      - `notes/a1.md`, whose body has `[x](/tables/x.md)`.
    - `BETA` (name `beta`):
      - `notes/b1.md`, whose body has `[b2](/notes/b2.md)` and `[x](/tables/x.md)`;
      - `notes/b2.md`;
      - `tables/y.md`.
  - `two_bundles_share_a_database`:
    - Sync `ALPHA`, then `BETA`. Each reports `concepts_inserted == 3`. Beta reports `concepts_deleted == 0`.
    - `membership` equals exactly `{(tables, alpha), (tables/x, alpha), (notes/a1, alpha), (notes/b1, beta), (notes/b2, beta), (tables/y, beta)}`. One containment per concept.
    - Snapshot `links`, `hierarchy`, and the `match $h isa hierarchy;` count. Re-sync `ALPHA`: `concepts_skipped == 3`, `concepts_deleted == 0`. `membership`, `links` and `hierarchy` equal their snapshots.
    - `match $r isa containment;` counts exactly 6, which catches a duplicate containment the set would hide. The hierarchy count equals its snapshot.
  - `resync_garbage_collects_only_its_own_bundle`:
    - Sync both bundles. Delete `notes/a1.md` from alpha's directory and re-sync alpha.
    - `concepts_deleted == 1`.
    - `concept_ids` is the six IDs minus `notes/a1`.
    - Every beta membership row is unchanged.
  - `colliding_concept_id_is_refused`:
    - Sync `ALPHA`. Snapshot `concept_ids`, `membership`, `links`, `hierarchy` and `bundle_names`.
    - Sync `BETA` plus `tables/x.md`. The result matches `Err(DbError::ConceptIdCollision { bundle, ids })` with `bundle == "beta"` and `ids == vec![("tables/x".into(), Some("alpha".into()))]`.
    - All five snapshots are unchanged. In particular, `bundle_names == {"alpha"}`, so no `beta` bundle entity was left behind.
  - `uncontained_concept_counts_as_a_collision`:
    - Raw-insert `insert $c isa concept, has concept-id "notes/b1";` and commit.
    - Sync `BETA`. It gives `ConceptIdCollision` with `ids == vec![("notes/b1".into(), None)]`.
    - `concept_ids == {"notes/b1"}`.
  - `renamed_bundle_is_refused_with_guidance` (Review Focus 1):
    - Sync `ALPHA`. Rewrite its `bundle.json` name to `atlas` and sync again. The new name shares no text with `alpha`, so the message check below can only pass on the holder's name.
    - The result is `ConceptIdCollision` with `bundle == "atlas"` and 3 ids, each `Some("alpha")`.
    - `err.to_string()` contains `alpha` and `fresh database`.
    - `bundle_names == {"alpha"}`.
  - `empty_bundle_deletes_nothing_from_others` (Review Focus 2):
    - Sync both bundles. Then sync a directory holding only `bundle.json` named `gamma`.
    - The result has `concepts_inserted == 0` and `concepts_deleted == 0`.
    - `concept_ids` is unchanged.
    - `bundle_names == {alpha, beta, gamma}`.
  - `shared_legacy_concept_is_released_not_deleted` (the legacy shape):
    - Raw setup in one write transaction: `bundle` entities named `alpha` and `beta`, a `concept` `notes/shared`, and two `containment`s linking it to each bundle. Also a `concept` `notes/beta-only`, contained by `beta` alone.
    - Sync `ALPHA`. It does not ship either ID. The result has `concepts_deleted == 0` and `concepts_released == 1`.
    - `concept_ids` still contains `notes/shared` and `notes/beta-only`. `membership` contains `(notes/shared, beta)` and no `(notes/shared, alpha)`.
    - Then sync a directory holding only `bundle.json` named `beta`. The result has `concepts_deleted == 2`, since `beta` now owns both alone, and `concept_ids` holds only alpha's three.
- [ ] **Step 3: Run the tests and confirm they fail.** Run `cargo test -p gecko-engine --lib --no-default-features -- syncer::bundle db::router` and `cargo test -p gecko-bin --test multi_bundle_test --no-default-features -- --test-threads=1`. Expected: compile errors on `StoredConcept`, `owned_hashes`, `find_collisions`, `split_gc`, `concepts_released` and `ConceptIdCollision`.
- [ ] **Step 4: Implement** the variant, `collision_message`, `StoredConcept`, `owned_hashes`, `find_collisions`, `split_gc`, `fetch_stored_concepts`, `release_containment`, the scoped `create_containment`, the `concepts_released` field and the `sync_bundle` order above.
  - Print `  Concepts released: {}` in `print_sync_result`, after the deleted line.
  - Rewrite the comments the change touches so they describe the code as it now stands:
    - The module doc `:1-5`: concept IDs are plain OKF paths, unique per database. A database holds one bundle by default and may hold several. Each sync acts only on the concepts its bundle contains, and refuses IDs that another bundle holds.
    - The step comments in `sync_bundle`.
    - The `upsert_bundle` doc: drop "singleton" and "exactly one bundle per database".
    - The `create_containment` doc.
    - The `sync_bundle` doc gains one sentence on the collision refusal.
    - The `delete_concepts_gc` doc says it is given only concepts this bundle owns alone. A short doc on `release_containment` says when it is used instead.
- [ ] **Step 5: Run the tests and confirm they pass.**
  - Run the Step 3 commands.
  - Run `cargo test -p gecko-bin --no-default-features --test bundle_sync_test --test no_drag_test --test sandbox_test -- --test-threads=1`.
  - Run `cargo test -p gecko-bin --no-default-features --features cyber --test cyber_stix_sync_test --test cyber_detect_test -- --test-threads=1`.
  - Run both clippy commands and fmt.
  - Expected: all PASS with no existing assertion edited, and clippy clean.
- [ ] **Step 6: Commit.** `feat(sync): scope a sync to its own bundle and refuse concept-ID collisions`

### Task 2: Links and hierarchy resolve within the bundle

**Files:**
- Modify: `core/gecko-engine/src/syncer/bundle.rs`, `create_hierarchy` `:618-637` and `create_links` `:639-666`
- Test: `gecko-bin/tests/multi_bundle_test.rs`

**Interfaces:**
- Consumes: Task 1's fixtures and read helpers.
- Produces:
  - `create_hierarchy(tx, manifest)` keeps its signature.
  - `create_links(tx, manifest) -> Result<usize, DbError>` keeps its signature and its attempted count.
  - Each query gains a `$name: string` given column (`manifest.bundle_name`) and adds, to its `match`:
    `$b isa bundle, has bundle-name $bn; $bn == $name; $m1 isa containment, links (container: $b, member: <end 1>); $m2 isa containment, links (container: $b, member: <end 2>);`
    - For links, the ends are `$s` and `$t`.
    - For hierarchy, they are `$p` and `$c`.
  - Citations stay unscoped, since `external-resource` is keyed by URL.

- [ ] **Step 1: Write the failing tests** in `multi_bundle_test.rs`.
  - `link_does_not_cross_bundles`:
    - Sync `ALPHA`, then `BETA`.
    - `links` equals exactly `{(notes/a1, tables/x), (notes/b1, notes/b2)}`. Beta's link to `/tables/x` stays unresolved, so `(notes/b1, tables/x)` is absent.
    - `hierarchy` equals exactly `{(tables, tables/x)}`. Beta's `tables/y` does not attach to alpha's `tables`.
  - `link_does_not_cross_bundles_in_either_order`: on a fresh database, sync `BETA`, then `ALPHA`, then `BETA` again. `create_links` re-runs every manifest link on each sync, so this is a second route to the leak. `links` equals exactly the same two-pair set, and `hierarchy` equals `{(tables, tables/x)}`.
  - `updated_concept_relinks_within_its_bundle` (Review Focus 3):
    - Sync both bundles.
    - Rewrite `notes/a1.md` with a new title and the same `[x](/tables/x.md)` link. Re-sync alpha.
    - `concepts_updated == 1`.
    - `links` still equals `{(notes/a1, tables/x), (notes/b1, notes/b2)}`. As a set equality, this proves the link exists once and no cross-bundle link appeared.
    - Count `match $l isa concept-link;` and assert it is 2, which catches a duplicate the set would hide.
- [ ] **Step 2: Run and confirm the failure.** Run `cargo test -p gecko-bin --test multi_bundle_test --no-default-features -- --test-threads=1`. Expected: both `link_does_not_cross_bundles` tests FAIL on the extra `(notes/b1, tables/x)` link and the `(tables, tables/y)` edge.
- [ ] **Step 3: Implement** the scoped `match` clauses. Rewrite the `create_links` doc: a link whose target is not in this bundle stays unresolved (OKF v0.2 §6.1). Add the same sentence for hierarchy edges to `create_hierarchy`'s doc.
- [ ] **Step 4: Run the tests and confirm they pass.** Run the Step 2 command, then the Task 1 Step 5 test commands, both clippy commands and fmt. Expected: all PASS, clippy clean.
- [ ] **Step 5: Commit.** `feat(sync): resolve links and hierarchy within the syncing bundle`

### Task 3: The `bundle` `@doc` and the core-schema migration

**Files:**
- Modify: `core/gecko-engine/schema/core_schema.tql:52` (the `bundle` `@doc`)
- Modify: `core/gecko-engine/src/db/mod.rs` (add `CORE_MIGRATIONS` and a `#[cfg(test)] mod tests`)
- Modify: `gecko-bin/src/main.rs:600-612` (`apply_all_schemas`)
- Test: `core/gecko-engine/src/db/mod.rs` tests; `gecko-bin/tests/schema_init_test.rs`

**Interfaces:**
- Consumes:
  - `gecko_extension_api::SchemaMigration { id, applies_when, tql }`;
  - `TypeDbRouter::apply_schema_with_migrations(&mut self, schema_content: &str, migrations: &[SchemaMigration])` (`router.rs:266-307`);
  - the precedent `MEM_MIGRATIONS` (`core/mem-gecko/src/lib.rs:50-54`) and its test `migrations_lead_to_the_shipped_schema` (`:121-143`).
- Produces: `pub const CORE_MIGRATIONS: &[SchemaMigration]` in `gecko_engine::db`, with one entry:
  - `id: "core/bundle-shared-database-doc"`;
  - `applies_when: "entity bundle @doc(\"The single OKF documentation bundle ingested into this database, keyed by its declared name; bundle-path is machine-local provenance, not identity.\")"`;
  - `tql: "redefine entity bundle @doc(\"<new sentence>\");"`.
- **New `@doc`, one greenfield sentence:** `An OKF documentation bundle synced into this database, keyed by its declared name and containing its concepts; several bundles may share a database, and bundle-path is machine-local provenance, not identity.` The `core_schema.tql` text and the migration's `tql` carry the identical sentence.
- **`apply_all_schemas`:** the embedded core schema goes through `db.apply_schema_with_migrations(&core_schema, gecko_engine::db::CORE_MIGRATIONS)`. A `--schema` custom core schema keeps plain `apply_schema`. A custom file may still hold the old `@doc`, which the migration would make fail under `define`.

- [ ] **Step 1: Write the failing tests.**
  - `core_migrations_lead_to_the_shipped_schema` in `db/mod.rs`, mirroring mem's test over `CORE_SCHEMA` and `CORE_MIGRATIONS`:
    - `CORE_SCHEMA` no longer contains any `applies_when`.
    - The statement each migration installs (its `tql` without `redefine ` and the trailing `;`) is contained in `CORE_SCHEMA`.
  - `previous_core_schema_upgrades_to_the_shipped_schema` in `gecko-bin/tests/schema_init_test.rs`, on a `TestDb`:
    - Build the previous schema as `CORE_SCHEMA.replace(<new @doc statement>, <old @doc statement>)`. Assert the replacement changed the text, so the test cannot pass vacuously. Apply it.
    - A plain `apply_schema(CORE_SCHEMA)` is `Err`, and its text contains `DEX15`.
    - Then run `apply_schema_with_migrations(CORE_SCHEMA, CORE_MIGRATIONS)` twice. Both are `Ok`; the second skips the migration.
    - `database_schema()` no longer contains any `applies_when`, and does contain the new sentence.
  - If the exported schema renders the `bundle` header differently from `entity bundle @doc(...)` (the first live run shows it), take `applies_when` from the exported text, as `mem/population-member-window-doc` did.
- [ ] **Step 2: Run and confirm the failure.** Run `cargo test -p gecko-engine --lib --no-default-features -- db::tests` and `cargo test -p gecko-bin --test schema_init_test --no-default-features -- --test-threads=1`. Expected: compile error on `CORE_MIGRATIONS`.
- [ ] **Step 3: Implement.** Change the `@doc` in `core_schema.tql`, add `CORE_MIGRATIONS` with a doc comment in the style of `MEM_MIGRATIONS`, and change the core call in `apply_all_schemas` as above.
- [ ] **Step 4: Run the tests and confirm they pass.**
  - Run the Step 2 commands.
  - Run `cargo test -p gecko-bin --bins --no-default-features`, which includes `apply_all_schemas_refuses_before_touching_the_database`.
  - Run `cargo test -p gecko-bin --no-default-features --test bundle_sync_test --test multi_bundle_test -- --test-threads=1`.
  - Run both clippy commands and fmt.
  - Expected: PASS, clippy clean.
- [ ] **Step 5: Commit.** `feat(schema): bundle @doc for shared databases, with a core-schema migration`

### Task 4: `sync-stix --bundle-name` and the real `bundle-path`

**Files:**
- Modify: `gecko-bin/src/main.rs`:
  - the `SyncStix` variant `:114-119`;
  - the `SyncStix` arm `:325-352`;
  - the tests module at `:1116`.
- Test: `gecko-bin/tests/multi_bundle_test.rs` (a new `#[cfg(feature = "cyber")] mod cli`)

**Interfaces:**
- Produces:
  - `SyncStix { bundle_path: PathBuf, #[arg(long)] bundle_name: Option<String> }`. The doc on `bundle_name`: the bundle name to import under, default `stix-sync`. Several imports share a database when each has its own name and they pass the same `--database`; without `--database`, the database is named after the bundle.
  - `#[cfg(feature = "cyber")] fn apply_stix_naming(manifest: &mut OkfBundle, bundle_name: Option<&str>, stix_file: &Path) -> Result<()>`:
    - `Some(name)` replaces `manifest.bundle_name`. A name that is empty after `trim()` is an error naming `--bundle-name`. A non-empty name is stored as given, untrimmed, the same as `bundle.json`'s `name` (`parser.rs` `read_bundle_manifest`).
    - It sets `manifest.bundle_path` to `std::fs::canonicalize(stix_file)` as a string (the same provenance form `parse_bundle` records). It uses the context "Failed to resolve STIX file path".
- Consumes: `cyber_gecko::stix::to_okf` (unchanged), `resolve_db`, `sync_manifest`.
- **Arm order:**
  1. `to_okf`.
  2. `apply_stix_naming` — before `resolve_db`, so the default database is the effective bundle name, which is still `stix-sync` when no override is given.
  3. `make_config`, then sync as today.
  - `apply_stix_naming` runs before any database contact, so an empty name fails without touching TypeDB.

- [ ] **Step 1: Write the failing unit tests** in the `main.rs` tests module, `#[cfg(feature = "cyber")]`. Each uses a `tempfile` STIX file and a manifest from `to_okf(r#"{"type":"bundle","objects":[]}"#)`.
  - `stix_naming_keeps_the_default_name`: with `None`, `bundle_name == "stix-sync"`, and `bundle_path == canonicalize(file)` as a string.
  - `stix_naming_overrides_the_name_and_records_the_file_path`: with `Some("attack-mobile")`, the name is `attack-mobile` and the path is the same as above.
  - `stix_naming_refuses_an_empty_bundle_name` (Review Focus 5): `Some("  ")` is `Err`, and `{:#}` contains `--bundle-name`.
- [ ] **Step 2: Write the failing CLI tests** in `multi_bundle_test.rs`, inside `#[cfg(feature = "cyber")] mod cli { use super::*; … }`.
  - **Helpers:**
    - `fn write_config(dir: &Path) -> PathBuf` writes a `gecko.toml` with `[typedb]` `endpoint = "localhost:1729"`, `mode = "external"`, and `[extensions]` `enabled = ["cyber"]`. There is no `[semantic_index]`, so the index is off.
    - `fn gecko(config: &Path) -> Command` returns `Command::new(env!("CARGO_BIN_EXE_gecko"))` with `--config`.
    - `fn run_ok(cmd: &mut Command) -> String` asserts success, printing stdout and stderr on failure, and returns stdout.
    - `fn run_err(cmd: &mut Command) -> String` asserts failure and returns stdout + stderr. The tracing `error!` line goes to stdout.
  - **STIX fixtures**, as consts written to the tempdir:
    - `ENTERPRISE`:
      - the MITRE `identity` `identity--c78cb6e5-0c4b-4611-8297-d1b8b55e40b5`;
      - the T1059 `attack-pattern` with the data-source/data-component/`detects` trio from `cyber_detect_test.rs:35-64`.
    - `ENTERPRISE_NEXT`: `ENTERPRISE` without the `x-mitre-data-source` object.
    - `MOBILE`: the same `identity`, plus an `attack-pattern` `attack-pattern--00000000-0000-0000-0000-0000000000b1` with `T1404`.
    - `OTHER`: an `attack-pattern` `…0000000000c1` with `T1003`, plus a `malware` `…0000000000c2`. It shares no IDs with the others.
  - `sync_stix_bundle_name_override`:
    - `sync-stix enterprise.json --database N` succeeds. `bundle_names == {"stix-sync"}`.
    - `sync-stix other.json --bundle-name other-intel --database N` succeeds.
      - `bundle_names == {"stix-sync", "other-intel"}`.
      - `OTHER`'s two concept IDs are contained only by `other-intel`.
      - `ENTERPRISE`'s IDs are contained only by `stix-sync`.
      - `other-intel`'s `bundle-path` equals `canonicalize(other.json)`.
    - `sync-stix enterprise-next.json --database N` succeeds.
      - `stix/x-mitre-data-source/00000000-0000-0000-0000-0000000000e5` is gone.
      - The other `stix-sync` IDs remain.
      - Every `other-intel` membership row is unchanged.
  - `second_attack_domain_is_refused`:
    - `sync-stix enterprise.json --bundle-name attack-enterprise --database N` succeeds.
    - `sync-stix mobile.json --bundle-name attack-mobile --database N` fails. Its output contains `stix/identity/c78cb6e5-0c4b-4611-8297-d1b8b55e40b5` and `attack-enterprise`.
    - `bundle_names == {"attack-enterprise"}`, and `concept_ids` is unchanged from after the first import.
  - `bundle_name_override_selects_the_default_database` (Review Focus 5):
    - `let target = TestDb::new("gecko_test_stix_name");`
    - `sync-stix other.json --bundle-name <target.name>` with **no** `--database` succeeds.
    - `bundle_names(&target) == {target.name}`.
    - The `TestDb` drop deletes that database.
  - Every other CLI test uses a `TestDb` passed as `--database`.
- [ ] **Step 3: Run and confirm the failure.** Run `cargo test -p gecko-bin --bins --no-default-features --features cyber stix_naming` and `cargo test -p gecko-bin --test multi_bundle_test --no-default-features --features cyber -- --test-threads=1 cli::`. Expected: compile error on `apply_stix_naming`, and clap rejecting `--bundle-name`.
- [ ] **Step 4: Implement** the variant field, `apply_stix_naming` and the arm order above.
- [ ] **Step 5: Run the tests and confirm they pass.** Run the Step 3 commands, `cargo test -p gecko-bin --bins --no-default-features`, both clippy commands and fmt. Expected: PASS, clippy clean.
- [ ] **Step 6: Commit.** `feat(cli): sync-stix --bundle-name, recording the STIX file as bundle-path`

### Task 5: Playbooks and ATT&CK in one database, end to end

**Files:**
- Modify: `gecko-bin/tests/multi_bundle_test.rs` (the `cli` module)
- Modify: `gecko-bin/tests/cyber_detect_test.rs:86-104` (`seed_detection_graph`)

**Interfaces:**
- Consumes: Task 4's `cli` helpers and the `ENTERPRISE` fixture; `parse_bundle` and `sync_bundle`.
- **Playbook fixture `DETECT_PLAYBOOK`**, at `playbooks/detect-t1059.md` in a bundle named `detect-playbooks`:
  - Frontmatter: `type: playbook`, `title: Detect T1059`, `engine: quickjs`, `scopes: [cyber-gecko:claim]`.
  - One `js` fence:
    `gecko.callExtension("cyber-gecko", "claim", { logic_id: "playbooks/detect-t1059", technique_id: "T1059", method: "external-tool", confidence: 0.9 });`

- [ ] **Step 1: Write the end-to-end test** `playbooks_and_attack_share_a_database` in the `cli` module.
  - Call the binary with `--database N` each time, in this order: `sync <playbook dir>`, then `sync-stix enterprise.json`, then `run playbooks/detect-t1059`. Syncing the playbooks first proves the STIX sync does not collect them.
  - The `run` stdout contains `Execution successful!`.
  - `match $d isa technique-detection;` counts exactly 1.
  - `bundle_names == {"detect-playbooks", "stix-sync"}`.
  - `membership` contains `("playbooks/detect-t1059", "detect-playbooks")` and `("stix/attack-pattern/00000000-0000-0000-0000-0000000000a1", "stix-sync")`.
- [ ] **Step 2: Change `cyber_detect_test` setup.**
  - `seed_detection_graph` keeps its signature. It creates a `TempDir` inside, which only needs to outlive `parse_bundle`.
  - It writes a `detect-playbooks` bundle holding `playbooks/detect-t1059.md` (`type: playbook`, no program needed) and syncs it with `sync_bundle(&mut db, &parse_bundle(dir).expect("parse playbook bundle"), &[])` **after** the STIX sync.
  - Delete the raw `insert $c isa concept …` transaction and update the doc comment to match.
  - Every assertion in the file stays as it is.
- [ ] **Step 3: Run the tests and confirm they pass.** Run `cargo test -p gecko-bin --no-default-features --features cyber --test multi_bundle_test --test cyber_detect_test -- --test-threads=1`. Expected: PASS. Before Task 1 this flow would have failed, because the second sync collected the first bundle's concepts. Then run both clippy commands and fmt.
- [ ] **Step 4: Commit.** `test: playbooks and an ATT&CK import share one database end to end`

### Task 6: Docs and the ledger

**Files:**
- Modify: `CLAUDE.md:47`, `README.md:121`
- Modify comments:
  - `core/gecko-engine/src/okf/parser.rs:39-41`, `:231-232`, `:313-315`;
  - `core/gecko-engine/src/okf/types.rs:151-155`;
  - `core/gecko-engine/src/okf/linker.rs:117-118`;
  - `core/gecko-engine/schema/core_schema.tql:50-51` (the `#` comment; Task 3 changed the `@doc`);
  - `gecko-bin/tests/sandbox_test.rs:52`;
  - `gecko-bin/src/main.rs:52-53` (the `--database` doc), `:647-649` (the `sync_manifest` doc), `:866` (the `fetch_playbook` doc).
- Modify: `docs/refactor/OWED-WORK-LEDGER.md`

**The rule, in every place:** one bundle per database by default (`sync` picks the database from `bundle.json`'s name). Bundles may share a database with `--database`. Concept IDs are plain bundle-relative OKF paths and must not collide across the bundles of one database; a colliding sync is refused.
- `CLAUDE.md`: replace "GECKO is **single-bundle-scoped** (one bundle per database)" with that rule, keeping the `playbooks/hello_js` example.
- The `--database` doc: "Defaults to the bundle's declared name for `schema-init`/`sync`/`sync-stix`, otherwise "gecko". One database per bundle by default; pass the same `--database` to sync several bundles into one."
- `linker.rs:117`: concept IDs are plain bundle-relative paths, and a link resolves only within its own bundle at sync time.

- [ ] **Step 1: Update the code comments and `CLAUDE.md`/`README.md`** as above. Afterwards, `git grep -n -i "single-bundle\|singleton bundle" -- core gecko-bin CLAUDE.md README.md` must return nothing.
- [ ] **Step 2: Update the ledger.**
  - **Header and "Reading the IDs":** add **O** OKF bundles and OKF v0.2 to the track list.
  - **"Where things stand":** add a bullet. Several bundles can share a database (**O1** ✅, the multi-bundle databases PR, branch `feat/multi-bundle-databases`), which unblocks cyber's detection flow end to end. OKF v0.2 adoption is owed (**O2–O5**).
  - **Sequence block:** add these lines.
    ```
    O1 ✅ ─► A4 + A10
    O2, O3, O4 ─► independent        O5 ─► needs D-N
    S5 ─► needs a design (after O1)
    ```
    - Keep the existing `S1 ✅ ─► A4 + A10 …` lines.
    - In "Recommended next", say **A4 + A10** follow **O1**.
  - **A4 row:** "Blocked on" becomes `O1`.
  - **Track S:** add an **S5** row after S4.
    - Item: **Objects shared across STIX imports.** Two ATT&CK domains (Enterprise and Mobile) cannot share a database. They ship the same MITRE `identity` and `marking-definition` objects, and some groups and software, under the same STIX ids, so the second import is refused as a concept-ID collision (O1). Under the default bundle name `stix-sync`, a second domain replaces the first instead, subject to the belief-edge guard of the belief-tier PR (A4 + A10): a re-import whose garbage collection would delete a record a belief edge references is refused. Sharing objects across imports needs a design: an object held by several imports (multi-containment ownership, collected only when no import holds it) and `attack-id` uniqueness across domains.
    - Type: design + engine + adapter.
    - Blocked on: O1; the design.
  - **New "Track O — OKF bundles and OKF v0.2"**, between Track S and Track B, with the same columns as Track S.
    - **Intro:** [OKF v0.2](https://raw.githubusercontent.com/GoogleCloudPlatform/knowledge-catalog/refs/heads/main/okf/SPEC.md) supersedes v0.1, which GECKO's parser implements. Concept IDs (file paths), links (§6.1, broken links tolerated) and bundle layout already match. The items are what v0.2 renames or adds (§13), plus O1, which v0.2 does not require but cyber's detection flow does. A v0.1 bundle stays consumable under v0.2's fallbacks.
    - **O1** ✅ **Several bundles in one database — LANDS in the multi-bundle databases PR (branch `feat/multi-bundle-databases`).**
      - What it does: a sync acts only on the concepts its bundle contains (GC, change detection, containment, hierarchy, links). GC releases, rather than deletes, a concept another bundle also contains. An incoming concept ID held by another bundle or by none is refused with `DbError::ConceptIdCollision`, and nothing is written. Links and hierarchy resolve within the bundle. `sync-stix --bundle-name` lets several imports share a database. One bundle per database stays the default.
      - It also adds `CORE_MIGRATIONS`, whose first entry, `core/bundle-shared-database-doc`, brings the `bundle` `@doc` forward on existing databases.
      - Owning section: the spec path.
      - Type: engine + CLI.
      - Done when: the `multi_bundle_test.rs` test names; `core_migrations_lead_to_the_shipped_schema` and `previous_core_schema_upgrades_to_the_shipped_schema`; and `cyber_detect_test` seeding through a synced bundle.
    - **O2** **`generated: {by, at}` replaces `timestamp`** (§5.2, §13.1; breaking).
      - GECKO reads `timestamp` at `core/gecko-engine/src/okf/parser.rs:172` into the `timestamp` attribute.
      - Read `generated.at` (and `generated.by`, §7 actor convention), with the v0.1 `timestamp` fallback v0.2 allows.
      - Type: parser + schema.
      - Blocked on: —.
    - **O3** **`sources` frontmatter and keyed footnotes replace body citations** (§5.1, §13.1; breaking).
      - GECKO treats every http(s) body link as a citation (`core/gecko-engine/src/okf/linker.rs:35`).
      - Read `sources` (and its credibility signals) into `external-resource`/`citation`, keeping the body parse as the v0.1 fallback.
      - Type: parser + schema.
      - Blocked on: —.
    - **O4** **Trust and lifecycle fields:** `verified` (§5.2; trust tiers keyed on `human:` actors, §5.3, §7), `status` (§5.4), `stale_after` (§5.5), and `okf_version` in the root `index.md` (§8, §12).
      - Today the concept-level keys land only in `metadata-json`, and `index.md` is a reserved file the parser skips.
      - Decide how they map to GECKO and to mem's belief and provenance model: a human-verified record versus a belief's confidence and entrenchment, and `stale_after` versus validity windows.
      - Type: design + parser.
      - Blocked on: — (the mapping is the item's first step).
    - **O5** **`Attested Computation` concept type** (§10): `runtime`, `parameters`, `computation`, `executor` with its `receipt`, and a deterministic `attester`.
      - It overlaps GECKO's playbooks, `engine`, `scopes` and wasm sandbox.
      - OKF fixes the interface, not the packaging, and defers attester sandboxing to the consumer.
      - Type: design + engine.
      - Blocked on: D-N.
  - **Open decisions:** add **D-N**, gating O5. How GECKO treats an Attested Computation:
    - as a playbook (engine, scopes and sandbox run), with its attester as a second sandboxed program, or as its own concept kind;
    - where receipts and verdicts live (the record tier, or beliefs with provenance);
    - how the attester is sandboxed.
- [ ] **Step 3: Check and commit.** Run `cargo fmt --all -- --check`, both clippy commands (comments only, but the doc comments compile), and `cargo test -p gecko-engine --doc --no-default-features`, since `parser.rs` doc examples sit near the edited comments. Then run the full suite: `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --test-threads=1`. Expected: all green. Commit: `docs: several bundles per database; ledger Track O, S5 and D-N`
