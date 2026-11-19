# Belief-edge seam (A1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A mem-owned, per-extension-bound writer that commits a new belief together with the extension's declared, owned belief edges in one transaction. A write-count check ensures that no `match … insert` can silently commit nothing.

**Architecture:**
- **Extension API.** `gecko-extension-api` gains the declaration types, `GeckoExtension::belief_edges()` and a `BeliefEdgeWriter` trait.
- **Write count.** `GraphWrite` gains `expect_rows`, which `RouterGraphStore` enforces before commit.
- **Registry.** mem-gecko gains a `belief_edge` module: a token scanner over schema fragments, plus a registry that enforces ownership (D-B).
- **Bound writer.** It lives in `writer/edges.rs`, next to the belief writer whose internals it shares. It validates each request, checks the live schema once per relation, and appends the edge ops to the belief's own op list.
- **Assembler.** It builds the registry on the `Run` path.

**Tech Stack:** Rust 2024, TypeDB 3.12 via `typedb-driver` 3.12.3, `async-trait`, `thiserror`, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-11-19-belief-edge-seam-design.md` (governs). Owning spec: SPEC-W5 §3.2, as amended by §3.5.

## Global Constraints

- **Parameters.** All values go through `given` params (`Params` in `core/mem-gecko/src/tql.rs`). A label may be interpolated **only** after it matches `^[a-z][a-z0-9-]*$`.
- **Query text.** `core/mem-gecko/src/tql.rs` owns all query text. There is no TQL under `core/mem-gecko/src/writer/` or `core/mem-gecko/src/belief_edge/`.
- **Comments are greenfield.** No plan or task tags (`A1`, `Task 3`), no changelog phrasing.
- **Never fabricate success.** Every invalid input returns an `Err`, and no partial write commits.
- **Tier-2 tests:**
  - throwaway databases named with a ULID, never `gecko`;
  - run with `--test-threads=1`;
  - start TypeDB with `just up`; stop it with `just down` only if this session started it.
- **Unit tests** take the core schema from `include_str!("../../../gecko-engine/schema/core_schema.tql")` (adjust the depth to the file) and mem's schema from `MemGecko::new().schema()`.
- **Gates before every commit:**
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features -- -D warnings`.

  No `pub(crate)` item may land without a non-test caller; make it `pub`, or land it with its caller.
- **Final gate:** `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --test-threads=1`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Metacharacters in a value.** A key value carrying TypeQL metacharacters (`x"; delete $b;`) must be a parameter and never appear in query text. Pinned in Task 5.
2. **Look-alike labels.** None of these may count as a definition:
   - a relation whose name is a prefix of another (`probe-link` vs `probe-link-x`);
   - a label inside an `@doc` string or a comment.

   And the label-first form `x-link relates extra;` must count as touching `x-link`. Pinned in Task 4.
3. **Edge counts.** An empty `edges` slice behaves exactly like `assert_belief`. Two edges of the same relation in one call both land. Pinned in Tasks 5 and 6.
4. **Extensions with no declarations.** A handle for an extension that declares nothing, including `mem-gecko`, is refused at `edge_writer_for`. Pinned in Task 5.
5. **Indexing and provenance.** A belief written through the edge writer is indexed and gets retrieval provenance exactly as `assert_belief` does. Pinned in Task 5, with provenance on and an index present.

---

### Task 1: Write-count enforcement in `GraphWrite` / `RouterGraphStore`

