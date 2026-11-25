# Second Cut Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** delete the divergence mechanism, the consolidation daemon and the belief-tier types no
production path writes, so that the shipped schema's `@doc`s describe only what GECKO holds.

**Architecture:** this is a deletion pass in five slices:
1. the `memory-item` `@doc` migration, which is the risky part, so it is probed first;
2. consolidation;
3. divergence;
4. the schema diet;
5. guard, rig and comments, then docs.

Every slice leaves the workspace green. Existing databases keep the cut types as inert leftovers.

**Tech Stack:** Rust 2024 workspace, TypeDB 3.15 (TypeQL schema `.tql`), cargo nextest.

**Spec:** `docs/superpowers/specs/2026-11-24-second-cut-design.md`. The file:line inventory it was
written from is `/private/tmp/claude-501/-Users-eddie-Documents-repos-gecko/b58caccd-4178-4fc7-a581-9b53edddbe9b/scratchpad/x1-inventory.md`.
It is unverified: grep before deleting.

## Global Constraints

- All TQL is parameterized (`Params`), never interpolated.
- Comments are greenfield, with no plan tags or changelog phrasing. `invariant N` and `design §X.Y`
  stay.
- `@doc` on every remaining type and function. A changed `@doc` on an existing type needs an
  `undefine @doc` migration.
- No `#[allow(dead_code)]`. Code a cut leaves uncalled is deleted.
- Frozen fixtures under `*/tests/fixtures/` keep their text. Ratified and locked docs are never
  edited: INVARIANTS, SPEC-W2-W3, SPEC-W5, CONTRACT-W8 and CONFLICT-REPORT. Neither are the
  historical docs: LEDGER-HISTORY, REVIEW-2026-11, g1-g2-validated and probes/W2-W3-PROBE-LOG.
- Exact counts are re-derived from live runs, never hand-edited to pass:
  - surface labels;
  - fingerprint keys;
  - migration counts;
  - `CUT_LEFTOVERS`.
- Integration tests run with `--test-threads=1` / `-j 1`. Never touch the database named `gecko`.
- Before each commit: `cargo fmt --all -- --check`, plus both
  `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`
  and `cargo clippy --workspace --exclude js-sandbox --no-default-features --features cyber --all-targets -- -D warnings`.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- The suite command, used at every task's end:
  `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1`.

## Review Focus

1. **A database initialized by today's `main`** upgrades to the shipped schema without a refusal:
   the new `memory-item` `@doc` installs and the cut types stay as leftovers. Task 1 pins it with a
   live test.
2. **Index reconciliation on a fresh database** still enumerates episodes once
   `consolidation-state` is gone. Task 2: the existing `semantic_index_integration` tests run on
   fresh databases.
3. **`gecko sync` on a database holding leftover cut types** still fingerprints the belief tier
   identically at both ends. Leftover types stay on that database's surface, which is fine.
   `tightened_schema_applies_to_a_populated_database` covers it (Tasks 2–4).
4. **A guest playbook** calling `mem.*` sees no behaviour change, because nothing cut was reachable.
   The guest and `mem_host` tests stay green, with no edits.
5. **An introspecting agent** reading `@doc`s on a fresh database finds no mention of a cut type.
   The guards added in Tasks 2–4 cover the declarations. The `@doc` prose is checked by the final search.

---

### Task 1: `memory-item`'s `@doc` migration (probe first)

**Files:**
- Modify: `core/gecko-extension-api/src/migration.rs:61-75` (the reinstalled-`@doc` check) and its
  unit tests.
- Modify: `core/mem-gecko/schema/mem_types.tql:53-56` (the comment and `@doc`).
- Modify: `core/mem-gecko/src/tql.rs` (`MEM_MIGRATIONS`: add `mem/memory-item-doc` next to
  `mem/belief-required-window-doc`).
- Modify: `gecko-bin/tests/schema_init_test.rs:484-489`, adding the new id to the
  `tightened_mem_migrations` filter.
- Test: `core/mem-gecko/tests/schema_migration_test.rs`.

