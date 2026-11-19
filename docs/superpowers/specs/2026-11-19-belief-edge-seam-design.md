# Belief-edge seam (ledger A1) — design

**Status:** draft for the user's approval (builder check done; open question answered). Owning spec: SPEC-W5 §3.2 as amended by §3.5
(decisions D-B and D-D). Review input: REVIEW-2026-11 §3 and an independent pre-implementation
review (2026-11-19) whose findings are folded in below.

## Intent

An extension that links its own domain records to a mem belief — cyber's `technique-detection`
and `alert-disposition` today, other domains' relations later — needs a sanctioned way to write
that link. Today cyber writes the belief through `EpistemicWriter::assert_belief` and then the
relation with a raw `GraphStore::write` (`extensions/cyber-gecko/src/detect/mod.rs:142,196`) in a
second transaction. That raw write is the breach W5a exists to catch, and the two-transaction
split can leave an orphan belief.

A1 builds the mem-owned seam those writes move onto. The user intends to extend GECKO to other
domains, so the seam is the template every future domain copies: it has to be general (no core
method per domain relation, invariants 3 and 5) and not free-form (an extension can write only
relations it declared and owns).

## Goals

- Extensions **declare** their belief-referencing relations at registration.
- Mem builds a **registry** from the declarations and refuses any relation the declaring extension
  does not own (D-B).
- One method writes **a new belief together with its declared edges in one transaction**. Mem
  authors all the TQL; the belief role is always the newly minted belief.
- Each extension writes through a handle **bound to its own name**, so it can write only the
  relations it owns. Extension Y cannot write cyber's `technique-detection`, even by accident
  (decided by the user, 2026-11-19).
- Every malformed or unsatisfiable request fails with an error **before or within** the single
  transaction, so nothing — belief included — is committed.

## Non-goals

- Cyber's migration onto the seam (A2) and the read-only handle / neutral sync writes it lands
  with (A9).
- Mem-owned edges: `supersession`, `resolves`, `scoped`, `divergence-precedence`. They get
  dedicated `MemWriter` methods (A5, A7), never registry entries.
- Attaching edges to an **existing** belief or episode. A8 (`sighting-of`) needs an episode-side
  variant later; see "Provenance" below for why it needs its own design.
- Multiple player types or lookup keys per role.
- Defending against a deliberately hostile extension (SPEC-W5 §3.5 "Limit").

## Anti-goals

- A seam that reports `Ok` while writing nothing, or writing the belief without its edge.
- A seam an extension can use to write a mem-owned relation, directly or through a subtype.
- Query text assembled from caller-supplied strings that were not validated.

## Approaches considered

1. **Edge-only writer** — a method that links an existing belief. Rejected: it keeps cyber's
   two-transaction split and leaves "a belief the run is entitled to reference" undefined
   (REVIEW §3 point 4).
2. **Belief-plus-edges in one call (chosen).** The edge ops are appended to the op list
   `assert_belief` already builds and commits once (`core/mem-gecko/src/writer/beliefs.rs:45-83`).
   The belief role is filled from the freshly minted id, so a caller cannot point it elsewhere.
   This is the same shape the co-mint writer (A5) needs.
3. **Folding A2 into A1.** Rejected (user and reviewer agree): A2 is coupled to A9, and bundling
   would bury the seam. A1 is proven with a test-only extension fixture shaped like cyber.

## Design

### 1. Extension API (`core/gecko-extension-api`)

New module (e.g. `belief_edge.rs`), re-exported from `lib.rs`:

```rust
pub struct BeliefEdgeDecl {
    pub relation: &'static str,
    pub belief_role: &'static str,           // filled with the new belief
    pub roles: &'static [RoleDecl],          // the non-belief roles
    pub owned_attrs: &'static [AttrDecl],
}
pub struct RoleDecl {
    pub role: &'static str,
    pub player: &'static str,                // one type; subtypes admitted (`isa` is polymorphic)
    pub key: &'static str,                   // attribute that identifies the player
}
pub struct AttrDecl {
    pub attr: &'static str,
    pub value: AttrValueType,                // String | Datetime | Double | Long | Boolean
    pub required: bool,                      // true for a `@card(1)` attribute
}

pub struct BeliefEdge {                      // one edge to write with the belief
    pub relation: String,
    pub players: Vec<(String, GraphValue)>,  // (role, key value)
    pub attrs: Vec<(String, GraphValue)>,    // (attr, value)
}
```

