# Belief-edge seam (A1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A mem-owned, per-extension-bound writer that commits a new belief together with the extension's declared, owned belief edges in one transaction, and a write-count check so no `match … insert` can silently commit nothing.

**Architecture:**
- `gecko-extension-api` gains the declaration types, `GeckoExtension::belief_edges()` and a `BeliefEdgeWriter` trait.
- `GraphWrite` gains `expect_rows`, enforced by `RouterGraphStore` before commit.
- mem-gecko gains a `belief_edge` module:
  - a token scanner over schema fragments;
  - a registry that enforces ownership (D-B);
  - the bound writer, which validates the request, checks the live schema once per relation, and appends the edge ops to the belief's own op list.
- The assembler builds the registry in the `Run` path.

**Tech Stack:** Rust 2024, TypeDB 3.12 via `typedb-driver`, `async-trait`, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-11-19-belief-edge-seam-design.md` (governs). Owning spec: SPEC-W5 §3.2 as amended by §3.5.

## Global Constraints

- All values go through `given` params (`Params` in `core/mem-gecko/src/tql.rs`). Labels may be interpolated **only** after they match `^[a-z][a-z0-9-]*$`.
- `core/mem-gecko/src/tql.rs` owns all query text; no TQL under `core/mem-gecko/src/writer/` or `belief_edge/`.
- Comments are greenfield: no plan/task tags (`A1`, `Task 3`), no changelog phrasing.
- Never fabricate success: every invalid input is an `Err`, and no partial write commits.
- Tier-2 tests use throwaway databases named with a ULID, never `gecko`, and run with `--test-threads=1`. Start TypeDB with `just up`; stop it with `just down` only if this session started it.
- Gates before every commit:
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features -- -D warnings`.
- Final gate: `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --test-threads=1`.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **A key value carrying TypeQL metacharacters** (`x"; delete $b;`) must be a parameter and never appear in query text. Pinned in Task 6.
2. **Similar relation names.** A relation whose name is a prefix of another (`probe-link` vs `probe-link-x`), or a label that appears only inside an `@doc` string or a comment, must not be mistaken for a definition. Pinned in Task 4.
3. **An empty `edges` slice** behaves exactly like `assert_belief`: `Ok`, and the belief is written. **Two edges of the same relation in one call** both land, each with its own count. Pinned in Task 6.
4. **A handle for an extension that declares nothing** (including `mem-gecko`) is refused at `edge_writer_for`, not at first write. Pinned in Task 6.
5. **A belief written through the edge writer** is indexed and gets retrieval provenance exactly as `assert_belief` does, because both share one op-list builder. Pinned in Task 6 by comparing the two op lists.

---

### Task 1: Write-count enforcement in `GraphWrite` / `RouterGraphStore`

This task doubles as the design doc's probe. **If Step 2 shows that `insert` answers do not yield one row per inserted match under `query_with_rows`, stop and report to the user before continuing** (spec §2). If `update` answers do not count, record that and continue; Task 2 then counts the link op only.

**Files:**
- Modify: `core/gecko-extension-api/src/graph.rs:56-91`
- Modify: `core/gecko-engine/src/db/graph_store.rs:71-98`
- Modify: `core/mem-gecko/src/tql.rs:668` (the one struct literal; add `expect_rows: None`)
- Create: `core/mem-gecko/tests/belief_edge_integration.rs`. Use the same fixture shape as `writer_reader_integration.rs:30-90`: `mod common;`, a `Fixture` with `raw_fetch`, `raw_write` and `drop_db`. The database prefix is `betest_`.

**Interfaces:**
- Produces:
  - `pub expect_rows: Option<usize>` on `GraphWrite`, with `#[serde(default)]`. It counts answer rows **per given row**, so the total expected is `n * rows.len()`.
  - `pub fn expecting(self, n: usize) -> GraphWrite`.
  - `single` and `plain` keep `None`.
- On a mismatch, `RouterGraphStore::write` returns `EpistemicError::Storage(format!("write expected {exp} row(s), matched {got}: {first 80 chars of query}"))`, drops the transaction uncommitted, and stops processing ops.