**Interfaces:**
- Produces: the migration id `mem/memory-item-doc`, with `applies_when` set to the old `@doc`'s
  exact text and `tql` set to the probed `undefine @doc from …;` form.

- [ ] **Step 1: Probe.** Use a throwaway database and a scratch test, not committed, in the style of
  `schema_migration_test.rs`.
  - Apply the shipped schema.
  - Run `undefine @doc from memory-item sub okf-concept;`, and if that fails,
    `undefine @doc from memory-item;`.
  - Re-apply the shipped schema with a changed `memory-item` `@doc`.
  - Record which form works, or that none does, in the ledger as a `Ruling:`.
  - **If none works,** take the spec's fallback: keep the `@doc` byte-identical, skip Steps 2–5, add
    the KNOWN-LIMITATIONS sentence in Task 6, and go to Task 2.
- [ ] **Step 2: Write the failing tests.**
  - **In `migration.rs`:**
    - `a_doc_undefine_on_an_abstract_type_is_accepted`: a fragment with
      `entity t @abstract, sub base @doc("x");` and the migration `undefine @doc from t sub base;`
      (or the probed form) pass `check_migrations`.
    - The existing refusal of a target with no shipped `@doc` still refuses.
  - **In `schema_migration_test.rs`:** `a_database_holding_the_old_memory_item_doc_upgrades`.
    - No new frozen fixture. Build the old schema in memory as `shipped.replace(NEW_DOC, OLD_DOC)`,
      the pattern `changed_function_body_reaches_an_initialized_database` already uses.
    - Apply it, then apply the shipped schema.
    - Assert that it succeeds, and that the exported schema carries the new `@doc` text.
  - The existing `previous_release_upgrades_to_the_shipped_schema` and the pre-tightening upgrades
    also hold the old `@doc`. After Step 4 they are the red tests until the migration lands.
- [ ] **Step 3: Run them and verify they fail.**
  Run: `cargo test -p gecko-extension-api --lib migration` and
  `cargo test -p mem-gecko --no-default-features --test schema_migration_test -- --test-threads=1`.
  Expected:
  - the abstract-form test is refused by the checker;
  - the upgrade test fails its "carries the new text" assertion (the `@doc` is unchanged until
    Step 4).
  - After the Step 4 `@doc` edit and before the migration, the upgrade tests fail with TypeDB's
    changed-`@doc` refusal. Watch that happen too.
- [ ] **Step 4: Implement.**
  - In the checker's reinstalled check, skip annotations (`@abstract`, with or without a following
    `,`) after the label.
    - For the bare-label form (`undefine @doc from memory-item;`), skip the shipped `sub …` clause
      **only when the target itself has no `sub` clause**. A target with `sub X` must still match `X`
      exactly, so `an_undefine_doc_the_shipped_schema_does_not_replace_is_rejected` keeps refusing.
    - Unit tests cover `@abstract,` (as in `mem_types.tql:56`) and `@abstract` without a comma (as
      in `migration.rs:446`).
    - Cover whichever form the probe picked.
  - Fix the "Four-layer substrate" comment at `core/mem-gecko/src/lib.rs:284`.
  - Write the new `@doc`, one sentence:
    `"Abstract supertype of the substrate's memory: episodes and beliefs, linked by derivation, supersession, assumption and provenance edges."`
  - Add the migration.
  - Fix the `:53` comment: no "four-layer", no `A2.1`.
- [ ] **Step 5: Run the tests and verify they pass.** Run the Step 3 commands, plus
  `cargo test -p gecko-bin --no-default-features --test schema_init_test -- --test-threads=1`.
  Expected: all pass, including `previous_release_upgrades_to_the_shipped_schema` and
  `tightened_schema_applies_to_a_populated_database`. `("mem", N)` is unchanged here, because
  `added_by_the_tightening` (`schema_init_test.rs:838-848`) counts only `-required-roles` ids and
  four named ids. It goes 20 → 19 → 18 → 13 over Tasks 2–4.
- [ ] **Step 6: Run the suite, fmt and both clippy runs, then commit** with the message
  `feat(mem): memory-item's @doc names what it holds, with a migration for existing databases`.

