# Daemon tick + content dedup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A real consolidation daemon whose `tick` mints one run, dedups duplicate episodes under it, records each tombstone as a `consolidation-step`, and reports counts that reconcile with the graph.

**Architecture:** A new `consolidation-step` relation records each daemon change against the tick's `doc-run` and agent. `MemWriter::dedup_episodes` enforces the whole duplicate rule inside one guarded write. `MemConsolidationDaemon` finds cohorts with one database query and dedups each loser.

**Tech Stack:** Rust, TypeDB 3.15 (TypeQL 3), tokio, cargo nextest.

**Spec:** `docs/superpowers/specs/2026-11-21-daemon-tick-dedup-design.md`

## Global Constraints

- Duplicate rule (spec §2, verbatim): both exactly `episode` (`isa!`); neither plays `prediction-resolution:resolving-episode`; neither has `consolidation-state "tombstoned"`; same `title`; same `event-time`; identical owner sets, as two nested negations so the match stays at one row.
- Keeper order: earliest `ingest-time`, ties broken by lowest `concept-id`.
- Daemon run: `RunContext::new(ActorId::new("consolidation-daemon"), ProvenanceSource::Tool { name: "consolidation-daemon".into() }, now)`.
- Step: `consolidation-action` `"tombstone"`, `created-at` = `ctx.occurred_at`, roles `affected` (memory-item), `run` (doc-run), `performer` (agent).
- `consolidation-action` is an open string (no `@values`). Every new type and role has a one-sentence greenfield `@doc`.
- All TQL lives in `core/mem-gecko/src/tql.rs` and is parameterized; never `#[allow(dead_code)]`; comments greenfield; stream loops handle `Some(Err(e))`.
- Integration tests: TypeDB on localhost:1729, `--test-threads=1`, never touch database `gecko`.
- Before each commit: `cargo fmt --all`; clippy `--workspace --exclude js-sandbox --all-targets -- -D warnings` with `--no-default-features` and with `--no-default-features --features cyber`.
- Commits end with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- A cohort of four where two members share a second owner: only the two with identical owner sets dedup against each other — Task 2 test.
- An episode with no owner at all (owner set empty): it is not merged with an owned one — Task 1 test (direct call refused).
- Ingest-time ties: the keeper is the lowest `concept-id` — Task 2 test.
- A tick on a database with no duplicates writes no run, step or agent — Task 2 test.
- An episode with no `title` is never matched — Task 1 test.

---

### Task 1: Schema, guarded `dedup_episodes`, index outcome, rebuild filter

**Files:**
- Modify: `core/mem-gecko/schema/mem_types.tql`, `core/mem-gecko/src/tql.rs` (`tombstone_episode_op` → a guarded dedup op; delete `episodes_share_content_read`; `enumerate_episodes_read`), `core/mem-gecko/src/writer/consolidation.rs`, `core/mem-gecko/src/writer/index.rs` (`best_effort_remove`), `core/mem-gecko/src/lib.rs` (re-export `IndexOutcome` beside the existing re-exports near line 21)
- Modify: `gecko-bin/tests/belief_tier_reconciliation_test.rs` — the file is `#![cfg(feature = "cyber")]`, so only the `--features cyber` build sees it. Pass a ctx at the `"dedup"` drive (~937) and in `DedupDaemon::tick` (mint one `RunContext` in `tick`, ~593); add `("consolidation-step", LabelDisposition::WriterOnly { methods: &["dedup", "daemon tick"], owner: "C1" })` to `DISPOSITIONS` and `"C1"` to `KNOWN_OWNERS` (~478). The new relation lands on the belief-tier surface (its `affected` role is played only by `memory-item`), so without this the rig's ratchet fails.
- Test: `core/mem-gecko/tests/writer_reader_integration.rs` (update the two existing dedup tests at ~1247 and ~1314; add new ones)

**Interfaces:**
- Produces: `pub enum IndexOutcome { Removed, MarkedDirty, NoIndex }` (mem-gecko, re-exported from the crate root); `pub async fn dedup_episodes(&self, ctx: &RunContext, keeper: &MemId, loser: &MemId) -> Result<IndexOutcome>`; `best_effort_remove(&self, id) -> IndexOutcome`. `NoIndex` when embedder or index is absent.

**The guarded op (pin this shape; unverified against TypeDB — if a construct is rejected, STOP and report BLOCKED; do not fall back to a read plus a write).** Ops are `[ensure_run(ctx), ensure_agent(ctx), guarded.expecting(1)]`, so a mismatch is op index 2 (mapping pattern: `name_missing_ids` in `writer/beliefs.rs`). Values `$kid $lid $rid $aid $act $now` come through `Params`:

```
match
  $k isa! episode, has concept-id $kc, has title $t, has event-time $et, has ingest-time $ki; $kc == $kid;
  $l isa! episode, has concept-id $lc, has title $t, has event-time $et, has ingest-time $li; $lc == $lid;
  { $ki < $li; } or { $ki == $li; $kc < $lc; };
  not { $k has consolidation-state "tombstoned"; };
  not { $l has consolidation-state "tombstoned"; };
  not { $p1 isa prediction-resolution, links (resolving-episode: $k); };
  not { $p2 isa prediction-resolution, links (resolving-episode: $l); };
  not { $o1 isa ownership, links (owned: $k, owner: $a1); not { $o2 isa ownership, links (owned: $l, owner: $a1); }; };
  not { $o3 isa ownership, links (owned: $l, owner: $a2); not { $o4 isa ownership, links (owned: $k, owner: $a2); }; };
  $run isa doc-run, has run-id $rr; $rr == $rid;
  $ag isa agent, has agent-id $aa; $aa == $aid;
update $l has consolidation-state "tombstoned";
insert (affected: $l, run: $run, performer: $ag) isa consolidation-step,
  has consolidation-action == $act, has created-at == $now;
```

Keep the `or` branches mutually exclusive and bind no owner outside a `not`, or the row count exceeds 1; use distinct variable names in sibling `not` blocks. The schema needs `memory-item plays consolidation-step:affected`, `doc-run plays consolidation-step:run`, `agent plays consolidation-step:performer`.

**Fixtures.** `observe` always writes a title and exactly one owner, so untitled, unowned, two-owner and `execution-episode` fixtures are seeded with `Fixture::raw_write` (`writer_reader_integration.rs` ~88). Two prediction-resolving episodes must be resolved under ONE ctx (their title and event time come from the ctx), or the case passes trivially.

- [ ] **Step 1: Tests** (live): update `dedup_episodes_tombstones_loser_and_removes_its_vector` to pass a ctx, assert `IndexOutcome::Removed`, and assert one `consolidation-step` linking the loser, `ctx.run_id`'s `doc-run`, agent `ctx.actor`, action `"tombstone"`. Update `dedup_episodes_refuses_non_duplicates`. Add `dedup_episodes_refuses_and_commits_nothing` covering: different event time; owner sets {X} vs {X, Y}; an owned vs an unowned episode; an episode without `title`; reversed keeper/loser (later ingest as keeper); tied ingest time with the higher `concept-id` as keeper; an already-tombstoned loser; two prediction-resolving episodes resolved under one ctx; two titled `execution-episode`s with the same owner and times (refused by `isa!`). Each → `InvalidInput` containing both ids, and no step and no tombstone written. Add `index_rebuild_skips_a_tombstoned_episode`.
- [ ] **Step 2: Run; expect failures** (`cargo nextest run -p mem-gecko --no-default-features --test-threads 1`).
- [ ] **Step 3: Implement** the schema (spec §1), the guarded op (spec §3: ensure run and agent, then one `match … update … insert …` pipeline, `.expecting(1)`), the outcome, and the rebuild filter. Map the mismatch to `InvalidInput` by op index.
- [ ] **Step 4: Run mem-gecko tests, `-p gecko-engine`, and `gecko-bin` `--features cyber` tests `schema_init_test` and `belief_tier_reconciliation_test`; all pass.**
- [ ] **Step 5: Commit** `feat(mem): dedup_episodes enforces the duplicate rule in one write and records a consolidation-step`.

### Task 2: Run-scoped trait and `MemConsolidationDaemon`

**Files:**
- Modify: `core/mem-gecko/src/consolidation.rs`, `core/mem-gecko/src/tql.rs` (cohort read), `core/mem-gecko/src/writer/consolidation.rs` (cohort read method), `core/mem-gecko/src/lib.rs`
- Test: `core/mem-gecko/src/consolidation.rs` unit tests; new `core/mem-gecko/tests/consolidation_daemon_test.rs`

**Interfaces:**
- Consumes: Task 1's `dedup_episodes`, `IndexOutcome`.
- Produces: every job method takes `ctx: &RunContext` (`tick` keeps `now: DateTime`); `ConsolidationReport` gains `run_id: Option<RunId>` and `vectors_marked_dirty: usize`; `pub struct MemConsolidationDaemon` with `pub fn new(writer: Arc<MemWriter>) -> Self`; `MemWriter::duplicate_episode_candidates(&self) -> Result<Vec<DuplicateCandidate>>` where `pub struct DuplicateCandidate { pub id: MemId, pub title: String, pub event_time: DateTime, pub ingest_time: DateTime, pub owners: BTreeSet<String> }` (in `writer/consolidation.rs`, re-exported at the crate root). The cohort read joins ownership, so it returns one row per (episode, owner); fold rows into one candidate per id. Unowned episodes are skipped by the read (and the Task 1 guard refuses an owned-vs-unowned pair). `MemConsolidationDaemon::dedup_episodes_in(&self, ctx) -> Result<Vec<IndexOutcome>>` (inherent) does the work; the trait's `dedup_content_hash(ctx)` returns its length, and `tick` tallies `vectors_removed` / `vectors_marked_dirty` from the outcomes. A mid-tick error is `EpistemicError::Storage` whose message contains the run id.

