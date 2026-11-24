# Populations Cut Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete populations, `scoped` and the cohort machinery from the mem substrate, and
return `blast-radius` to a belief-dependency closure over `rests-on` and `derivation`.

**Architecture:** This is a deletion done in two code tasks and a docs task.
1. Task 1 narrows `blast-radius` and removes its `-at` twins while the population types
   still exist, so the closure change is reviewed on its own.
2. Task 2 removes the population schema, writer, planner, migrations and their tests.
3. Task 3 closes the ledger rows and documents the existing-database caveat.

A guard test grows across Tasks 1 and 2 so the deleted labels can't return.

**Tech Stack:** Rust, TypeDB 3.15, TypeQL schema and functions, and nextest.

**Spec:** `docs/superpowers/specs/2026-11-24-populations-cut-design.md`

## Global Constraints

- **Code rules:**
  - Every remaining type, role and function carries a one-sentence greenfield `@doc`.
  - Comments are greenfield: no "used to", "no longer", "removed" or plan tags.
  - No `#[allow(dead_code)]`. Delete what becomes unused.
  - All TQL is parameterized.
- **Tests:**
  - Integration tests run with `-j 1` (`--test-threads=1`).
  - Never touch the database named `gecko`, and don't disrupt a TypeDB you didn't start.
- **Before each commit:**
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`;
  - the same clippy run with `--features cyber`.
- **Docs and fixtures:**
  - Ratified docs (`docs/refactor/SPEC-W2-W3.md`, `docs/refactor/SPEC-W5.md`,
    `docs/refactor/CONTRACT-W8.md`, `docs/refactor/INVARIANTS.md`) are never edited.
  - Frozen fixtures keep their text: `mem_schema_95ebac1.tql`, `cyber_schema_1cd621d.tql`,
    `gecko-bin/tests/fixtures/schemas_pre_tightening/`, and the `canonical-entity` bodies
    moved out of `closure_functions_ref.tql`.
- **Existing databases are not migrated.** No undefine/redefine shims. A test that applies
  a frozen older schema stays if the upgrade still applies; otherwise delete it and record
  a ruling.
- **Exact counts** (belief-tier surface, edge lists) are re-derived from the shipped schema,
  never hand-edited to pass.
- **Commit trailer:** `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Full suite:** `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1`.
- **The cyber-less build:** before each code commit, also run
  `cargo test --workspace --exclude js-sandbox --no-default-features --no-run` and
  `cargo nextest run -p gecko-bin --no-default-features -j 1 -E 'binary(bundle_sync_test)'`.
  Most of `schema_init_test.rs` is `#[cfg(feature = "cyber")]`; `bundle_sync_test.rs` is not.

## Review Focus

1. **A `rests-on` cycle:** `blast-radius` over a cycle terminates and returns each member
   once (Task 1 `blast_radius_terminates_on_a_rests_on_cycle`).
2. **A belief nothing depends on** has an empty blast radius, not an error (Task 1
   `blast_radius_of_a_leaf_belief_is_empty`).
3. **Re-applying the shipped schema** to a database initialized by it plans zero function
   redefinitions after both tasks. The existing `reapplying_the_schema_is_idempotent`
   must still pass unchanged in substance.
4. **A cyber database synced after the cut:** the belief-tier guard counts the new
   surface, so `sync-stix` still commits. The existing
   `memwriter_is_the_sole_belief_tier_writer` and the cyber sync tests cover this and
   must pass.
5. **Label leakage:** no shipped `.tql`, Rust source or live doc references a deleted
   label (Task 2 guard, Task 3 search).
6. **A database initialized by today's `main`:** what applying the narrowed schema does
   to it is observed by a one-off probe and stated in KNOWN-LIMITATIONS, not assumed. A
   small synthetic test pins TypeDB's callee-narrowing behaviour (R6, Task 1 Step 5).

## Rulings from the plan review

These bind both tasks.