---

### Task 2: Cut consolidation (U2)

**Files:**
- Delete: `core/mem-gecko/src/consolidation.rs`, `core/mem-gecko/src/writer/consolidation.rs` and
  `core/mem-gecko/tests/consolidation_daemon_test.rs`.
- Modify: `core/mem-gecko/src/lib.rs`:
  - the module and the re-exports at `:14`, `:20-25`;
  - the `consolidation-state` test at `:329-344`.
- Modify: `core/mem-gecko/src/writer/mod.rs:20,24,34-35`. That covers the module doc, `mod`,
  `DuplicateCandidate` and the `IndexOutcome` re-export.
- Modify: `core/mem-gecko/src/writer/index.rs`. `IndexOutcome` is deleted, `best_effort_remove`
  returns `()`, and the doc at `:44` is fixed.
- Modify: `core/mem-gecko/src/writer/tests.rs:331-341`. Delete
  `supersede_with_failing_remove_reports_failed`.
- Modify: `core/mem-gecko/src/tql.rs`:
  - the guard at `:560`;
  - the builders `duplicate_episode_candidates_read`, `dedup_episodes_ops` and
    `retrieval_events_safe_to_tombstone_read`;
  - the migration `mem/consolidation-step-required-roles`.
- Modify: `core/mem-gecko/schema/mem_types.tql`:
  - `consolidation-state` (`:9`) and its `owns` (`:61`);
  - `consolidation-action` (`:43`);
  - `consolidation-step` (`:235-240`) and its plays (`:76`, `:195`, `:228`).
- Modify: `core/gecko-extension-api/src/error.rs:6-18` (`NotYetImplemented`) and
  `epistemic.rs:7`.
- Modify tests:
  - `core/mem-gecko/tests/writer_reader_integration.rs`:
    - the dedup, tombstone and `IndexOutcome` tests at about `:2232-2722` and `:2932-2996`;
    - the helpers left unused: `RecordingIndex` (`:214-239`), `seed_episode` and `by_id`;
    - the `IndexOutcome` import (`:26`);
    - `recalling_writer` and `recall_once` stay;
  - `core/mem-gecko/tests/schema_acceptance_test.rs:281-327` and the `CONSOLIDATION_STEP` case and
    rows;
  - `gecko-bin/tests/no_drag_test.rs:188-203`;
  - `gecko-bin/tests/belief_tier_reconciliation_test.rs`: the imports, `by_id`, the
    `consolidation-step` row, and the dedup and daemon drives and deltas;
  - `gecko-bin/tests/schema_init_test.rs:994-997`, `:1195-1213`, the `("mem", N)` count and
    `CUT_LEFTOVERS`.

- [ ] **Step 0: Move the guard matcher, then add this slice's needles.**
  - Move `deleted_labels_absent` from `core/mem-gecko/src/lib.rs:105-170` to
    `core/gecko-extension-api/src/migration.rs`, `#[doc(hidden)] pub`, next to `check_migrations`,
    with its unit tests. The populations guard keeps passing.
  - Add a test `shipped_mem_schema_declares_no_consolidation_labels` in the mem lib tests, with these
    needles:
    - the bare attribute names `consolidation-state` and `consolidation-action`, which also catch
      `has` and `owns`;
    - `relation consolidation-step` and the role reference `consolidation-step:`.
  - Run it. It must **fail** against the working tree.
- [ ] **Step 1: Delete in one commit's worth of edits.** The attribute and the `:560` guard must go
  together.
- [ ] **Step 2: Run `cargo build --workspace --exclude js-sandbox --no-default-features --features cyber --all-targets`.**
  Expected: it compiles. Every remaining reference is a compile error to fix by deletion, never by
  stubbing.
- [ ] **Step 3: Re-derive the counts.** Run
  `cargo test -p gecko-bin --no-default-features --features cyber --test schema_init_test tightened_schema_applies -- --test-threads=1`.
  - Update `("mem", N)` (minus 1) and `CUT_LEFTOVERS` from the printed diff, adding only parts of
    the cut types.
  - Run `belief_tier_reconciliation_test` the same way. The ratchet must pass with the
    `consolidation-step` row gone.
