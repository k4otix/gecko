# Mem write counts + daemon reconciliation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make four mem writes fail and commit nothing when an ID matches nothing, stop the sandbox bridge from fabricating empty ID lists, and reconcile a real daemon tick in the belief-tier rig.

**Architecture:** `.expecting(1)` on each op; callers map the mismatch by op index into a `Storage` error naming the given IDs. The rig gains a test `ConsolidationDaemon` driving `dedup_episodes`.

**Tech Stack:** Rust, TypeDB 3.15, tokio, cargo nextest.

**Spec:** `docs/superpowers/specs/2026-11-20-mem-write-counts-design.md`

## Global Constraints

- Every remapped error is `EpistemicError::Storage`, contains each given ID, and contains `nothing was committed`.
- `contest`/`record_prediction` messages say `no belief`; derive/`rests_on` say `no memory item`.
- All TQL parameterized; comments greenfield; never `#[allow(dead_code)]`.
- Integration tests: TypeDB on localhost:1729, `--test-threads=1`, never touch database `gecko`.
- Before each commit: `cargo fmt --all`; clippy `--workspace --exclude js-sandbox --all-targets -- -D warnings` with `--no-default-features` and with `--no-default-features --features cyber`.
- Commits end with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- A derive with valid evidence plus retrieval-provenance ops still commits (the derivation is not the last op) — covered by the existing mem_recall e2e test; run it.
- A derive through `assert_belief_with_edges` with a bad evidence ID gets the derive message, not an edge message — Task 1 test.
- `mem.derive` with no `evidence` key still succeeds as an underived belief — Task 2 test.
- `contest([a, a])` is refused before any write — Task 1 test.
- A daemon tick whose delta is empty must not satisfy C8 — Task 3 asserts non-empty.

---

### Task 1: Counts and error mapping in mem-gecko

**Files:**
- Modify: `core/mem-gecko/src/tql.rs` (`derivation_op`, `contest_ops`, `rests_on_op`, `record_prediction_op`)
- Modify: `core/mem-gecko/src/writer/beliefs.rs` (`belief_ops`, `assert_belief`, `contest`, `rests_on`, `record_prediction`)
- Modify: `core/mem-gecko/src/writer/edges.rs` (`assert_belief_with_edges`)
- Test: `core/mem-gecko/src/writer/tests.rs`, `core/mem-gecko/tests/writer_reader_integration.rs`, `core/mem-gecko/tests/belief_edge_integration.rs`

**Interfaces:**
- Produces: `MemWriter::belief_ops(...) -> (Vec<GraphWrite>, Option<usize>)` — the ops and the derivation op's index; `pub(super) fn name_missing_evidence(err: EpistemicError, derivation: Option<usize>, belief: &MemId, evidence: &[MemId]) -> EpistemicError` in `beliefs.rs`, returning `err` unchanged unless it is a mismatch on that index.

- [ ] **Step 1: Unit tests** in `writer/tests.rs`: `derivation_op`, the last `contest_ops` op, `rests_on_op`, `record_prediction_op` each have `expect_rows == Some(1)`; `belief_ops` with one evidence ID returns `Some(i)` where `ops[i]` is the derivation, and `None` with no evidence.
- [ ] **Step 2: Live tests** in `writer_reader_integration.rs`, modelled on `supersede_of_a_missing_belief_errors_and_commits_nothing`:
  - `derive_with_a_missing_evidence_id_errors_and_commits_nothing`: one real episode + `mem/ep/does-not-exist`; `Storage` containing both IDs, `no memory item`, `nothing was committed`; no belief with that title exists.
  - `contest_with_a_missing_claim_errors_and_commits_nothing`: one real belief + `mem/bel/does-not-exist`; `Storage` with `no belief`; no `anomaly` exists.
  - `contest_refuses_a_repeated_claim`: `[a, a]` → `InvalidInput`.
  - `rests_on_with_a_missing_assumption_errors_and_commits_nothing`: no `rests-on` exists.
  - `record_prediction_on_a_missing_belief_errors_and_commits_nothing`: no `prediction-resolution` exists.
  - In `belief_edge_integration.rs`: `belief_with_edges_and_a_missing_evidence_id_names_the_evidence`: message contains `no memory item` and the missing ID, not `belief edge`.