- **R1. `tightened_schema_applies_to_a_populated_database`**
  (`gecko-bin/tests/schema_init_test.rs:921`) is kept. It is the only test that drives
  every writer path across an upgrade.
  - `schema_parts` (`:897`) joins each type's capabilities into one string, so a leftover
    `plays` line would make the whole `entity belief` and `entity concept` strings
    differ. It changes to capability granularity: the set of `(type head, capability)`
    pairs (a type with no capability contributes its head alone) and the set of function
    texts.
  - The exact equality (`:1300-1304`) becomes two checks on those sets. Every part of
    `fresh` is in `after`. And `after` minus `fresh` is exactly a named constant,
    `CUT_LEFTOVERS`, listing what the cut leaves behind in an upgraded database.
  - `CUT_LEFTOVERS` is what the frozen pre-tightening fixture declares and the shipped
    schema does not, in the fixture's own text:
    - Task 1 adds the function `blast-radius-at` only. The fixture predates the step
      functions (`schemas_pre_tightening/mem_functions.tql:69`, `:189`), so it has no
      `blast-step-at`.
    - Task 2 adds the functions `population-members` and `population-members-at`
      (fixture `:144`, `:179`); the `population`, `scoped` and `population-member` types
      with each of their capabilities; the four attributes; and the pairs
      `belief` / `plays scoped:scoped-belief` and `concept` / `plays population-member:member`.
  - This pins the KNOWN-LIMITATIONS claim that leftovers are inert and named.
  - Task 2 also does three things:
    - drops the population writer calls (`:1137-1157`);
    - drops the one-player `population-member` seed and its asserts (`:1233-1237`,
      `:1307-1320`);
    - changes the literal `("mem", 22)` (`:1274`) to `("mem", 20)`. The map counts the
      migrations `added_by_the_tightening` (`:829`) matches, not all of mem's, so it
      stays a literal like `core` and `cyber`; two of the three deleted migrations match.
- **R2. `old_cyber_detection_types_migrate_away`** (`schema_init_test.rs:171`) keeps its
  subject: cyber's migrations from the frozen `cyber_schema_1cd621d.tql`.
  - That fixture declares `population plays sensor-coverage:coverage-scope` (`:143`), so
    it needs a mem schema that has `population`. Task 2 seeds the database as R1's test
    does, with `PRE_CORE` and the file's own `pre_mem_schema()` (`:376`), then applies
    the shipped core and mem with their migrations, and only then the frozen cyber
    schema. No cross-crate fixture is read.
  - The registry check (`:183`) pairs the shipped mem with the frozen cyber schema. If
    it refuses the frozen `population` reference, build it from the pre-tightening pair
    as `:695` does, and say so in the report.
  - The population seed (`:214`) and `after.contains("entity population")` (`:249`)
    stay. They pin that `cyber/undefine-sensor-coverage` drops the `coverage-scope`
    plays and leaves the player entity alone. Both are allowed hits in Task 2 Step 5.
- **R3. The guard matches whole labels, not bare words.** The kept divergence `@doc`s say
  "scoped member" and "is scoped to" (`mem_types.tql:291-297`), and rewording them would
  trip DEX15 on frozen upgrades.
  - A label character is `[A-Za-z0-9_-]`, the same set as `is_label_char` in
    `gecko-bin/tests/belief_tier_monopoly_test.rs:51`.
  - A boundary is checked only on a side where the needle itself ends in a label
    character. `scoped:` therefore has a leading boundary and no trailing one, so it
    matches `plays scoped:scoped-belief`.
  - The four attributes and the role `scoped-belief` are bare whole-label needles. None
    appears in any surviving schema line, so that is tighter than matching their
    `attribute` and `owns` forms.
- **R4. Tests that use populations only as a vehicle are re-pointed, not deleted** (see
  each task).
- **R5. Ratified and historical docs are excluded from the doc search.**
  - Historical: `docs/refactor/REVIEW-2026-11.md`, `CONFLICT-REPORT.md`,
    `g1-g2-validated.md` and `docs/refactor/probes/`.
  - `docs/refactor/INVARIANTS.md` is `RATIFIED` (`:3`) and forbids in-place edits to its
    §1 text (`:8`). It is not edited, not even to add a pointer. The design lists the
    passages it supersedes, and the ledger carries the pointer (Task 3).
  - In the ledger, a landed row is history. Its text stays, and it gains the same
    ✂ marker as every other landed row the cut supersedes.