- [ ] **Step 4: Run the suite, fmt and both clippy runs, then commit** with the message
  `refactor(mem): cut the consolidation daemon and the tombstone state (U2)`.

---

### Task 3: Cut divergence (U1)

**Files:**
- Delete: `core/mem-gecko/src/disposition.rs`, `gecko-bin/tests/probes/w2_disposition.tql` and
  `gecko-bin/tests/probes/w2_precedence.tql`.
- Modify: `core/mem-gecko/src/lib.rs:15,24`.
- Modify: `core/gecko-extension-api/src/epistemic.rs:92-110,325-351`. `rank()` and its test go;
  `provable()` stays.
- Modify: `core/mem-gecko/schema/mem_types.tql:259-276`, the whole block.
- Modify: `core/mem-gecko/src/tql.rs`, the migration `mem/divergence-precedence-required-roles`.
- Rename: `gecko-bin/tests/schema_probe_w2w3.rs` → `gecko-bin/tests/schema_probe_w3.rs`.
  - Delete the W2 tests (P-PREC-1…5, P-DISP-1…3) and the items they leave dead:
    - `W2_DISPOSITION` (`:81`) and `use chrono::NaiveDateTime;` (`:34`);
    - `DispKeys`, `method_rank`, `entrenchment_rank`, `host_disposition` and `read_disp_keys`.
  - `apply_delta` and `DbError` stay, because P-REDEF uses them (`:1002`).
  - Trim P-COMP to the define-clean composition with cyber, with no divergence inserts.
  - Trim P-W5-1 to its core and cyber relation checks, under a name without "divergence".
  - Keep the W3 test and P-REDEF, and fix P-REDEF's banner.
  - Rewrite the module doc.
- Modify tests:
  - `core/mem-gecko/tests/schema_acceptance_test.rs:464,481,609-…,704-705`;
  - the `divergence-exception` and `divergence-precedence` rows of the rig;
  - `CUT_LEFTOVERS` gains the divergence parts. The pre-tightening fixture declares them
    (`schemas_pre_tightening/mem_types.tql:286-297`), so re-derive them as in Task 2.

- [ ] **Step 0: Add the needles,** and watch them fail. The test is
  `shipped_mem_schema_declares_no_divergence_labels`, with these needles:
  - `entity divergence-exception`, `relation divergence-precedence`;
  - `divergence-precedence:`, `exception-scope`, `exception-belief`.
- [ ] **Step 1: Delete and trim.**
- [ ] **Step 2: Compile,** as in Task 2 Step 2.
- [ ] **Step 3: Re-derive.** Run `schema_init_test` (`("mem", N)` minus 1),
  `belief_tier_reconciliation_test` and `schema_probe_w3` with `--features cyber -- --test-threads=1`.
  Expected: all pass.
- [ ] **Step 4: Run the suite, fmt and both clippy runs, then commit** with the message
  `refactor(mem): cut the divergence mechanism and the host disposition ranker (U1)`.

---

### Task 4: The schema diet (U3)

**Files:**
- Modify: `core/mem-gecko/schema/mem_types.tql`. Delete:
  - the 12 labels (`intention`, `goal-hierarchy`, `working-set`, `playbook`, `execution`,
    `tool-invocation`, `learned-constraint`, `contextualizes`, `evidence`, `justification`, `support`
    and `attack`);
  - their plays on `memory-item` (`:64-71`), `episode` (`:88-89`), `doc-run` (`:227`) and `concept`
    (`:308`);
  - the attributes `execution-status`, `deadline`, `stability-tier`, `corroboration-count`,
    `justification-status` and `tool-name`.

  Keep `condition`, `execution-episode` and `created-at`.
- Modify: `extensions/cyber-gecko/schema/cyber_schema.tql`: `sighting` (`:200-202`), `sighting-of`
  (`:204-206`) and `observable plays sighting-of` (`:118`).