- `AttrValueType` maps one-to-one onto `GraphValue`'s variants and onto the TypeQL type name used
  in the `given` stage.
- `GeckoExtension::belief_edges(&self) -> &[BeliefEdgeDecl]` defaults to `&[]`.
- A separate trait, `BeliefEdgeWriter`, has one method:
  `assert_belief_with_edges(ctx, draft, evidence, method, edges: &[BeliefEdge]) -> Result<MemId>`.
  - It is **not** a method on `EpistemicWriter`. The only way to obtain one is the bound handle
    below, so there is no unbound path to the edge write.
  - `EpistemicWriter` and its implementors (`MemWriter`, `DummyWriter`) are unchanged.

### 1a. The per-extension bound handle

- Mem provides `MemWriter::edge_writer_for(extension: &str) -> Arc<dyn BeliefEdgeWriter>`.
  - It returns a small wrapper holding the writer, the registry and the extension's name.
  - It errors if the registry has no declarations from that extension.
- The assembler (`build_host_bridge`) calls it once per activated extension that declares belief
  edges, and passes the result into that extension's bridge. The name comes from the host's own
  extension list, never from the extension or the guest, the same way `RunContext` identity is
  host-stamped.
- Before anything else, `assert_belief_with_edges` refuses any edge whose relation is not
  registered **to the bound extension**. The error names the relation and the owner.
- A1 adds the method and the wiring point. Cyber's bridge starts taking the handle in A2; until
  then, no production extension declares edges and the handle is not built.

### 2. Write-count enforcement (`GraphWrite`, `RouterGraphStore`)

Without this, a `match … insert` whose match finds no player is a successful no-op:
`RouterGraphStore::write` discards every answer (`core/gecko-engine/src/db/graph_store.rs:71-98`).
The belief would commit with no edge and the call would return `Ok`.

- `GraphWrite` gains `expect_rows: Option<usize>`. When set, the op must produce exactly that
  many answer rows **per given row**. `None` keeps today's behavior. The `single`/`plain`
  constructors default to `None`, and a builder method sets it.
- `RouterGraphStore::write` consumes each op's answer stream and counts the rows. On a mismatch it
  returns `EpistemicError::Storage` **without committing**, so the transaction rolls back.
- The in-memory `RecordingGraphStore` (`core/mem-gecko/src/writer/tests.rs:60`) records the field
  so unit tests can assert it is set.
- Edge ops set `expect_rows = Some(1)`. This also catches a non-unique key, which would otherwise
  insert two edges.
- **Probe first:** confirm on TypeDB 3.12 that an `insert` answer is a row stream with one row
  per inserted match, including under `query_with_rows` with one given row. Record the result in
  the ledger's implementation notes. **If the probe is negative** *(builder's call)*, stop and
  report back before building the write path. Write-count enforcement is the guard against a
  silent edge-less commit, so it is not optional.
- **`supersede` is fixed in A1** (decided by the user, 2026-11-19):
  - **The gap.** For a missing `old` id, `read_entrenchment` returns `None`. The new belief
    inserts, but the link and the flip in `supersession_ops` (`tql.rs:277-304`) match nothing.
    The call returns `Ok(new_id)`: a new belief that supersedes nothing. A guest reaches this
    through `mem.supersede` (`core/gecko-engine/src/sandbox/mem_host.rs:235-243`). It is
    fabricated success, not an entrenchment bypass, since nothing more entrenched is replaced.
  - **The fix.** The link op sets `expect_rows = Some(1)`. That is the authoritative check:
    in-transaction, with no check-then-write race. The flip op sets it too if the probe shows
    `match … update` answers count rows the same way. If they don't, the link alone suffices,
    because both ops share one transaction.
  - **Keep `read_entrenchment` as is.** `None` also means "belief with no tier", which is
    handled deliberately (`beliefs.rs:100-103`).
  - **Test (Tier 2).** Superseding a missing id returns `Err`, and no new belief is committed.