- **R6. The post-R3 database is probed, not guessed** (Task 1 Step 5).
  - A database initialized by today's `main` stores `blast-step($r: okf-concept)` and a
    `blast-radius($r: okf-concept)` that calls it. Neither frozen fixture has that
    shape: `mem_schema_95ebac1.tql:370` already takes `memory-item`, and the
    pre-tightening fixture has no `blast-step`.
  - `plan_queries` (`core/gecko-engine/src/db/schema_apply.rs:187-217`) redefines the
    callee first. The two functions are separate call groups, so the planner's own
    refusal (`:194`) doesn't fire, and TypeDB checks the narrowed `blast-step` against
    the stored caller.
  - **No frozen copy of today's schema is checked in.** Today's schema is exactly the
    unsupported shape the cut removes. The implementer runs a one-off probe instead:
    - export today's composed mem schema with `git show <base>:…` into the SDD
      workspace, never under `tests/fixtures/`;
    - apply it to a throwaway, uniquely named database;
    - seed A, and M resting on A;
    - apply the narrowed shipped schema with its migrations;
    - record the outcome, with the full error text if it refuses, in the report and the
      ledger, then drop the database.
  - **The lasting test is synthetic.** `narrowing_a_callee_under_a_stored_caller_*`, in
    `schema_migration_test.rs`, is modeled on
    `callee_signature_change_lands_before_its_caller`. It uses two tiny test functions,
    a caller and a callee whose param narrows from a supertype to a subtype. It is named
    for the outcome the probe saw (`…_applies` or `…_is_refused_naming_the_function`).
    It uses no population labels and no frozen schema.
  - **If the apply fails,** such a database is recreated, with no shim. If TypeDB's error
    names neither function, attach the function being redefined to the error. Make that
    a small change where the redefine runs and fails: the transaction query or commit in
    `core/gecko-engine/src/db/router.rs:396-402`, or the plan in `schema_apply.rs`. The
    synthetic test asserts the name, and this change is in scope.

---

### Task 1: `blast-radius` is a belief-dependency closure

**Files:**
- Modify:
  - `core/mem-gecko/schema/mem_functions.tql`: `blast-radius` and `blast-step` (around
    `:63-87`); delete `blast-radius-at` and `blast-step-at` and their comment block
    (around `:186-207`).
  - `core/mem-gecko/src/writer/reader.rs`: delete `blast_radius_at` (`:94-101`).
  - `core/mem-gecko/src/tql.rs`: delete `blast_radius_at_read` (`:1052-1059`).
  - `core/gecko-extension-api/src/epistemic.rs:268-272`: the `blast_radius` doc.
  - `core/mem-gecko/src/lib.rs:278-290`: drop the widened and `-at` blast asserts; keep
    the population ones until Task 2.
  - `core/mem-gecko/tests/closure_functions_test.rs`: its blast half checks against a
    Rust oracle instead of the reference body (Step 6).
  - `core/mem-gecko/tests/writer_reader_integration.rs`: delete
    `blast_radius_at_follows_the_materialized_window` and `present_blast_radius`.
  - `core/mem-gecko/tests/schema_migration_test.rs`: the `blast-radius-at` assert in
    `previous_release_upgrades_to_the_shipped_schema` (`:121`), and the synthetic R6
    test.
  - `gecko-bin/tests/schema_init_test.rs`: in
    `tightened_schema_applies_to_a_populated_database`, change `schema_parts`, and add
    `CUT_LEFTOVERS` with the `blast-radius-at` function and the two set checks (R1).
  - `gecko-bin/tests/schema_probe_w2w3.rs:1749`
    (`shipped_function_mutation_requires_redefine_not_define`): its subject is define
    versus redefine.
    - Narrow `REDEF_CHANGED_BLAST` (`:1731`) to `$r: memory-item`, and reword its doc
      (`:1726-1730`) without "folded (b1)" or a population branch.
    - Re-point the identical-define check (`:1782`) from `W3_ASOF_FUNCTIONS` to an inline
      const holding a kept shipped function, such as `canonical-step` or `believed-at`.
  - `gecko-bin/tests/schema_probe_w2w3.rs`: delete the probe tests whose subject is the
    cohort or windowed blast:
    - `p_base_t2_blast_radius_misses_cohort_belief`;
    - `blast_radius_widens_param_and_reaches_cohort` and its `_recursive` variant;
    - `p_br_3_*`, `p_br_4_*`, `p_br_5_*` and `p_br_6_*`;
    - `windowed_blast_radius_traversal_respects_membership_window`.
  - Any other test asserting the widened or `-at` blast signature.
- Create:
  - `core/mem-gecko/tests/fixtures/closure_canonical_ref.tql`, holding the
    `canonical-entity-ref` and `canonical-entity-at-ref` functions copied **verbatim**
    from `closure_functions_ref.tql`.