- Modify: `core/mem-gecko/src/tql.rs:28-47,73-77`. Delete the five cleanup migrations.
- Modify tests:
  - `core/mem-gecko/src/lib.rs:289-290`;
  - `core/mem-gecko/tests/schema_acceptance_test.rs`:
    - `belief_evidence_restson_supersession_roundtrip` (`:118-197`), trimmed to `rests-on` and
      supersession and renamed `belief_restson_supersession_roundtrip`;
    - the `EVIDENCE`, `JUSTIFICATION`, `SUPPORT`, `ATTACK` and `EXECUTION` cases and macro rows;
    - `a_negative_corroboration_count_is_refused` (`:1015-1024`);
  - the 14 rig rows (12 mem and 2 cyber);
  - `hostile_extension_raw_belief_write_is_flagged` (`belief_tier_reconciliation_test.rs:1136-1151`):
    its `type_hint` `"working-set"` becomes `"anomaly"`. A surviving concrete `memory-item`
    subtype keeps the test proving a belief-tier type is refused, not merely an unknown one;
  - `gecko-bin/tests/no_drag_test.rs:128-131`: the `evidence:*` and `justification:*` entries in
    `BEARER_ROLES`;
  - `schema_init_test`'s `("mem", N)` (minus 5) and `CUT_LEFTOVERS`.

- [ ] **Step 0: Add the needles,** and watch them fail.
  - **The mem test** `shipped_mem_schema_declares_no_diet_labels` matches:
    - every deleted type as `entity X` or `relation X`;
    - every deleted relation's roles as `X:` (`evidence:`, `execution:`, `goal-hierarchy:`, …),
      the populations guard's form;
    - the unambiguous attribute names bare: `execution-status`, `stability-tier`,
      `corroboration-count`, `justification-status` and `tool-name`;
    - `deadline` as `attribute deadline`, `owns deadline` and `has deadline`.
  - **The cyber test** `shipped_cyber_schema_declares_no_cut_labels`, in
    `extensions/cyber-gecko/src/lib.rs` tests, matches `entity sighting`, `relation sighting-of`
    and `sighting-of:`.
  - `sighting` and `evidence` are matched only in those forms.
- [ ] **Step 1: Delete.** Include the plays on `intention` and `learned-constraint` (spec goal 3).
  `agent`'s one cut play went in Task 2.
- [ ] **Step 2: Compile,** as in Task 2 Step 2.
- [ ] **Step 3: Re-derive** `("mem", N)` and `CUT_LEFTOVERS` from a live `schema_init_test` run,
  and run the rig.
  - Expected: the rig's surface loses exactly the 14 labels, 17 over Tasks 2–4.
  - `belief_tier_surface_test.rs` passes unchanged.
  - `old_cyber_detection_types_migrate_away` passes unchanged.
- [ ] **Step 4: Run the suite, fmt and both clippy runs, then commit** with the message
  `refactor(schema): cut the belief-tier types nothing writes (U3)`.

---

### Task 5: Rig owners and comments

**Files:**
- Modify: `gecko-bin/tests/belief_tier_reconciliation_test.rs`:
  - `pivot`'s owner becomes `M1`, and `resolution` and `resolves` become `M3-r`;
  - `KNOWN_OWNERS = ["M1", "M2", "M3-r"]`.
- Modify comments:
  - `core/mem-gecko/schema/mem_types.tql` and `mem_functions.tql`: the plan tags `A2.x` and `A3`,
    the `# N —` ordinals and "sketch said `long`";
  - `core/mem-gecko/src/lib.rs:3-7`, the crate doc;
  - `core/gecko-engine/src/pipeline.rs:143-146`;
  - `extensions/cyber-gecko/src/detect/mod.rs:195`.

- [ ] **Step 1: Owners,** then run the rig.
  Run: `cargo test -p gecko-bin --no-default-features --features cyber --test belief_tier_reconciliation_test -- --test-threads=1`.
  Expected: it passes, and the rig's owner check refuses an owner not in `KNOWN_OWNERS`.