- **The probe covers `update` as well as `insert`.**
- **The remaining mem writes are a follow-up, not A1.** A1 logs a ledger item, ranked by guest
  reachability:
  1. `derivation_op`, via `mem.derive`: one missing evidence id silently stores the belief as
     underived. Expect 1, since it is one conjunctive match. This is a behavior change for any
     playbook passing ids that match nothing today.
  2. `contest_ops`, via `mem.contest`: a missing claim returns the id of an anomaly that was
     never written. Expect 1.
  3. `rests_on_op` and `record_prediction_op`: not guest-exposed. Expect 1.
  4. `resolve_prediction_ops`: `record_prediction` has no uniqueness guard
     (`beliefs.rs:160-162`), so the right count needs a decision first.

  Not every write gets a count: the populations ops match zero rows by design, as their
  guards.

### 3. Registry (`core/mem-gecko`, new `belief_edge.rs`)

`BeliefEdgeRegistry::build(core_schema: &str, mem_schema: &str, extensions: &[(&str /*name*/,
&str /*schema*/, &[BeliefEdgeDecl])]) -> Result<Self, RegistryError>` is a **public pure
function**. It can be tested without TypeDB, and A2's `cyber_detect_test` can construct a writer
that has the registry.

The checks, all at build time:

- **Labels.** Every relation, role, player, key and attribute label matches
  `^[a-z][a-z0-9-]*$`. Labels are interpolated into query text because they cannot be `given`
  parameters; this check is the seam's exception to "all TQL is parameterized".
- **Ownership (D-B).** The declaring extension's fragment introduces the relation:
  - its token stream contains a `relation <label>` statement head;
  - no other fragment (core, mem, or another extension) contains one;
  - that statement has no `sub` clause, so a subtype of a mem relation, whose instances are
    `isa supersession`, cannot pass;
  - none of its roles is declared `as` another role.
- **Scanner.** The check scans **tokens**, not lines. It strips `#` comments and `"…"` string
  literals first, because every relation carries `@doc("…")` prose and mem's types have trailing
  comments (`core/mem-gecko/schema/mem_types.tql:126,130`). It must not be a line-anchored regex.
- **Shape.** The belief role and every declared role are distinct. Each declaration names its
  relation once, and no two declarations share one.
