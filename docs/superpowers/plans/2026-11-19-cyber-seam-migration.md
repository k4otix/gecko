# Cyber Seam Migration (A2, A9, A12, A3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the extension-side holes in the belief-write monopoly:
- move cyber's belief edges onto the A1 seam;
- give extension bridges a read-only graph handle;
- run STIX post-sync writes inside the sync transaction;
- refuse extension `undefine`/`redefine` of core or mem, at schema time;
- add the W5a static lint.

**Architecture:**
- **mem-gecko:** the registry gains statement modes and a `ModifiesBase` refusal.
- **gecko-bin:** runs the registry before any extension schema is applied.
- **gecko-extension-api:** splits `GraphReader` out of `GraphStore`.
- **gecko-engine:** `sync_bundle` runs extension-supplied `GraphWrite`s in its own transaction.
- **cyber-gecko:** declares its two belief edges and writes them through a bound `BeliefEdgeWriter`, with reads through a `GraphReader`.
- **gecko-bin tests:** a TypeDB-backed lint derives the belief-tier surface by introspection and scans production sources.

**Tech Stack:** Rust 2024 workspace, TypeDB 3.15 (`typedb-driver ~3.15`), tokio, async-trait, nextest.

**Spec:** `docs/superpowers/specs/2026-11-19-cyber-seam-migration-design.md` (owning spec `docs/refactor/SPEC-W5.md` §1.1, §2.1, §3.3, §3.5).

## Global Constraints

- **Parameterized TQL.** All values go through `given`/`GraphValue` params. Only fixed schema labels are interpolated.
- **Greenfield comments.** No plan or epic tags (`A2`, `A12`, …) and no changelog phrasing in code comments. The CLAUDE.md "Comments are greenfield" rule applies.
- **Never fabricate success on bad input.** Errors, not defaults.
- **Integration tests use throwaway databases** via `common::TestDb`. Never touch the database named `gecko`. Run them with `--test-threads=1`.
- **Lint and format must pass, both clippy runs, as CI runs them (`ci.yml:137,140`):**
  - `cargo clippy --locked --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`
  - `cargo clippy --locked --workspace --exclude js-sandbox --no-default-features --features cyber --all-targets -- -D warnings`
  - `cargo fmt --all -- --check`
  - The pre-commit hook runs the fmt check.
  - Every task runs both clippy commands before its commit. Code reachable only under `cyber` is `#[cfg(feature = "cyber")]`-gated; never silence dead code with `#[allow(dead_code)]`.
- **CI test command:** `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber`, against TypeDB on `localhost:1729`.
- **`Cargo.lock` is tracked and CI uses `--locked`.** A task that changes dependencies commits the lockfile.
- **Commits:** end every message with the co-author attribution line your session specifies. Commit on the feature branch only, and never push.
- **Unchanged behaviour:**
  - `cyber_detect_test` and `cyber_stix_sync_test` keep every existing assertion. Only setup changes.
  - Detect argument validation, error messages and result JSON are unchanged.

## Review Focus

1. **Re-running `sync-stix` on the same bundle** must succeed and create no duplicate typed relations (the not-guards with `expect_rows: None`). Task 2 pins this with a second `sync_bundle` run in `cyber_stix_sync_test`.
2. **Cyber active, cyber declarations missing** (`edge_writer_for` errors). The run must fail with that error and must not degrade to a bridge without edges. Task 3 pins this with a wiring unit test.
3. **A schema-init with an extension that undefines a mem type** must fail before any extension text reaches TypeDB. Task 1b pins this with a serviceless `main.rs` unit test: `apply_all_schemas` over an unreachable router returns the `ModifiesBase` refusal, not a connection error.
4. **A belief literal with the `isa` before the write verb** (`match $b isa belief …; update …`) must be flagged. Task 4 pins this as a detector unit case.
5. **A STIX relation whose endpoint is missing** keeps today's behaviour: it is skipped silently, as a zero-row match. It does not abort the import. Task 2 pins this in a `cyber_post_sync` unit test that asserts `expect_rows == None`.

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `core/mem-gecko/src/belief_edge/scan.rs` | modify | `Statement` carries a `Mode`; `introduces_function()` |
| `core/mem-gecko/src/belief_edge/registry.rs` | modify | `ModifiesBase`; base function set; checks |
| `gecko-bin/src/config.rs` | modify | `check_extension_schemas` + tests |
| `gecko-bin/src/main.rs` | modify | call the check in `apply_all_schemas`; sync-stix wiring; bridge wiring |
| `extensions/README.md`, `CLAUDE.md` | modify | rule lines, bridge shape, post-sync |
| `core/gecko-engine/src/db/graph_store.rs` | modify | shared `run_graph_writes`; `impl GraphReader` |
| `core/gecko-engine/src/syncer/bundle.rs` | modify | `sync_bundle(.., extension_writes)` |
| `extensions/cyber-gecko/src/stix/mod.rs`, `Cargo.toml` | modify | pure `cyber_post_sync`; drop `typedb-driver` |
| `core/gecko-extension-api/src/graph.rs`, `lib.rs` | modify | `GraphReader` trait + re-export |
| `core/mem-gecko/src/writer/tests.rs` | modify | split test store impl |
| `extensions/cyber-gecko/src/lib.rs`, `src/detect/mod.rs` | modify | belief-edge decls; seam-based handlers; new bridge signature |
| `gecko-bin/tests/common/surface.rs` | create | `read_col`, `belief_edge_surface` |
| `gecko-bin/tests/belief_tier_monopoly_test.rs` | create | detector + live lint |
| `gecko-bin/tests/schema_probe_w2w3.rs` | modify | use the shared surface helper |
| tests: `bundle_sync_test.rs`, `cyber_stix_sync_test.rs`, `cyber_detect_test.rs`, `no_drag_test.rs`, `sandbox_test.rs` | modify | new signatures |
| `justfile` | modify | `test-it` gains `--features cyber` |
| `docs/refactor/OWED-WORK-LEDGER.md`, `docs/refactor/SPEC-W5.md` | modify | status, implementation note |

