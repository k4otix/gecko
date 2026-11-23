# R3 Follow-ons Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Close ledger R3: scope `believed_at` to its caller, batch the detect reads through a new `GraphReader::read_rows`, and move every non-fixture query to current relation syntax.

**Spec:** `docs/superpowers/specs/2026-11-22-typedb-review-fixes-design.md` (R3 follow-ons), with ledger R3 in `docs/refactor/OWED-WORK-LEDGER.md`.


## Context

Ledger item R3 (`docs/refactor/OWED-WORK-LEDGER.md`) still holds three follow-ons from
the TypeDB review (spec `docs/superpowers/specs/2026-11-22-typedb-review-fixes-design.md`):

1. **`EpistemicReader::believed_at` is not scoped to the caller.**
   - It returns every belief as of T, whoever asks. That is the hole pass 1 closed for
     as-of recall.
   - No production caller exists yet, so the hole is latent until one appears.
   - Its `BeliefQuery` argument is silently ignored, which breaks the "never fabricate
     success" rule.
2. **Per-row reads (N+1).** `blinded` and `recommend_sources` issue about three reads per
   blind (technique, platform): its analytics, each analytic's live requirement, and
   each analytic's gaps. `GraphReader` can only send a single `given` row.
3. **The deprecated `(roles) isa rel` syntax.** It appears at 212 sites outside
   `tests/fixtures/`, including 37 in production code. A future TypeDB release may drop
   the form.

Outcome: all three land in one PR, and R3 is closed.

**Process,** as the user asked: a Fable review of this plan (done, folded in below),
then execution in the recommended mode, then one PR once everything is verified.

**The recommended mode** is subagent-driven for Tasks 1–3, which need judgment and
review. Tasks 4–5 are one scripted substitution; the controller runs the script and
reviews the diff, and a reviewer subagent checks it.

Tasks run strictly in order 1 → 2 → 3 → 4 → 5, with no parallel implementers:
- Tasks 1 and 4 both touch `tql.rs`.
- Tasks 1 and 5 both touch `writer_reader_integration.rs`.

**Setup.** Branch `fix/r3-follow-ons`. This plan is saved as
`docs/superpowers/plans/2026-11-23-r3-follow-ons.md`, with the review spec above as its
Spec, and the SDD ledger lives in `.superpowers/sdd/`.

## Global constraints

- Parameterized TQL only.
- Comments are greenfield.
- No `#[allow(dead_code)]`.
- Integration tests run with `--test-threads=1`. Never touch the `gecko` database.
- Before each commit: fmt plus both clippy configurations.
- Ratified docs are never edited.
- Frozen fixtures under `*/tests/fixtures/` keep their text. They record earlier
  releases: `schemas_pre_tightening/`, `mem_schema_95ebac1.tql`,
  `cyber_schema_1cd621d.tql` and `closure_functions_ref.tql`.
- New test code uses the current relation syntax from the start.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Don't hard-code test counts in briefs.

## Task 1: `believed_at` is scoped to the caller

- **New signature,** in `core/gecko-extension-api/src/epistemic.rs:273`:
  `async fn believed_at(&self, ctx: &RunContext, at: DateTime) -> Result<Vec<MemId>>`.
  - The trait doc says the result is the beliefs visible to `ctx.actor`, newest
    `valid-from` first.
- **Remove `BeliefQuery`,** which nothing else uses. Delete the type and its re-exports
  and imports:
  - `gecko-extension-api/src/lib.rs:45`
  - `gecko-engine/src/extension.rs:16`
  - `mem-gecko/src/writer/reader.rs:11`
  - `gecko-engine/src/sandbox/mem_host.rs:467`
  - `mem-gecko/tests/writer_reader_integration.rs:21`
- **The mem implementation** (`reader.rs:299`) reuses `tql::believed_at_chunks_read(at, &ctx.actor)`
  (`tql.rs:1117`, already `visible-to`-gated and sorted) and maps the ids, ignoring the
  title. Delete `tql::believed_at_read`.