- **No memory-item players on non-belief roles** *(builder's call)*. A non-belief role's
  player may not be `memory-item`, any subtype of it (`belief`, `episode`, an extension's
  `sighting sub execution-episode`), or a supertype of it (`okf-concept`, which would admit beliefs
  polymorphically). Otherwise the seam could point an edge at an existing belief or episode,
  which is A8's case, and the transitive-provenance property (§5) would break. The check resolves
  the hierarchy by walking `sub` heads across every scanned fragment. `concept` is allowed:
  `concept` and `memory-item` are sibling subtypes of `okf-concept`
  (`core_schema.tql:63`, `mem_types.tql:61`).

**Which fragments are scanned** *(builder's calls)*:
- Core, from the embedded text.
- Mem, passed as `mem_schema` and excluded from the extension slice, so it is not counted twice.
- Every **activated** extension: its `schema()` plus the `tql` of each of its
  `schema_migrations()`, since a migration can define types.

Compiled extensions that are not activated are not scanned. Their schema reaches the database
only if they were activated at an earlier `schema-init`. That is a second documented blind spot,
alongside `--schema`.

**Build failure** *(builder's call)*: `gecko run` fails with an error naming the extension and
the declaration. A bad declaration is a defect in compiled-in code, not a config typo, so failing
fast is correct. Only the `Run` arm builds the registry, so `up`, `drop`, `databases` and the sync
commands are unaffected.

The registry is built in the `Run` arm only. `build_epistemic_writer` has a single caller
(`gecko-bin/src/main.rs:968`). Build it before `extensions` moves into the base callback (`:984`)
and pass it into `MemWriter` before the `Arc::new` erasure points (`:785`, `:857`). The core schema
text is hoisted out of `apply_all_schemas` (`main.rs:613`) into a shared constant.

**Documented blind spot.** `schema-init --schema <file>` replaces the core schema in the database
(`main.rs:608-611`), but the registry checks ownership against the embedded core text. A custom
core that defines an extension's relation would not be caught.

### 4. The write (the bound handle's `assert_belief_with_edges`)

1. **Pre-transaction validation.** Every rule below is checked against the registered
   declaration, and any failure is an error with nothing written:
   - the relation is registered to the handle's bound extension;
   - every declared role is supplied exactly once, and no unknown role is supplied;
   - every `required` attribute is supplied, and no unknown attribute is;
   - each supplied value's `GraphValue` variant matches its declared value type.

   The schema does not enforce roles itself: `technique-detection`'s roles carry no `@card`
   (`extensions/cyber-gecko/schema/cyber_schema.tql:151-154`).
2. **Live-schema check, once per relation per writer.** Before a relation's first write, the
   writer confirms against the database that:
   - the relation relates each declared role;
   - `belief` plays the belief role;
   - each player type plays its role;
   - each key is owned by its player type with `@key` or `@unique`;
   - each attribute is owned by the relation.

   A pass is cached; a failure is an error and nothing is cached.
   - `GraphStore::read` returns only document streams (`graph_store.rs:118-126`), so these must
     be `fetch`-form type queries.
   - The only introspection precedent uses `select` on the raw driver
     (`gecko-bin/tests/schema_probe_w2w3.rs:1984-1992`).
   - **Probe the query shapes first.** A validator that silently sees zero rows would falsely
     refuse or falsely pass.
   - **If the probe shows `fetch` cannot express a check** *(builder's call)*, drop that check
     and note it in the ledger. Do not add a row-returning method to `GraphStore`, which would
     collide with A9's `GraphReader` split. Every condition this check covers still fails at
     commit, inside the single transaction, so nothing is committed. The live check only makes
     the error earlier and clearer.
3. **One transaction.** `assert_belief`'s op list is reused, with the edge ops appended. Each
   edge op:
   - matches the new belief by `concept-id`;
   - matches each player by `isa <player>, has <key> $k` with the key value as a `given` param;
   - inserts the relation with its attributes, which are also `given` params;
   - sets `expect_rows = Some(1)`.

   The query text is built in `tql.rs` from validated labels only. There is no `not { … }`
   idempotence guard: every call mints a new belief, so the guard cyber uses today
   (`detect/mod.rs:131`) is vacuous.
4. Index the belief as `assert_belief` does, best-effort after commit.

`assert_belief` and `assert_belief_with_edges` share one private helper that builds the
belief op list, so the two cannot drift apart.

### 5. Provenance

Neither cyber relation owns provenance attributes. An edge's run identity is **transitive**: it is
the co-minted belief's `source-link` to the `doc-run` (`tql.rs:228`). That holds only because the
belief role is always the belief minted in the same call. A8, which attaches edges to an existing
item, breaks this property and must design its own stamp. This doc records that as A8's
obligation.

### 6. Tests

**Serviceless (mem-gecko unit tests):**
- **Ownership refusals:**
  - a relation defined by mem;
  - one defined by core;
  - one defined by another extension;
  - a role-addition to someone else's relation;
  - `relation x sub supersession`;
  - a `relates … as` role;
  - a label inside a `@doc` string or a comment that must not count as a definition.
- **Label pattern refusals.**
- **Request validation:** missing role, duplicate role, unknown role, missing required attribute,
  wrong value type, unregistered relation. Each is an error with zero ops sent to the
  `RecordingGraphStore`.
- **Ownership at write time:** a handle bound to extension Y refuses a relation registered to
  extension X; `edge_writer_for` errors for an extension with no declarations.

**Tier-2 (live TypeDB, `--test-threads=1`, throwaway database):** a test-only extension fragment
mirrors cyber's real shapes. It has three roles, one player looked up by a `@unique` non-key
attribute on a **subtype**, and an owned `@card(1) @values(…)` attribute.
- `belief_edge_decl_registry_writes_run_stamped`:
  - the edge exists with the right players and attribute;
  - its belief player is the returned id;
  - that belief's `source-link` reaches `ctx.run_id`.
- **Missing player:** a key that matches nothing returns `Err`, and zero beliefs are committed.
- **Bad attribute value:** a value outside `@values` returns `Err`, and zero beliefs are committed.
- **Duplicate key match:** two players share the lookup value, so the count check fails, and zero
  beliefs are committed.
- **Live-schema mismatch:** a declaration naming a role the relation does not have returns `Err`
  before any write.

The existing suite stays green: `cyber_detect_test` is unchanged in A1.

## Left to the builder

- Module and file names, error enum shape, and whether `RegistryError` folds into
  `EpistemicError`.
- How `MemWriter` receives the registry: a builder method or a constructor argument.
- Exact `fetch` shapes for the live-schema check, after the probe.
- Ledger and SPEC-W5 §5 updates: mark A1 landed, name the shipped tests, and add the
  write-count audit item above.