---

### Task 1a: Registry refuses extension `undefine`/`redefine` of core or mem

**Files:**
- Modify: `core/mem-gecko/src/belief_edge/scan.rs`
- Modify: `core/mem-gecko/src/belief_edge/registry.rs`
- Test: the unit-test modules in the same two files

**Interfaces:**
- Produces:
  - `pub(super) enum Mode { Define, Redefine, Undefine }` in `scan.rs`.
  - `Statement::mode(&self) -> Mode`.
  - `Statement::introduces_function(&self) -> Option<&str>` returns the label that directly follows the first `fun` token anywhere in the statement (a `windows(2)` scan, like `supertypes()`). This is deliberately wider than "`fun` at the statement start", to match the engine's line-based function detector (`core/gecko-engine/src/db/schema_apply.rs:19-21`) fail-closed.
  - `RegistryError::ModifiesBase { extension: String, mode: &'static str, label: String }`, where `mode` is `"undefine"`, `"redefine"` or `"define"`. The `"define"` value is used only for the function-name case.
- Consumes: the existing `statements`, `introduced_labels` and `BeliefEdgeRegistry::build`.

**Mode rules:**
- A statement's mode is the last leading `define`/`redefine`/`undefine` token seen in the same text, at the start of this statement or an earlier one.
- Text before any keyword is `Define`.
- `statements()` assigns modes. `Statement::body()` strips a leading `undefine` as well.
- Mode tracking is per text: mem's `mem_text` and each fragment's schema and migrations are scanned separately, as today.

**Checks in `build`**, in each extension's loop, after the rename check and before `check_no_reparenting`. `base_fns` is the set of `introduces_function()` over core and mem statements.
- **Any mode:** if `stmt.introduces_function()` is in `base_fns`, refuse with `ModifiesBase { mode: <stmt mode>, label }`.
- **`Undefine` or `Redefine` mode:** if any token is in `base_types ∪ base_fns`, refuse with `ModifiesBase { mode, label: <first such token> }`.

- [ ] **Step 1: Write the failing tests.** In `scan.rs` tests, add `scanner_carries_the_query_mode_across_statements`:
  - `statements("define entity a; entity b; undefine owns x from a; c; redefine entity d @abstract;")` yields the modes `[Define, Define, Undefine, Undefine, Redefine]`.
  - `statements("entity a;")[0].mode() == Mode::Define`.
  - `statements("define\nfun f() -> boolean:\n match $x isa a;\n return check;")[0].introduces_function() == Some("f")`.

  In `registry.rs` tests, add `refuses_an_extension_that_modifies_a_base_type_or_function`. Loop over these fragment texts, each in its own extension fragment with no declarations (rows that name `probe-link` use `PROBE_SCHEMA` as the fragment schema and the row text as a migration). Each must give exactly `Err(ModifiesBase { extension, mode, label })` with the shown `mode` and `label`:

  | fragment text | mode | label |
  |---|---|---|
  | `"undefine belief;"` | `undefine` | `belief` |
  | `"undefine owns confidence from belief;"` | `undefine` | `confidence` |
  | `"undefine plays probe-link:claim from belief;"` | `undefine` | `belief` |
  | `"undefine @abstract from memory-item;"` | `undefine` | `memory-item` |
  | `"undefine fun believed-at;"` | `undefine` | `believed-at` |
  | `"redefine attribute confidence value string;"` | `redefine` | `confidence` |
  | `"redefine belief owns confidence @card(1);"` | `redefine` | `belief` |
  | `"redefine\nfun believed-at($t: datetime) -> { belief }:\n match $b isa belief;\n return { $b };"` | `redefine` | `believed-at` |
  | `"redefine probe-link owns confidence @card(1);"` | `redefine` | `confidence` |
  | `"redefine probe-link relates evidence @card(0..1);"` | `redefine` | `evidence` |
  | `"define\nfun believed-at($b: belief) -> boolean:\n match $b isa belief;\n return check;"` | `define` | `believed-at` |

  - The `label` column is the first base token in the statement. For `owns confidence from belief`, that is `confidence`.
  - The `relates evidence` row is deliberately fail-closed: an own-relation role whose name collides with a base label is refused in `redefine` mode. Keep it pinned.
  - Before writing the table, confirm that `confidence`, `memory-item`, `evidence` and `believed-at` are introduced by mem's schema text. If one is not, replace it with a mem label that is, and note it in the report.

  Add `accepts_an_extension_that_modifies_only_its_own_types`:
  - A fragment with schema `PROBE_SCHEMA` (the existing fixture, `registry.rs:472`), its existing `decl("probe-link")` declaration, and the migrations `"undefine owns probe-note from probe-link;"` and `"redefine relation probe-link @abstract;"` builds `Ok`. Carrying the declaration makes the ownership checks run over the undefine/redefine statements too.

  Add `refuses_an_explicit_redefine_of_an_own_function_whose_body_names_a_base_type`:
  - `"redefine\nfun probe-fn() -> boolean:\n match $b isa belief, has belief-state \"asserted\";\n return check;"` gives `ModifiesBase { mode: "redefine", label: "belief" }`.

  Add `rename_of_a_base_type_still_reports_rename_unsupported`:
  - `"redefine belief label idea;"` gives `RenameUnsupported`.