- **The test double** in `mem_host.rs:494` takes the new signature.
- **Tests** in `writer_reader_integration.rs`, reusing pass 1's as-of fixtures:
  - `believed_at_returns_the_as_of_set` takes a ctx;
  - new `believed_at_hides_another_agents_private_belief`;
  - new `believed_at_returns_a_team_belief_to_another_agent`;
  - new `believed_at_for_an_actor_with_no_history_is_empty`.
- **Docs:** remove the R3 `believed_at` clause from the ledger, and the sentence at
  `KNOWN-LIMITATIONS.md:115`.

## Task 2: `GraphReader::read_rows`

- **New trait method,** in `core/gecko-extension-api/src/graph.rs`:
  `async fn read_rows(&self, query: &str, vars: &[String], rows: &[Vec<GraphValue>]) -> Result<Vec<serde_json::Value>, EpistemicError>`.
- **Its doc states:**
  - it returns the documents from every row, in no promised order;
  - a caller identifies a document's row by fetching an attribute the row matched;
  - rows should be distinct;
  - `limit`, `sort` and `reduce` apply across all rows, so a batched query must not use
    them per row.
- **Validation,** shared by both implementations:
  - a row whose length differs from `vars.len()` is `InvalidInput`;
  - empty `vars` is `InvalidInput`; use `read` instead;
  - empty `rows` returns `vec![]` with no query.
- **The default implementation** loops over `read`, so the four test doubles keep
  compiling.
- **The `RouterGraphStore` override** (`gecko-engine/src/db/graph_store.rs:148`) sends
  one query with `GivenRows::new(vars, rows.len())`, mirroring the write path at
  `:115`.
  - It does no dedup: `GraphValue` is only `PartialEq`, and the default implementation
    couldn't match it.
  - It handles `Some(Err)` in the stream loop explicitly.
- **Probe first.** On TypeDB 3.15, confirm that a multi-row `given` read returns each
  row's documents.
- **Tests:**
  - an engine integration test: two rows return the union, each document carrying its
    matched key; empty rows issue no query; a mismatched row length is refused;
  - a unit test of the default implementation and its validation over a fake reader.

## Task 3: batch the detect reads

This consumes Task 2. In `extensions/cyber-gecko/src/detect/mod.rs`:

- **The gather.** `analytic_readings(graph, pairs: &BTreeSet<(String, String)>) -> Result<BTreeMap<(tid, platform), Vec<Reading>>, String>`.
  - **Seeding:** the map starts with every requested pair holding an empty `Vec`, so a
    pair whose analytics vanished still reaches the `graph_changed` checks
    (`blind_platform_entries` `:514`, `gap_sets` `:691`).
  - **What a `Reading` holds:** attack-id, concept id, `has_requirements`, and gap rows
    (name, key, channels, plus component name and id).
- **Three batched reads** replace about 3N single reads. Rows are built from
  `BTreeSet`s, so they are distinct. Every key fetched back is a matched attribute, so
  no `given` variable needs fetching.
  1. **Analytics:** `live_analytics` over every pair, fetching `$tc`, `$ap` and `$ac`.
     - A duplicate attack-id resolves deterministically, by keeping the smallest concept
       id. Today the last row wins (`:457`).
  2. **Requirements:** `has_live_requirement` over every analytic, with **no `limit 1`**
     (C1). It fetches `$ac`, and the set of returned ids is the analytics that have
     requirements.
  3. **Gaps:** one read over every (analytic, platform) whose analytic has requirements.
     - It adds `$a has platform $ap; $ap == $plat` and fetches `$ac` and `$ap` to key
       the rows.
     - Gap rows for an analytic without requirements are never read, so `missing` stays
       empty when `no_log_sources` is true.
- **Building the outputs.** `blind_analytics`/`missing_log_sources` (for `blinded`) and
  `blind_gap_sets` (for `recommend_sources`) build from the gather.
  - The "a gap's data component has no name" error stays in the `blinded` builder only.
  - `blind_platform_entries` and `gap_sets` are unchanged.
  - The per-pair cache in `recommend_sources` is removed.
  - Output order is rebuilt host-side, as today.