- [ ] **Step 1: Write the failing Tier-2 tests** in `belief_edge_integration.rs`. Each uses `RouterGraphStore` directly over a fixture with the core and mem schemas, and seeds two `concept`s with `raw_write`.
  - `expect_rows_accepts_an_exact_insert_match`: an insert of a `support` relation between the two seeded items (or any relation both can play; choose from `mem_types.tql`) with `.expecting(1)` returns `Ok`, and `raw_fetch` sees the relation.
  - `expect_rows_rolls_back_a_zero_match`: one transaction with two ops.
    - Op 1 is a plain insert of a third concept.
    - Op 2 is a `match` on a non-existent id plus an insert, `.expecting(1)`.
    - The call returns `Err` whose message contains `"expected 1"`, and the third concept is **absent**.
  - `expect_rows_rolls_back_a_double_match`: two concepts share a tag value, and an insert matches by that tag with `.expecting(1)`. The call returns `Err` and nothing is inserted.
  - `expect_rows_counts_an_update`: a `match … update` of one belief's `belief-state`, `.expecting(1)`, returns `Ok`. Record in the test's doc comment whether the count held.
- [ ] **Step 2: Run** `cargo nextest run -p mem-gecko --test belief_edge_integration --no-default-features --test-threads=1`.
  - Expected: fails to compile (no `expecting`).
  - After Step 3, if the zero-match test still commits, the answer is not a row stream: **stop and report**.
- [ ] **Step 3: Implement.**
  - In `write`, keep the `QueryAnswer` from `tx.query` and `tx.query_with_rows`.
  - When `op.expect_rows` is `Some`, drain `answer.into_rows()` and count the rows. A `Some(Err(e))` mid-stream is a `Storage` error.
  - Compare the count with `n * op.rows.len()`, or with `n` for a parameterless op.
  - When `expect_rows` is `None`, leave today's behavior.
- [ ] **Step 4: Run** the same command. Expected: 4 pass. Then run `cargo nextest run -p mem-gecko --no-default-features --test-threads=1`. Expected: all existing tests pass.
- [ ] **Step 5: Commit**: `feat(engine): GraphWrite.expect_rows rolls back a write that matched the wrong number of rows`.

### Task 2: `supersede` refuses a missing old belief

**Files:**
- Modify: `core/mem-gecko/src/tql.rs:277-304` (`supersession_ops`)
- Test: `core/mem-gecko/tests/writer_reader_integration.rs`

**Interfaces:**
- Consumes: `GraphWrite::expecting` (Task 1).

- [ ] **Step 1: Write the failing test** `supersede_of_a_missing_belief_errors_and_commits_nothing`.
  - `writer.supersede(ctx, MemId::new("mem/bel/does-not-exist"), draft, "r")` returns `Err(EpistemicError::Storage(_))`.
  - A `raw_fetch` counting `belief`s whose text equals the draft's text returns 0.
- [ ] **Step 2: Run** `cargo nextest run -p mem-gecko --test writer_reader_integration supersede_of_a_missing --no-default-features --test-threads=1`. Expected: FAIL (returns `Ok`).
- [ ] **Step 3: Implement.**
  - The link op gets `.expecting(1)`.
  - The flip op gets `.expecting(1)` only if Task 1's update test held.
  - `read_entrenchment` is unchanged (spec §2).
- [ ] **Step 4: Run** the same command, then the whole `writer_reader_integration` test. Expected: PASS, with the supersede tests still green.
- [ ] **Step 5: Commit**: `fix(mem): supersede of a missing belief errors instead of committing an orphan`.

### Task 3: Extension-API declaration types and the `BeliefEdgeWriter` trait

**Files:**
- Create: `core/gecko-extension-api/src/belief_edge.rs`
- Modify: `core/gecko-extension-api/src/lib.rs` (`pub mod belief_edge;`, re-exports, and `GeckoExtension::belief_edges`)