- Delete:
  - `core/mem-gecko/tests/fixtures/closure_functions_ref.tql`;
  - `gecko-bin/tests/probes/w3_blast_radius_b1.tql`, `w3_blast_radius_b2.tql` and
    `w3_blast_radius_b3.tql`;
  - `gecko-bin/tests/probes/w3_asof_functions.tql` and the `W3_ASOF_FUNCTIONS` const
    (`:117`). Line `:1782` is its only reader.
- Test:
  - `core/mem-gecko/src/lib.rs` tests: the guard;
  - `core/mem-gecko/tests/writer_reader_integration.rs`: the new blast tests.

**Interfaces:**
- Produces:
  - `fun blast-radius($r: memory-item) -> { memory-item }` and
    `fun blast-step($r: memory-item) -> { memory-item }`, the latter over `rests-on`
    (assumption → resting) and `derivation` (source → derived) only.
  - `EpistemicReader::blast_radius(&self, retracted: MemId) -> Result<Vec<MemId>>`,
    with its signature unchanged.
  - The guard helper in `core/mem-gecko/src/lib.rs` tests,
    `fn deleted_labels_absent(text: &str, needles: &[&str]) -> Vec<String>`.
    - It returns every needle found at a label boundary, as R3 defines one: where the
      needle starts with a label character, the text before it is a start or a non-label
      character; where it ends with one, the text after it is an end or a non-label
      character.
    - A needle may be a multi-word declaration (`relation scoped`), a role reference
      ending in `:` (`scoped:`), or a bare label (`blast-radius-at`).
    - Task 2 extends the needle list.

- [ ] **Step 1: Write the guard test (RED)**

In `core/mem-gecko/src/lib.rs` tests, add
`shipped_mem_schema_has_no_cohort_blast_labels`. It asserts that
`deleted_labels_absent(&schema, &["blast-radius-at", "blast-step-at"])` is empty, where
`schema` is the shipped mem types plus functions text the existing schema tests read. Also
assert that the shipped text contains `fun blast-radius($r: memory-item) -> { memory-item }`.

Write `deleted_labels_absent` with plain string scanning; no `regex` dependency. Add a
second unit test, `deleted_labels_absent_matches_whole_labels_only`. It checks that:
- `relation scoped @doc(` matches `relation scoped`, and `plays scoped:scoped-belief`
  matches `scoped:`;
- `blast-radius-at(` matches `blast-radius-at`, but `blast-radius(` does not;
- `relation scoped-thing`, `relation scoped_thing`, `relation scopedX` and the prose "for
  a scoped member" match neither `relation scoped` nor `scoped:`;
- `plays unscoped:role` and `plays exception-scoped:role` do not match `scoped:`.

- [ ] **Step 2: Write the blast behaviour tests**

In `writer_reader_integration.rs`, next to `blast_radius_propagates_transitively`, which
already covers the mixed rests-on then derivation chain:
- **`blast_radius_of_a_leaf_belief_is_empty`:** a belief with no dependents returns
  `Ok(vec![])`.
- **`blast_radius_terminates_on_a_rests_on_cycle`:** A rests on B, and B rests on A.
  Write both edges with `rests_on`, which has no cycle check
  (`core/mem-gecko/src/writer/beliefs.rs:312-328`). Assert that `blast_radius(A)` has
  length 2 and, sorted, equals `[A, B]`, so a member returned twice fails.
- **`blast_radius_follows_a_derivation_chain`:** C is derived from B, and B from A.
  `blast_radius(A)` is {B, C}.

- [ ] **Step 3: Run the new tests**

Run: `cargo nextest run -p mem-gecko --no-default-features -j 1 -E 'test(shipped_mem_schema_has_no_cohort_blast_labels) | test(deleted_labels_absent) | test(blast_radius_)'`

Expected:
- The guard test FAILS, naming `blast-radius-at` and `blast-step-at`, or the `okf-concept`
  signature.
- The label-matcher test passes.
- The three blast tests pass on today's schema, since they use no population data. Record
  that in the report.

- [ ] **Step 4: Narrow `blast-radius` and delete the twins**

- In `mem_functions.tql`:
  - set both signatures to `$r: memory-item`;
  - drop the `population-member`/`scoped` disjunct from `blast-step`;
  - rewrite the comment block and both `@doc`s, so they name only `rests-on` assumptions
    and derivation sources;
  - delete `blast-radius-at`, `blast-step-at` and their comment.
