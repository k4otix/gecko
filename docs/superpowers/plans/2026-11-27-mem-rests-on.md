# `mem.restsOn` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A playbook declares that a belief rests on others with `mem.restsOn(resting, assumptions)`. The edge is visibility-gated and stamped with its declarer, run and time, and the cyber worked example shows a non-empty `blast_radius`.

**Architecture:** `rests-on` gains `declarer`, `run` and `created-at` roles. `MemWriter::rests_on` writes a stamped, visibility-gated batch, using a visibility predicate it shares with `derivation_op`. A new `MemHostFn::RestsOn` and a guest `mem.restsOn` reach it behind `mem:rests_on`. `record_verdict` takes `assumes`.

**Tech Stack:** Rust 2024, TypeDB 3.15 (TypeQL), Boa 0.22 guest on wasm32-wasip1, wasmtime 51, cargo nextest.

**Spec:** `docs/superpowers/specs/2026-11-27-mem-rests-on-design.md`

## Global Constraints

- **Never touch the `gecko` database.** Integration tests run with `-j 1`.
- **All TQL is parameterized.** Comments are greenfield. No `#[allow(dead_code)]`. Every schema type and role carries an `@doc`.
- **Frozen fixtures** under `*/tests/fixtures/` keep their text.
- **Never loosen the belief-edge registry.**
- **Exact strings:**
  - "rests_on requires at least one assumption";
  - "rests_on names assumption <id> more than once";
  - "rests_on: <id> cannot rest on itself";
  - the missing-id noun "memory item visible to the caller";
  - the host's "mem call field '<key>' must be a non-empty id".
- **Names:**
  - the scope is `mem:rests_on`;
  - the host function is `"rests_on"`;
  - the guest function is `mem.restsOn`;
  - the reply is `{ "resting": <id>, "assumptions": [<ids>] }`.
