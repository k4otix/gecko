# Query Correctness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the eight query and write correctness findings from the TypeDB review, each pinned by a test that failed first.

**Architecture:** The fixes live in mem's query text (`core/mem-gecko/src/tql.rs`), mem's schema functions and types, the core syncer's link writers, and cyber's SRO writes and `precision` read. Writes that must affect one row get `.expecting(1)` and a named error. Rows are deduplicated host-side before multi-row writes.

**Tech Stack:** Rust, TypeDB 3.15 / TypeQL, cargo nextest.

**Spec:** `docs/superpowers/specs/2026-11-22-typedb-review-fixes-design.md` (Pass 1)

## Global Constraints

- **TQL:**
  - all TQL is parameterized (`Params` / `given`), never interpolated;
  - every new function carries a one-sentence greenfield `@doc`;
  - changed function bodies reach live databases through the automatic function redefine (`schema_apply.rs`);
  - changed type annotations need a mem migration (`MEM_MIGRATIONS`) only when they replace an explicit annotation.
- **Errors:** every refused write is `EpistemicError::Storage`, names each given id and contains `nothing was committed`, following `name_missing_ids` (`core/mem-gecko/src/writer/beliefs.rs:70`).
- **Code style:** comments are greenfield; never `#[allow(dead_code)]`; the TypeDB guides are at `.claude/skills/typedb-typeql/SKILL.md` and `.claude/skills/typedb-modeling/SKILL.md`.
- **Integration tests:** TypeDB on localhost:1729, `--test-threads=1`, never the database `gecko`.
- **Before each commit:** `cargo fmt --all`; `cargo clippy --workspace --exclude js-sandbox --all-targets --no-default-features -- -D warnings`; the same with `--features cyber`.
- **Commit trailer:** commits end with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- **Belief owners and visibility.** A belief owned by agent A with visibility `private` is invisible to agent B in both present and as-of recall, and visible to A in both. Covered in Task 1.
- **A run whose derive consumed two retrievals.** Both retrieval events are linked to the belief and both `was-used` flips happen; the new `expecting(1)` must not refuse this normal path. Covered in Task 2.
- **Re-syncing a bundle whose document links the same page twice.** It yields one `concept-link`, stays at one on a re-sync, and leaves its other links unchanged. Covered in Task 4.
- **An existing database whose data conforms** upgrades through `schema-init` with the new ownership and source-link cardinalities. Covered in Task 3 by the reconciliation rig and the mem suites, run after the schema change.
- **`precision` with no asserted dispositions** returns zero counts, not an error. Covered in Task 4.

---

### Task 1: As-of recall is visibility-gated

**Files:**
- Modify: `core/mem-gecko/schema/mem_functions.tql` (`gate` ~98; new `visible-to`)
- Modify: `core/mem-gecko/src/tql.rs` (`believed_at_chunks_read` ~937)
- Modify: `core/mem-gecko/src/writer/reader.rs` (`recall_as_of` ~144, its caller ~263)
- Test: `core/mem-gecko/tests/writer_reader_integration.rs`

**Interfaces:**
- Produces:
  - `fun visible-to($n: belief, $agent: agent) -> boolean`, the ownership disjunction moved out of `gate`; `gate` calls it;
  - `believed_at_chunks_read(at: DateTime, actor: &ActorId) -> Read`;
  - `recall_as_of(&self, ctx: &RunContext, at: DateTime, budget: ContextBudget)`.

- [ ] **Step 1: Write the failing tests:**
  - `as_of_recall_hides_another_agents_private_belief`: agent A asserts a private belief, and B's `recall` with `as_of` after its `valid-from` does not return it;
  - `as_of_recall_returns_a_team_belief_to_another_agent`;
  - `as_of_recall_returns_the_owners_private_belief`;
  - `as_of_recall_is_newest_first_and_truncates_deterministically`: three visible beliefs, `max_chunks: 2`, and the two newest `valid-from` come back in that order on two calls.