- Delete `MemWriter::blast_radius_at` and `tql::blast_radius_at_read`.
- Update the `EpistemicReader::blast_radius` doc to one accurate sentence over the two
  edges.

- [ ] **Step 5: Probe the post-R3 upgrade and pin the behaviour (R6)**

- **Run the one-off probe** R6 describes, before Step 4's schema edit is committed, so
  `<base>` is this task's BASE. Record the outcome in the report and the ledger.
- **Add the synthetic test** in `schema_migration_test.rs` and name it for that outcome:
  - **It applies:** `narrowing_a_callee_under_a_stored_caller_applies`. Both applies of
    the usual two-apply loop succeed, the export shows the narrowed callee, and the caller
    still answers.
  - **It refuses:** `narrowing_a_callee_under_a_stored_caller_is_refused_naming_the_function`.
    The error names the callee, adding the name per R6 if TypeDB's own text lacks it, and
    the stored caller still answers.

Task 3's KNOWN-LIMITATIONS bullet states the probe's outcome and names this test.

- [ ] **Step 6: Remove the tests and probes of the deleted behaviour**

- Apply the Files list.
- Move the two `canonical-entity` oracle functions verbatim into
  `closure_canonical_ref.tql`, and point the closure test's `canonical-entity` half at it.
- **The blast half keeps its random graphs.** Replace the `blast-radius-ref` comparison
  with `fn reachable(g: &Graph, from: usize) -> BTreeSet<usize>`: the beliefs reachable
  in one or more steps over `g.rests_on` (assumption → resting) and `g.derivations`
  (source → derived, any method). A belief on a cycle reaches itself.
  - `blast-radius` is checked for belief nodes only; a concept no longer type-checks.
  - The `blast-radius-at` checks go.
- Remove `scoped`, `memberships` and the population inserts from `Graph`, its strategy
  and `insert`. Neither remaining half reads them.
- `nonempty > 20` (`:294-297`) stays. Report the printed count. If it is under 40, raise
  `CASES`; don't lower the bound.
- Update the closure test's module doc.
- In `previous_release_upgrades_to_the_shipped_schema`, drop the `blast-radius-at`
  assert. `mem_schema_95ebac1.tql:370` already declares
  `blast-radius($r: memory-item)`, so this upgrade changes the body only.

- [ ] **Step 7: Run the suite**

Run the full suite and the cyber-less build (Global Constraints).

Expected: all tests pass, including `reapplying_the_schema_is_idempotent`, the guard
test and the synthetic R6 test.

- [ ] **Step 8: Commit**

Run fmt and both clippy configurations, then commit:
`refactor(mem): blast-radius follows rests-on and derivation only`

---

### Task 2: Populations, `scoped` and the membership cache are gone

**Files:**
- Modify:
  - `core/mem-gecko/schema/mem_types.tql`:
    - delete the attributes `spec-dialect`, `spec-text`, `spec-hash` and `computed-at`
      (around `:32` and `:46-49`);
    - delete `population`, `scoped` and `population-member` (around `:256-273`);
    - delete `belief plays scoped:scoped-belief` (around `:104`) and
      `concept plays population-member:member` (around `:335`);
    - correct the comment at around `:275` and the comment over `concept`'s plays at
      around `:327-330`;
    - retitle the section headers at `:46` and `:254` for what remains under them.
  - `core/mem-gecko/schema/mem_functions.tql`: delete `population-members` and
    `population-members-at` and their comments, fix the as-of section comment
    (around `:173-178`) that names them, and retitle the section header at `:135`.
  - `core/mem-gecko/src/writer/mod.rs`: drop `mod populations`, the `population_locks`
    field and its initializer, the `HashMap` import only they use (`:33`), and the
    module-doc mention.
  - `core/mem-gecko/src/writer/reader.rs`: delete `population_members` (`:88`) and any
    population wording.
  - `core/mem-gecko/src/lib.rs`: drop `pub mod populations`, the `MaterializeReport`
    re-export, and the population schema-text asserts.
  - `core/mem-gecko/src/tql.rs`: delete
    - the migrations `mem/population-member-window-doc`, `mem/scoped-required-roles` and
      `mem/population-member-required-roles`, and any comment referring to them;
    - every population TQL builder (`population_spec_read`, `define_population_op`,
      `population_rows_read`, `population_members_read`, and the open, close and
      refresh membership ops), with `member_rows_op` (`:878`) and
      `existing_concepts_read` (`:859`), whose only caller is `writer/populations.rs:200`.
  - `core/mem-gecko/src/belief_edge/scan.rs:256`: in the unit test
    `scanner_finds_relation_heads_in_mem`, replace `scoped` with `resolves`. A
    one-element list would trip `clippy::single_element_loop`.
  - Tests:
    - `core/mem-gecko/tests/writer_reader_integration.rs`, `schema_acceptance_test.rs`,
      `schema_migration_test.rs` and `closure_functions_test.rs`;
    - in `gecko-bin/tests`: `schema_init_test.rs`, `belief_tier_reconciliation_test.rs`
      (around `:907-928`), `belief_tier_surface_test.rs` (`:75`, `:83`),
      `bundle_sync_test.rs` (around `:1017-1050`) and `schema_probe_w2w3.rs`.