- **The guest:** after editing `core/js-sandbox/src/main.rs`, run `just build-guest`, and commit `quickjs.wasm` in the same commit. `cargo clippy -p js-sandbox --target wasm32-wasip1 -- -D warnings` must be clean.
- **Before each commit:**
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`;
  - the same clippy run with `--features cyber`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- **Variable collisions in the shared visibility predicate.** The predicate is emitted for `$r` and for every `$a{k}`, so its helper variables (`$vo`, `$vv`, `$vp`) must carry a suffix unique to each item. The resting item uses tag `r`, and the assumptions use `a0`, `a1`, …. Pinned by the Task 1 test that gates both directions in one call.
- **A refused write that reveals existence.** An invisible id must give the same message as a missing one, both as the resting item and as an assumption. Pinned in Task 1.
- **An `assumes: null` in `record_verdict`.** It is checked with `assumes !== undefined`, so `null` is refused as not an array, the same as `[]`. Pinned in Task 3.
- **The run and agent nodes for a first-ever write.** `ensure_run` and `ensure_agent` must precede the link op, or a fresh run's `rests-on` matches nothing. Pinned by the end-to-end test in Task 2, whose run writes `rests-on` before anything else.
- **The populated upgrade's happy path.** It must still cover every other writer once `rests_on` leaves its population. Pinned by Task 1 keeping `tightened_schema_applies_to_a_populated_database` green.

---

### Task 1: the stamped, gated `rests-on`

**Files:**
- Modify: `core/mem-gecko/schema/mem_types.tql`:
  - `rests-on` at `:149-151`;
  - `agent plays rests-on:declarer` beside `:138`;
  - `doc-run plays rests-on:run` beside `:171`.
- Modify: `core/mem-gecko/src/tql.rs`:
  - the new `visible_item_clause`;
  - `derivation_op` at `:336-385` uses it;
  - `rests_on_op` at `:546` is replaced by `rests_on_ops`.
- Modify: `core/mem-gecko/src/writer/beliefs.rs:345-361`, `MemWriter::rests_on`.
- Modify: `core/gecko-extension-api/src/epistemic.rs:221`, the trait doc.
- Modify the tests that insert a bare `rests-on`:
  - `core/mem-gecko/tests/schema_acceptance_test.rs:132` and `:623`;
  - `core/mem-gecko/tests/closure_functions_test.rs:134`;
  - `gecko-bin/tests/belief_tier_surface_test.rs:222` (its `Links` expectation at `:255` may move);
  - `gecko-bin/tests/schema_probe_w3.rs:237`.
- Modify: `gecko-bin/tests/schema_init_test.rs`. Drop the `rests_on` call at `:1252` from the populated test, and add `an_unstamped_rests_on_fails_the_upgrade`.
- Test: `core/mem-gecko/tests/writer_reader_integration.rs`.
- Modify: `docs/refactor/KNOWN-LIMITATIONS.md`, in "Schema constraints and upgrades" (`:87-93`), to name an unstamped `rests-on` among the rows that fail the upgrade.

**Interfaces:**
- **Produces, in `tql.rs`:**
  - `fn visible_item_clause(var: &str, tag: &str) -> String`, the derivation evidence predicate with `$vo{tag}`, `$vv{tag}` and `$vp{tag}`, reading the actor as `$ag`;
  - `pub(crate) fn rests_on_ops(ctx: &RunContext, resting: &MemId, assumptions: &[MemId]) -> Vec<GraphWrite>`, which returns `[ensure_run(ctx), ensure_agent(ctx), link]`.
- **`EpistemicWriter::rests_on`:** the signature is unchanged.

- [ ] **Step 1: Write the failing writer tests** in `writer_reader_integration.rs`, using the file's existing agent and visibility fixtures from the retract and supersede tests:
  - `rests_on_stamps_the_declarer_the_run_and_the_time`. A raw fetch of the edge returns the `declarer`'s agent-id `== ctx.actor`, the `run`'s run-id `== ctx.run_id`, and `created-at == ctx.occurred_at`.
  - `rests_on_another_agents_private_belief_is_refused_and_commits_nothing`. There are two cases: another agent's private belief as the assumption, then as the resting item. Each error equals the message for a missing id in the same position. Afterwards, there are no `rests-on` rows.
  - `rests_on_a_team_belief_of_another_agent_is_allowed`.
  - `rests_on_refuses_a_repeated_assumption`: `[b, b]` gives "rests_on names assumption <b> more than once", with nothing committed.
  - `rests_on_refuses_a_self_dependency`: `rests_on(a, [a])` gives "rests_on: <a> cannot rest on itself".
  - `rests_on_gates_the_resting_item_and_every_assumption_in_one_call`. Here `resting` is visible, `assumptions = [visible, another agent's private]`, and the call is refused. This pins that the variables don't collide.
  - Update `rests_on_with_a_missing_assumption_errors_and_commits_nothing` (`:1811`) to the new noun.
- [ ] **Step 2: Write the failing schema tests.**
  - **In `schema_acceptance_test.rs`:**
    - `an_unstamped_rests_on_is_refused_at_commit`: a `rests-on` with only `resting` and `assumption` fails at commit;
    - the `:132` insert gains `declarer`, `run` and `created-at`;
    - `a_rests_on_missing_its_resting_is_refused_at_commit` (`:623`) supplies the stamp, so it is refused only for `resting`.
  - **In `schema_init_test.rs`:** `an_unstamped_rests_on_fails_the_upgrade`.
    - Apply the pre-tightening schema, the same `PRE_CORE` fixtures as the populated test.
    - Insert a bare `rests-on` between two beliefs with raw parameterized TQL.
    - Apply the shipped schema, and assert `Err` whose text contains `rests-on`.
- [ ] **Step 3: Run them.**
  - Run: `cargo nextest run -p mem-gecko --test writer_reader_integration --test schema_acceptance_test -j 1 rests_on`
  - Expected: FAIL. The stamp tests fail, because the edge has no declarer. The repeat and self tests fail, because the call is accepted. The private-belief tests fail, because the call is accepted.
- [ ] **Step 4: Implement.**
  - **The schema:** as the spec's Design shows. Every new role gets an `@doc`. The relation's `@doc` and the existing roles' `@doc`s stay byte-identical.
  - **`visible_item_clause`:** extract the predicate from `derivation_op`. `derivation_op` passes `tag = i.to_string()`, so its TQL is unchanged.
  - **`rests_on_ops`:**
    - The link op matches `$run`/`$rr == $rid`, `$ag`/`$aa == $aid`, and `$r`, binding `$a{k}` through `bind_id_list`.
    - It appends `visible_item_clause("$r", "r")` and, for each assumption, `visible_item_clause("$a{k}", "a{k}")`.
    - It inserts `rests-on (resting: $r, assumption: …, declarer: $ag, run: $run), has created-at == $ca`, `expecting(1)`.
  - **`MemWriter::rests_on`:**
    - validates empty, repeat (the `contest` loop) and self, in that order;
    - writes `rests_on_ops`, and maps errors with `name_missing_ids(e, Some(last), "rests_on", "memory item visible to the caller", &ids)`.
  - **The trait doc:** every id must be visible to `ctx.actor`, and the edge is stamped with the actor, the run and `occurred_at`.
- [ ] **Step 5: Fix the other bare inserts.**
  - `closure_functions_test.rs:134` gains a stamp. It inserts one agent and one run per graph.
  - `belief_tier_surface_test.rs:222` and `schema_probe_w3.rs:237` gain a stamp, or go through the writer if they already hold one.
  - The populated test at `schema_init_test.rs:1252` drops its `rests_on` call.
- [ ] **Step 6: Run them.**
  - Run: `cargo nextest run -p mem-gecko -j 1 && cargo nextest run -p gecko-bin --no-default-features --test schema_init_test --test belief_tier_surface_test --test schema_probe_w3 --test belief_tier_reconciliation_test -j 1`
  - Expected: PASS, including every `blast_radius_*`, `derive` and closure test unchanged.
  - The reconciliation rig still drives `rests_on` through its `WriterOnly` entry at this point. Task 2 moves it.
- [ ] **Step 7: Add the "Schema constraints and upgrades" line** to `KNOWN-LIMITATIONS.md`: a `rests-on` without a declarer, run or `created-at` fails the upgrade and needs a manual cleanup. Today's writers produce none.
- [ ] **Step 8: Commit** with the message `rests-on is stamped with its declarer, run and time, and gated on what the caller can see`.

### Task 2: `mem.restsOn` in the host and the guest

**Files:**
- Modify: `core/gecko-engine/src/sandbox/mem_host.rs`:
  - `MemHostFn` and `from_name` (`:78-90`);
  - the accepted keys (`:157-162`);
  - the dispatch (`:337-390`);
  - the unit tests.
- Modify: `core/js-sandbox/src/main.rs`. Add `mem_rests_on` beside `mem_retract` (`:270`), registered beside `:458`. Regenerate `core/gecko-engine/src/sandbox/quickjs.wasm`.
- Test: `core/gecko-engine/tests/wasm_tests.rs`, whose stub writer's `rests_on` at `:583` records its arguments instead of `unreachable!`.
- Test: `gecko-bin/tests/mem_recall_e2e_test.rs`.
- Modify: `gecko-bin/tests/belief_tier_reconciliation_test.rs`:
  - the `rig_mem` playbook and its scopes;
  - the step-5 reference writes;
  - delete the `rests-on` `WriterOnly` entry (`:503-507`) and the `rests_on` drive (`:846`);
  - `pivot` stays `Owed`.

**Interfaces:**
- **Consumes:** `EpistemicWriter::rests_on(&self, ctx, resting: MemId, assumptions: &[MemId]) -> Result<()>` (Task 1).
- **Produces:** the host function `"rests_on"`, accepting `["resting", "assumptions"]` and replying `{ resting, assumptions }`; and the guest `mem.restsOn(resting, assumptions)`.

- [ ] **Step 1: Write the failing host unit tests** in `mem_host.rs` `mod tests`, with the file's mock writer:
  - `rests_on_reaches_the_writer_and_echoes_its_ids`: `{"resting": "mem/bel/a", "assumptions": ["mem/bel/b"]}` gives the reply `{"resting": "mem/bel/a", "assumptions": ["mem/bel/b"]}`, and the writer saw those ids.
  - `rests_on_refuses_bad_fields`. Each of these errors names its field, and the writer isn't called:
    - missing `resting`;
    - missing `assumptions`;
    - `assumptions: "x"`;
    - `assumptions: [1]`;
    - `resting: ""`;
    - `assumptions: [""]`;
    - an extra key `"reason"`.
- [ ] **Step 2: Write the failing guest tests** in `wasm_tests.rs`, using the existing `mem.*` guest-test shape with the recording stub:
  - `mem_rests_on_reaches_the_writer`: `mem.restsOn("mem/bel/a", ["mem/bel/b"])`, granted `mem:rests_on`, returns `{resting, assumptions}`, and the stub recorded those ids.
  - `mem_rests_on_without_its_scope_is_refused_before_the_writer`: ungranted, the run fails on S3, and the stub recorded nothing.
  - `mem_rests_on_without_assumptions_is_refused_by_the_host`: `mem.restsOn("mem/bel/a")` fails naming `assumptions`.
- [ ] **Step 3: Write the failing end-to-end test** `a_playbook_declares_an_assumption_and_learns_what_rested_on_it`, in `mem_recall_e2e_test.rs`, in the shape of `a_playbook_retracts_a_root_and_learns_what_rested_on_it` (`:411`).
  - One playbook, granted `mem:remember`, `mem:derive`, `mem:rests_on` and `mem:retract`, does the following:
    - derives a root belief and a dependent belief, each with no evidence between them;
    - calls `mem.restsOn(dependent.id, [root.id])`;
    - then calls `mem.retract(root.id, "wrong")`;
    - returns the retract reply.
  - The test asserts that `blast_radius == [dependent.id]`.
- [ ] **Step 4: Run them.**
  - Run: `cargo nextest run -p gecko-engine --no-default-features rests_on && cargo nextest run -p gecko-bin --no-default-features --test mem_recall_e2e_test -j 1 declares_an_assumption`
  - Expected: FAIL. The host tests fail, because `"rests_on"` is an unknown mem function. The guest and end-to-end tests fail, because `mem.restsOn` is not a function.
- [ ] **Step 5: Implement the host.**
  - Add `MemHostFn::RestsOn`.
  - `resting` goes through `req_str`, then a non-empty check. `assumptions` goes through `required_mem_ids`, then a non-empty check on each entry. The message is "mem call field '<key>' must be a non-empty id".
  - Call `writer.rests_on`, and reply with the echo.
- [ ] **Step 6: Implement the guest.**
  - `mem_rests_on` uses `require_str_arg(args, 0, "resting")` and `opt_arg(args, 1, "assumptions", …)`, then `host_call("mem", "rests_on", …)`.
  - Register it as `restsOn` with arity 2.
  - Run `cargo clippy -p js-sandbox --target wasm32-wasip1 -- -D warnings`, then `just build-guest`.
- [ ] **Step 7: Update the rig.**
  - `rig_mem` grants `mem:rests_on` and calls `mem.restsOn` between two of its beliefs.
  - The step-5 reference writes add the matching `rests_on`.
  - Delete the `WriterOnly` `rests-on` entry and the `rests_on` drive.
- [ ] **Step 8: Run them.**
  - Run: `cargo nextest run -p gecko-engine --no-default-features && cargo nextest run -p gecko-bin --no-default-features --test mem_recall_e2e_test --test belief_tier_reconciliation_test -j 1`
  - Expected: PASS, including every existing guest test against the rebuilt wasm.
- [ ] **Step 9: Commit** with the message `mem.restsOn declares what a belief assumes`. The commit includes `quickjs.wasm`.

### Task 3: the cyber worked example and docs

**Files:**
- Modify: `extensions/cyber-gecko/sample-bundle/playbooks/record_verdict.md`.
- Modify: `extensions/cyber-gecko/README.md`, the worked example (`:120-184`) and the paragraph on `assumes` versus `evidence`.
- Modify: `gecko-bin/tests/cyber_sample_test.rs`.
- Modify: `docs/refactor/KNOWN-LIMITATIONS.md`:
  - `:9`, adding `restsOn` to the enumerated surface;
  - `:26`, where "the five functions" loses its count;
  - "Writes act only on what the caller can see" (`:197-230`), with a `rests_on` bullet; and `:229`, where `rests_on` leaves the ungated line.
- Modify: `docs/refactor/OWED-WORK-LEDGER.md`.

**Interfaces:**
- **Consumes:** `mem.restsOn` (Task 2).

- [ ] **Step 1: Write the failing tests** in `cyber_sample_test.rs`.
  - **The walkthrough** (`the_readme_walkthrough_runs`):
    - it substitutes `<powershell_belief_id>`, the first claim's `belief_id`, into the new step 6 args and step 9 args;
    - it asserts that step 6's reply keys are `["assumes", "belief_id", "logic_id", "verdict"]`;
    - it asserts that step 9's `blast_radius == [<verdict belief_id>]`;
    - it asserts that the coverage report after step 9 has `T1059.001` undetected.
  - **`record_verdict_refuses_a_bad_assumes_before_recording`:** for each `assumes` of `[]`, `["a","a"]`, `[""]`, `"x"` and `null`:
    - the run exits non-zero;
    - `detection_precision` afterwards reports `true_positives == 0`.
- [ ] **Step 2: Run them.**
  - Run: `cargo nextest run -p gecko-bin --no-default-features --features cyber --test cyber_sample_test -j 1`
  - Expected: FAIL. The README has no step 9 or `assumes`, and the playbook records the verdict regardless.
- [ ] **Step 3: Implement the playbook.**
  - Scopes: `[cyber-gecko:disposition, mem:rests_on]`.
  - The body checks `assumes !== undefined` first. It throws a `TypeError` naming the problem unless `assumes` is an array, non-empty, every entry a non-empty string, and no entry repeated.
  - Then it calls `disposition`, and calls `mem.restsOn(verdict.belief_id, assumes)` when given.
  - It returns `{ ...verdict, assumes }` when given, or `verdict` otherwise.
  - The prose says what `assumes` means, and that a missing or invisible id fails the run after the verdict is recorded.
- [ ] **Step 4: Update the README.**
  - Step 6 passes `"assumes":["<powershell_belief_id>"]`, and its shown reply gains `assumes`.
  - Step 8 is unchanged.
  - A new step 9 withdraws the PowerShell claim, showing `blast_radius` holding the verdict's id. `T1059.001` is undetected again (706).
  - Add one paragraph on `assumes` versus `evidence`.
  - The step-4 sentence ("keep its `belief_id`") names both later uses.
- [ ] **Step 5: Run the tests.**
  - Run: `cargo nextest run -p gecko-bin --no-default-features --features cyber --test cyber_sample_test -j 1`
  - Expected: PASS.
- [ ] **Step 6: Update the docs.**
  - **`KNOWN-LIMITATIONS.md`:** the `:9` and `:26` edits, and the `rests_on` bullet in the gated list. The bullet names the Task 1 tests: every id is visible to the caller, and a refusal looks like a missing id. `rests_on` leaves the `:229` line.
  - **The ledger:**
    - M1 becomes a `pivot`-only deferred row with the same trigger;
    - a Landed row for the `rests-on` half names the tests;
    - a note says the other `mem.*` functions accept `""` ids.
- [ ] **Step 7: Commit** with the message `The cyber worked example shows a verdict resting on its claim`.

## Verification

- `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1` passes. The baseline is 855; the new tests add to it.
- fmt, both clippy configurations and the `js-sandbox` wasm clippy are clean.
- `grep -rn 'rests_on_op\b' core` finds nothing.
