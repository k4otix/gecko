# Belief-tier runtime check, sync guard, and the extension API boundary (A4 + A10)

**Status:** draft for the user's approval, 2026-11-20. Revised after a builder check and a Fable
review.
**Ledger items:** A4 (W5a dynamic rig), A10 (extensions depend only on `gecko-extension-api`).
**Owning sections:** SPEC-W5 §1.2, §2.2, §3.5 (D-D step 3), §5 rows 2, 3, 8.
**Depends on:** `2026-11-20-multi-bundle-databases-design.md` (ledger O1), which lands first in
its own PR. The rig needs a playbook bundle and a STIX import in one database.

## Intent

The belief tier (every `memory-item` and every relation one of them plays) must be written only
through mem's writer. A3 (PR #57) checks this statically, by scanning source text. Two gaps
remain:

- A write the static scan cannot see — a label assembled at runtime, or a write on a path the
  scan does not read — is caught by nothing. SPEC-W5 §2.2 owes the runtime half: drive every
  path that could write the belief tier, then reconcile per-type counts against what mem's writer
  accounts for.
- Extensions still compile against `gecko-engine` (a raw-write-capable crate), and every
  extension is offered mem's full `EpistemicWriter` through `inject_epistemic_host_fns`.
  SPEC-W5 §3.5 D-D step 3 owes the structural fix before a second extension exists.

The user also chose to turn detection into prevention for the sync path: every sync is guarded in
production, so a belief-tier write during a sync rolls the sync back instead of only failing a
test.

## Goals

1. **A10 — API boundary.**
   - The whole of `core/gecko-engine/src/okf/types.rs` (`OkfBundle`, `OkfConcept`, `OkfLink`,
     `OkfCitation`, `HierarchyEdge`, `TypedAttribute`, `TypedAttrValue`, and `ScriptEngine`
     with its inherent impl) and `ExtensionCallback` move into `gecko-extension-api`. The engine
     files become `pub use` re-exports, so engine and `gecko-bin` code compiles unchanged.
     `ExecutionResult` stays in the engine. The API crate already has `chrono` and `serde`.
   - `cyber-gecko` drops `gecko-engine` from its dependencies; `Cargo.lock` is committed.
   - A CI guard fails if any crate under `extensions/` depends on `gecko-engine` or
     `typedb-driver`.
   - `inject_epistemic_host_fns` is removed from `GeckoExtension`, with mem's override.
     `build_host_bridge` binds the host-built writer itself. No extension can be offered the
     writer.
2. **The sync guard (production).** Every `sync_bundle` call fingerprints the belief tier at the
   start of its write transaction and again just before commit, and refuses the sync if the
   fingerprint changed.
3. **A4 — the runtime reconciliation rig.** Two live tests:
   - `memwriter_is_the_sole_belief_tier_writer`: drives every production path through the real
     `gecko` binary and asserts each belief-tier type changes by exactly what mem's writer
     accounts for, measured by a reference run.
   - `hostile_extension_raw_belief_write_is_flagged`: raw belief-tier writes during a sync are
     refused by the guard and leave the graph unchanged; the same writes, sent around the guard,
     are reported by the rig.

## Non-goals and anti-goals

- **Not** defending against a deliberately hostile compiled-in extension (SPEC-W5 §3.5 "Limit").
- **Not** Track S (ATT&CK detection modelling), the plain-`define` gap on core types, or OKF v0.2.
- **Not** a runtime guard on `gecko run`: since PR #57 bridges hold only a `GraphReader`, and the
  base dispatcher holds no graph handle, so the run path has no raw write handle to guard. The rig
  still drives it, to prove that.
- **Anti-goal:** a guard or rig that can pass vacuously — an empty surface, a count query that
  errors, a failed playbook that exits 0, or a CI guard that inspects zero crates.
- **Anti-goal:** a hand-maintained list of belief-tier types. The surface is introspected
  (SPEC-W5 §1.1); the rig's disposition table only records *why* each introspected label is in
  its state.

## Deviations from SPEC-W5 (recorded as a §5 implementation note)

- **The hostile adversary (§2.2 row 4, §5 row 3).** SPEC-W5 describes a stub extension composed
  through `build_host_bridge` that writes on a captured graph handle. Since PR #57 (D-D step 1) a
  bridge receives only `Arc<dyn GraphReader>`, so that adversary no longer compiles. The remaining
  raw route is an extension's sync-time output (`extension_writes`, and `type_hint` on the
  concepts an importer returns), so that is what the hostile test drives. This applies D-D; it is
  not a new decision, and it does not need separate ratification.
