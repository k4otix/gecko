# `coverage_gaps` by tactic Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `coverage_gaps` gives each gap row its live tactics, and adds a `by_tactic` breakdown
with each live tactic's `live` and `undetected` counts and its gap techniques.

**Architecture:** The gap read is unchanged. One more read covers the live `technique-tactic`
join, and the grouping happens host-side in `extensions/cyber-gecko/src/detect/mod.rs`.

**Tech Stack:** Rust 2024, TypeDB 3.15, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-11-25-coverage-gaps-by-tactic-design.md`

## Global Constraints

- All TQL is parameterized. The new read binds no values.
- Comments are greenfield. Never fabricate a value: an absent title is `null`, and a live tactic
  with no `attack-id` is an error naming its concept id.
- No schema change.
- Integration tests run with `-j 1` and never touch the database named `gecko`.
- Before each commit, `cargo fmt --all -- --check` and both clippy runs (with and without
  `--features cyber`) are clean.
- Frozen fixtures under `*/tests/fixtures/` keep their text. `gecko-bin/tests/common/structure.rs`
  is shared test code and may change.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **A technique serving the same tactic twice** (two `technique-tactic` edges to one tactic)
   counts once in `live` and appears once in `techniques` and `tactics`. Pin it with sets, and
   with a unit test of the grouping function over duplicate rows.
2. **A live tactic with no `attack-id`** fails with its concept id. Pin it in the grouping unit
   test.
3. **A tactic with no title** is grouped with `name: null`. Pin it in the grouping unit test.
4. **A gap technique with no live tactic** has `tactics: []`. A unit test pins this.
5. **Output order is deterministic** whatever order the rows arrive in. The unit test feeds rows
   unsorted.

---

### Task 1: The grouping, the read and its tests

**Files:**
- Modify: `extensions/cyber-gecko/src/detect/mod.rs`. Change `coverage_gaps` (`:235`), and add a
  pure function `group_by_tactic` with its unit tests in the module's `#[cfg(test)]` block. If
  the module has no such block, add one.
- Modify: `gecko-bin/tests/common/structure.rs`:
  - `:121`: the retired sub-technique gains the phase `execution`;
  - `:91`: each tactic gets a distinct `name`, `"Execution"` and `"Persistence"`, so a
    `name`/`shortname` swap is caught.
- Modify: `gecko-bin/tests/cyber_stix_sync_test.rs:1091-1096`. The exact `technique-tactic`
  pair list gains `(RETIRED_SUB_STIX_ID, EXECUTION_STIX_ID)`. Two tests read it (`:1117` and
  `:1258`). If a tactic title assertion exists, update it to match.
- Test: `gecko-bin/tests/cyber_detect_test.rs`.

**Interfaces:**
- Produces: the report's new fields, as the spec shows:
  - `undetected_techniques[*].tactics: [tactic attack_id]`;
  - `by_tactic: [{ attack_id, concept_id, shortname, name, live, undetected, techniques }]`.

- [ ] **Step 1: Write the failing unit tests.**
  `group_by_tactic(pairs: &[Value], unclaimed: &BTreeMap<String, Option<String>>) -> Result<(BTreeMap<String, Vec<String>>, Vec<Value>), String>`.
  - `pairs` are the tactic-read rows, with the fields `tech_cid`, `tech_aid`, `tac_cid`,
    `tac_aid`, `shortname` and `name`. `tac_aid` and `name` may be null or missing.
  - `unclaimed` maps a gap technique's concept id to its `attack_id`, if it has one.
  - `live` and `undetected` are counted by technique concept id.
  - A `techniques` entry is the technique's `attack_id`, or its concept id when it has none;
    `tech_aid` may be null.
  - It returns:
    - a map from gap technique concept id to its sorted, deduplicated tactic `attack_id`s;
    - the `by_tactic` array, sorted.
  - Tests:
    - `group_by_tactic_counts_live_and_undetected`. Unsorted rows: T2 and T1 serve TA2, T1
      serves TA1, and T1 is a gap. Expect TA1 with `live` 1, `undetected` 1 and `["T1"]`, then
      TA2 with `live` 2, `undetected` 1 and `["T1"]`. T1's tactics are `["TA1", "TA2"]`.
    - `group_by_tactic_counts_a_duplicate_pair_once`.
    - `group_by_tactic_keeps_a_fully_covered_tactic`: `undetected` 0 and `techniques: []`.
    - `group_by_tactic_gives_a_titleless_tactic_a_null_name`.
    - `group_by_tactic_refuses_a_tactic_without_an_attack_id`: the error contains the tactic's
      concept id.
    - `group_by_tactic_lists_an_id_less_gap_technique_by_concept_id`.