- [ ] **Step 2: Run the tests and confirm they fail.** Run `cargo test -p mem-gecko --lib belief_edge --no-default-features`. Expected: compile errors or failures on `mode`, `introduces_function` and `ModifiesBase`.
- [ ] **Step 3: Implement** `Mode`, the mode assignment in `statements()`, `introduces_function` and the `ModifiesBase` checks, as specified above. The error message is per mode: for `undefine`/`redefine`, "extension `{extension}` {mode}s core or mem type or function `{label}`"; for `define`, "extension `{extension}` defines a function named like core or mem function `{label}`". Both end with "an extension may change only its own schema".
- [ ] **Step 4: Run the tests and confirm they pass.** Run the same command, then `cargo test -p mem-gecko --no-default-features --lib`. Expected: PASS, with all existing registry and scanner tests green.
- [ ] **Step 5: Commit.** `feat(mem): belief-edge registry refuses extension undefine/redefine of core or mem types and functions`

### Task 1b: Run the registry before extension schemas are applied

**Files:**
- Modify: `gecko-bin/src/config.rs`
- Modify: `gecko-bin/src/main.rs` (`apply_all_schemas` at about line 603, `build_host_bridge` at about line 972)
- Modify: `extensions/README.md`
- Test: `gecko-bin/src/config.rs` tests module; a new `#[cfg(test)] mod tests` at the end of `gecko-bin/src/main.rs` (Task 3 extends it)

**Interfaces:**
- Consumes: `mem_gecko::BeliefEdgeRegistry::from_extensions(core_schema: &str, extensions: &[Box<dyn GeckoExtension>]) -> Result<BeliefEdgeRegistry, RegistryError>`.
- Produces: `pub fn check_extension_schemas(core_schema: &str, extensions: &[Box<dyn GeckoExtension>]) -> anyhow::Result<mem_gecko::BeliefEdgeRegistry>` in `config.rs`. It maps the error with context `"belief-edge declarations"`.

- [ ] **Step 1: Write the failing tests** in the `config.rs` tests module.
  - `check_refuses_an_extension_that_undefines_a_mem_type`:
    - A private test struct implements `GeckoExtension`, with name `"bad-ext"`, schema `"define entity bad-thing;"` and one migration whose `tql` is `"undefine belief;"`. Use the `SchemaMigration` fields as defined in `gecko-extension-api`.
    - `check_extension_schemas(gecko_engine::db::CORE_SCHEMA, &[mem_gecko::extension(), Box::new(BadExt)])` is `Err`.
    - Assert on the cause, not the text: `err.downcast_ref::<mem_gecko::RegistryError>()` matches `RegistryError::ModifiesBase { extension, mode: "undefine", label }` with `extension == "bad-ext"` and `label == "belief"`. (The context string `"belief-edge declarations"` contains "belief", so a text check would pass on any registry error.) If `RegistryError` is not re-exported from `mem_gecko`, re-export it.
  - `check_builds_the_registry_from_mem_alone`: `check_extension_schemas(CORE_SCHEMA, &[mem_gecko::extension()])` is `Ok`. Un-gated, so mem's own migrations are pinned in the default build.
  - `#[cfg(feature = "cyber")] check_builds_the_registry_from_the_real_extensions`:
    - `check_extension_schemas(CORE_SCHEMA, &build_extensions(&cfg_with(&["cyber"])).unwrap())` is `Ok`.
    - Cyber's declarations arrive in Task 3, which extends this test.
  - **Ordering test** `apply_all_schemas_refuses_before_touching_the_database`, in the new `main.rs` test module (`gecko-bin` is bin-only, so this must be a bin unit test). It needs no service:
    - Build a `TypeDbRouter` over the unreachable address `127.0.0.1:1`. First confirm in `core/gecko-engine` that router construction is lazy (connects on first transaction); if it connects eagerly, report NEEDS_CONTEXT instead of improvising.
    - Call `apply_all_schemas` (current signature, no custom core schema) with `[mem_gecko::extension(), Box::new(BadExt)]`.
    - Assert the error's cause is `ModifiesBase` (as above), not a connection error. This proves the check precedes all database contact, which stands in for the spec's "`belief` still in the schema afterwards" assertion.
    - Share `BadExt` between the two modules via a `#[cfg(test)] pub(crate)` item in `config.rs`.
- [ ] **Step 2: Run and confirm the failure.** Run `cargo test -p gecko-bin --bins --no-default-features --features cyber check_`. Expected: compile error, because `check_extension_schemas` does not exist yet.
- [ ] **Step 3: Implement and wire.**
  - Add `check_extension_schemas`.
  - In `apply_all_schemas`, call `check_extension_schemas(&core_schema, extensions)?` as the first statement after `core_schema` is resolved, before `db.apply_schema(&core_schema)`. It applies even with a custom core schema, which the registry then validates against.
  - In `build_host_bridge`, replace the inline `from_extensions` call with `check_extension_schemas(gecko_engine::db::CORE_SCHEMA, &extensions)?`.