**Interfaces:**
- Produces, exactly as in spec §1:
  - `BeliefEdgeDecl { relation, belief_role, roles: &'static [RoleDecl], owned_attrs: &'static [AttrDecl] }`, with all labels `&'static str`;
  - `RoleDecl { role, player, key }`;
  - `AttrDecl { attr, value: AttrValueType, required: bool }`;
  - `AttrValueType { String, Datetime, Double, Long, Boolean }` with:
    - `fn tql(&self) -> &'static str`, giving `"string" | "datetime" | "double" | "integer" | "boolean"`;
    - `fn admits(&self, v: &GraphValue) -> bool`;
  - `BeliefEdge { relation: String, players: Vec<(String, GraphValue)>, attrs: Vec<(String, GraphValue)> }`;
  - `#[async_trait] pub trait BeliefEdgeWriter: Send + Sync { async fn assert_belief_with_edges(&self, ctx: &RunContext, draft: BeliefDraft, evidence: &[MemId], method: DerivationMethod, edges: &[BeliefEdge]) -> Result<MemId, EpistemicError>; }`;
  - `GeckoExtension::belief_edges(&self) -> &[BeliefEdgeDecl] { &[] }`.
- All derive `Debug, Clone, PartialEq`; `AttrValueType` also derives `Copy, Eq`.

- [ ] **Step 1: Write the failing unit tests** in `belief_edge.rs`:
  - `attr_value_type_maps_to_tql_and_admits_its_variant`: each variant's `tql()` matches the list above; `admits` is true for its own `GraphValue` variant and false for each other variant.
- [ ] **Step 2: Run** `cargo nextest run -p gecko-extension-api --no-default-features`. Expected: compile failure.
- [ ] **Step 3: Implement** the types, the trait and the default method. Each type and the trait get a one-sentence doc comment.
- [ ] **Step 4: Run** the same command. Expected: PASS. `cargo build --workspace --exclude js-sandbox --no-default-features --features cyber` succeeds, since existing extensions inherit the default.
- [ ] **Step 5: Commit**: `feat(api): belief-edge declaration types and BeliefEdgeWriter trait`.

### Task 4: Schema-fragment token scanner

**Files:**
- Create: `core/mem-gecko/src/belief_edge/mod.rs` (declares `mod scan; mod registry; mod handle;`)
- Create: `core/mem-gecko/src/belief_edge/scan.rs`
- Modify: `core/mem-gecko/src/lib.rs` (`pub mod belief_edge;`)

**Interfaces:**
- Produces:
  - `pub(crate) struct TypeHead { kind: Kind, label: String, sub: Option<String>, relates_as: bool }`, where `Kind` is `Entity | Relation | Attribute`;
  - `pub(crate) fn type_heads(fragment: &str) -> Vec<TypeHead>`.

**Algorithm:**
1. Strip `#`-to-end-of-line comments and `"…"` string literals, honoring `\"` escapes.
2. Tokenize on whitespace and on the punctuation `, ; ( ) : { }`.
3. Split into statements at `;`.
4. A statement is a type head only when its first token, after an optional leading `define`, is `entity`, `relation` or `attribute`. The next token is the label.
5. `sub` sets `sub` to the following token. Any `relates X as Y` inside the statement sets `relates_as`.
6. All other statements are ignored. That covers `X plays …`, `fun` bodies, and `match` and `return` lines.

- [ ] **Step 1: Write the failing unit tests** in `scan.rs`:
  - `finds_relation_heads_in_the_cyber_and_mem_fragments`: over `include_str!` of `extensions/cyber-gecko/schema/cyber_schema.tql`, the heads include `technique-detection`, `alert-disposition` and `sighting-of` as `Relation` with `sub == None`; over the mem schema text (`MemGecko::new().schema()`), they include `supersession` and `scoped`.
  - `ignores_labels_inside_doc_strings_and_comments`: a fragment with `entity a @doc("relation fake-rel, relates x");` and `# relation also-fake` yields no `fake-rel` and no `also-fake`.
  - `distinguishes_prefix_labels`: a fragment that defines `probe-link-x` only. `type_heads` has no `probe-link`.
  - `records_sub_and_relates_as`: `relation r2 sub r1, relates q as p;` yields `sub == Some("r1")` and `relates_as == true`.
  - `function_bodies_yield_no_heads`: the mem functions text adds no heads beyond those in the types text.