- [ ] **Step 2: Run** `cargo nextest run -p cyber-gecko --lib group_by_tactic`. Expected:
  compile failure.
- [ ] **Step 3: Implement `group_by_tactic`.** Use `BTreeMap`/`BTreeSet` (tactics keyed by
  attack id, technique counts by concept id), so the order needs no explicit sort. Then wire `coverage_gaps`:
  - `unclaimed` becomes a map from concept id to an optional attack id, read with a lenient
    accessor, never `row_str`;
  - run the tactic read
    `match technique-tactic (technique: $t, tactic: $tac); {live t} {live tac} fetch { "tech_cid": $t.concept-id, "tech_aid": $t.attack-id, "tac_cid": $tac.concept-id, "tac_aid": $tac.attack-id, "shortname": $tac.tactic-shortname, "name": $tac.title };`;
  - set every gap row's `tactics` (`[]` when absent);
  - add `by_tactic` to the report.

  Update the function's doc comment and the module doc (`:17`).
- [ ] **Step 4: Run** the step 2 command. Expected: PASS.
- [ ] **Step 5: Write the live tests** in `cyber_detect_test.rs`, using `structure_bridge`,
  `claim_technique`, `gap_row` and a new helper `tactic_row(gaps, attack_id) -> &Value`:
  - `coverage_gaps_groups_gaps_by_tactic`: execution (`EXECUTION_ATTACK_ID`) has `live` 2,
    `undetected` 2, and `techniques` `[PARENT, SUB1]` sorted. Persistence is the same with `SUB2`.
    The parent's row `tactics` is `[EXECUTION, PERSISTENCE]` sorted. Its `shortname` is
    `"execution"`, and `name` equals the fixture tactic's `name`.
  - `a_fully_covered_tactic_stays_with_no_gaps`: claim PARENT and SUB1. Execution has `live` 2,
    `undetected` 0 and `techniques: []`. Persistence has `undetected` 1 with `[SUB2]`.
  - `a_retired_technique_counts_in_no_tactic`: execution's `live` is 2, not 3. A deprecated
    technique keeps only `retired-attack-id`, so an id check would be vacuous; the count is the
    check.
  - `a_dead_tactic_is_not_grouped`: `edited_structure_bridge` (`:1724`) re-syncs the fixture
    with `object_mut(.., PERSISTENCE_STIX_ID)["x_mitre_deprecated"] = json!(true)`, as at
    `:1848`. Every fixture object already carries `deprecated false`, so a raw insert would
    break the cardinality. Persistence leaves `by_tactic`, and the parent's `tactics` is
    `[EXECUTION]`.
- [ ] **Step 6: Run** `cargo nextest run -p gecko-bin --test cyber_detect_test --no-default-features --features cyber -j 1 coverage_gaps tactic`,
  then the whole `cyber_detect_test` and `cyber_stix_sync_test` binaries. Expected: PASS. Before
  relying on the live tests, confirm they fail with the tactic `live(..)` filters removed.
- [ ] **Step 7: Commit:** "coverage_gaps groups its gaps by tactic".

### Task 2: Docs and the ledger

**Files:** `extensions/cyber-gecko/README.md` (the `coverage_gaps` description),
`docs/refactor/OWED-WORK-LEDGER.md`.

- [ ] **Step 1:** In the README, describe `tactics` and `by_tactic` (`live`, `undetected`,
  `techniques`, sorted by tactic id, dead tactics and techniques excluded).
- [ ] **Step 2:** In the ledger, move S6 to Landed with its tests and the design path, below N4.
  "Next up" drops S6 and renumbers.
- [ ] **Step 3:** Run fmt, both clippy runs and the full suite:
  `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1`.
  Expected: all pass.
- [ ] **Step 4: Commit:** "docs: coverage_gaps by tactic in the cyber README and ledger (S6)".

## Finish

- Final whole-branch review on Fable, with one fix pass.
- A PR against `main`, with the CI checks watched.