- [ ] **Step 1: Tests** in `consolidation_daemon_test.rs` (the `Fixture` in `writer_reader_integration.rs` is private to that file: copy the minimal fixture into the new file, or add the tests to `writer_reader_integration.rs` — implementer's choice, say which):
  - `tick_dedups_a_cohort_and_reports_its_run`: three same-owner/text/time episodes with distinct ingest times; report `episodes_deduped == 2`, `run_id == Some(r)`; the earliest survives; two steps for run `r` and agent `consolidation-daemon`.
  - `tick_leaves_non_duplicates_alone`: different owner set, different event time, two same-outcome prediction resolutions, a recall's retrieval event → zero deduped.
  - `a_second_tick_does_nothing`: report zero; no new run, step or agent.
  - `a_tick_with_no_duplicates_writes_nothing`.
  - `ingest_time_ties_keep_the_lowest_id`.
  - `mixed_owner_cohort_dedups_only_identical_owner_sets` (four same-text/time episodes, two owned by {X}, two by {X, Y}: 2 deduped, 2 keepers survive).
  - `tick_reports_index_outcomes`: with a recording index, `vectors_removed == 2` for a three-episode cohort; without an index, both counts 0.
  - `a_failing_tick_names_its_run`: a tick whose dedup fails (e.g. a graph store that errors on write) returns `Storage` containing the run id.
  - `a_referenced_loser_is_tombstoned_and_keeps_its_derivation`.
  - `daemon_tick_writes_share_one_run_context` (contract row 7): every step from one tick links that tick's run.
  - Update `consolidation.rs` unit tests for the new signatures.
- [ ] **Step 2: Run; expect failures.**
- [ ] **Step 3: Implement** the trait change, report fields, the cohort read (one DB join on shared title and event-time returning id, ingest-time and owner ids of episodes that have a duplicate; group host-side by (title, event-time, sorted owners); keeper order per Global Constraints), and the daemon. A mid-tick error is wrapped to name the run id.
- [ ] **Step 4: Run mem-gecko tests; pass.**
- [ ] **Step 5: Commit** `feat(mem): a consolidation daemon whose tick dedups episodes under one run`.

### Task 3: Reconciliation rig drives the real daemon

**Files:** `gecko-bin/tests/belief_tier_reconciliation_test.rs`, `gecko-bin/Cargo.toml` (drop `async-trait` dev-dep if unused)

- [ ] **Step 1:** Replace `DedupDaemon` with `MemConsolidationDaemon` (the disposition and ctx changes landed in Task 1); the `"daemon tick"` drive's expected `episodes_deduped` is 2 (it also sees the `"dedup"` drive's keeper) and it asserts the report's `run_id` is `Some`.
- [ ] **Step 2: Run** `cargo nextest run -p gecko-bin --no-default-features --features cyber --test belief_tier_reconciliation_test --test-threads 1`; pass. Then the mutation check: make the daemon's tick skip dedup and confirm the rig fails; restore.
- [ ] **Step 3: Commit** `test(rig): the reconciliation rig drives the real consolidation daemon`.

### Task 4: Docs and full verification

**Files:** `docs/refactor/OWED-WORK-LEDGER.md` (C1 ✅; C2 `dedup_content_hash` ✅, others owed; the C8 row (~231, "a test daemon" → the real daemon); the line listing remaining contract rows (~233: row 7 landed); A11's `tombstone_episode_op` note (~127); errata D-G (~278) entries for CONTRACT-W8's stale text — candidates `CONTRACT-W8.md` ~17, ~40, ~175-176, ~204, ~228 — and any in SPEC-W5), `docs/refactor/KNOWN-LIMITATIONS.md` (daemon section ~80-88), `core/mem-gecko/src/consolidation.rs` module doc.

- [ ] **Step 1:** Write the doc changes. Contract row 1 is stated as still owed (dedup mints no belief).
- [ ] **Step 2:** fmt, both clippy runs, then `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --test-threads 1`; all pass.
- [ ] **Step 3: Commit** `docs: mark C1 and the dedup job done`.