- Delete: `core/mem-gecko/src/populations.rs`, `core/mem-gecko/src/writer/populations.rs`
  and `gecko-bin/tests/probes/w3_population_window.tql`.
- Test: extend the Task 1 guard.

**Interfaces:**
- Consumes: Task 1's `deleted_labels_absent`.
- Produces: a mem schema with no population types. The belief-tier surface is the shipped
  surface minus `scoped`.

- [ ] **Step 1: Extend the guard (RED)**

Rename the Task 1 guard to `shipped_mem_schema_has_no_population_labels`. Its full needle
list (R3; design goal 7):

```
"entity population", "relation scoped", "relation population-member",
"isa population", "isa scoped", "scoped:", "population-member:", "scoped-belief",
"spec-dialect", "spec-text", "spec-hash", "computed-at",
"population-members", "population-members-at", "blast-radius-at", "blast-step-at"
```

- [ ] **Step 2: Run it**

Run: `cargo nextest run -p mem-gecko --no-default-features -E 'test(shipped_mem_schema_has_no_population_labels)'`

Expected: FAIL, listing the population labels.

- [ ] **Step 3: Delete the schema, functions and Rust**

Apply the Files list's Modify and Delete entries for schema and `src/`. Compile with
`cargo build --workspace --exclude js-sandbox --no-default-features --features cyber`. Then
delete or trim every test the compiler or the suite flags:
- A test whose subject is populations, `scoped`, membership windows or `computed-at` is
  deleted. In `writer_reader_integration.rs` that is every materialize, close, define and
  membership-window test and their helpers. In `schema_acceptance_test.rs` it includes
  `population_materialized_and_strict_membership` and
  `same_spec_hash_shares_one_population`.
- A test that only partly uses them is trimmed to its remaining subject. In
  `schema_probe_w2w3.rs`, the divergence tests keep their divergence assertions:
  - `member_scoped_exception_leaves_population_belief_asserted_for_others` keeps its
    member-concept `exception-scope` check;
  - `canonical_entity_at_recursive_asof_closure` keeps `canonical-entity-at`.
  - Each kept test's name is updated if it names populations. So are the module doc
    (`:10-48`) and the kept divergence tests' comments and assert messages (`:344`,
    `:394-444`, `:485-493`, `:528-627`): "population belief" becomes the general belief
    the exception overrides, and "the populations writer" goes.
  - `believed_at_and_members_at_agree_as_of_t` (`:1644`) and
    `p_base_t5_population_members_ignores_time` (`:254`) are deleted. The first would be
    left with a lone `believed-at` read, which `believed_at_returns_the_as_of_set`
    (`writer_reader_integration.rs:444`) already covers.
  - `p_w5_1_folded_surface_contains_divergence_precedence` (`:2003-2010`): drop `scoped`
    from the expected surface list, and from the comments at `:1960` and `:2002`.
- `belief_tier_surface_test.rs`:
  - drop `scoped` from the expected belief-edge list;
  - drop `population-member` from the record list, since the relation no longer exists;
  - re-derive any count.
- `belief_tier_reconciliation_test.rs`: drop the population steps, keeping every other
  production path's reconciliation.
- `bundle_sync_test.rs` (`:1017-1050`): the test isn't cyber-gated, and without the
  population half a cyber-less build would insert nothing and prove nothing.
  - Replace the population half with a relation the test defines itself, after the
    first sync: `test-ext-link` with roles `left @card(1)` and `right @card(1)`, each
    with a `@doc`, played by `concept`. No memory item plays it, so it stays off the
    belief-tier surface and the sync's fingerprint check.
  - Insert one link from `notes/a` to `notes/b`. After the GC of `notes/a`, assert that
    no `test-ext-link` remains and that `notes/b` does.
  - Keep the cyber-gated `uses` half as it is.
  - `source-link` and `resolves` are belief-tier edges, so a concept playing them is
    retired, not GC'd. They would change the subject.