- [ ] **Step 4: Update the docs.** In `extensions/README.md`, add a schema-fragment bullet after the "No type or role renames" bullet:
  - The registry refuses an extension fragment or migration that `undefine`s or `redefine`s anything core or mem introduces, or that defines a function with a core or mem function name. It runs before any extension schema is applied (`schema-init`, `sync`, `sync-stix`, `run`).
  - An extension changes its own functions by editing them in its `define` fragment. The engine redefines changed functions itself. An explicit `redefine fun` whose body names a core or mem type is refused.
  - Retiring a belief edge cannot be done from an extension fragment. It is a reviewed change to the fragments, applied to a fresh database or by a migration that mem reviews.
- [ ] **Step 5: Run the tests and confirm they pass.** Run the Step 2 command plus `cargo test -p gecko-bin --bins --no-default-features`, then both clippy commands. Expected: PASS, clippy clean.
- [ ] **Step 6: Commit.** `feat: refuse extension schemas that modify core or mem before any schema is applied`

### Task 2: Neutral sync writes (STIX post-sync inside the sync transaction)

**Files:**
- Modify: `core/gecko-engine/src/db/graph_store.rs:97-…` (factor the op loop out)
- Modify: `core/gecko-engine/src/syncer/bundle.rs:124-192`
- Modify: `extensions/cyber-gecko/src/stix/mod.rs:308-360`
- Modify: `extensions/cyber-gecko/Cargo.toml` (remove `typedb-driver.workspace = true`) and `Cargo.lock` (regenerated; commit it)
- Modify: `gecko-bin/src/main.rs` (`sync_manifest` at about line 657, the `SyncStix` arm at about lines 325-358, `cmd_sync`)
- Modify the callers to pass `&[]`: `gecko-bin/tests/bundle_sync_test.rs`, `no_drag_test.rs`, `sandbox_test.rs` and `cyber_detect_test.rs`. The last also replaces its post-sync block.
- Test: `gecko-bin/tests/bundle_sync_test.rs`, `gecko-bin/tests/cyber_stix_sync_test.rs`, cyber `stix` unit tests

**Interfaces:**
- Produces:
  - `pub async fn run_graph_writes(tx: &typedb_driver::Transaction, ops: &[GraphWrite]) -> Result<(), EpistemicError>` in `gecko_engine::db::graph_store`, re-exported from `gecko_engine::db` like `RouterGraphStore`. `RouterGraphStore::write` calls it between `begin_write` and `commit`. It keeps the empty-rows skip, the parameterless path and the row-count check exactly.
  - `pub async fn sync_bundle(db: &mut TypeDbRouter, manifest: &OkfBundle, extension_writes: &[GraphWrite]) -> Result<SyncResult, DbError>`. It runs `run_graph_writes(&tx, extension_writes)` after `create_citations` and before `commit`, mapping the error to `DbError::Transaction(e.to_string())`. If an existing `DbError` variant fits writes better, use that one.
  - `pub fn cyber_post_sync(typed_rels: &[TypedRelation]) -> Vec<GraphWrite>` (cyber). Each shape gives one `GraphWrite { query: <today's query text>, vars: ["src","tgt"], rows: <pairs>, expect_rows: None }`. Unknown `rel_type`s are skipped.
  - `async fn sync_manifest(config, extensions, manifest, extension_writes: &[GraphWrite]) -> Result<SyncResult>`. No caller needs the router any more, so it is no longer returned; rewrite the doc comment at `main.rs:653-656` to describe what it does now (no "for any extension post-sync step").

