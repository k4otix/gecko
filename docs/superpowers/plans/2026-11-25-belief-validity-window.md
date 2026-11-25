# Belief Validity Window Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** as-of reads return the beliefs that held at T, across supersessions.

**Architecture:**
- **The write side.** `supersede` closes the old belief's `valid-to` at the supersede instant,
  through a reusable never-extend builder that N4's `retract` will also use.
- **The read side.** `believed-at` reads the half-open window `[valid-from, valid-to)`. It drops
  the `belief-state` gate and also excludes a belief superseded at or before T, so older
  databases answer correctly.

**Tech Stack:** Rust 2024, TypeDB 3.15 (TypeQL functions and writes), cargo nextest.

**Spec:** `docs/superpowers/specs/2026-11-25-belief-validity-window-design.md`. This plan also
carries an external review of it (2026-11-25); its points are folded in below, and the spec's
Verification list is extended accordingly.

## Global Constraints

- All TQL is parameterized (`Params`); no value is interpolated.
- Comments are greenfield; every `@doc` is accurate.
- No `#[allow(dead_code)]`.
- Integration tests run with `--test-threads=1` / `-j 1`. Never touch the database named
  `gecko`.
- Before each commit, run all three:
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features --features cyber --all-targets -- -D warnings`.
- Ratified docs are not edited (INVARIANTS, SPEC-*, CONTRACT-W8, CONFLICT-REPORT). Frozen
  fixtures keep their text.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- The suite: `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1`.
- The writer test file: `core/mem-gecko/tests/writer_reader_integration.rs`. Run it with
  `cargo test -p mem-gecko --no-default-features --test writer_reader_integration -- --test-threads=1`.

## Review Focus

1. **A supersede of a belief that already has a later `valid-to`** must shorten it, not violate
   `@card(0..1)`. Task 1 probes it; Task 2 pins it.
2. **The re-applied schema must plan no function rewrite** after the new `believed-at`. That
   depends on how TypeDB exports a role-only relation with `has` inside `not`. Task 1 probes it;
   Task 3 pins it with the existing idempotence test.
3. **A legacy superseded belief with no `valid-to`** must be returned before its supersession
   and not from the supersede instant on. Task 3 pins both sides of the boundary.
4. **A refused supersede** must leave every window as it was. Task 2 pins it.
5. **An intermediate belief in a chain A → B → C** holds exactly over `[t2, t3)`. Task 3 pins it.

---

### Task 1: Probe the three unproven TypeQL behaviours

**Files:**
- Modify: `docs/refactor/TYPEDB-NOTES.md`, where the findings are recorded.
- Scratch, not committed: a throwaway test file in `core/mem-gecko/tests/`.

- [ ] **Step 1: Probe on a throwaway database** with the shipped core and mem schema applied.
  1. **Parameterized update:** `update $o has valid-to == $t;` against `update $o has valid-to $t;`,
     with `$t` given as a datetime parameter.
  2. **`update` on `valid-to @card(0..1)`:**
     - when the belief has no `valid-to` (it must insert it);
     - when it has a later one (it must replace it, with no cardinality error).
  3. **The `believed-at` body with `not { supersession (superseded: $b), has created-at $ca; $ca <= $t; };`:**
     - it defines as a persisted function;
     - after `database_schema()` export, `schema_apply::plan_queries` against the shipped text plans
       no rewrite (`FunctionDef::normalized`).

     If the export differs, use the explicit form
     `not { $s isa supersession, links (superseded: $b), has created-at $ca; $ca <= $t; };` and
     probe that.
- [ ] **Step 2: Choose the forms.**
  - If update item 1 or 2 fails, use the fallback: two ops with no row expectation.
    - An insert guarded by `not { $o has valid-to $vt; }`.
    - `delete has $vt of $o; insert $o has valid-to == $t;`, guarded by
      `$o has valid-to $vt; $vt > $t;`.
  - Record each choice as a `Ruling:` in the ledger.
- [ ] **Step 3: Record the findings** in `TYPEDB-NOTES.md`: the update form, its behaviour on an
  absent and a present `@card(0..1)` attribute, and the chosen `not` form and its export. Delete the
  scratch file.
- [ ] **Step 4: Commit** with the message `docs: TypeDB 3.15 notes on parameterized update and role-only relation patterns`.

---

### Task 2: `supersede` closes the old belief's window

**Files:**
- Modify: `core/mem-gecko/src/tql.rs`:
  - add the new builder `close_valid_to_op`;
  - use it in `supersession_ops` (`:393`);
  - rewrite the doc comment (`:390`).
- Modify: the comment at `core/mem-gecko/src/writer/beliefs.rs:249`, which now mentions closing the
  window.
- Test: `core/mem-gecko/src/writer/tests.rs` and `core/mem-gecko/tests/writer_reader_integration.rs`.

**Interfaces:**
- Produces: `pub(crate) fn close_valid_to_op(id: &MemId, at: DateTime) -> GraphWrite`.
  - It sets `valid-to = at` unless the belief's `valid-to` is already at or before `at`.
  - It carries no row expectation.
  - It is the never-extend close that N4's `retract` reuses.

- [ ] **Step 1: Write the failing tests.**
  - **Unit test,** in `writer/tests.rs`: `supersession_ops_closes_the_old_window_in_the_same_batch`.
    - `supersession_ops(...)` returns three ops, whose row expectations are `Some(1)`, `Some(1)`
      and `None`.
    - The third op's query text contains `valid-to`, and does not contain the `created_at` value's
      formatted text: the time travels as a parameter.
    - Check the `GraphWrite` fields the existing unit tests read.
  - **Live tests,** in `writer_reader_integration.rs`, reading `valid-to` back with the fixture's
    `raw_fetch`:
    - `a_supersede_keeps_an_earlier_valid_to`: raw-insert `valid-to 2026-02-01` on a belief, then
      supersede it at `2026-03-01`. `valid-to` is still `2026-02-01`.
    - `a_supersede_shortens_a_later_valid_to`: raw-insert `valid-to 2026-12-01`, then supersede at
      `2026-03-01`. `valid-to` is `2026-03-01`.
    - `a_refused_supersede_leaves_valid_to_untouched`, with two cases:
      - **Out of scope:** another agent's private belief, which has no `valid-to`. The refused
        supersede leaves it with no `valid-to`.
      - **Superseded twice:** supersede once at `t1`, then try again at `t2`. The second attempt is
        refused and `valid-to` stays `t1`.
- [ ] **Step 2: Run them and verify they fail.**
  Run: the writer test file filtered to `a_supersede_` and `a_refused_supersede`, plus
  `cargo test -p mem-gecko --no-default-features --lib supersession_ops`.
  Expected:
  - the unit test fails on the op count, 2 against 3;
  - the keep and refused tests may already pass, because nothing writes `valid-to` today;
  - the shortening test fails.
- [ ] **Step 3: Implement** `close_valid_to_op` with the probed form, and append it to
  `supersession_ops`.
- [ ] **Step 4: Run them and verify they pass.** Also run `believed_at_reads_across_a_supersede`.
  It still fails, because the state gate remains until Task 3.
- [ ] **Step 5: Run the suite, fmt and clippy, then commit** with the message
  `fix(mem): supersede closes the old belief's validity window`.