- `schema_init_test.rs`: apply R1 and R2.
- `belief_tier_reconciliation_test.rs:535`: delete the `("scoped", Owed { owner: "A7" })`
  row.
- `core/mem-gecko/tests/schema_migration_test.rs`:
  - `previous_release_upgrades_to_the_shipped_schema` (`:122`): drop the
    `population-members-at` assert.
  - `changed_function_body_reaches_an_initialized_database` (`:128`): its subject is a
    body redefine reaching a live database. Re-point `NEW_BODY`/`OLD_BODY` and `members()`
    from `population-members` to a kept function, such as `canonical-step`.
  - `the_registry_accepts_the_tightening_migrations` (`:483`) and the tightening-form
    probes: keep them. They run on the frozen pre-tightening fixture, which still has
    `population-member`. They are listed as allowed hits in Step 5.
  - `the_valid_from_migration_applies_once_to_an_existing_database` (`:545`): drop the
    population seed (`:563-565`). Keep the `@card(0..1)` count assert (`:600-604`). The
    one line it counts is the frozen fixture's `population-member`, the only other owner
    of `valid-from`, so it pins that the migration leaves a second owner alone. Its
    message says so. It is an allowed hit in Step 5.
  - Reword the doc comments at `:20` and `:237` so neither describes a fixture by a
    deleted type.
- `core/mem-gecko/tests/schema_acceptance_test.rs`:
  - `a_database_holding_a_one_player_row_upgrades` (`:892`): trim to `derivation`.
  - Drop the `scoped` and `population-member` role cases (`:736-743`, `:852-855`).
  - Drop the `$pop isa population` line from `ROLE_PLAYERS` (`:610`) and from
    `MATCH_PLAYERS` (`:627`). Every remaining `missing_role_refused!` case runs both.
- `core/mem-gecko/tests/writer_reader_integration.rs:2078`
  (`population_members_and_canonical_entity`): trim it to the `canonical_entity` reader
  coverage and rename it `canonical_entity_reads_the_resolution_closure`.
- `gecko-bin/tests/schema_init_test.rs:440-632` (`tightened_mem`, `tightened_mem_migrations`,
  `tightening_forms_apply_to_the_pre_tightening_core_and_mem`): keep them, since they run
  on frozen fixtures. They are allowed hits in Step 5.
- `gecko-bin/tests/schema_probe_w2w3.rs:1824`
  (`full_candidate_stack_composes_define_clean_with_cyber`): delete the population halves
  (a) and (b), and keep the rest.
- `core/gecko-extension-api/src/lib.rs:86`: change the doc example id to a surviving
  migration id, such as `mem/belief-valid-from-required`.
- `core/mem-gecko/src/tql.rs:125-126`: reword the comment that explains a kept
  migration's trigger through `population-member`. The `applies_when` of
  `mem/belief-valid-from-required` (`:129`) keeps its three-line text. An upgraded
  database still prints the leftover `population-member`'s
  `owns valid-from @card(0..1)`, so a shorter trigger would fire on every apply.

- [ ] **Step 4: Run the suite**

Run the full suite and the cyber-less build (Global Constraints).

Expected: all tests pass, including the guard, `reapplying_the_schema_is_idempotent`,
`memwriter_is_the_sole_belief_tier_writer` and Task 1's synthetic R6 test, unchanged.

- [ ] **Step 5: Search**

Run: `rg -n 'population|\bscoped\b|spec-hash|spec_hash|computed-at|computed_at|blast-radius-at|blast_radius_at|MaterializeReport' core extensions gecko-bin -g '!**/tests/fixtures/**'`

Expected: the only hits are these.
- The frozen-fixture tests R1 and R2, the valid-from migration test and the tightening
  probes keep, and the `CUT_LEFTOVERS` constant.