- [ ] **Step 2: Run** `cargo nextest run -p mem-gecko belief_edge::scan --no-default-features`. Expected: compile failure.
- [ ] **Step 3: Implement** `type_heads` per the algorithm above.
- [ ] **Step 4: Run** the same command. Expected: 5 pass.
- [ ] **Step 5: Commit**: `feat(mem): schema-fragment type-head scanner`.

### Task 5: `BeliefEdgeRegistry` (ownership, labels, shape, players)

**Files:**
- Create: `core/mem-gecko/src/belief_edge/registry.rs`
- Modify: `core/mem-gecko/src/lib.rs` (re-export `BeliefEdgeRegistry`, `ExtensionFragment`, `RegistryError`)

**Interfaces:**
- Consumes: `type_heads` (Task 4); the decl types (Task 3).
- Produces:
  - `pub struct ExtensionFragment<'a> { pub name: &'a str, pub schema: &'a str, pub migrations: &'a [SchemaMigration], pub decls: &'a [BeliefEdgeDecl] }`, with `ExtensionFragment::of(ext: &'a dyn GeckoExtension) -> Self`.
  - `#[derive(Debug, thiserror::Error)] pub enum RegistryError`, whose variants each name the extension and the label:
    - `BadLabel`, `NotOwned`, `SubtypedRelation`, `RoleAlias`, `DuplicateRelation`, `DuplicateRole`, `MemoryItemPlayer`.
  - `#[derive(Debug, Clone, Default)] pub struct BeliefEdgeRegistry` with:
    - `pub fn build(core_schema: &str, mem_schema: &str, extensions: &[ExtensionFragment]) -> Result<Self, RegistryError>`;
    - `pub fn from_extensions(core_schema: &str, extensions: &[Box<dyn GeckoExtension>]) -> Result<Self, RegistryError>`. It takes mem's schema from the element named `"mem-gecko"`, excludes that element from the fragment list, and errors if it is absent.
    - `pub(crate) fn decl(&self, extension: &str, relation: &str) -> Option<&BeliefEdgeDecl>`;
    - `pub(crate) fn declares_any(&self, extension: &str) -> bool`.

**Rules:** spec §3, bullet for bullet. "Fragment text" for an extension means its `schema` plus each migration's `tql`.
- **Ownership:** exactly one scanned fragment has a `Relation` head with the label, and it is the declarer's. That head has `sub == None` and `relates_as == false`.
- **Memory-item players:** build a `label → sub` map from every scanned `Entity` head (core, mem, extensions). Refuse a non-belief role's player `P` when any of these holds:
  - `P == "memory-item"`;
  - `P`'s `sub` chain reaches `memory-item`;
  - `memory-item`'s chain reaches `P`.

- [ ] **Step 1: Write the failing unit tests.** Use small inline fragment strings, plus the real core and mem text for the "defined by" cases.
  - `accepts_an_owned_relation`: a fixture extension defines `probe-link`, and the declaration builds.
  - `refuses_a_mem_relation`: declaring `supersession` returns `NotOwned`.
  - `refuses_a_core_relation`: choose any `relation` head in `core_schema.tql`; it returns `NotOwned`.
  - `refuses_another_extensions_relation`: X defines `x-link`, and Y declares it. Returns `NotOwned`.
  - `refuses_a_role_addition`: Y's fragment has `relation x-link, relates extra;`. Both fragments now head `x-link`, so the result is `NotOwned`.
  - `refuses_a_subtype_relation`: `relation my-edge sub supersession, …` returns `SubtypedRelation`.
  - `refuses_a_relates_as_role` returns `RoleAlias`.
  - `refuses_bad_labels`: `"Probe-Link"`, `"probe_link"`, `"probe link"` and `""` return `BadLabel`.
  - `refuses_duplicate_roles`: `belief_role` equals a `RoleDecl.role`, so the result is `DuplicateRole`.
  - `refuses_memory_item_players`: players `belief`, `episode`, `okf-concept`, and a fixture `sighting sub execution-episode`, each return `MemoryItemPlayer`. Player `concept` builds.
  - `migration_text_counts_as_the_extensions_fragment`: a relation defined only in a migration's `tql` is owned.
  - `from_extensions_excludes_mem_and_requires_it`: with mem plus a fixture extension, it builds; without mem, it errors.