---

### Task 3: `believed-at` reads the window and the supersession edge

**Files:**
- Modify: `core/mem-gecko/schema/mem_functions.tql`:
  - the `believed-at` body and `@doc` (`:20`);
  - the stale comment over `canonical-entity-at` (`:157-159`).
- Modify: the stale comment at `core/mem-gecko/src/tql.rs:804` ("Supersession is not applied as of
  `at`").
- Test: `core/mem-gecko/tests/writer_reader_integration.rs` and
  `core/mem-gecko/tests/schema_migration_test.rs`.

- [ ] **Step 1: Write the failing tests.** `believed_at_reads_across_a_supersede` already exists
  and fails. Add:
  - `a_supersede_chain_gives_the_middle_belief_its_window`: A → B at `t2`, then B → C at `t3`.
    - As of `t1 < T < t2`: `[A]`.
    - As of `t2 <= T < t3`: `[B]`.
    - As of `T >= t3`: `[C]`.
  - `as_of_recall_reads_across_a_supersede`: the same as the single-supersede test, through
    `recall` with `as_of` (the `recall_as_of` helper).
  - `a_backdated_supersede_is_returned_by_no_as_of_read`: assert at `2026-03-01`, then supersede
    with `occurred_at` `2026-01-01`. No as-of read returns the old belief, and the successor holds
    from `2026-01-01`.
  - `a_legacy_superseded_belief_without_valid_to_ends_at_its_supersession`: supersede at `t2`
    through the writer, then delete the old belief's `valid-to` with a raw write, as an older
    database would hold it.
    - Before `t2`: `[old]`.
    - At `t2`: `[new]`, which pins `<=`.
    - After `t2`: `[new]`.
  - In `schema_migration_test.rs`, `previous_release_upgrades_to_the_shipped_schema` asserts that
    the upgraded schema's `believed-at` carries the supersession clause, and no longer carries
    `has belief-state "asserted"` in its body. Match on a substring of the new body.
- [ ] **Step 2: Run them and verify they fail.**
  Run: the writer test file filtered to `believed_at_reads_across`, `chain`,
  `as_of_recall_reads_across`, `backdated` and `legacy`, plus `cargo test -p mem-gecko --no-default-features --test schema_migration_test -- --test-threads=1`.
  Expected:
  - the across, chain and as-of-recall tests fail on the before-the-supersede read;
  - the legacy test may pass after its own supersede, but fails before `t2`;
  - the migration assertion fails.
- [ ] **Step 3: Implement.**
  - Write the `believed-at` body with the probed `not` form.
  - The `@doc` says it returns the beliefs whose half-open window `[valid-from, valid-to)` contains
    `$t` and that no supersession at or before `$t` replaced, and that `belief-state` is not read.
  - Fix the two stale comments.
- [ ] **Step 4: Run them and verify they pass.**
  - Run the writer test file, `schema_migration_test` (including
    `reapplying_the_schema_is_idempotent`) and
    `cargo test -p gecko-bin --no-default-features --features cyber --test schema_init_test -- --test-threads=1`.
  - Expected: all pass, and `a_same_instant_supersede_leaves_the_old_belief_an_empty_window` still
    passes with the gate gone.
- [ ] **Step 5: Run the suite, fmt and clippy, then commit** with the message
  `fix(mem): as-of reads follow validity windows across supersessions`.

---

### Task 4: Docs

**Files:**
- Modify: `docs/refactor/KNOWN-LIMITATIONS.md:149-156`, the "As-of recall and supersede" section.
  - **Keep** the visibility sentence (an unknown actor gets an empty result) and the
    supersede-needs-an-asserted-belief sentence (`superseding_a_retracted_belief_is_refused`).
  - **Replace** the middle with three points:
    - as-of recall at T returns the beliefs whose window contains T and that no supersession at
      or before T replaced, across supersessions;
    - a database written before this change answers correctly through the supersession clause;
    - `believed-at` no longer reads `belief-state`, so a belief raw-written as `retracted`, or as
      `superseded` with no supersession edge, stays in as-of reads unless its `valid-to` is
      closed. No writer retracts today, and N4 will close `valid-to`.
  - **Add** a sentence: the new `believed-at` reaches a database when `gecko schema-init` or
    `sync` next applies the schema.
- Modify: `docs/refactor/OWED-WORK-LEDGER.md`:
  - remove B1 from "Next up", renumbering, and from "Open work";
  - add it to "Landed since the review", with the design and test names;
  - in the D-A row, note the 2026-11-25 refinement: `believed-at` also excludes beliefs superseded
    at or before T;
  - point N4's row at `close_valid_to_op`.
- Modify: the spec's goal 2 wording, to say "a resolution superseded through the writer now leaves
  `canonical-step-at` from its supersede on". `canonical-step-at` has no supersession clause, and
  no pre-B1 database holds superseded resolutions, because no resolution writer exists.

- [ ] **Step 1: Edit.**
- [ ] **Step 2: Run the guard.**
  Run: `cargo test -p gecko-bin --no-default-features --bin gecko documented_config_snippets_are_accepted`.
  Expected: pass.
- [ ] **Step 3: Commit** with the message `docs: as-of recall follows validity windows; ledger closes B1`.

---

## Finish

- Run the full suite with `-j 1`, fmt, both clippy runs, `cargo audit` and
  `cargo deny check licenses bans sources`.
- Final whole-branch review on Fable, with one fix pass.
- Push `fix/belief-validity-window` and open a PR against `main`, then watch CI.