This task doubles as the design doc's probe.
- **If `insert` answers do not yield one row per inserted match under `query_with_rows`** (Step 4's zero-match test still commits), **stop and report to the user** (spec §2).
- **If `update` answers are not row streams**, record that and continue. Task 2 then counts only the link op.

**Files:**
- Modify: `core/gecko-extension-api/src/graph.rs:56-91`
- Modify: `core/gecko-engine/src/db/graph_store.rs:71-98`
- Modify: `core/mem-gecko/src/tql.rs:668` (the one struct literal; add `expect_rows: None`)
- Create: `core/mem-gecko/tests/belief_edge_integration.rs`. Use the fixture shape of `writer_reader_integration.rs:30-90`: `mod common;`, a `Fixture` with `raw_fetch`, `raw_write` and `drop_db`, and the database prefix `betest_`.

**Interfaces:**
- Produces:
  - `pub expect_rows: Option<usize>` on `GraphWrite`, with `#[serde(default)]`. The total expected is `n * rows.len()` (or `n` for a parameterless op).
  - `pub fn expecting(self, n: usize) -> GraphWrite`. The `single` and `plain` constructors keep `None`.
  - **On a mismatch,** `RouterGraphStore::write` returns `EpistemicError::Storage(format!("write expected {exp} row(s), matched {got}: {first 80 chars of query}"))`. It drops the transaction uncommitted and processes no further ops.
  - **For an op with `expect_rows` set whose answer is not a row stream** (`!answer.is_row_stream()`), it returns `Storage("write answer is not a row stream: …")`. `into_rows()` panics on a non-row answer (`typedb-driver-3.12.3/src/answer/mod.rs:95-96`), so check first.

- [ ] **Step 1: Write the failing Tier-2 tests.** Each runs `RouterGraphStore` directly over a fixture with the core and mem schemas, and seeds two `concept`s with `raw_write`.
  - `expect_rows_accepts_an_exact_insert_match`: an insert of `hierarchy (parent: $a, child: $b)` matching the two seeded concepts by `concept-id`, `.expecting(1)`. Returns `Ok`, and `raw_fetch` sees the relation.
  - `expect_rows_rolls_back_a_zero_match`: one call with two ops:
    1. a plain insert of a third concept;
    2. a `hierarchy` insert matching a non-existent `concept-id`, `.expecting(1)`.

    Returns `Err` containing `"expected 1"`, and the third concept is absent.
  - `expect_rows_rolls_back_a_double_match`: both seeded concepts carry `tag "shared"`. An insert matching `$c isa concept, has tag "shared"` and inserting a `hierarchy` with a third, pre-seeded concept as parent, `.expecting(1)`. Returns `Err`, and no `hierarchy` exists.
  - `expect_rows_counts_an_update`: `match … update` of one seeded belief's `belief-state`, `.expecting(1)`. Seed the belief through `MemWriter::assert_belief`. Returns `Ok`. If the answer is not a row stream, the test instead asserts the "not a row stream" error and its doc comment records that.
- [ ] **Step 2: Run** `cargo nextest run -p mem-gecko --test belief_edge_integration --no-default-features --test-threads=1`. Expected: compile failure (no `expecting`).
- [ ] **Step 3: Implement.**
  - Keep the `QueryAnswer` from `tx.query` and `tx.query_with_rows`.
  - When `expect_rows` is set, check `is_row_stream()`, drain `into_rows()`, and count. A `Some(Err(e))` mid-stream is a `Storage` error.
  - `None` keeps today's behavior.
- [ ] **Step 4: Run** the Step 2 command. Expected: 4 pass. Apply the stop rule above. Then run `cargo nextest run -p mem-gecko --no-default-features --test-threads=1`. Expected: everything that existed before still passes.
- [ ] **Step 5: Commit:** `feat(engine): GraphWrite.expect_rows rolls back a write that matched the wrong number of rows`.

### Task 2: `supersede` refuses a missing old belief

**Files:**
- Modify: `core/mem-gecko/src/tql.rs:277-304` (`supersession_ops`)
- Test: `core/mem-gecko/tests/writer_reader_integration.rs`

**Interfaces:**
- Consumes: `GraphWrite::expecting` (Task 1).

- [ ] **Step 1: Write the failing test** `supersede_of_a_missing_belief_errors_and_commits_nothing`.
  - `writer.supersede(ctx, MemId::new("mem/bel/does-not-exist"), draft, "r")` returns `Err(EpistemicError::Storage(_))`.
  - A `raw_fetch` counting `belief`s whose text equals the draft's returns 0.
- [ ] **Step 2: Run** `cargo nextest run -p mem-gecko --test writer_reader_integration supersede_of_a_missing --no-default-features --test-threads=1`. Expected: FAIL (it returns `Ok`).
- [ ] **Step 3: Implement.**
  - The link op gets `.expecting(1)`.
  - The flip op gets `.expecting(1)` only if Task 1's update test showed a row stream.
  - `read_entrenchment` is unchanged (spec §2).
- [ ] **Step 4: Run** the Step 2 command, then the whole of `writer_reader_integration`. Expected: all pass.
- [ ] **Step 5: Commit:** `fix(mem): supersede of a missing belief errors instead of committing an orphan`.

### Task 3: Extension-API declaration types and the `BeliefEdgeWriter` trait

**Files:**
- Create: `core/gecko-extension-api/src/belief_edge.rs`
- Modify: `core/gecko-extension-api/src/lib.rs`:
  - `pub mod belief_edge;` and the re-exports;
  - `GeckoExtension::belief_edges`.

**Interfaces:**
- Produces, as in spec §1:
  - `BeliefEdgeDecl { relation, belief_role, roles: &'static [RoleDecl], owned_attrs: &'static [AttrDecl] }`, with all labels `&'static str`.
  - `RoleDecl { role, player, key }`.
  - `AttrDecl { attr, value: AttrValueType, required: bool }`.
  - `AttrValueType { String, Datetime, Double, Long, Boolean }`, with:
    - `fn tql(&self) -> &'static str`, returning `"string" | "datetime" | "double" | "integer" | "boolean"`;
    - `fn admits(&self, v: &GraphValue) -> bool`.
  - `BeliefEdge { relation: String, players: Vec<(String, GraphValue)>, attrs: Vec<(String, GraphValue)> }`.
  - The trait:

    ```rust
    #[async_trait]
    pub trait BeliefEdgeWriter: Send + Sync {
        async fn assert_belief_with_edges(
            &self,
            ctx: &RunContext,
            draft: BeliefDraft,
            evidence: &[MemId],
            method: DerivationMethod,
            edges: &[BeliefEdge],
        ) -> Result<MemId, EpistemicError>;
    }
    ```
  - `GeckoExtension::belief_edges(&self) -> &[BeliefEdgeDecl] { &[] }`.
- All types derive `Debug, Clone, PartialEq`; `AttrValueType` also derives `Copy, Eq`.

- [ ] **Step 1: Write the failing unit test** `attr_value_type_maps_to_tql_and_admits_its_variant`.
  - Each variant's `tql()` matches the list above.
  - `admits` is true for its own `GraphValue` variant and false for each other variant.
- [ ] **Step 2: Run** `cargo nextest run -p gecko-extension-api --no-default-features`. Expected: compile failure.
- [ ] **Step 3: Implement.** Each type and the trait get a one-sentence doc comment.
- [ ] **Step 4: Run** the Step 2 command. Expected: PASS. Also run `cargo build --workspace --exclude js-sandbox --no-default-features --features cyber`. Expected: success.
- [ ] **Step 5: Commit:** `feat(api): belief-edge declaration types and BeliefEdgeWriter trait`.

### Task 4: Fragment scanner and `BeliefEdgeRegistry`

The scanner and the registry land together, so that the scanner has a caller (see the clippy gate).

**Files:**
- Create: `core/mem-gecko/src/belief_edge/mod.rs` (`mod scan; mod registry; pub use registry::…;`)
- Create: `core/mem-gecko/src/belief_edge/scan.rs`
- Create: `core/mem-gecko/src/belief_edge/registry.rs`
- Modify: `core/mem-gecko/src/lib.rs`:
  - `pub mod belief_edge;`;
  - re-export `BeliefEdgeRegistry`, `ExtensionFragment` and `RegistryError`.
- Modify: `core/mem-gecko/Cargo.toml` (`thiserror = { workspace = true }`)

**Interfaces:**
- Consumes: the decl types (Task 3).
- Produces:
  - `scan.rs`, private to the module:
    - `struct TypeHead { kind: Kind, label: String, sub: Option<String>, relates_as: bool }`, where `Kind` is `Entity | Relation | Attribute | Touch`;
    - `fn type_heads(fragment: &str) -> Vec<TypeHead>`.
  - `ExtensionFragment<'a> { pub name: &'a str, pub schema: &'a str, pub migrations: &'a [SchemaMigration], pub decls: &'a [BeliefEdgeDecl] }`, with `pub fn of(ext: &'a dyn GeckoExtension) -> Self`.
  - `#[derive(Debug, thiserror::Error)] pub enum RegistryError`. Every variant names the extension and the label:
    - `BadLabel`, `NotOwned`, `SubtypedRelation`, `RoleAlias`, `DuplicateRelation`, `DuplicateRole`, `MemoryItemPlayer`, `MemMissing`.
  - `#[derive(Debug, Clone, Default)] pub struct BeliefEdgeRegistry`, with:
    - `pub fn build(core_schema: &str, mem_schema: &str, extensions: &[ExtensionFragment]) -> Result<Self, RegistryError>`;
    - `pub fn from_extensions(core_schema: &str, extensions: &[Box<dyn GeckoExtension>]) -> Result<Self, RegistryError>`. It takes mem's schema from the element named `"mem-gecko"`, excludes that element from the fragments, and returns `MemMissing` if it is absent.
    - `pub fn decl(&self, extension: &str, relation: &str) -> Option<&BeliefEdgeDecl>`;
    - `pub fn owner(&self, relation: &str) -> Option<&str>`;
    - `pub fn declares_any(&self, extension: &str) -> bool`.

**Scanner algorithm:**
1. Strip `#`-to-end-of-line comments and `"…"` literals, honoring `\"`.
2. Tokenize on whitespace and `, ; ( ) : { }`.
3. Split statements at `;`.
4. A statement is a head when its first token, after an optional `define`, is `entity`, `relation` or `attribute`. The next token is the label.
   - `sub` sets `sub` to the following token.
   - Any `relates X as Y` sets `relates_as`.
5. A statement whose first token is a bare label and whose second token is `relates`, `owns`, `plays` or `sub` is a `Touch` of that label: the label-first form, which also extends a type.
6. All other statements are ignored: `fun` bodies, and `match` and `return` lines.

**Registry rules (spec §3).** An extension's fragment text is its `schema` plus each migration's `tql`.
- **Ownership:**
  - The declarer's fragment has a `Relation` head for the label.
  - No other fragment (core, mem, or another extension) has a `Relation` head or a `Touch` for it.
  - That head has `sub == None` and `relates_as == false`.
- **Memory-item players.** Build `label → sub` from every `Entity` head in every fragment. Refuse a non-belief player `P` when:
  - `P == "memory-item"`;
  - `P`'s chain reaches `memory-item`; or
  - `memory-item`'s chain reaches `P`.

- [ ] **Step 1: Write the failing unit tests**, using inline fragments plus the real core and mem text.
  - **Scanner:**
    - `scanner_finds_relation_heads_in_mem`: mem text has `Relation` heads `supersession` and `scoped`, with `sub == None`.
    - `scanner_ignores_doc_strings_and_comments`: `entity a @doc("relation fake-rel, relates x");` and `# relation also-fake` yield neither label.
    - `scanner_distinguishes_prefix_labels`: a fragment defining only `probe-link-x` has no `probe-link`.
    - `scanner_records_sub_relates_as_and_touch`: `relation r2 sub r1, relates q as p;` gives `sub == Some("r1")` and `relates_as`. `x-link relates extra;` gives `Touch` `x-link`.
    - `scanner_function_bodies_yield_no_heads`: mem's functions text adds no heads beyond its types text.
  - **Registry:**
    - `accepts_an_owned_relation`: an extension `probe-ext` defines and declares `probe-link`. It builds; `decl` returns the declaration and `owner("probe-link") == Some("probe-ext")`.
    - `refuses_a_mem_relation` (`supersession`) and `refuses_a_core_relation` (`hierarchy`): both return `NotOwned`.
    - `refuses_another_extensions_relation`: X defines `x-link`, and Y declares it. Returns `NotOwned`.
    - `refuses_a_role_addition`: covers both forms in Y's fragment, `relation x-link, relates extra;` and `x-link relates extra;`. With X owning and declaring `x-link`, X's declaration returns `NotOwned`.
    - `refuses_a_subtype_relation` (`relation my-edge sub supersession, …`) returns `SubtypedRelation`.
    - `refuses_a_relates_as_role` returns `RoleAlias`.
    - `refuses_bad_labels`: `"Probe-Link"`, `"probe_link"`, `"probe link"` and `""` return `BadLabel`.
    - `refuses_duplicate_relation_and_roles`: the same relation declared twice returns `DuplicateRelation`; `belief_role` equal to a `RoleDecl.role` returns `DuplicateRole`.
    - `refuses_memory_item_players`: `belief`, `episode`, `okf-concept`, and a fixture `sighting sub execution-episode` each return `MemoryItemPlayer`. `concept` builds.
    - `migration_text_counts_as_the_extensions_fragment`: a relation defined only in a migration's `tql` is owned.
    - `from_extensions_excludes_mem_and_requires_it`: mem plus a fixture extension builds; without mem it returns `MemMissing`.
    - `errors_name_the_extension`: the `Display` of a `NotOwned` contains both the extension name and the label.
- [ ] **Step 2: Run** `cargo nextest run -p mem-gecko belief_edge --no-default-features`. Expected: compile failure.
- [ ] **Step 3: Implement** the scanner and registry as specified above.
- [ ] **Step 4: Run** the Step 2 command. Expected: all pass. Then run clippy per the gate. Expected: clean.
- [ ] **Step 5: Commit:** `feat(mem): belief-edge registry enforces extension ownership`.

### Task 5: The bound writer (validation, edge ops, one transaction, live check)

The live-schema check is built here, behind canned reads, so later tasks never invalidate these unit tests.

**Files:**
- Create: `core/mem-gecko/src/writer/edges.rs`, with `mod edges;` in `writer/mod.rs`.
- Modify: `core/mem-gecko/src/tql.rs`:
  - `belief_edge_op`;
  - `belief_edge_schema_reads`.
- Modify: `core/mem-gecko/src/writer/beliefs.rs`. Extract `assert_belief`'s op construction into `pub(super) fn belief_ops(&self, ctx: &RunContext, b: &BeliefDraft, id: &MemId, evidence: &[MemId], method: DerivationMethod) -> Vec<GraphWrite>`. Both callers sit under `writer/`, as do `validate_confidence`, `best_effort_index`, `belief_meta` and `graph`, which `edges.rs` reaches as a sibling module.
- Modify: `core/mem-gecko/src/writer/mod.rs`:
  - fields `belief_edges: BeliefEdgeRegistry` and `checked_relations: Mutex<HashSet<String>>`;
  - `pub fn with_belief_edges(mut self, r: BeliefEdgeRegistry) -> Self`;
  - `pub fn edge_writer_for(self: &Arc<Self>, extension: &str) -> Result<Arc<dyn BeliefEdgeWriter>, EpistemicError>`.
- Modify: `core/mem-gecko/src/writer/tests.rs`. `RecordingGraphStore` gains:
  - `read_reply: Vec<Value>`, returned by every `read` (default empty, today's behavior);
  - `reads: Mutex<usize>`, which counts calls.
- Test: `core/mem-gecko/src/writer/tests.rs`.

**Interfaces:**
- Consumes: `decl`, `owner` and `declares_any` (Task 4); `GraphWrite::expecting` (Task 1); the trait (Task 3).
- Produces:
  - **`tql::belief_edge_op(decl: &BeliefEdgeDecl, belief: &MemId, edge: &BeliefEdge) -> GraphWrite`.**
    - Its `given` stage has `$b: string` (the belief id), one `$p<i>` per role typed from the supplied `GraphValue` variant, and one `$a<i>: <AttrValueType::tql()>` per attribute.
    - The match binds `$bel isa belief, has concept-id $bc; $bc == $b;` and, for each player, `$x<i> isa <player>, has <key> $k<i>; $k<i> == $p<i>;`.
    - It inserts `(<belief_role>: $bel, <role_i>: $x<i>, …) isa <relation>, has <attr_i> == $a<i>, …;` and is `.expecting(1)`.
  - **`tql::belief_edge_schema_reads(decl) -> Vec<Read>`.** These are `fetch`-form type queries, one per condition in spec §4 step 2. Each passes when it returns at least one document.
    - **Probe each shape on 3.12** in a scratch Tier-2 test before relying on it. The conditions are:
      - the relation relates each role;
      - `belief` plays the belief role;
      - each player plays its role;
      - each key is owned `@key` or `@unique` (`concept-id @key` is inherited from `okf-concept`);
      - the relation owns each attribute.
    - **Drop any shape that cannot be expressed**, and list it for the ledger (Task 7). Do not add a method to `GraphStore`.
  - **A private `BoundEdgeWriter { writer: Arc<MemWriter>, extension: String }`** implements `BeliefEdgeWriter`, in this order:
    1. validate;
    2. run the live check, once per relation per writer (a pass inserts into `checked_relations`; a failure is `InvalidInput` naming the relation and the failed condition, and caches nothing);
    3. build `belief_ops(…)` plus one `belief_edge_op` per edge;
    4. make one `graph.write`;
    5. run the same best-effort index step as `assert_belief`.
  - **`edge_writer_for`** returns `InvalidInput` when `!declares_any(extension)`.

**Validation order.** Each failure is an `EpistemicError::InvalidInput` naming the relation, with zero ops written and zero reads made.
1. `validate_confidence(draft.confidence, Some(method))`.
2. The relation is registered to the bound extension. Otherwise the error names the owner, or says that no extension declares it.
3. Each declared role is supplied exactly once, and no unknown role is supplied.
4. Each required attribute is supplied, there are no unknown or duplicate attributes, and `admits` holds for each value.

- [ ] **Step 1: Write the failing unit tests.**

  **Fixture:** a registry built from an inline `probe-link` fragment:
  - role `probe-subject`: `concept`, looked up by `concept-id`;
  - role `probe-object`: `probe-target`, looked up by `probe-ref`;
  - a required `probe-verdict: String` attribute.

  Unless a test says otherwise, `RecordingGraphStore.read_reply` is `vec![json!({"ok": true})]`.
  - `edge_writer_for_refuses_an_extension_with_no_declarations`: both `"mem-gecko"` and `"nobody"`.
  - `bound_writer_refuses_relations_it_does_not_own`:
    - the handle is bound to `y`, and the relation is registered to `probe-ext`: the error names `probe-ext`;
    - a relation no extension declares: the error says so.
  - `bound_writer_validates_roles_and_attrs`: one case each for missing role, duplicate role, unknown role, missing required attribute, unknown attribute, and wrong value variant. Each returns `InvalidInput`; `writes` is empty and `reads` is 0.
  - `bound_writer_writes_belief_and_edges_in_one_call`: two `probe-link` edges.
    - There is exactly one write call.
    - Its ops are the `belief_ops(…)` query texts for the same draft, followed by two edge ops, each with `expect_rows == Some(1)`.
  - `empty_edges_matches_assert_belief`, with `with_retrieval_provenance(true)`, an `OkEmbedder` and `OkIndex` (extend `OkIndex` to count upserts if it doesn't already):
    - the op **query texts** equal `assert_belief`'s for the same inputs;
    - one index upsert happens, as for `assert_belief`.
  - `edge_op_keeps_values_out_of_query_text`: key value `x"; delete $b;` and attribute value `"yes"`. Neither appears in `op.query`; both appear in `op.rows[0]`.
  - `live_check_runs_once_and_failures_are_not_cached`:
    - with `read_reply` non-empty, two writes of `probe-link` make `belief_edge_schema_reads(decl).len()` reads in total;
    - with `read_reply` empty, a write returns `InvalidInput` and makes no write, and a second attempt reads again.
- [ ] **Step 2: Run** `cargo nextest run -p mem-gecko writer::tests --no-default-features`. Expected: compile failure.
- [ ] **Step 3: Implement**, including the probe of the read shapes. The probe goes in `belief_edge_integration.rs` as `#[ignore]` scratch tests, deleted before commit; its result is carried in the commit message.
- [ ] **Step 4: Run** the Step 2 command, then the whole of `cargo nextest run -p mem-gecko --no-default-features`, then clippy. Expected: all pass and clean.
- [ ] **Step 5: Commit:** `feat(mem): per-extension BeliefEdgeWriter writes a belief and its edges atomically`. List the read shapes that were kept or dropped in the commit body.

### Task 6: Tier-2 acceptance tests

**Files:**
- Test: `core/mem-gecko/tests/belief_edge_integration.rs`

**Fixture.** Apply `const PROBE_SCHEMA` after core and mem with `apply_schema`:

```typeql
define
  attribute probe-ref value string;
  attribute probe-tag value string;
  attribute probe-verdict value string @values("yes", "no");
  entity probe-target sub concept @doc("Test player looked up by a unique non-key attribute."),
      owns probe-ref @unique,
      owns probe-tag;
  relation probe-link @doc("Test belief edge."),
      owns probe-verdict @card(1),
      relates probe-subject @doc("Subject."),
      relates probe-object @doc("Object."),
      relates probe-claim @doc("The belief.");
  concept plays probe-link:probe-subject;
  probe-target plays probe-link:probe-object;
  belief plays probe-link:probe-claim;
```

**The declaration:**
- `belief_role "probe-claim"`;
- role `probe-subject`: `concept`, by `concept-id`;
- role `probe-object`: `probe-target`, by `probe-ref`;
- attribute `probe-verdict`: `String`, required.

Build the writer as `Arc::new(MemWriter::without_index(store).with_belief_edges(registry))`, and the handle with `edge_writer_for("probe-ext")`.

**Seeds:** concept `s1`; `probe-target`s `t1` and `t2`, both with `probe-tag "shared"`.

- [ ] **Step 1: Write the tests.**
  - `belief_edge_decl_registry_writes_run_stamped`: one edge to `s1` and `t1`, with verdict `"yes"`.
    - `raw_fetch` finds exactly one `probe-link`.
    - Its `probe-claim` player's `concept-id` is the returned id, its other players are `s1` and `t1`, and its verdict is `"yes"`.
    - That belief has a `source-link` to the `doc-run` whose run id equals `ctx.run_id`. Mirror the existing `source-link` assertions in `writer_reader_integration.rs`.
  - `missing_player_commits_no_belief`: `probe-ref "nope"`. Returns `Err(Storage)`; 0 beliefs with the draft's text.
  - `value_outside_values_commits_no_belief`: verdict `"maybe"`. Returns `Err`; 0 beliefs.
  - `non_unique_key_commits_no_belief`: a second registry whose `probe-object` key is `probe-tag`, written with value `"shared"`. Returns `Err`; 0 beliefs. This holds either way:
    - if the live key check exists, the error is `InvalidInput` and names `probe-tag`;
    - if Task 5 dropped that check, the count check returns `Storage`.

    The test asserts `Err`, 0 beliefs, and the variant matching Task 5's outcome.
  - `live_schema_mismatch_is_refused_before_writing`: a registry whose declaration names role `probe-missing`, built over a fragment string that defines it but was **not** applied. Asserts `Err(InvalidInput)`, which rules out a TypeDB rejection (that would be `Storage`); the message names `probe-missing`; 0 beliefs.
  - `two_edges_of_one_relation_both_land`: edges to `t1` and `t2`. Exactly 2 `probe-link`s share the one belief.
- [ ] **Step 2: Run** `cargo nextest run -p mem-gecko --test belief_edge_integration --no-default-features --test-threads=1`. Expected: all pass. These are acceptance tests over Task 5. If any fails, fix Task 5's code, not the assertion.
- [ ] **Step 3: Commit:** `test(mem): belief-edge seam acceptance tests against TypeDB`.

### Task 7: Assembler wiring, ledger, final gates

**Files:**
- Modify: `core/gecko-engine/src/db/mod.rs` (`pub const CORE_SCHEMA: &str = include_str!("../../schema/core_schema.tql");`)
- Modify: `gecko-bin/src/main.rs`:
  - **`:613`:** use `gecko_engine::db::CORE_SCHEMA`.
  - **`:779-860`:** the signature becomes `build_epistemic_writer(store, cfg, edges: BeliefEdgeRegistry) -> Result<Arc<mem_gecko::MemWriter>>`, applying `.with_belief_edges(edges)` before each `Arc::new`.
  - **`:959-990`:** `build_host_bridge` calls `BeliefEdgeRegistry::from_extensions(CORE_SCHEMA, &extensions)` before `extensions` moves into `base_cb`. It maps the error to `anyhow` with the context `"belief-edge declarations"`, and coerces the writer to `Arc<dyn EpistemicWriter>` wherever it is used today. No extension takes a handle yet; cyber does in A2.
- Modify: `core/mem-gecko/Cargo.toml` (dev-dependency `cyber-gecko = { path = "../../extensions/cyber-gecko" }`; no cycle, since cyber depends only on the API and engine crates).
- Test: `core/mem-gecko/src/belief_edge/registry.rs`
- Modify: `docs/refactor/OWED-WORK-LEDGER.md`:
  - A1 → ✅, with the test names as evidence;
  - a new item for the write-count audit, copying the four ranked entries in spec §2;
  - under "Implementation notes": Task 1's insert and update results, and any live checks Task 5 dropped;
  - move the reconcile anchor.
- Modify: `docs/refactor/SPEC-W5.md` §5. Append a note under row 5 that `belief_edge_decl_registry_writes_run_stamped` ships in `core/mem-gecko/tests/belief_edge_integration.rs`. Don't edit the ratified text.

The wiring is verified by the gecko-bin suite and the registry test below. A bad declaration failing `gecko run` is covered by `errors_name_the_extension` (Task 4), since the only declarations are compiled-in. Nothing in this task fails first.

- [ ] **Step 1: Add** `from_extensions_accepts_the_shipped_extensions`: `from_extensions(<core text>, &[mem_gecko::extension(), cyber_gecko::extension()])` returns `Ok`. It keeps the shipped set valid as A2 adds declarations.
- [ ] **Step 2: Implement** the wiring and the doc edits.
- [ ] **Step 3: Run the final gates.**
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features -- -D warnings`;
  - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --exclude js-sandbox --no-default-features --no-deps`;
  - `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --test-threads=1`.

  Expected: all clean. The suite is the previous 288 plus the new tests, with `cyber_detect_test`, `epistemic_bridge_test` and `mem_recall_e2e_test` unchanged.
- [ ] **Step 4: Commit:** `feat(bin): build the belief-edge registry for runs; record A1 in the ledger`.