- [ ] **Step 2: Run** `cargo nextest run -p mem-gecko belief_edge::registry --no-default-features`. Expected: compile failure.
- [ ] **Step 3: Implement** `build` and `from_extensions`.
- [ ] **Step 4: Run** the same command. Expected: 12 pass.
- [ ] **Step 5: Commit**: `feat(mem): belief-edge registry enforces extension ownership`.

### Task 6: Bound writer — validation, edge ops, one transaction

**Files:**
- Create: `core/mem-gecko/src/belief_edge/handle.rs`
- Modify: `core/mem-gecko/src/tql.rs` (add `belief_edge_op`)
- Modify: `core/mem-gecko/src/writer/beliefs.rs`. Extract `assert_belief`'s op-list construction into `pub(super) fn belief_ops(&self, ctx, &b, &id, evidence, method) -> Vec<GraphWrite>`, and have `assert_belief` call it.
- Modify: `core/mem-gecko/src/writer/mod.rs`:
  - fields `belief_edges: BeliefEdgeRegistry` and `checked_relations: Mutex<HashSet<String>>`;
  - `pub fn with_belief_edges(mut self, r: BeliefEdgeRegistry) -> Self`;
  - `pub fn edge_writer_for(self: &Arc<Self>, extension: &str) -> Result<Arc<dyn BeliefEdgeWriter>, EpistemicError>`.
- Test: `core/mem-gecko/src/writer/tests.rs`, using `RecordingGraphStore`.

**Interfaces:**
- Consumes: the registry's `decl` and `declares_any` (Task 5); `GraphWrite::expecting` (Task 1).
- Produces:
  - `tql::belief_edge_op(decl: &BeliefEdgeDecl, belief: &MemId, edge: &BeliefEdge) -> GraphWrite`. It returns a `given` stage with:
    - `$b` (belief id);
    - one `$p<i>: <key value type>` per role, typed from the `GraphValue` variant;
    - one `$a<i>: <AttrValueType::tql()>` per attribute.
  - It then `match`es the belief by `concept-id` and each player by `$x<i> isa <player>, has <key> $k<i>; $k<i> == $p<i>;`, and inserts `(belief_role: $bel, role_i: $x<i>…) isa <relation>, has <attr_i> == $a<i>…`. It is `.expecting(1)`.
  - The handle is a private `BoundEdgeWriter { writer: Arc<MemWriter>, extension: String }`. It implements `BeliefEdgeWriter`, which:
    1. validates the request;
    2. runs the live check (Task 7; a no-op stub in this task);
    3. builds `belief_ops(…)` plus one `belief_edge_op` per edge;
    4. makes one `graph.write`;
    5. runs the same best-effort index step as `assert_belief`.
  - `edge_writer_for` errors with `InvalidInput` when `!declares_any(extension)`.

**Validation order**, each failure `EpistemicError::InvalidInput` naming the relation, with zero ops sent:
1. `validate_confidence`.
2. The relation is registered to the bound extension; otherwise the error says which extension owns it, if any.
3. Each declared role is supplied exactly once, and no unknown role is supplied.
4. Each required attribute is supplied, there are no unknown or duplicate attributes, and `AttrValueType::admits` holds for each value.

- [ ] **Step 1: Write the failing unit tests** in `writer/tests.rs`. The fixture registry is built from an inline `probe-link` fragment with roles `probe-subject` (`concept` by `concept-id`) and `probe-object`, and a required `probe-verdict: String` attribute.
  - `edge_writer_for_refuses_an_extension_with_no_declarations`: covers `"mem-gecko"` and `"nobody"`.
  - `bound_writer_refuses_another_extensions_relation`: the handle is bound to `y`, and the relation is registered to `x`.
  - `bound_writer_validates_roles_and_attrs`: one case each for missing role, duplicate role, unknown role, missing required attribute, unknown attribute and wrong value variant. Each returns `InvalidInput`, and `RecordingGraphStore.writes` is empty.
  - `bound_writer_writes_belief_and_edges_in_one_call`: two `probe-link` edges.
    - Exactly one `write` call is recorded.
    - Its ops are `belief_ops(…)` for the same draft, then two edge ops, each with `expect_rows == Some(1)`.
  - `empty_edges_matches_assert_belief`: the ops equal `assert_belief`'s ops for the same inputs, ignoring the minted id.
  - `edge_op_keeps_values_out_of_query_text`: a key value `x"; delete $b;` and an attribute value `"yes"`. Neither string appears in `op.query`; both appear in `op.rows[0]`.