- **New tests:**
  - a gather unit test: a pair with no analytics reaches the `graph_changed` error;
  - an integration test with two analytics on one platform, one without requirements
    and one with gaps. It pins C1: `blinded` reports both correctly, and
    `recommend_sources` plans through the gapped one.
- **Existing tests:** every `blinded`, `recommend_sources` and coverage-property test in
  `gecko-bin/tests/cyber_detect_test.rs` passes unchanged.
- **Timing.** Re-run the opt-in S4 check with
  `GECKO_ATTACK_BUNDLE=/private/tmp/claude-501/-Users-eddie-Documents-repos-gecko/b58caccd-4178-4fc7-a581-9b53edddbe9b/scratchpad/attack/ea.json`.
  It is skipped, and the skip noted, if the bundle is gone.
  - The output must be identical: 22 blind claims, 21 steps, 24 sources.
  - Record the new `blinded` and `recommend_sources` times in the S4 ledger entry. The
    baseline is 0.59 s and 0.56 s.

## Task 4: current relation syntax in production, with a guard

- **Probe first.** Against TypeDB 3.15, in match, `not {}`, a function body and insert,
  confirm each rewritten form with variable and literal attribute tails:
  - `rel (roles);`
  - `rel (roles), has a $v;`
  - `rel (roles), has a == $v;`
  - `rel (roles), has a "x";`
  - `rel (roles), has was-used false;`
  - `rel (roles), has t 2026-01-01T00:00:00;`
- **The rewrite.** One scripted substitution covers every site; all are single-line
  with flat role lists, and none is variable-bound or uses `isa!`:
  `\(([^()]*)\)\s*isa\s+([a-z][a-z0-9-]*|\{\})` → `\2 (\1)`.
  - It applies to `mem_functions.tql`, `cyber_functions.tql` and `mem-gecko/src/tql.rs`.
  - In `tql.rs` that includes the templated role lists (`:428`, `:502`, `:531`,
    `:635`), the multi-line `, has` tails (`:456`, `:745`, `:913`), and the belief-edge
    insert at **`:1209`**, which becomes `insert {} ({role_players}){has};` (C2).
  - The controller reviews the whole diff by hand.
- **Guard tests,** with a hand-rolled scan for `)`, optional whitespace, then `isa` as a
  whole word (`\)\s*isa\b`), so no `regex` dev-dependency is needed:
  - `shipped_queries_use_current_relation_syntax` in `mem-gecko` and in `cyber-gecko`,
    scanning the shipped `.tql` and `tql.rs`;
  - a workspace test in `gecko-bin`, scanning every `*.rs` and `*.tql` under `core/`,
    `extensions/` and `gecko-bin/` except `**/tests/fixtures/**`.
  
  Each names the file and line it finds.
- **Re-apply is a no-op** (I6): extend `reapplying_the_schema_is_idempotent`
  (`mem-gecko/tests/schema_migration_test.rs:64`) to assert that a second apply plans
  zero function redefinitions. This catches TypeDB's export printing the new form
  differently from the shipped text.

## Task 5: current relation syntax in tests

- Run the same script over every test file and probe outside `tests/fixtures/`.
  - The heaviest files are `cyber_stix_sync_test.rs`, `schema_acceptance_test.rs`,
    `schema_init_test.rs`, `writer_reader_integration.rs` and
    `schema_migration_test.rs`.
  - It also covers `gecko-bin/tests/probes/*.tql`.
- After it, the workspace guard from Task 4 passes, and the full suite passes with no
  behaviour change.

## Finish

- **Ledger R3** ✅: name each item with its tests and the S4 timings.
- **Final whole-branch review** on Fable, with one fix pass.
- **PR** against `main`, with the CI checks watched.

## Verification

- `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1` passes. The baseline is 698 passed, 5 skipped; the new tests add to it.
- fmt, both clippy configurations, `cargo audit` and `cargo deny check licenses bans sources` are clean.
- `rg '\)\s*isa\b' -g '*.rs' -g '*.tql' -g '!**/tests/fixtures/**' core extensions gecko-bin`
  finds nothing, and the workspace guard enforces this.
- The S4 check on ATT&CK 20.1 gives an identical `recommend_sources` list, and the new
  timings are recorded.