- [ ] **Step 2: Comments.** Comments are not stored, so no database changes. Confirm with
  `schema_init_test`'s re-apply idempotence test.
- [ ] **Step 3: Run the suite, fmt and both clippy runs, then commit** with the message
  `chore: rig owners point at live ledger rows; greenfield schema comments`.

---

### Task 6: Docs and the final search

**Files:**
- Modify: `docs/refactor/KNOWN-LIMITATIONS.md`:
  - `:82-86`, the optional-roles list;
  - `:171-186`, the daemon actor paragraph and section. Lines `:165-170` are live caveats and
    stay;
  - `:101-117`, the leftover note, extended to the X1 types and stating the Task 1 outcome;
  - one line on test-made `tombstoned` episodes re-indexing.
- Modify: `docs/refactor/OWED-WORK-LEDGER.md`:
  - X1 moves to "Landed since the review";
  - M1's text names `pivot`;
  - the cut rows A8, M4, M5 and M6 (`:100-104`) point here;
  - the U1 and U2 cut bullets (`:93-99`) point at `2026-11-24-second-cut-design.md`, as the
    populations bullet does (`:91-92`);
  - "Next up" is renumbered;
  - "Superseded and stale passages" points to the design.

- [ ] **Step 1: Edit.**
- [ ] **Step 2: Final search, in two passes.**
  - **What both passes filter out:**
    - `target/`, `tests/fixtures/` and `docs/superpowers/`;
    - the ratified and locked docs (INVARIANTS, SPEC-W2-W3, SPEC-W5, CONTRACT-W8, CONFLICT-REPORT);
    - the historical docs: `docs/refactor/LEDGER-HISTORY.md`, `docs/refactor/REVIEW-2026-11.md`,
      `g1-g2-validated.md` and `docs/refactor/probes/W2-W3-PROBE-LOG.md`;
    - HNSW-internal "tombstone".
  - **Pass 1, identifiers and hyphenated labels,** substring match:
    `grep -rnE "ConsolidationDaemon|NotYetImplemented|IndexOutcome|dedup_episodes|DuplicateCandidate|retrieval_events_safe_to_tombstone|disposition::|\.rank\(\)|consolidation-state|consolidation-step|consolidation-action|tombstoned|divergence-|exception-scope|exception-belief|four-layer|goal-hierarchy|working-set|tool-invocation|learned-constraint|contextualizes|sighting-of|execution-status|stability-tier|corroboration-count|justification-status|tool-name" --include='*.rs' --include='*.tql' --include='*.md' --include='*.toml' .`
  - **Pass 2, ambiguous labels.**
    - It runs on schema text only: the `.tql` files, plus the TQL string literals in `.rs` found by
      searching for the keyword forms.
    - macOS `grep -E` has no `\b`, so word ends use `[[:<:]]` and `[[:>:]]`.
    - Command: `grep -rnE "(entity|relation|attribute|isa|plays|owns) +(intention|playbook|execution|evidence|justification|support|attack|sighting|deadline)[[:>:]]|(plays|links \() *(evidence|justification|support|attack|execution|playbook|sighting|intention):" --include='*.tql' --include='*.rs' .`
  - **Expected hits:**
    - the synthetic `belief_edge` labels;
    - the `attack`/`attack-pattern` boundary test in `belief_tier_monopoly_test.rs`;
    - `schema_init_test.rs:468-469`, the `"  entity playbook sub memory-item"` anchor into the
      frozen pre-tightening schema, which is correct as it is.
  - Classify every hit in the ledger.
- [ ] **Step 3: Run the guard snippet test.**
  Run: `cargo test -p gecko-bin --no-default-features --bin gecko documented_config_snippets_are_accepted`.
  Expected: it passes.
- [ ] **Step 4: Commit** with the message
  `docs: the second cut lands; limitations and ledger match the schema`.

---

## Finish

- Run the full suite with `-j 1`, fmt, both clippy runs, `cargo audit` and
  `cargo deny check licenses bans sources`.
- Final whole-branch review on Fable, with one fix pass.
- Push `refactor/second-cut` and open a PR against `main`, then watch CI.