- [ ] **Step 1: Write the failing tests.**
  - **Cyber `stix` unit test** `post_sync_groups_relations_into_one_write_per_shape`: two `uses` relations with the same source and target types, plus one of another shape, plus one with an unknown `rel_type`.
    - The result has 2 ops.
    - The `uses` op has `vars == ["src","tgt"]`, 2 rows and `expect_rows == None`.
    - No op's query mentions the unknown type.
  - **`bundle_sync_test.rs`** `failing_extension_write_rolls_back_the_whole_sync`, on a fresh `TestDb` with core and mem schema:
    - `sync_bundle(&mut db, &bundle, &[GraphWrite::plain("insert $x isa no-such-type;")])` is `Err`.
    - Afterwards `match $c isa concept;` counts 0 and `match $b isa bundle;` counts 0.
  - **`cyber_stix_sync_test.rs`:**
    - Replace the post-sync transaction with `sync_bundle(&mut db, &manifest, &cyber_post_sync(&typed_rels))`. Every existing assertion stays.
    - Then run the same `sync_bundle` call a second time. Open a **new** read transaction after it (the test's existing `tx` at `cyber_stix_sync_test.rs:127` is a snapshot from before) and assert that the counts of `uses`, `indicates`, `requires-evidence` and `evidence-of` equal their first-sync values (Review Focus 1).
- [ ] **Step 2: Run and confirm the failure.** Run `cargo test -p cyber-gecko --no-default-features post_sync` and `cargo test -p gecko-bin --test bundle_sync_test --no-default-features -- --test-threads=1`. Expected: compile errors on the new signatures.
- [ ] **Step 3: Implement.**
  - Add `run_graph_writes` and the `sync_bundle` parameter.
  - Make `cyber_post_sync` pure and remove `typedb-driver` from cyber's `Cargo.toml`. `git grep typedb_driver extensions/` must be empty.
  - Update `sync_manifest` and `cmd_sync` (`&[]`).
  - In the `SyncStix` arm, compute `let writes = cyber_gecko::stix::cyber_post_sync(&typed_rels);` and print `Linking {} typed relations...` before the sync. Call `sync_manifest(config, &extensions, &manifest, &writes)` and delete the separate begin/commit block.
  - Update every other caller to pass `&[]`, and in `cyber_detect_test.rs` pass `&cyber_post_sync(&typed_rels)`.
  - Update the doc comment on `sync_bundle` to say it also runs the extension writes in the same transaction.
- [ ] **Step 4: Run the tests and confirm they pass.** Run `cargo test -p cyber-gecko --no-default-features`. Then run `cargo test -p gecko-bin --no-default-features --features cyber --test bundle_sync_test --test cyber_stix_sync_test --test cyber_detect_test --test no_drag_test --test sandbox_test -- --test-threads=1`. Then check the dependency is gone from cyber's own manifest: `! grep -q typedb-driver extensions/cyber-gecko/Cargo.toml` (it stays a transitive dependency through `gecko-engine`, which is expected). Then `cargo metadata --locked --format-version 1 >/dev/null` must succeed, and both clippy commands must be clean. Expected: PASS.
- [ ] **Step 5: Commit**, including `Cargo.lock`. `feat: extension post-sync returns GraphWrites that run inside the sync transaction`

### Task 3: `GraphReader` split and cyber on the belief-edge seam

**Files:**
- Modify: `core/gecko-extension-api/src/graph.rs:156-185`, `core/gecko-extension-api/src/lib.rs:49`
- Modify: `core/gecko-engine/src/db/graph_store.rs` (`impl GraphReader for RouterGraphStore`)
- Modify: `core/mem-gecko/src/writer/tests.rs:69` (`RecordingGraphStore`)
- Modify: `extensions/cyber-gecko/src/lib.rs` (add `belief_edges()`)
- Modify: `extensions/cyber-gecko/src/detect/mod.rs`
- Modify: `gecko-bin/src/main.rs` (`build_host_bridge`)
- Modify: `gecko-bin/src/config.rs` tests (extend the Task 1b registry test)
- Modify: `gecko-bin/tests/cyber_detect_test.rs`
- Modify: `extensions/README.md` (host-bridge section, checklist step 4), `CLAUDE.md` ("Host-import seams" paragraph)

**Interfaces:**
- Produces:
  - `#[async_trait] pub trait GraphReader: Send + Sync { async fn read(&self, query: &str, vars: &[String], row: &[GraphValue]) -> Result<Vec<serde_json::Value>, EpistemicError>; }`, with `read`'s doc unchanged.
  - `pub trait GraphStore: GraphReader { async fn write(..) }`.
  - `GraphReader` re-exported from `gecko_extension_api`.
  - `pub fn cyber_extension_callback(graph: Arc<dyn GraphReader>, edges: Arc<dyn BeliefEdgeWriter>, ctx: RunContext, base: ExtensionCallback) -> ExtensionCallback`.
  - `pub static CYBER_BELIEF_EDGES: [BeliefEdgeDecl; 2]` in cyber, returned by `CyberGecko::belief_edges()`. The values come from the table below.
- Consumes:
  - `MemWriter::edge_writer_for(self: &Arc<Self>, extension: &str) -> Result<Arc<dyn BeliefEdgeWriter>, EpistemicError>`.
  - `MemWriter::without_index(store).with_belief_edges(registry)`.
  - `BeliefEdgeWriter::assert_belief_with_edges(ctx, draft, evidence, method, edges: &[BeliefEdge])`.

**Declarations:**

| relation | belief_role | roles (role / player / key) | owned_attrs |
|---|---|---|---|
| `technique-detection` | `detection-claim` | `detecting-logic` / `concept` / `concept-id`; `detected-technique` / `attack-pattern` / `attack-id` | — |
| `alert-disposition` | `disposition-belief` | `dispositioned-logic` / `concept` / `concept-id` | `detection-verdict`, `AttrValueType::String`, `required: true` |

- [ ] **Step 1: Write the failing tests.**
  - **`config.rs` registry test.** Extend `check_builds_the_registry_from_the_real_extensions`:
    - `reg.owner("technique-detection") == Some("cyber-gecko")`;
    - `reg.owner("alert-disposition") == Some("cyber-gecko")`.
  - **`cyber_detect_test.rs`:**
    - Build the bridge from:
      - a `RouterGraphStore` as `store: Arc<RouterGraphStore>`;
      - `registry = BeliefEdgeRegistry::from_extensions(CORE_SCHEMA, &[mem_gecko::extension(), cyber_gecko::extension()])`;
      - `mem = Arc::new(MemWriter::without_index(store.clone()).with_belief_edges(registry))`;
      - `cyber_extension_callback(store.clone() as Arc<dyn GraphReader>, mem.edge_writer_for("cyber-gecko").unwrap(), ctx, base)`.
    - Keep every existing assertion.
  - **New test `edge_to_an_unknown_technique_commits_no_belief`**, in the same file and on the same schema and sync setup. Factor a shared setup fn.
    - The draft is the same shape `claim` builds (copy its construction from `detect/mod.rs`), with `confidence: Some(0.9)`, method `DerivationMethod::ExternalTool` (use the variant name the API defines for `"external-tool"`), and no evidence.
    - **Control first:** count `match $b isa belief;`, call `edges.assert_belief_with_edges(&ctx, draft.clone(), &[], method, &[edge("T1059")])` and assert `Ok` and the count rose by exactly 1.
    - **Then the failing call:** `edge("T9999")`, where `edge(t) = BeliefEdge { relation: "technique-detection".into(), players: vec![("detecting-logic".into(), LOGIC_ID.into()), ("detected-technique".into(), t.into())], attrs: vec![] }`.
    - Assert it is `Err`, that its text contains both `technique-detection` and `T9999` (the row-count failure as rewritten by `name_failing_edge`, `core/mem-gecko/src/writer/edges.rs:112`), and that the belief count is unchanged from after the control. The text assertion is what proves the failure came from the edge write, not from pre-write validation.
  - **Wiring unit test** `cyber_bridge_requires_cyber_belief_edges` (Review Focus 2), `#[cfg(feature = "cyber")]`, in the `main.rs` test module that Task 1b created. It needs no service, because `MemWriter::without_index` over a store that is never used does not connect.
    - Factor the cyber wiring into `#[cfg(feature = "cyber")] fn cyber_bridge_handles(mem: &Arc<MemWriter>) -> Result<Arc<dyn BeliefEdgeWriter>>`, in `main.rs` next to `build_host_bridge`. (The reader is just `store.clone()`, so it is not threaded through the helper.)
    - A `MemWriter` with an empty `BeliefEdgeRegistry::default()` gives `Err` whose `{:#}` contains `"declares no belief edges"`. Extract it with `.err().expect(..)`, since `Arc<dyn BeliefEdgeWriter>` has no `Debug`.
- [ ] **Step 2: Run and confirm the failure.** Run `cargo test -p gecko-bin --bins --no-default-features --features cyber` and `cargo test -p gecko-bin --test cyber_detect_test --no-default-features --features cyber -- --test-threads=1`. Expected: compile errors on `GraphReader`, the new bridge signature and the missing owners.
- [ ] **Step 3: Implement the `GraphReader` split.** Split the trait in `graph.rs`, add the re-export, and split the `read` impls in `RouterGraphStore` and `RecordingGraphStore`. Fix any `use` lists the compiler flags.
- [ ] **Step 4: Implement cyber on the seam.**
  - Add `CYBER_BELIEF_EDGES` and `belief_edges()`.
  - In `detect/mod.rs`:
    - Change every `&Arc<dyn GraphStore>` to `&Arc<dyn GraphReader>` and every `writer` to `edges: &Arc<dyn BeliefEdgeWriter>`.
    - In `claim` and `disposition`, replace the `writer.assert_belief` + raw `GraphWrite` + `graph.write` with one `edges.assert_belief_with_edges(ctx, belief, &evidence, method, &[edge])`. The edges are as in the design. The `disposition` attr is `("detection-verdict", verdict)`.
    - Remove the `GraphWrite` and `EpistemicWriter` imports.
    - Rewrite the module and function doc comments to describe the seam and the read-only handle.
  - `git grep -n 'graph.write\|GraphStore' -- 'extensions/*/src'` must be empty. (Scoped to source; the README legitimately names `GraphStore`.)
- [ ] **Step 5: Wire it in `build_host_bridge`.**
  - Let `mem_writer: Arc<MemWriter> = build_epistemic_writer(store.clone(), cfg, edges).await?`.
  - Let `writer: Arc<dyn EpistemicWriter> = mem_writer.clone()`.
  - When cyber is active, `let cyber_edges = cyber_bridge_handles(&mem_writer)?;` and pass `store.clone() as Arc<dyn GraphReader>` and `cyber_edges` to `cyber_extension_callback`. `store` is an `Arc<RouterGraphStore>`.
  - Remove the second `RouterGraphStore`, the `graph_router` clone it needed, and `GraphStore` from the imports if unused.
  - Leave the `inject_epistemic_host_fns` loop as it is (owed with A10).
- [ ] **Step 6: Update the docs.**
  - **`extensions/README.md`:**
    - The section-2 intro (`:95-97`) says a bridge import may read the graph, write beliefs with the extension's declared edges, or use run identity. It no longer says "write the graph" or "write beliefs through the `EpistemicWriter`" (greenfield: describe the current seam, don't narrate the change).
    - The trait table (`:23-29`) gains `schema_migrations()` and `belief_edges()` rows, one accurate sentence each.
    - The bridge example signature becomes `graph: Arc<dyn GraphReader>` and `edges: Arc<dyn BeliefEdgeWriter>`, with the comments "parameterized reads only" and "writes a belief with this extension's declared edges".
    - The "The `GraphStore` seam" bullet becomes a "`GraphReader`" bullet. Bridges get read-only access; belief edges are declared via `belief_edges()` and written through `BeliefEdgeWriter`; only mem holds a `GraphStore`.
    - Add a bullet: an import-time post-sync step returns `Vec<GraphWrite>`, which the engine runs inside the sync transaction.
    - Checklist step 4 names `GraphReader` and `BeliefEdgeWriter`.
    - The cyber reference-bridge line says it routes the detect imports to a `GraphReader` and a `BeliefEdgeWriter`.
  - **`CLAUDE.md`, "Host-import seams":** "captures a `GraphReader`/`BeliefEdgeWriter`/`RunContext` (mem's own bridge captures the `EpistemicWriter`)".