- **The accounting baseline (§2.2 row 1).** SPEC-W5 has "the harness itself mints beliefs/edges
  through MemWriter". The rig keeps that literally — the reference run does exactly this — and
  compares the production run against it.

## Approaches considered

**How the rig reaches the real wiring.** `gecko-bin` is a binary crate, so its integration tests
cannot call `build_host_bridge`.
- (a) Split the assembly code into a library inside `gecko-bin`. Real wiring, but a sizeable move
  of `main.rs` for test access only.
- (b) Copy the wiring into the test, as `mem_recall_e2e_test.rs` does. Cheap, but it tests a
  copy.
- (c) **Chosen:** drive the built binary as a subprocess (`env!("CARGO_BIN_EXE_gecko")`, already
  used by `config_init_test.rs` and `orchestrator_test.rs`). The hostile path cannot be compiled
  into the binary, so the test drives it by calling `sync_bundle` in-process.

**Where the guard lives.** (a) **Chosen:** inside `sync_bundle`, around the whole transaction, so
every caller is covered and a refused write never commits. (b) A `gecko-bin` wrapper leaves other
callers unguarded. (c) A static check of query text has the A3 lint's blind spots.

**What the guard compares.** (a) Instance counts only: misses updates, attribute changes and
re-linking. (b) Full content (every instance's attributes and role players): exact, but each
sync then reads the whole belief tier, which grows without bound in a long-lived database.
(c) **Chosen:** per label, the instance count plus the number of attribute ownerships and the
number of role-player links. It catches inserts, deletes, attribute adds and removals, and
re-linking that changes a count. The stated residual is a same-count swap: an attribute value
replaced in place, or one instance deleted and an identical-shape one inserted, in the same sync.

**How extensions stop being offered the writer.** (a) **Chosen:** remove the hook from the
trait. (b) Call it only for the extension named `mem`: leaves a trait method that invites misuse.

## Design

### A10

- New module(s) in `gecko-extension-api` (names left to the builder) hold the moved types with
  their derives, docs and `serde` attributes.
- **CI guard**, its own step in `ci.yml` next to the candle guard:
  - enumerate workspace members with `cargo metadata --no-deps --format-version 1`, keep those
    whose `manifest_path` is under `extensions/`, and fail if that list is empty;
  - for each, run `cargo tree -p <name> --all-features -e normal,build,dev --prefix none` and
    fail on any line starting `gecko-engine ` or `typedb-driver `, printing the crate and the
    dependency.
  - The plan includes a one-off negative check: re-add the dependency locally and confirm the
    step fails.
- **Hook removal.** Delete `inject_epistemic_host_fns` from `GeckoExtension`, mem's override and
  the loop in `build_host_bridge`. The writer now reaches `epistemic_extension_callback` without
  `SandboxCtx`, so `SandboxCtx` is removed from the API crate unless the builder finds another
  user (and the unreachable `None` arm in `build_host_bridge` goes with it).
  `epistemic_bridge_test.rs::hook_delivers_working_writer_to_domain_extension` and its
  `DomainDouble` are deleted; its run-stamping tests stay. Rig step 5 below is the proof that the
  writer reaches the sandbox. Docs updated: `extensions/README.md`, `CLAUDE.md`,
  `core/gecko-extension-api/src/lib.rs`, the `build_host_bridge` doc comment, and
  `mem-gecko/src/lib.rs`.

### The belief-tier surface, as production code

- `gecko-engine` gains `db::surface::belief_tier_surface(tx, root: &str) -> Result<BeliefTierSurface, DbError>`,
  with `BeliefTierSurface { entities: BTreeSet<String>, relations: BTreeSet<String> }`:
  the `root` subtype closure, and every relation with a role played only by that closure (the
  SPEC-W5 §1.1 rule, today the P-W5-1 recipe in `gecko-bin/tests/common/surface.rs`). It errors
  if `entities` lacks `root`, or if the closure plays roles but `relations` is empty, and it
  treats `Some(Err(_))` in any answer stream as an error.
- `mem-gecko` exports `pub const BELIEF_TIER_ROOT: &str = "memory-item";`. The engine never names
  a mem label.
- `gecko-bin/tests/common/surface.rs` delegates to the production function. The A3 lint and the
  P-W5-1 probe keep their pass/ignore sets.
- `validate_schema_label` moves from the syncer to a shared spot in the engine's `db` module, so
  the surface code and the syncer use one copy.

### The sync guard

- `sync_bundle` takes a new required parameter, `belief_tier_root: &str`. Every caller passes
  `mem_gecko::BELIEF_TIER_ROOT`. No default and no `Option`. Its doc says the schema must contain
  the root type.
- At the start of the write transaction, before any write: compute the surface and a
  **fingerprint** — for each label (entities and relations, counted with `isa`, so supertypes are
  included): instance count, attribute-ownership count (`$x isa <label>, has $a`), and, for
  relations, role-player count (`$r isa <label>, links ($p)`). Each query must return exactly one
  `reduce` row; anything else is an error, never zero.
- Just before commit (after the extension writes), fingerprint again. On any difference, return a
  new `DbError::BeliefTierWrite { changes: Vec<BeliefTierChange> }`, where each change gives the
  label, the measure (instances, attributes or links), and before and after values. The
  transaction is dropped uncommitted.
- The concept-insert path refuses a `type_hint` that is not a subtype of `concept`, with a schema
  error, before inserting. This closes the route where an importer's `OkfConcept` carries a
  memory-item subtype (`working-set`, mem's `playbook`), on top of the fingerprint catching it.
- **Pre-GC check (amendment, 2026-11-20, for the user to ratify).** Before garbage collection
  deletes anything, the sync looks up every concept it would delete that plays a role in a
  belief-tier relation (any relation on the surface). If there is one, it refuses with
  `DbError::BeliefEdgeReferencesRemovedConcept`, which names each concept ID with the relation
  that references it. Nothing commits. The fingerprint stays as the backstop. Without the
  check, the fingerprint refuses the same sync, but its error names only a relation and two
  counts. The cases that hit this are deleting or renaming a playbook that has made a `claim`
  or a `disposition`, and re-importing an ATT&CK release that drops a technique with a claim.
  The recovery available today is to restore the removed file, so the concept stays, or to
  sync into a fresh database. Superseding the referencing belief does not release the
  reference, because the edge stays in the graph. "Retire, not delete" is ledger item O6.
- **Ledger item O6, "retire, not delete" (Track O; follow-on work, not part of this change).**
  When a concept that a belief edge references leaves its bundle, the sync keeps the entity
  and its key. It clears the concept's content, including `code-block` and `scope`, so a
  removed playbook cannot run, and marks the concept retired. It reports the retirement in
  `SyncResult`. STIX already treats revoked objects this way: they are ingested with a
  `revoked` flag, not deleted. Skipping GC for referenced concepts was rejected, because it
  would leave a deleted playbook runnable.
- Labels are interpolated, but they come from the live schema, and each passes
  `validate_schema_label` first.
- **Cost.** With mem and cyber composed, the surface has 13 entities and 24 relations. One
  fingerprint is therefore 98 count queries, and a sync runs 196, whatever the bundle's size.
  Each count scans that label's instances and ownerships, so the cost grows with the belief
  tier. That is the growth approach (b) was rejected for, but here it costs only counts. The
  plan measures the overhead on an unchanged-bundle sync.

### A4 — the rig

New test file `gecko-bin/tests/belief_tier_reconciliation_test.rs`, `#![cfg(feature = "cyber")]`,
Tier 2.

- **Fingerprint.** The rig uses the same production fingerprint function the guard uses (exported
  from the engine's `db::surface` module), so the rig and the guard cannot measure differently.
  A step's delta is `after - before`, per label and measure.
- **Two databases.** *Production* is driven only through the `gecko` binary. *Reference* is driven
  only through in-process writer calls (`MemWriter` and its `edge_writer_for("cyber-gecko")`)
  making the same calls with the same arguments, after the same `schema-init`, `sync` and
  `sync-stix` setup. The expected delta for each production step is the reference database's
  delta for the matching step. No delta numbers are kept by hand.
- **Subprocess setup.** Every binary call passes `--config <temp gecko.toml>` and `--database
  <production db>`, with `GECKO_CACHE_DIR` set to a temp directory. The config enables cyber and
  the semantic index with the stub embedder and a temp index path. Each `run` step also asserts
  stdout contains `Execution successful!`, since `gecko run` exits 0 when the script fails.
- **Fixtures** are written to a temp directory by the test: an OKF bundle with the two playbooks
  (scopes `mem:remember`, `mem:recall`, `mem:derive`, `mem:supersede`, `mem:contest`,
  `cyber-gecko:claim` and `cyber-gecko:disposition` as each needs), and the STIX bundle from
  `cyber_stix_sync_test.rs` (`BUNDLE`, which carries technique `T1059`), as a JSON file.
- **Steps** (production through the binary; reference through the writer):
  1. `schema-init`: expected delta zero.
  2. `sync` of the OKF playbook bundle: expected zero.
  3. `sync-stix` of the STIX bundle, into the same database: expected zero. This exercises the
     guarded extension-write path.
  4. Re-`sync` of the OKF bundle and re-`sync-stix`: expected zero, and both bundles' concepts
     remain (multi-bundle sanity).
  5. `run` of the mem playbook, calling `remember`, `recall`, `derive` (using the recalled ids, so
     retrieval provenance is written), `supersede` and `contest`: expected delta equals the
     reference.
  6. `run` of the cyber playbook, calling `claim` on `T1059` and then `disposition`: expected
     delta equals the reference.
  7. `gecko status` (read-only): expected zero.
- **Disposition table (§1.2 ratchet).** Every introspected label must appear in exactly one of:
  - *accounted*: some step's delta is non-zero for it;
  - *writer-only*: written only by `MemWriter` methods no production path calls today
    (`rests_on`, `record_prediction`/`resolve_prediction`, the populations, disposition and
    consolidation writers). The rig drives each of these in-process on the reference database
    alone, to prove the method writes only its own labels, and records the owning ledger item
    that will add a production caller (A5, A6, A7, P1, C-track);
  - *owed*: no code writes it today, with an owner (`sighting-of` → A8);
  - *allowlisted*: with an owner (none expected; `population-member` is not on the surface).
  - Every owner is a ledger item. A writer-only or owed label that no ledger item owns is
    owned by the open decision **D-O**, "owners for schema-only belief-tier types". The rig
    holds the D-O labels in one pinned constant, so adding one takes an explicit edit.
  A label in none of them fails, naming the label; a table entry for a label not on the surface
  also fails, so the table cannot rot.

### `hostile_extension_raw_belief_write_is_flagged`

- **Setup:** schema-init, the two syncs and the cyber `run` step on a fresh production database,
  so a belief and a technique exist.
- **Through the guard:** call `sync_bundle` in-process with the *same* STIX manifest as the setup
  sync (so garbage collection removes nothing) plus a test-only extension's writes: a raw
  `insert $b isa belief, …` with the attributes `belief` requires, and a raw
  `technique-detection` linking the existing technique to the existing belief. Assert
  `Err(DbError::BeliefTierWrite { changes })`, that the set of changed labels is exactly
  `{memory-item, belief, technique-detection}` (supertypes included), and that a fingerprint is
  unchanged.
- **Through an importer:** a manifest whose extra concept carries `type_hint: Some("working-set")`
  is refused by the `type_hint` check, and the fingerprint is unchanged.
- **Around the guard (non-vacuity):** run the same two raw writes in a raw write transaction that
  bypasses `sync_bundle`, and assert the rig's reconciliation reports exactly the same label set
  the guard reported.

## Constraints

- All values are parameterized; only schema labels are interpolated, and they are validated.
- Greenfield comments; never fabricate success; integration tests use throwaway databases, never
  `gecko`, and run with `--test-threads=1`.
- Both CI clippy runs (with and without `cyber`, `--locked`) and fmt stay clean; `Cargo.lock` is
  committed when dependencies change.
- Docs updated for the new rules:
  - `extensions/README.md`: the hook is gone, sync-time writes are guarded, and a removed
    concept that a belief edge references is refused;
  - `CLAUDE.md`;
  - `docs/refactor/KNOWN-LIMITATIONS.md`: the guard's same-count residual, and the pre-GC
    refusal with its recovery;
  - the ledger: A4 and A10 done, a new O6, and a new open decision D-O;
  - a SPEC-W5 §5 implementation note for rows 2, 3 and 8, covering the deviations above and
    marking the pre-GC check as an amendment the user should ratify.

## Left to the builder

- Module names in `gecko-extension-api` for the moved types.
- Whether fingerprint and reconciliation helpers for the tests live in the rig file or in
  `tests/common`, as long as the measurement itself is the production function.
- Playbook contents, within the steps above.