- [ ] **Step 2: Run, and expect the first and fourth to fail:** `cargo nextest run -p mem-gecko --no-default-features --test writer_reader_integration as_of_recall --test-threads 1`.
- [ ] **Step 3: Implement.**
  - The read matches the actor's `agent` by `agent-id`, requires `true == visible-to($b, $ag);`, sorts `$vf desc` then the concept id, and fetches id and title.
  - `max_chunks` truncation stays host-side.
  - Supersession is not applied as of T.
- [ ] **Step 4: Run the whole mem suite** (`cargo nextest run -p mem-gecko --no-default-features --test-threads 1`) and `gecko-bin --test mem_recall_e2e_test`. All pass, including the existing `gate` tests.
- [ ] **Step 5: Commit** `fix(mem): as-of recall returns only beliefs visible to the caller, newest first`.

### Task 2: Mem writes that must affect one row say so

**Files:**
- Modify: `core/mem-gecko/src/tql.rs`:
  - `supersession_ops` ~277;
  - `resolve_prediction_ops` ~601;
  - `informs_synthesis_op` ~483;
  - `set_was_used_op` ~497;
  - `retrieval_events_safe_to_tombstone_read` ~589.
- Modify: `core/mem-gecko/src/writer/beliefs.rs` (`supersede` error mapping ~190; `resolve_prediction` ~260) and the writer that issues the `informs-synthesis` / `was-used` ops (find with `grep -n informs_synthesis_op core/mem-gecko/src`).
- Test: `core/mem-gecko/tests/writer_reader_integration.rs`, `core/mem-gecko/tests/consolidation_daemon_test.rs` (or wherever `retrieval_events_safe_to_tombstone` is tested today).

**Interfaces:** none new. The error messages:
- supersede of a non-asserted belief: `supersede of <id>: no asserted belief with that id; nothing was committed: <raw>`;
- `resolve_prediction` with no open prediction: `name_missing_ids(…, "resolve_prediction", "open prediction", &[&b])`;
- `informs-synthesis` / `was-used` misses name both ids.

- [ ] **Step 1: Write the failing tests:**
  - `superseding_an_already_superseded_belief_is_refused_and_commits_nothing`: supersede A with B, then A with C. The second errors naming A, and no B/C fork exists (one supersession on A).
  - `superseding_a_retracted_belief_is_refused`: A stays `retracted`.
  - `resolve_prediction_without_a_recorded_prediction_is_refused_and_commits_nothing`: no resolving episode exists afterwards.
  - `resolving_a_prediction_twice_is_refused`.
  - `a_derive_that_used_two_retrievals_links_both`: the Review Focus normal path; it must still pass.
  - `a_retrieval_event_feeding_a_live_belief_is_not_safe_to_tombstone`: one event informs B1 and B2, and only B1 is superseded, so the event is not returned. It is returned once both are superseded, and appears once.
- [ ] **Step 2: Run, and expect the refused-path and tombstone tests to fail.**
- [ ] **Step 3: Implement.**
  - Supersede: add `$o has belief-state "asserted";` to the link op's match.
  - `resolve_prediction`: the link op gets `not { $pr has outcome $prev; };` and `.expecting(1)`, mapped by op index.
  - `informs_synthesis_op` / `set_was_used_op`: `.expecting(1)`, mapped by op index at their caller.
  - Tombstone read:
    ```typeql
    match $re isa retrieval-event, has concept-id $rid;
          informs-synthesis (retrieval: $re, synthesized: $b);
          not { informs-synthesis (retrieval: $re, synthesized: $b2); not { true == is-superseded($b2); }; };
          not { $re has consolidation-state "tombstoned"; };
    select $rid; distinct;
    fetch { "id": $rid };
    ```
- [ ] **Step 4: Run the mem suite, `mem_recall_e2e_test` and `belief_tier_reconciliation_test` (`--features cyber`). All pass.**
- [ ] **Step 5: Commit** `fix(mem): supersede, resolve_prediction and retrieval links refuse a write that matches nothing; tombstoning waits for every informed belief`.

### Task 3: The schema counts ownership and provenance