- [ ] **Step 7: Run the tests and confirm they pass.**
  - Run the Step 2 commands.
  - Run `cargo test -p mem-gecko --no-default-features --lib`, `cargo test -p mem-gecko --no-default-features --no-run` (compiles mem's integration tests, which import `GraphStore`), and `cargo test -p cyber-gecko --no-default-features`.
  - Run both clippy commands from Global Constraints.
  - Expected: PASS, and clippy clean.
- [ ] **Step 8: Commit.** `feat(cyber): write detection and disposition edges through the belief-edge seam over a read-only graph handle`

### Task 4: W5a static lint `no_raw_belief_tier_tql_outside_mem_tql`, and close-out docs

**Files:**
- Create: `gecko-bin/tests/common/surface.rs`
- Create: `gecko-bin/tests/belief_tier_monopoly_test.rs`
- Modify: `gecko-bin/tests/schema_probe_w2w3.rs:1967-2025` (remove the local fn and include the shared one)
- Modify: `justfile:48` (`test-it` gains `--features cyber`)
- Modify: `docs/refactor/OWED-WORK-LEDGER.md` (rows A2, A3, A9, A12 → ✅ "this branch"; A10 note; `inject_epistemic_host_fns` owed note on A10)
- Modify: `docs/refactor/SPEC-W5.md` (§5, an implementation note for row 1 after the row-5 note, in the same style)

**Interfaces:**
- Produces, in `common/surface.rs`, which is included by each user with `#[path = "common/surface.rs"] mod surface;` and **not** declared in `common/mod.rs`. It holds exactly the two functions both crates use, so neither crate has dead code in it:
  - `pub async fn read_col(..)`, moved from the probe (`schema_probe_w2w3.rs:131`) with its signature unchanged; the probe drops its own copy and imports `surface::{read_col, belief_edge_surface}`.
  - `pub async fn belief_edge_surface(db: &TestDb) -> BTreeSet<String>`, moved from the probe, with the `HashMap`/`HashSet`/`StreamExt` imports it needs.
  - `surface.rs` takes `TestDb` through `super::common::TestDb`, or a path the includer provides. Pick whichever compiles cleanly in both crates.
- Produces, in the lint test file:
  - `async fn belief_tier_surface(db: &TestDb) -> BTreeSet<String>`: the `memory-item` subtype closure ∪ `belief_edge_surface`. It lives here, not in `surface.rs`.
  - `enum SourceKind { Rust, Tql }`;
  - `fn belief_tier_hits(source: &str, kind: SourceKind, surface: &BTreeSet<String>) -> Vec<(usize, String)>`, returning 1-based line and label;
  - `fn production_text(rust_source: &str) -> String`, which blanks the braced bodies of inline `#[cfg(test)] mod <name> { … }` blocks while keeping the line count. Recognized shape: `#[cfg(test)]`, optionally other attributes, then `[pub(..)] mod <name> {`. Brace matching skips string and char literals and comments, reusing the Rust literal lexer, so a `"{"` inside the module does not desync it. `mod <name>;` (out-of-line) blanks nothing.
- Consumes: `common::TestDb` and the schema-apply pattern of `apply_composed_schema` (core, then mem, then cyber).

**Feature gating (both clippy runs must be clean).** The detector and its unit tests are un-gated. Put `#[cfg(feature = "cyber")]` on `mod common;`, on `#[path = "common/surface.rs"] mod surface;`, on `belief_tier_surface`, on the file-walk helpers, on the live test, and on their `use` lines. `SourceKind::Tql` stays live in the cyber-less build through the un-gated `.tql` unit test. Do not use `#[allow(dead_code)]`.

**Detection rules (pinned):**
- Units:
  - **Rust:** each string literal's contents. Handle ordinary `"…"` with escapes and `\` line continuations, and raw `r"…"`/`r#"…"#`. Skip char literals (`'"'`, `'\''`) and lifetimes (`'a`), and ignore `//` and `/* */` comments.
  - **Tql:** the whole text with `#` comments **and** quoted strings (`"…"` and `'…'`, honoring `\`) removed, so a `@doc` sentence containing "delete" or "put" cannot trip the lint.
- **A hit:** a unit containing a whole-word write verb (`insert`, `put`, `update` or `delete`) **and**, anywhere in the unit, a label in `surface` that is either preceded by the token `isa` or directly followed by `(` (optionally after whitespace). The second form catches anonymous-relation writes like `insert technique-detection (detection-claim: $b);`. This widens the spec's `isa`-only rule toward SPEC-W5 §2.1's "a write verb applied to any name on the surface".
- **Labels:** a label token is a maximal run of `[A-Za-z0-9_-]`, so `attack-pattern` never matches `attack`.
- **Line:** the line of the matched label.

- [ ] **Step 1: Write the detector unit tests.** These are not feature-gated and need no service. Use `surface = {"belief", "technique-detection", "alert-disposition", "attack", "supersession"}`.
  - `flags_todays_cyber_claim_and_disposition_literals`: verbatim copies of the two `GraphWrite::single("…")` Rust sources from `detect/mod.rs` as they were before Task 3 removed them. Get them with `git show main:extensions/cyber-gecko/src/detect/mod.rs` (lines 127-134 and 183-188), and keep them as `"…"` literals with `\` continuations. Each gives ≥1 hit. The labels include `technique-detection` and `alert-disposition`, respectively.
  - `flags_an_anonymous_relation_insert`: `"match $b isa concept; insert technique-detection (detection-claim: $b);"` gives a hit on `technique-detection`.
  - `tql_units_ignore_comments_and_doc_strings`, with `SourceKind::Tql`:
    - `"insert $b isa belief;"` gives 1 hit.
    - `"# insert $b isa belief;\nmatch $b isa belief;"` gives 0 hits.
    - `"fun f() -> { belief } @doc(\"Rows to delete.\"):\n match $b isa belief;\n return { $b };"` gives 0 hits.
  - `flags_match_then_update_of_a_belief`: `"match $b isa belief, has concept-id == $id; update $b has belief-state \"retracted\";"` gives a hit on `belief`.
  - `flags_put_delete_and_raw_string_forms`: `"put $x isa belief;"`, `"match $r isa supersession; delete $r;"` and `r#"insert $r isa technique-detection, links (detection-claim: $b);"#` each give a hit.
  - `ignores_reads_record_types_and_comments`:
    - `"match $d isa technique-detection; fetch { \"d\": $d };"` gives 0 hits.
    - `"insert $t isa attack-pattern, has attack-id \"T1\";"` gives 0 hits.
    - `// insert $b isa belief;` as a Rust comment gives 0 hits.
  - `survives_char_literals_and_lifetimes`: `fn f<'a>(c: char) -> bool { c == '"' || c == '\'' }\nconst Q: &str = "insert $b isa belief;";` gives one hit, on line 2.
  - `production_text_keeps_code_after_an_out_of_line_test_mod`:
    - For `"#[cfg(test)]\nmod tests;\nconst Q: &str = \"insert $b isa belief;\";"`, `belief_tier_hits(&production_text(src), Rust, &s)` gives 1 hit.
    - For `"#[cfg(test)]\nmod tests {\n const Q: &str = \"insert $b isa belief;\";\n}"`, it gives 0 hits.
    - For `"#[cfg(test)]\nmod tests {\n const B: &str = \"{\";\n const Q: &str = \"insert $b isa belief;\";\n}\nconst P: &str = \"insert $b isa belief;\";"`, it gives exactly 1 hit, on line 6.
- [ ] **Step 2: Run and confirm the failure.** Run `cargo test -p gecko-bin --test belief_tier_monopoly_test --no-default-features`. Expected: compile errors on the undefined functions.
- [ ] **Step 3: Implement** `SourceKind`, `belief_tier_hits` and `production_text`, and make Step 1 pass.
- [ ] **Step 4: Write the live lint.** `#[cfg(feature = "cyber")] #[tokio::test(flavor = "multi_thread", worker_threads = 2)] async fn no_raw_belief_tier_tql_outside_mem_tql()`:
  - **Apply the schema.** Use a `TestDb::new("gecko_test_monopoly")` and apply core, then mem, then cyber.
  - **Surface non-vacuity.** Take `surface = belief_tier_surface(&db)` and assert it contains `belief`, `technique-detection`, `alert-disposition` and `supersession`.
  - **Walk the workspace.** Start from `concat!(env!("CARGO_MANIFEST_DIR"), "/..")`.
    - Rust files: every `.rs` under a `src/` directory of `core/*`, `extensions/*` and `gecko-bin`. Skip any path with a `tests` component, `target/`, and files named `tests.rs`.
    - TQL files: every `.tql` under `core/`, `extensions/` and `gecko-bin/`, skipping any path with a `tests` or `target` component or a component starting with `.`.
    - Rust text goes through `production_text`.
  - **Files-scanned non-vacuity.** Assert that the scanned set includes `extensions/cyber-gecko/src/detect/mod.rs` and `core/mem-gecko/src/writer/mod.rs`.
  - **Positive control.** Hits in `core/mem-gecko/src/tql.rs` include the labels `belief` and `supersession`.
  - **The assertion.** Hits in every other file are empty. On failure, list each hit as `path:line: label`.
- [ ] **Step 5: Run the live lint.** Run `cargo test -p gecko-bin --test belief_tier_monopoly_test --no-default-features --features cyber -- --test-threads=1`. Expected: PASS. Task 3 removed cyber's literals, and the positive control holds.
  - If it fails on a file other than cyber's, stop and report the hit. Do not widen the allowed list.
- [ ] **Step 6: Move the probe's helpers.** Remove `belief_edge_surface` and `read_col` from `schema_probe_w2w3.rs`, include `surface.rs`, and import both from it. Run `cargo test -p gecko-bin --test schema_probe_w2w3 --no-default-features --features cyber -- --test-threads=1`. Expected: the same pass/ignore set as before.
- [ ] **Step 7: Update `justfile` and the docs.**
  - **`justfile`:** `test-it` becomes `cargo test --test '*' --no-default-features --features cyber -- --test-threads=1`.
  - **Ledger:** mark A2, A3, A9 and A12 ✅ with "(this branch)". The A12 text notes `redefine` and functions are covered and that the check runs at schema time. Add a note to A10: `typedb-driver` is gone from cyber, while `gecko-engine` and the CI guard remain, and `inject_epistemic_host_fns` still offers every extension the full writer.
  - **SPEC-W5 §5:** add the row-1 implementation note: the test ships in `gecko-bin/tests/belief_tier_monopoly_test.rs`, the surface is introspected, the scope is production sources, a hit is a write verb plus a surface label after `isa` or before `(`, and the positive control is on `tql.rs`. Also fix the stale sentence at `SPEC-W5.md:501` (the cyber bridge "still captures the graph…"): the bridge holds a `GraphReader` and a bound `BeliefEdgeWriter`.
- [ ] **Step 8: Run the full verification.**
  - `cargo fmt --all -- --check`.
  - Both clippy commands from Global Constraints.
  - `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --test-threads=1`.
  - Expected: all green.
- [ ] **Step 9: Commit.** `test: W5a static lint — no raw belief-tier TQL outside mem's tql.rs`
