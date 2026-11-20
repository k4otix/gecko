# Cyber onto the belief-edge seam, read-only extension handles, undefine/redefine refusal, and the W5a static lint (ledger A2, A9, A12, A3) — design

**Status:** draft for the user's approval (builder check done; its one open question answered). Owning spec: SPEC-W5 §2.1, §3.3 and §3.5 (D-B, D-D
steps 1–2). Builds on the A1 seam (`docs/superpowers/specs/2026-11-19-belief-edge-seam-design.md`,
merged in #54).

## Intent

A1 gave extensions a sanctioned way to write a belief together with the relations that link it to
their records. Nothing uses it yet: cyber still writes `technique-detection` and `alert-disposition`
with a raw `GraphStore::write` after the belief commits, and its bridge holds a write-capable graph
handle it needs only for reads. This batch closes the extension-side holes in the belief-write
monopoly (invariant 2) and adds the first check that fails CI when one reopens:

- **A2** moves cyber's two belief edges onto the seam, so each belief and its edge commit in one
  transaction (today a failed edge leaves a belief with no edge).
- **A9** gives extension bridges a read-only graph handle and turns cyber's STIX post-sync into a
  pure function returning `GraphWrite`s that the engine runs inside the sync transaction. Cyber
  stops depending on `typedb-driver`.
- **A12** makes the registry refuse an extension `undefine` or `redefine` that names a core or mem
  type or function.
- **A3** adds the W5a static lint `no_raw_belief_tier_tql_outside_mem_tql`: no production source
  outside `core/mem-gecko/src/tql.rs` may author an `insert`/`put`/`update` of a belief-tier type.

The user plans more domain extensions; cyber is the template they will copy, so the result must
read as the right way to write an extension, and the lint is what keeps the next one honest.

## Goals

- Cyber writes no belief-tier TQL. `claim` and `disposition` each make one
  `assert_belief_with_edges` call; `detect/mod.rs` has no `graph.write`.
- Cyber's bridge holds only an `Arc<dyn GraphReader>` and an `Arc<dyn BeliefEdgeWriter>` bound to
  `cyber-gecko`. No extension *bridge* holds a `GraphStore` or an `EpistemicWriter`. The
  `GeckoExtension::inject_epistemic_host_fns` hook still offers every extension the full writer
  (`main.rs:982-984`). Cyber ignores it (the default no-op), and narrowing it is owed with A10.
  A note goes in the ledger.
- A STIX import is all-or-nothing: the bundle sync and the typed-relation writes share one
  transaction.
- `extensions/cyber-gecko/Cargo.toml` no longer lists `typedb-driver`.
- The registry refuses an extension `undefine` or `redefine` of anything core or mem introduces,
  fail-closed.
- The static lint passes on the finished branch, and its scanner is shown to flag today's cyber
  literals.
- `cyber_detect_test` and `cyber_stix_sync_test` stay green with their assertions unchanged
  (SPEC-W5 §5 row 7). Only how they build the bridge and run the sync changes.

## Non-goals

- **A4**, the dynamic rig (`memwriter_is_the_sole_belief_tier_writer`,
  `hostile_extension_raw_belief_write_is_flagged`). It is owed separately.
- **The rest of A10.** Cyber keeps its `gecko-engine` dependency (for `ExtensionCallback` and the OKF
  types), and there is no CI dependency guard yet. Only the `typedb-driver` dependency goes.
- **A8**, `sighting-of`. It stays owed-to-seam-when-written. The lint covers it automatically if
  anyone writes it raw.
- Narrowing mem's own handles (for example `MemReader` to `GraphReader`). Mem is the one holder of
  a `GraphStore`.
- A `GeckoExtension` trait method for post-sync. The hook stays a free function the `sync-stix`
  arm calls; moving it onto the trait belongs with A10.

## Anti-goals (failure even if it technically works)

- **A lint that passes vacuously.** If introspection returns an empty or partial surface, or the
  scanner never reads the files, the lint must fail, not pass.
- **A STIX import that half-commits.** If any typed-relation write fails, the bundle sync must not
  be committed.
- **A bridge that can still reach a raw write.** That includes a `GraphStore` hidden behind a
  wider trait object, or a writer handle not bound to the extension.
- **Changed detect behaviour.** Argument validation, error messages for missing logic or technique,
  the result JSON of `claim` and `disposition`, and the read functions all stay as they are.

## Constraints

- All TQL is parameterized. The only interpolated text is fixed schema labels, as today.
- Comments are greenfield (CLAUDE.md).
- Integration tests use throwaway databases, never `gecko`, and run with `--test-threads=1`.
- CI runs `cargo nextest … --no-default-features --features cyber` against TypeDB 3.15, and clippy
  `--all-targets -D warnings`. The lint is a TypeDB-backed (Tier-2) test, like `no_drag_test`.
- Trait upcasting (`Arc<dyn GraphStore>` to `Arc<dyn GraphReader>`) is stable on the workspace
  toolchain.

## Approaches (A3's belief-tier surface — the open choice)

1. **Live introspection (chosen).** Apply the composed schema to a throwaway database and derive
   the surface with the P-W5-1 recipe (`belief_edge_surface`, `schema_probe_w2w3.rs:1967`): the
   `memory-item` subtype closure, plus every relation with a role played only by memory items.
   It matches SPEC-W5 §1.1 ("introspection-derived, not a hand list"). A new belief-edge relation is
   covered with no test edit. The cost is a TypeDB dependency, which CI already provides.
2. **Static schema-text closure.** Derive the same set from the `.tql` text with no database. It
   would run in Tier 1, but it is a second hand-written schema interpreter that can drift from
   TypeDB's real answer. Rejected.
3. **A hand list.** Rejected by SPEC-W5 §1.1 by name.

Other choices, decided as stated in the conversation:

- **Atomic STIX import.** The spec says the post-sync writes run "in its sync transaction", so they
  do.
- **Cyber's bridge loses `EpistemicWriter`.** After A2 it never writes a belief without an edge,
  and least privilege is the point of D-D. This was my call, put to the user without objection.
- **A12 covers functions as well as types.** `undefine fun <mem function>;` breaks mem as surely as
  `undefine belief;`. My call, stated to the user.
- **A3 is included and A4 deferred.** The user's choice.
- **A12 covers `redefine` too.** The user's choice, from the builder check.

## Design

### A9 — `GraphReader` and neutral sync writes

**API (`core/gecko-extension-api/src/graph.rs`).**

- Add `#[async_trait] pub trait GraphReader: Send + Sync` holding `read` (signature and doc
  unchanged).
- `GraphStore` becomes `pub trait GraphStore: GraphReader` holding only `write`.
- Re-export `GraphReader` wherever `GraphStore` is re-exported.
- Every implementor (`RouterGraphStore` and the in-memory test stores in mem-gecko) splits its
  `read` into an `impl GraphReader`. This change is mechanical.

**Engine.**

- Factor the op loop of `RouterGraphStore::write` (`db/graph_store.rs`) into one function that runs
  `&[GraphWrite]` on an open driver transaction. It keeps the empty-rows skip, the parameterless
  path and the row-count check, and `RouterGraphStore::write` calls it.
- `sync_bundle` takes a new parameter `extension_writes: &[GraphWrite]` and runs it through that
  function after step 6 and before `commit`. Any error rolls the whole sync back, surfaced as a
  `DbError`.
- Existing callers pass `&[]`. `sync_manifest` in `main.rs` takes the slice through to
  `sync_bundle`.

**Cyber post-sync.**

- `cyber_post_sync` becomes `pub fn cyber_post_sync(typed_rels: &[TypedRelation]) -> Vec<GraphWrite>`.
  It keeps the same grouping by shape and the same query text. Each shape is one multi-row
  `GraphWrite`, with `vars` `["src", "tgt"]` and `expect_rows: None`.
- `None` keeps today's semantics: the not-guard makes a re-import match zero rows, and a dangling
  STIX ref also matches zero.
- The function is infallible.
- `typedb-driver` leaves `extensions/cyber-gecko/Cargo.toml`. The builder confirms there is no other
  use.

**`sync-stix` arm.** It computes the writes from `typed_rels`, prints the typed-relation count, and
calls `sync_manifest(config, &extensions, &manifest, &writes)`. The separate `begin_write` /
`cyber_post_sync` / `commit` block is deleted.

### A2 — cyber on the seam

**Declarations.** Cyber's `GeckoExtension::belief_edges()` returns two static `BeliefEdgeDecl`s that
match `cyber_schema.tql`:

| relation | belief_role | roles (role / player / key) | owned_attrs |
|---|---|---|---|
| `technique-detection` | `detection-claim` | `detecting-logic` / `concept` / `concept-id`; `detected-technique` / `attack-pattern` / `attack-id` | — |
| `alert-disposition` | `disposition-belief` | `dispositioned-logic` / `concept` / `concept-id` | `detection-verdict`, a string |

**Handlers (`detect/mod.rs`).**

- `claim` builds the same `BeliefDraft` and calls `edges.assert_belief_with_edges(ctx, draft,
  &evidence, method, &[edge])`. The edge is `technique-detection` with the players
  `("detecting-logic", logic_id)` and `("detected-technique", technique_id)`.
- `disposition` does the same with `alert-disposition`, the player `("dispositioned-logic",
  logic_id)` and the attr `("detection-verdict", verdict)`.
- Validation, the `ensure_*` existence checks (now reads through `GraphReader`) and the result
  JSON are unchanged.
- The raw `GraphWrite`s and their not-guards are deleted. Each call mints a fresh belief, so the
  guard had nothing to deduplicate.

**Bridge signature.** The new signature is:

```rust
pub fn cyber_extension_callback(
    graph: Arc<dyn GraphReader>,
    edges: Arc<dyn BeliefEdgeWriter>,
    ctx: RunContext,
    base: ExtensionCallback,
) -> ExtensionCallback
```

**Wiring (`build_host_bridge`).**

- Keep the concrete `Arc<MemWriter>` that `build_epistemic_writer` already returns.
- Coerce a clone to `Arc<dyn EpistemicWriter>` for mem's own bridge.
- When cyber is active, pass `mem_writer.edge_writer_for("cyber-gecko")?` and a read-only handle
  (`store` upcast to `Arc<dyn GraphReader>`; a second `RouterGraphStore` over the same router is
  also fine).
- If cyber is active and the registry holds no declarations for it, the run fails before it starts.

**Docs.** Update `extensions/README.md`: the bridge shape example, the note on the `GraphStore` seam
(bridges get `GraphReader`, belief edges go through `BeliefEdgeWriter`), checklist step 4, and a
sentence on post-sync returning `GraphWrite`s. Update the matching CLAUDE.md "Host-import seams"
paragraph, which names `GraphStore`/`EpistemicWriter` as what a bridge captures.

### A12 — refuse `undefine` and `redefine` of core/mem

Scope decided by the user (2026-11-19): A12 covers `redefine` as well as `undefine`. An extension
that redefines a mem type (`redefine attribute confidence value string;`, or a changed cardinality on
`belief owns …`) breaks mem as surely as one that undefines it.

- **Scanner (`belief_edge/scan.rs`).** Each `Statement` records the query mode it belongs to: the
  last leading `define`, `redefine` or `undefine` seen in the same text, carried forward across
  statements until another one appears. Text with no leading keyword is in define mode.
- **Registry.**
  - `base_types` (from `introduced_labels(core ∪ mem)`) gains the function names that core and mem
    introduce (`fun <name>`).
  - A new `RegistryError::ModifiesBase { extension, mode, label }` (`mode` is `undefine` or
    `redefine`) is raised in `build`'s per-extension loop, beside the alias and rename checks. It
    fires when any statement in undefine or redefine mode has a token in that set.
  - **Functions, in any mode.** An extension statement that introduces `fun <name>` with a core or
    mem function name is refused even under plain `define`. The engine rewrites
    `define fun X` into `redefine fun X` whenever the database already stores `X`, mem's
    functions included (`db/schema_apply.rs:170-176, 212-215`), so a define-mode function with a
    base name would silently replace mem's function.
  - **Changing an extension's own function** goes through that same automatic redefine: the
    extension edits the function in its `define` fragment. An explicit `redefine`/`undefine` block
    is checked token by token, and function bodies inherit the block's mode. So an explicit
    `redefine fun` of the extension's own function whose body names a base type is refused. Both of
    cyber's functions name `belief-state`. A unit test pins this with a realistic body, and
    `extensions/README.md` says to change functions through `define`. *(My call, from the Fable
    review.)*
  - Statements are tokenized, so the check catches `undefine belief;`, `owns x from belief;`,
    `plays r:x from belief;`, `@abstract from belief;`, `fun believed-at;`,
    `redefine attribute confidence value string;`, `redefine belief owns confidence @card(1);` and
    `redefine fun believed-at(…)`.
  - The existing rename refusal stays, with its own error. A rename of a base type is reported as
    `RenameUnsupported`, not `ModifiesBase`: the rename check runs first.
  - An extension's undefine or redefine of its own types, roles and functions stays allowed.
  - A statement that names a base type anywhere is **refused**, even when its subject is the
    extension's own. Two examples are removing the extension's own role-addition from a mem type
    (`undefine plays technique-detection:detection-claim from belief;`) and
    `redefine my-type owns confidence @card(1);`. This is fail-closed, like the rest of the
    registry.
  - Mem's own migrations (for example the `redefine` of `population-member` in `MEM_MIGRATIONS`) are
    part of the base text, never extension statements, so they are unaffected. A unit test builds
    the registry from the real extensions to pin this.
  - Today no extension uses `undefine` or `redefine` (checked: none in `extensions/`).
  - A role name that collides with a base label (`attack`, `evidence`, `support`, `scope`, …) in
    an extension's own `redefine … relates evidence @card(..)` is also refused. This is
    fail-closed; an extension avoids such role names or makes the change through `define`.
- **Where the check runs: before any extension schema is applied.** The registry is built only in
  `build_host_bridge` (the `run` arm). `apply_all_schemas` (`main.rs:603-634`, reached from
  `schema-init`, `sync` and `sync-stix`) applies extension fragments and migrations with no
  registry. Without a change, `undefine belief;` would be committed at schema-init, and the refusal
  would fire only on the next `run`. So `apply_all_schemas` builds
  `BeliefEdgeRegistry::from_extensions(core_schema, extensions)` first and fails before applying any
  extension text. That one call runs every registry rule (alias, rename, reparenting, D-B ownership,
  A12) at schema time too. A test drives `apply_all_schemas` (or the helper it calls) with a fixture
  extension whose fragment has `undefine belief;`. It must return the refusal, and `belief` must
  still be in the schema afterwards. *(My call, from the Fable review.)*
- **Docs.** `extensions/README.md` gets a rule line beside the alias and rename lines. It covers
  undefine and redefine, changing functions through `define`, and retiring a belief edge. Retiring
  one cannot be done from an extension fragment, because it names a mem type. It is a reviewed
  change to the fragments, applied to a fresh database or by a migration that mem reviews.

### A3 — the static lint

**Location.** `gecko-bin/tests/belief_tier_monopoly_test.rs`, beside `no_drag_test.rs`, with test
`no_raw_belief_tier_tql_outside_mem_tql`. It is `cfg(feature = "cyber")`, because the composed
schema must include cyber's fragment for cyber's relations to be on the surface. CI enables cyber.
`just test-it` runs `--no-default-features` without cyber (`justfile:48`), so it gains
`--features cyber`, matching CI, and the lint runs locally too. *(My call.)*

**Test-helper plumbing.** `gecko-bin/tests/common/mod.rs` has no `allow(dead_code)`. A helper there
that some test crates do not call fails `clippy --all-targets -D warnings`. So `belief_edge_surface`
goes in its own file, included only by the two crates that use it (for example
`#[path = "common/surface.rs"] mod surface;`).

**Surface.**

- Move `belief_edge_surface` from `schema_probe_w2w3.rs` into `gecko-bin/tests/common/` so the probe
  and the lint share one derivation.
- The lint's surface is the `memory-item` subtype closure (entities) plus the surface relations.
- The schema is composed the way `apply_composed_schema` does it (core, mem, cyber).
- **Non-vacuity:** the test asserts the surface contains `belief`, `technique-detection`,
  `alert-disposition` and `supersession` before it scans.

**Scan scope.** Production sources only:

- **Rust files:** every `.rs` file under a `src/` directory of a workspace member (`core/*`,
  `extensions/*`, `gecko-bin`). Skip files named `tests.rs` and anything under a `tests/` directory.
  Within a file, skip only the braced body of each inline `#[cfg(test)] mod <name> { … }` block. An
  out-of-line `#[cfg(test)] mod tests;` is a single item, and production code after it is still
  scanned. *(My call, from the builder check: a cut at the first `#[cfg(test)]` would silently skip
  most of `core/mem-gecko/src/writer/mod.rs`, which declares `mod tests;` at line 30.)* A unit test
  pins this: a source with `#[cfg(test)] mod tests;` followed by a flagged literal is still flagged.
- **TQL files:** every `.tql` file in the workspace outside `tests/` directories (so an extension
  that keeps queries in `src/queries/*.tql` is covered). Mem's schema `.tql` files hold no writes.
- **Why tests are out:** test sources legitimately author raw belief-tier writes as fixtures, and
  A4's hostile stub will too.
- **Allowed path:** the one allowed file is `core/mem-gecko/src/tql.rs`.
- **Non-vacuity:**
  - The test asserts it scanned a nonzero number of files, including
    `extensions/cyber-gecko/src/detect/mod.rs`.
  - **Live positive control:** the same scan run over the allowed file `core/mem-gecko/src/tql.rs`
    must yield hits for at least `belief` and `supersession`. If the literal extractor desyncs (for
    example on a char literal like `'"'`), this check fails instead of passing silently.

**Detection.**

- For Rust, take the contents of each string literal (ordinary, with `\` line continuations, and
  raw `r#"…"#`). For `.tql`, take the whole text with `#` comments removed.
- Flag a hit when a literal contains a whole-word write verb (`insert`, `put`, `update` or
  `delete`) **and**, anywhere in it, `isa <label>` with `<label>` on the surface. The order does not
  matter. `match $b isa belief, …; update $b has belief-state "retracted";` is the most direct
  entity breach, and its `isa` comes before the verb. *(From the Fable review; SPEC-W5 §2.1 says "a
  write verb applied to any name on the surface".)*
- Labels match as whole labels, with `-` counted as a label character. Surface labels such as
  `attack`, `evidence`, `execution` and `support` are prefixes of record types (`attack-pattern`,
  `evidence-of`), so a regex `\b` boundary would give false hits.
- A literal with no write verb (cyber's read functions, `match … isa technique-detection … fetch`)
  is not a hit.
- After A2, no production literal outside `tql.rs` trips the rule. The syncer's writes in
  `bundle.rs` touch only record types.
- The assertion message lists every hit as `path:line: label`.

**Proof it bites.**

- Unit tests on the detector, in the same test file and serviceless, feed it copies of today's
  cyber `claim` and `disposition` literals, and both must be flagged.
- Further cases: `match … isa belief …; update …` and a `delete` variant (flagged); an
  `insert $x isa attack-pattern` (not flagged); and a source with a char literal `'"'` and a
  lifetime `'a` before a flagged literal (still flagged).
- They also feed a `put` variant and a raw-string variant (flagged), a read-only `match … isa
  technique-detection … fetch` (not flagged) and a comment mentioning `insert … isa belief` (not
  flagged).
- So the lint proves it catches the breach without the branch ever carrying a red commit.

**Residual (SPEC-W5 §2.1, unchanged).** A label composed at run time (`isa {rel}`) is invisible to
the lint. Mem's seam composes labels from declarations, cyber's post-sync composes record-tier labels,
and A4 owns the dynamic half.

### Tests

- **gecko-extension-api:** it compiles with the split. Existing unit tests pass.
- **mem-gecko unit tests (`scan.rs`):** statement modes across `define`/`undefine`/`redefine`
  boundaries.
- **mem-gecko unit tests (`registry.rs`):**
  - each `undefine` and `redefine` refusal listed above, and the own-type undefine and redefine
    accepted;
  - the actual cyber fragment plus the two declarations builds a registry, with `owner` returning
    `cyber-gecko` for both relations.
- **cyber unit tests:** `cyber_post_sync` returns one op per shape with the expected rows and
  `expect_rows: None`, and skips unknown relation types.
- **gecko-engine:** a `sync_bundle` with a failing extension write commits nothing. Use a write that
  errors, for example a type the schema lacks. Put it in `bundle_sync_test.rs` or beside it.
- **gecko-bin:**
  - `cyber_detect_test` stays green with the new bridge wiring.
  - **Atomicity on the real cyber edges.** A `claim` against a missing technique fails in
    `ensure_logic_and_technique` before any write, so it cannot prove atomicity. Instead, a test
    calls the `cyber-gecko`-bound `BeliefEdgeWriter` directly, on the composed core+mem+cyber
    schema, with a `technique-detection` edge whose `attack-id` matches nothing. The call must fail,
    and the belief count must be unchanged. *(My call, from the builder check.)*
  - `detection-verdict` is declared `required: true` (`@card(1)` in the schema). *(My call.)*
  - `cyber_stix_sync_test` stays green, running the post-sync writes through `sync_bundle`.
  - Add the new lint test.
- **Docs:**
  - **Ledger:** A2, A3, A9 and A12 ✅ with PR references. A10 notes that `typedb-driver` is gone and
    the `gecko-engine` dependency and the guard remain.
  - **SPEC-W5:** a §5 row-1 implementation note, in the style of the row-5 note.

## Left to the builder

- Exact helper names and module layout, such as where the shared op runner lives.
- Whether cyber's read handle reuses `store` or builds a second `RouterGraphStore`.
- How `common/` is split into modules for `belief_edge_surface`.
- The string-literal extractor. It need not handle every Rust literal form, but the unit tests above
  pin the forms that matter.
- Task order. A natural order is A12, then A9, then A2, then A3, with A3's live lint going green
  only once A2 has removed cyber's literals.