- [ ] **Step 2: Run** `cargo nextest run -p mem-gecko writer::tests --no-default-features`. Expected: compile failure.
- [ ] **Step 3: Implement** per Interfaces.
- [ ] **Step 4: Run** the same command, then the whole of `cargo nextest run -p mem-gecko --no-default-features`. Expected: all pass.
- [ ] **Step 5: Commit**: `feat(mem): per-extension BeliefEdgeWriter writes a belief and its edges atomically`.

### Task 7: Live-schema check and the Tier-2 acceptance tests

**Files:**
- Modify: `core/mem-gecko/src/tql.rs` (add `belief_edge_schema_reads(decl) -> Vec<Read>`)
- Modify: `core/mem-gecko/src/belief_edge/handle.rs` (replace the stub)
- Test: `core/mem-gecko/tests/belief_edge_integration.rs`

**The fixture fragment**, applied after core and mem with `apply_schema`, as a `const PROBE_SCHEMA`:

```typeql
define
  attribute probe-ref value string;
  attribute probe-verdict value string @values("yes", "no");
  entity probe-target sub concept @doc("Test player looked up by a unique non-key attribute."), owns probe-ref @unique;
  relation probe-link @doc("Test belief edge."),
      owns probe-verdict @card(1),
      relates probe-subject @doc("Subject."),
      relates probe-object @doc("Object."),
      relates probe-claim @doc("The belief.");
  concept plays probe-link:probe-subject;
  probe-target plays probe-link:probe-object;
  belief plays probe-link:probe-claim;
```

The declaration is:
- `belief_role "probe-claim"`;
- roles `probe-subject` (`concept` by `concept-id`) and `probe-object` (`probe-target` by `probe-ref`);
- attribute `probe-verdict` (`String`, required).

The test builds the writer with `MemWriter::without_index(store).with_belief_edges(registry)` behind an `Arc`, and the handle with `edge_writer_for("probe-ext")`.

**The live check:**
- **First, probe** in a scratch test which `fetch`-form type queries `GraphStore::read` can answer on 3.12 for each condition in spec §4 step 2:
  - relates;
  - plays, including `belief` on the belief role;
  - player plays role;
  - key owned `@key` or `@unique`, including inherited `concept-id @key` from `okf-concept`;
  - relation owns attribute.
- **Implement** each check that works.
- **Drop** any that cannot be expressed, and list them in the ledger's implementation notes (spec §4). Do not add a method to `GraphStore`.
- A pass inserts the relation into `checked_relations`; a failure returns `InvalidInput` and caches nothing.

- [ ] **Step 1: Write the failing Tier-2 tests.**
  - `belief_edge_decl_registry_writes_run_stamped`:
    - Seed concept `s1` and a `probe-target` with `probe-ref "t1"`.
    - Write one edge with verdict `"yes"`.
    - `raw_fetch` finds exactly one `probe-link` whose `probe-claim` player's `concept-id` is the returned id, whose players are `s1` and `t1`, and whose verdict is `"yes"`.
    - That belief has a `source-link` to the `doc-run` whose `run-id` equals `ctx.run_id`. Mirror the existing `source-link` assertions in `writer_reader_integration.rs`.
  - `missing_player_commits_no_belief`: `probe-ref "nope"` returns `Err`, and a `raw_fetch` of beliefs with the draft's text returns 0.
  - `value_outside_values_commits_no_belief`: verdict `"maybe"` returns `Err`, and 0 beliefs.
  - There is no separate duplicate-key test here. The live check requires every lookup key to be `@key` or `@unique`, so the seam cannot produce a double match. The spec's "duplicate key match" case is covered at the mechanism level by Task 1's `expect_rows_rolls_back_a_double_match`. Say so in this file's module doc.
  - `live_schema_mismatch_is_refused_before_writing`: a second registry whose declaration names role `probe-missing`, built over a fragment string that defines it but was **not** applied to the database. Returns `Err` naming `probe-missing`, and 0 beliefs.
  - `two_edges_of_one_relation_both_land`: two edges to `t1` and a second target `t2`. Exactly 2 `probe-link`s share the one belief.