- The guard's needle list and its matcher test in `core/mem-gecko/src/lib.rs`.
- Prose that isn't about mem populations. Don't churn these:
  - the cyber "population of hosts" wording, and the divergence "scoped member"
    `@doc`s, including the unread candidate `gecko-bin/tests/probes/w2_precedence.tql`;
  - S3 scope wording (`core/gecko-engine/tests/wasm_tests.rs:61`,
    `gecko-bin/tests/no_drag_test.rs:181`);
  - TypeDB's scoped role labels (`core/gecko-engine/src/db/surface.rs:187`,
    `gecko-bin/tests/belief_tier_surface_test.rs:98`);
  - visibility-scoped beliefs (`core/mem-gecko/src/writer/reader.rs:256`);
  - the local named `scoped` in `extensions/cyber-gecko/src/stix/mod.rs:1527-1531`.

List each kept hit in the report, grouped by reason.

- [ ] **Step 6: Commit**

Run fmt and both clippy configurations, then commit:
`refactor(mem): remove populations, scoped and the membership cache`

---

### Task 3: Docs

**Files:**
- Modify:
  - `docs/refactor/OWED-WORK-LEDGER.md`;
  - `docs/refactor/KNOWN-LIMITATIONS.md`;
  - any `README.md` or `extensions/README.md` passage that names populations, if one
    exists.

- [ ] **Step 1: Close the ledger rows**

- **A7, B4, P1, P2, P3 and P4:** each is marked ✂ cut, with one line pointing to
  `docs/superpowers/specs/2026-11-24-populations-cut-design.md`.
- **A5 and A6:** each gains one sentence. The population-versus-instance case lost its
  subject; the instance-versus-instance case and the member-concept `exception-scope`
  stand. `exception-scope` names the one concept an exception applies to. Mem has no
  group-scoped belief producer after the cut, so the sentence must not imply one.
- **The dependency graph:** update the lines naming P1/P2/P3 or A7, and drop B4 from the
  `B3 ──► B4` line (`:96`).
- **Open rows that refer to populations as live** are corrected:
  - C7 (`:257`): its "windowed traversal has landed (`blast-radius-at`)" clause goes;
  - M3 (`:286`): "B3 and B4 read these" becomes B3 only;
  - A11 (`:151`): the "populations ops" sentence goes;
  - D-F (`:314`) closes with B4;
  - the Track P intro (`:228-231`) states the cut and points to the design.
- **Landed rows the cut supersedes** keep their text and gain one ✂ marker pointing to
  the design (R5): F3, F4 (`:126`), and #3, #4, #5, #6 and #9 (`:335-339`).
- **The INVARIANTS pointer lives here.** One line in the ledger says the cut design
  supersedes INVARIANTS' population passages, since that file can't carry it (R5).

- [ ] **Step 2: Add the existing-database note**

In `KNOWN-LIMITATIONS.md` under "Schema constraints and upgrades", add one bullet. It
states what Task 1's tests observed and names them. It has no "if" or "may".
- A database initialized before the tightening, or by the previous release, takes the
  shipped schema. It keeps the old population types and functions as inert leftovers
  (`tightened_schema_applies_to_a_populated_database`,
  `previous_release_upgrades_to_the_shipped_schema`).
- A database initialized after the step functions landed either does the same or
  refuses `schema-init`, whichever the R6 probe recorded (the ledger line). The bullet
  names the synthetic test that pins TypeDB's behaviour.
- If it refuses, the bullet quotes the function the error names and gives the recovery:
  `gecko drop <db>`, then `schema-init`, then re-sync.

- [ ] **Step 3: Search the docs**

Run: `rg -n 'population|\bscoped\b|blast-radius-at' docs README.md extensions/README.md -g '!docs/refactor/SPEC-*' -g '!docs/refactor/CONTRACT-W8.md' -g '!docs/refactor/INVARIANTS.md' -g '!docs/superpowers/**' -g '!docs/refactor/REVIEW-2026-11.md' -g '!docs/refactor/CONFLICT-REPORT.md' -g '!docs/refactor/g1-g2-validated.md' -g '!docs/refactor/probes/**'`

Expected: only these (R5).
- The closed and ✂-marked ledger rows, and the ledger's INVARIANTS pointer.
- Ledger history: the PR #50 summary (`:20-21`, `:29`), F7's file list (`:129`) and the
  TypeQL notes (`:370`, `:377`).
- The new limitation, and unrelated prose such as the cyber "population of hosts"
  limitation and "Scoped `believed_at`" (`:273`).

- [ ] **Step 4: Commit**

Commit: `docs(ledger): populations cut closes A7, B4 and P1-P4`