**Files:**
- Modify: `core/mem-gecko/schema/mem_types.tql` (~80-81, `memory-item`'s `plays`)
- Test: `core/mem-gecko/tests/schema_acceptance_test.rs`

**Interfaces:** `memory-item plays ownership:owned @card(1), plays source-link:memory @card(1..);`. Both are additive (no explicit annotation exists today); confirm that by reading the current lines.

- [ ] **Step 1: Write the failing tests:**
  - `a_memory_item_with_two_ownerships_is_refused_at_commit`;
  - `a_memory_item_with_no_source_link_is_refused_at_commit`.

  Both use raw inserts in a throwaway database with the mem schema, and assert that commit fails.
- [ ] **Step 2: Run, and expect both to fail (the commit succeeds today).**
- [ ] **Step 3: Implement the two annotations, each with its `@doc` unchanged.** If TypeDB refuses the additive form on an existing database, write a `MEM_MIGRATIONS` entry and record a ruling.
- [ ] **Step 4: Run the whole workspace suite:** `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --test-threads 1`. All pass. This is the conforming-data upgrade check, because every writer path re-applies the schema.
- [ ] **Step 5: Commit** `fix(mem): the schema requires exactly one ownership and at least one source-link per memory item`.

### Task 4: No duplicate rows in one write; precision ignores corrected verdicts

**Files:**
- Modify:
  - `core/gecko-engine/src/syncer/bundle.rs` (`create_links` ~1198, `create_citations` ~1244);
  - `extensions/cyber-gecko/src/stix/mod.rs` (generic SRO rows in `cyber_post_sync`);
  - `extensions/cyber-gecko/src/detect/mod.rs` (`precision` ~594).
- Test: `gecko-bin/tests/bundle_sync_test.rs`, `gecko-bin/tests/cyber_stix_sync_test.rs`, `gecko-bin/tests/cyber_detect_test.rs`, plus a unit test in `stix/mod.rs`.

**Interfaces:** none new.

- [ ] **Step 1: Write the failing tests:**
  - `a_document_linking_the_same_page_twice_gets_one_link`: also re-sync after editing another line; still one link.
  - `a_document_citing_the_same_url_twice_gets_one_citation`.
  - `repeated_sros_become_one_typed_relation`: unit test on `cyber_post_sync` rows, plus a live count.
  - `precision_ignores_a_superseded_disposition`: dispose FP, supersede that disposition belief with TP, and the counts show one TP and zero FP.
  - `precision_with_no_asserted_dispositions_is_zero`.
- [ ] **Step 2: Run, and expect them to fail.**
- [ ] **Step 3: Implement.**
  - Deduplicate rows host-side with a `BTreeSet` keyed as the spec says.
  - Add `$bel has belief-state "asserted";` to the `precision` read.
- [ ] **Step 4: Run `bundle_sync_test`, `multi_bundle_test`, `cyber_stix_sync_test`, `cyber_detect_test` (`--features cyber`). All pass.**
- [ ] **Step 5: Commit** `fix: one link per (source, target) within a sync; precision counts only current verdicts`.

### Task 5: Docs and full verification

**Files:** `docs/refactor/OWED-WORK-LEDGER.md`, `docs/refactor/KNOWN-LIMITATIONS.md`

- [ ] **Step 1: Ledger.**
  - Add a row for the TypeDB review: pass 1 ✅ with test names; pass 2 owed (plan `docs/superpowers/plans/2026-11-22-schema-tightening.md`).
  - List the follow-ons: batched multi-row reads (`GraphReader::read_rows`), deprecated relation syntax, function-body dedup, verb-named relations (declined).
  - Update A11's deferred list: `informs_synthesis_op` and `set_was_used_op` are now counted.
- [ ] **Step 2: KNOWN-LIMITATIONS.**
  - As-of recall is visibility-gated, but applies no supersession as of T.
  - A supersede requires a currently asserted belief.
- [ ] **Step 3: Full verification.** Run fmt, both clippy runs, and the workspace suite. All pass.
- [ ] **Step 4: Commit** `docs: record the TypeDB review's correctness pass`.