- [ ] **Step 3: Run; expect the new tests to fail** (`cargo nextest run -p mem-gecko --no-default-features --test-threads 1`).
- [ ] **Step 4: Implement** the counts, the `belief_ops` index, `name_missing_evidence` (applied in `assert_belief`, and in `assert_belief_with_edges` before `name_failing_edge`), the contest duplicate check (after the length check), and the contest / rests_on / record_prediction mappings by op index (contest: 2; the others: 0).
- [ ] **Step 5: Run mem-gecko tests and `gecko-bin --test mem_recall_e2e_test`; all pass.**
- [ ] **Step 6: Commit** `fix(mem): a derive, contest, rests_on or record_prediction whose ids match nothing fails and commits nothing`.

### Task 2: Sandbox bridge rejects malformed ID lists

**Files:**
- Modify: `core/gecko-engine/src/sandbox/mem_host.rs` (`mem_ids`, its two callers)
- Test: the `#[cfg(test)]` module of `mem_host.rs`

**Interfaces:**
- Produces: `fn mem_ids(payload: &Value, key: &str) -> Result<Vec<MemId>, String>`; absent key → `Ok(vec![])`; non-array → `Err` naming `key`; non-string entry → `Err` naming `key` and the entry's index.

- [ ] **Step 1: Tests** `mem_ids_rejects_a_non_array`, `mem_ids_rejects_a_non_string_entry`, `mem_ids_treats_an_absent_key_as_empty`, and a `dispatch_mem_call` test that `Derive` with `"evidence": [42]` errors without calling the writer.
- [ ] **Step 2: Run; expect failure.** `cargo nextest run -p gecko-engine --no-default-features mem_host`
- [ ] **Step 3: Implement**; callers use `?`.
- [ ] **Step 4: Run; pass.**
- [ ] **Step 5: Commit** `fix(sandbox): mem.derive and mem.contest reject a malformed id list`.

### Task 3: C8 — the daemon is a counted caller

**Files:**
- Modify: `gecko-bin/tests/belief_tier_reconciliation_test.rs`
- Modify: `core/mem-gecko/src/consolidation.rs` (module doc only if it claims ticks are unreconciled)

**Interfaces:**
- Consumes: `mem_gecko::{ConsolidationDaemon, ConsolidationReport, NoopConsolidationDaemon}`, `MemWriter::dedup_episodes`.

- [ ] **Step 1:** Add `struct DedupDaemon { writer: Arc<MemWriter>, keeper: MemId, loser: MemId }` implementing `ConsolidationDaemon::tick` as `dedup_episodes(keeper, loser)` returning `episodes_deduped: 1`.
- [ ] **Step 2:** After the `"dedup"` drive add `drive!("noop daemon tick", …)` (tick a `NoopConsolidationDaemon`) and `drive!("daemon tick", …)` (observe two duplicate episodes, then tick a `DedupDaemon`; assert the report's `episodes_deduped == 1`). Doc comment on the daemon drive names `daemon_is_enumerated_by_the_monopoly_test` (CONTRACT-W8 §3 row 5).
- [ ] **Step 3:** After the drives, assert the `"noop daemon tick"` delta is empty and the `"daemon tick"` delta is non-empty. If the tombstone is not visible to the fingerprint, stop and report — do not weaken the assertion.
- [ ] **Step 4: Run** `cargo nextest run -p gecko-bin --no-default-features --features cyber --test belief_tier_reconciliation_test --test-threads 1`; pass.
- [ ] **Step 5: Commit** `test(rig): a consolidation daemon tick is reconciled like any other writer`.

### Task 4: Docs and full verification

**Files:** `docs/refactor/OWED-WORK-LEDGER.md` (A11 row, C8 row, status bullets and dependency graph lines 66 and 73), `docs/refactor/KNOWN-LIMITATIONS.md` (daemon section)

- [ ] **Step 1:** A11: entries 1–3 ✅ with test names; entry 4 still open; add the deferred writes and the guest leniency. C8 ✅, landed as the `daemon tick` drive in `memwriter_is_the_sole_belief_tier_writer`. KNOWN-LIMITATIONS: a daemon tick is reconciled by the rig.
- [ ] **Step 2:** fmt, both clippy runs, then `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --test-threads 1`; all pass.
- [ ] **Step 3: Commit** `docs: mark A11 1–3 and C8 done`.