- [ ] **Step 2: Run** `cargo nextest run -p mem-gecko --test belief_edge_integration --no-default-features --test-threads=1`. Expected: the new tests fail (the stub passes everything, so the mismatch test fails at commit with the wrong error, or passes for the wrong reason). Confirm each failure message.
- [ ] **Step 3: Implement** the probed checks.
- [ ] **Step 4: Run** the same command. Expected: all tests in the file pass.
- [ ] **Step 5: Commit**: `feat(mem): belief-edge writer checks the live schema once per relation`.

### Task 8: Assembler wiring

**Files:**
- Modify: `core/gecko-engine/src/db/mod.rs` (add `pub const CORE_SCHEMA: &str = include_str!("../../schema/core_schema.tql");`)
- Modify: `gecko-bin/src/main.rs`:
  - `:613`: use `gecko_engine::db::CORE_SCHEMA`;
  - `:779-860`: `build_epistemic_writer(store, cfg, edges: BeliefEdgeRegistry) -> Result<Arc<mem_gecko::MemWriter>>` applies `.with_belief_edges(edges)` before each `Arc::new`;
  - `:959-990`: `build_host_bridge` calls `BeliefEdgeRegistry::from_extensions(CORE_SCHEMA, &extensions)` **before** `extensions` moves into `base_cb`, maps the error to `anyhow` with context `"belief-edge declarations"`, and coerces the concrete writer to `Arc<dyn EpistemicWriter>` where it is used today.
- No extension takes a handle yet; cyber does so in A2.

- [ ] **Step 1: Write the failing test.** In `core/mem-gecko/src/belief_edge/registry.rs` tests, add `from_extensions_accepts_the_shipped_extensions`: `from_extensions(<core text>, &[mem_gecko::extension(), cyber_gecko::extension()])` builds when `cyber` is a dev-dependency. Check `core/mem-gecko/Cargo.toml`. If cyber cannot be a dev-dependency without a cycle, put this test in `gecko-bin/tests/cyber_detect_test.rs` instead.
- [ ] **Step 2: Run it.** Expected: PASS once wired, since cyber declares nothing yet. This test pins that the shipped set stays valid as A2 adds declarations.
- [ ] **Step 3: Implement** the wiring above.
- [ ] **Step 4: Run** `cargo build -p gecko-bin --no-default-features --features cyber`, then `cargo nextest run -p gecko-bin --no-default-features --features cyber --test-threads=1`. Expected: build OK, all pass (`cyber_detect_test`, `epistemic_bridge_test` and `mem_recall_e2e_test` unchanged).
- [ ] **Step 5: Commit**: `feat(bin): build the belief-edge registry for runs`.

### Task 9: Ledger, spec record, full gates

**Files:**
- Modify: `docs/refactor/OWED-WORK-LEDGER.md`:
  - A1 → ✅ with the evidence (test names);
  - add a track-F-style item for the write-count audit, using the four ranked entries in spec §2 verbatim;
  - add the Task 1 probe result and any dropped live checks under "Implementation notes";
  - move the reconcile anchor.
- Modify: `docs/refactor/SPEC-W5.md` §5. Note under row 5 that `belief_edge_decl_registry_writes_run_stamped` ships in `core/mem-gecko/tests/belief_edge_integration.rs`. The ratified text is unchanged; append, don't edit.

- [ ] **Step 1:** Make the edits above.
- [ ] **Step 2: Run the final gates.**
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features -- -D warnings`;
  - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --exclude js-sandbox --no-default-features --no-deps`;
  - `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --test-threads=1`.

  Expected: all clean. The test count is the previous 288 plus the new tests.
- [ ] **Step 3: Commit**: `docs: record the belief-edge seam and the write-count audit item`.
