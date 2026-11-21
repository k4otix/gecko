# Daemon tick and content dedup (ledger C1, first C2 job) — short design

2026-11-21. The user chose the scope (structure plus dedup, no CLI), the run
record (a new `consolidation-step` relation), the duplicate rule (same owners,
text and event time) and that a referenced duplicate may be tombstoned. A
review against the code shaped the rest.

## Intent

The consolidation daemon is a trait of stubs and a no-op `tick`. CONTRACT-W8
CF-3 / §2a bind `tick` to one host-minted `RunContext` per tick, with a
`ConsolidationReport` whose counts reconcile against what that run wrote. This
lands that structure with one real job, `dedup_content_hash`, whose write
surface (`dedup_episodes`) already exists, so the structure is exercised rather
than vacuous.

**Goals.** A real daemon whose `tick` mints one run, runs the dedup job under
it, records every change it makes against that run and its actor, and reports
counts a test can check against the graph.

**Non-goals.** No CLI or scheduler. The other ten jobs stay
`NotYetImplemented`. Contract rows 1 and 6 stay owed: row 1 names
`dedup_content_hash`, but dedup mints no belief or derivation, so there is
nothing for it to check yet.

**Anti-goals.** Dedup must never tombstone across owners, never collapse two
real events that share text, never touch a retrieval event or a
prediction-resolving episode, and never bypass `MemWriter`. A report must never
claim work the graph does not show.

## Approaches

- **Record each change as a `consolidation-step` (chosen).** A new relation
  ties the affected item to the tick's `doc-run` and actor, with the action and
  time. Additive schema; reusable by later jobs; makes "one run per tick"
  checkable.
- *A second `source-link` tagged "consolidation"* — rejected: `source-link`
  means "where this memory came from", and a tombstoning tick is not that.
- *No record; reconcile against a test's measured delta* — rejected: the run
  identity would be minted and never stored, so contract row 7 could not be
  checked.

## Design

1. **Schema** (`core/mem-gecko/schema/mem_types.tql`): `attribute
   consolidation-action value string` (an open string, like `origin-kind`;
   dedup writes `"tombstone"`); `relation consolidation-step` relating
   `affected` (played by `memory-item`), `run` (played by `doc-run`) and
   `performer` (played by `agent`), owning `consolidation-action @card(1)` and
   `created-at @card(1)`. Every type and role carries a greenfield `@doc`.
   Additive, so `schema-init` upgrades an existing database.
2. **Duplicate rule.** Two episodes are duplicates when both are exactly
   `episode` (`isa!`, so no `execution-episode` or `retrieval-event`), neither
   plays `prediction-resolution:resolving-episode`, neither is tombstoned, they
   have the same `title` and `event-time`, and they have **identical owner
   sets** (no owner of one that the other lacks, both ways — expressed as two
   nested negations so the match stays at one row). In a cohort the keeper is
   the earliest `ingest-time`, ties broken by lowest `concept-id`; every other
   member is a loser. A loser that is a derivation source or was surfaced by a
   recall may be tombstoned (the user's call): a tombstone deletes nothing, so
   its row and every edge to it stay, and its identical keeper survives.
3. **`MemWriter::dedup_episodes(ctx, keeper, loser)`** gains the `RunContext`.
   One transaction: ensure the run and the agent, then one pipeline whose match
   enforces the whole rule *and* that `keeper` precedes `loser` in keeper
   order, then tombstones the loser and inserts its `consolidation-step`
   (`"tombstone"`, `created-at` = `ctx.occurred_at`, linked to the run and the
   agent); expect 1 row. A mismatch is `InvalidInput` naming both ids, and
   nothing is committed. The separate `episodes_share_content` read and its
   query are deleted. The loser's vector is then removed best-effort and the
   method returns what happened to it: `IndexOutcome::{Removed, MarkedDirty,
   NoIndex}` (no index API change; invariant 8 holds).
4. **Index rebuild** (`enumerate_episodes_read`) skips tombstoned episodes, so
   a rebuild does not bring back a removed vector.
5. **Trait.** Every job method takes `ctx: &RunContext`. `ConsolidationReport`
   gains `run_id: Option<RunId>` (`None` for the no-op daemon), and
   `vectors_removed` counts `IndexOutcome::Removed` only; a new
   `vectors_marked_dirty` counts the rest that had an index. A tick that fails
   midway returns an error naming its run id, and the steps already committed
   stay as the record.
6. **`MemConsolidationDaemon`** (new, in `consolidation.rs`) wraps
   `Arc<MemWriter>`. `tick(now)` mints `RunContext::new(ActorId
   "consolidation-daemon", ProvenanceSource::Tool { name:
   "consolidation-daemon" }, now)` and runs `dedup_content_hash(&ctx)`. The
   cohort read is a `MemWriter` method (query text in `tql.rs`) that joins in
   the database on shared title and event-time and returns only the id,
   ingest-time and owners of episodes that have a duplicate; cohorts are
   grouped host-side by (title, event-time, sorted owner set), then
   `dedup_episodes` runs once per loser. `NoopConsolidationDaemon` stays.
7. **Tests** (live TypeDB, `--test-threads=1`):
   - a tick over three same-owner, same-text, same-time episodes tombstones
     two, keeps the earliest, writes two steps for the tick's run and actor,
     and reports `episodes_deduped == 2` and that `run_id`;
   - not deduped: a different owner set (including {X} vs {X, Y}), a different
     event time, two same-outcome prediction resolutions, a retrieval event;
     a second tick does nothing and reports zero;
   - a referenced loser (a derivation source) is tombstoned and its derivation
     edge survives;
   - `daemon_tick_writes_share_one_run_context` (contract row 7);
   - `dedup_episodes` called directly on a non-duplicate pair, a reversed
     keeper/loser pair, or an already-tombstoned loser fails and commits
     nothing;
   - an index rebuild after a dedup does not contain the loser;
   - the reconciliation rig drives the real daemon in place of the test
     `DedupDaemon`; `consolidation-step` gets a `WriterOnly` disposition naming
     both dedup drives, `KNOWN_OWNERS` gains its owner, and the whole-graph
     tick's expected count becomes 2 (it also sees the "dedup" drive's keeper).
   Existing tests calling `dedup_episodes` or the trait jobs are updated.
8. **Docs.** Ledger: C1 ✅, C2's `dedup_content_hash` ✅ (the other two still
   owed), contract row 7 ✅, and A11's note on `tombstone_episode_op` updated.
   CONTRACT-W8 is ratified, so its now-stale text goes to the errata list
   (D-G) rather than being edited, as does any in SPEC-W5. KNOWN-LIMITATIONS'
   daemon section and `consolidation.rs`'s module doc describe what ships.

## Left to the builder

Role spellings beyond those named, the exact query shapes, the
`IndexOutcome` name, and test fixture details.
