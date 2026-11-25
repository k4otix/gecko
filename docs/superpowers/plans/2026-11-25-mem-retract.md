# `mem.retract` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A playbook withdraws a belief with `mem.retract(id, reason?)`, leaving a `retraction`
record of who withdrew it, in which run and why, and learns which visible, current beliefs rested
on it.

**Architecture:**
- The schema gains a `retraction` relation and a `retraction-reason` attribute.
- `MemWriter::retract` writes one atomic batch: the link, the state flip and the window close.
  After the commit it removes the vector best-effort, then reads the visible blast radius.
- `mem_host` gains `MemHostFn::Retract`, and the guest gains `mem.retract`, with a rebuilt
  `quickjs.wasm`.

**Tech Stack:** Rust 2024, TypeDB 3.15, wasmtime 50, Boa 0.22 guest on `wasm32-wasip1`,
`cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-11-25-mem-retract-design.md`

## Global Constraints

- All TQL is parameterized through `Params`; no value is interpolated into query text.
- Comments are greenfield. Every new type and role carries a one-sentence `@doc`.
- No `#[allow(dead_code)]`.
- Never fabricate success over bad input. The host is the single authority on argument values.
- Integration tests run with `-j 1` / `--test-threads=1` and never touch the database named
  `gecko`.
- Before each commit, all of these are clean:
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`;
  - the same clippy run with `--features cyber`;
  - `cargo clippy -p js-sandbox --target wasm32-wasip1 -- -D warnings`.
- After editing `core/js-sandbox/src/main.rs`, run `just build-guest` and commit `quickjs.wasm`
  in the same commit.
- Ratified docs (INVARIANTS, SPEC-W2-W3, SPEC-W5, CONTRACT-W8, CONFLICT-REPORT) and historical
  docs (LEDGER-HISTORY, REVIEW-2026-11, probe logs) are not edited. Frozen fixtures under
  `*/tests/fixtures/` keep their text.
- `memory-item`'s `@doc` is not touched.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **The refusal error never confirms existence.** Retracting a private belief of another agent,
   and retracting a missing id, give the same message. Pin this in Task 1's refusal tests by
   comparing the two messages with the ids masked.
2. **A refused retract leaves no `retraction` edge and no `doc-run` or `agent` side effect beyond
   `put`.** Pin it in Task 1 by counting `retraction` edges after a refusal: 0.
3. **A same-run assert and retract gives the empty window.** At the run's `occurred_at`, as-of
   recall does not return the belief. Pin it in Task 1 as
   `a_same_instant_retract_leaves_an_empty_window`.
4. **A guest `reason` of the wrong type** (`5`, `{}`) is refused by the host, not stringified by
   the guest. Pin it in Task 2's guest test.
5. **The blast radius never includes the retracted belief itself**, even through a `rests-on`
   cycle. Task 1's blast-radius test pins this.

---

### Task 1: Schema, TQL and `MemWriter::retract`

**Files:**
- Modify: `core/mem-gecko/schema/mem_types.tql`. Add the attribute next to
  `supersession-reason`, and the relation next to `supersession`. Add the plays on `belief`,
  `agent` and `doc-run`.
- Modify: `core/mem-gecko/src/tql.rs`. Add `retraction_ops` after `close_valid_to_op`, and
  `visible_blast_radius_read` after `blast_radius_read`.
- Modify: `core/gecko-extension-api/src/epistemic.rs`, the `EpistemicWriter` trait.
- Modify: `core/mem-gecko/src/writer/beliefs.rs`, the `impl EpistemicWriter for MemWriter`.
- Modify: the test doubles `ProbeWriter` in `core/gecko-engine/src/sandbox/mem_host.rs:800` and
  `UnreachableWriter` in `core/gecko-engine/tests/wasm_tests.rs:403`. They need the new method.
  `ProbeWriter` records a `Probed::Retract { id, reason }` and returns an empty `Vec`.
- Test: `core/mem-gecko/src/writer/tests.rs`, `core/mem-gecko/tests/writer_reader_integration.rs`,
  `core/mem-gecko/tests/semantic_index_integration.rs` and
  `core/mem-gecko/tests/schema_acceptance_test.rs`.
- Test (label lists): `gecko-bin/tests/no_drag_test.rs` (`BELIEF_ATTRS`, `BEARER_ROLES`),
  `gecko-bin/tests/belief_tier_monopoly_test.rs` (`:530`, `:580`).
  `belief_tier_surface_test.rs` asserts a superset, so it needs no change.

**Interfaces:**
- Produces:
  - `EpistemicWriter::retract(&self, ctx: &RunContext, id: MemId, reason: Option<&str>) -> Result<Vec<MemId>>`.
    The `Vec` is the visible, current blast radius, sorted ascending by id.
  - `tql::retraction_ops(ctx: &RunContext, id: &MemId, reason: Option<&str>) -> Vec<GraphWrite>`,
    five ops: `ensure_run`, `ensure_agent`, link (`expecting(1)`), flip (`expecting(1)`),
    `close_valid_to_op(id, ctx.occurred_at)`.
  - `tql::visible_blast_radius_read(id: &MemId, actor: &ActorId) -> Read`.
  - The schema labels `retraction` (roles `retracted`, `retractor`, `run`) and
    `retraction-reason`.

- [ ] **Step 1: Write the failing unit tests** in `writer/tests.rs`:
  - `retraction_ops_expectations`. It calls `retraction_ops` with a `RunContext` at
    `2026-03-01T00:00:00Z` and reason `Some("wrong host")`, then asserts:
    - `ops.iter().map(|o| o.expect_rows)` is `[None, None, Some(1), Some(1), None]`;
    - no op's `query` contains `"wrong host"`, `"2026-03-01"` or the id string;
    - `ops[2].rows[0]` contains `GraphValue::String("wrong host")` and `GraphValue::Datetime(at)`;
    - with `None`, `ops[2].query` does not contain `retraction-reason`.
  - `retract_survives_a_failing_remove`, mirroring `supersede_survives_a_failing_remove`:
    `removes == 1` and the call is `Ok`. It needs `RecordingGraphStore` to answer the
    blast-radius read with no rows.
  - `retract_without_embedder_never_removes`: `removes == 0`.
  - `a_failed_blast_radius_read_says_the_retraction_committed`. The graph store's writes succeed
    and its reads fail. The error is `Storage` and its message contains both `"committed"` and
    `"blast radius"`. `RecordingGraphStore::read` always succeeds (`tests.rs:102-112`), so add a
    `fail_read` field to it.
- [ ] **Step 2: Run** `cargo nextest run -p mem-gecko --lib --no-default-features retract`.
  Expected: compile failure (no `retraction_ops` or `retract`).
- [ ] **Step 3: Implement the schema, the TQL and `MemWriter::retract`.**
  - **The link op** matches:
    - `$b isa belief, has concept-id $bc, has belief-state "asserted"; $bc == $id;`
    - the agent and the `doc-run` by parameter;
    - `true == visible-to($b, $ag);`

    It inserts `retraction (retracted: $b, retractor: $ag, run: $run), has created-at == $ca`,
    adding `, has retraction-reason == $reason` only when `reason` is `Some`.
  - **The flip** mirrors `supersession_ops`'s flip, with `"retracted"`.
  - **`visible_blast_radius_read`** matches `$r` by id and the agent, then:
    ```
    let $m in blast-radius($r);
    $m isa belief, has concept-id $mid;
    not { $m is $r; };
    true == visible-to($m, $ag);
    not { true == is-superseded($m); };
    ```
    It fetches `{ "id": $mid }`. Both negations follow existing forms (`mem_functions.tql:61`,
    `:109`).
  - **`MemWriter::retract`:**
    - Write the ops. On a row-count mismatch at an op index `>= 2`, return
      `Storage("retract of {id}: not a current belief visible to the caller; nothing was committed: {msg}")`,
      as `supersede` does.
    - Then call `best_effort_remove(ConceptId(id))`.
    - Then run the read. The ids are deduplicated and sorted host-side. A read failure becomes
      `Storage("retract of {id} committed; reading its blast radius failed: {e}")`.
  - Write the `@doc` strings, one sentence each.
- [ ] **Step 4: Run** the same command. Expected: PASS.
- [ ] **Step 5: Write the failing live tests** in `writer_reader_integration.rs`. Reuse its
  fixtures: `belief(...)`, the ctx helpers, and raw reads through the router.
  - `retract_flips_state_closes_the_window_and_records_who_and_why`. After the retract:
    - `belief-state` is `"retracted"`;
    - `valid-to == ctx.occurred_at`;
    - exactly one `retraction` edge has `retractor` agent-id == `ctx.actor` and `run` run-id ==
      `ctx.run_id`, with `retraction-reason == "wrong host"` and `created-at == occurred_at`.
  - `retract_without_a_reason_writes_no_reason`: the edge has no `retraction-reason`.
  - `a_retracted_belief_leaves_present_state_recall_and_gate`: recall without `as_of` does not
    return it, and the `gate` read is false.
  - `as_of_recall_ends_a_retracted_belief_at_its_retraction`: assert at `t1`, retract at `t2`.
    `believed_at(t1 + 1s)` contains it; `believed_at(t2)` and `believed_at(t2 + 1s)` do not.
  - `a_same_instant_retract_leaves_an_empty_window`: assert and retract at `t`, then
    `believed_at(t)` is empty.
  - `retracting_another_agents_private_belief_is_refused_and_commits_nothing`. Afterwards:
    - the state is still `"asserted"`;
    - there are 0 `retraction` edges;
    - `valid-to` is absent;
    - the message, with the id replaced by `<id>`, equals the message from
      `retracting_a_missing_belief_is_refused`'s call (fold both into this test, or share a helper).
  - `retracting_a_team_belief_of_another_agent_is_allowed`: the retractor is the caller, not the
    owner.
  - `retracting_a_superseded_belief_is_refused`: the error contains
    `"not a current belief visible to the caller"`.
  - `retracting_twice_is_refused`: the second call errs, and there is exactly 1 `retraction` edge.
  - `retracting_a_missing_belief_is_refused`.
  - `retract_reports_the_visible_current_beliefs_that_rested_on_it`. Root `R`:
    - `D1` derived from `R`;
    - `D2` derived from `D1` (transitive);
    - `A` with `rests_on(A, [R])`;
    - `P`, another agent's private belief derived from `R`. `R` is built with
      `belief_vis(..., Visibility::Team)` so the other agent can cite it;
    - `S`, derived from `R` and then superseded (its successor is not derived from `R`);
    - `X`, derived from `R` and then retracted before `R` is;
    - `rests_on(R, [D1])`, which closes a cycle.

    `retract(R)` returns exactly `[A, D1, D2]` sorted by id. It does not contain `R`, `P`, `S`
    or `X`.
  - In `semantic_index_integration.rs`: `retract_removes_the_vector_from_the_index`. Indexed
    recall finds the belief before the retract and does not after it.
  - In `schema_acceptance_test.rs`: add `retraction` with roles `retracted`, `retractor` and
    `run` to `missing_role_refused!`.
- [ ] **Step 6: Run** `cargo nextest run -p mem-gecko --no-default-features -j 1 -E 'test(retract) | test(missing_role)'`.
  Expected: PASS. Then fold in the label lists (`no_drag_test.rs`, `belief_tier_monopoly_test.rs`). Also confirm the schema re-apply test
  (`reapplying_the_schema_is_idempotent`) passes.
- [ ] **Step 7: Run the full suite:**
  `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1`.
  Expected: all pass except `memwriter_is_the_sole_belief_tier_writer`. That test fails for
  certain until Task 2 adds the rig step, because `retraction` is on the surface with no
  production step. A `DISPOSITIONS` entry is not an option: `KNOWN_OWNERS` is M1, M2 and M3-r.
  Ruling, recorded up front: the Task 1 commit is knowingly red on that one test, and Task 2
  turns it green.
- [ ] **Step 8: Commit:** "MemWriter::retract withdraws a belief and records who and why".

### Task 2: Host bridge, guest, rig and end to end

**Files:**
- Modify: `core/gecko-engine/src/sandbox/mem_host.rs`:
  - `MemHostFn::Retract`, name `"retract"`, in `as_str`, `from_name` and `all()`, which becomes
    `[MemHostFn; 6]`;
  - the allow-list `&["id", "reason"]`;
  - the dispatch arm;
  - the module doc at `:4`, which gains `retract`.
- Modify: `core/js-sandbox/src/main.rs`. Add `mem_retract`, register it on the `mem` object, and
  rebuild `core/gecko-engine/src/sandbox/quickjs.wasm` with `just build-guest`.
- Test: the `mem_host.rs` unit tests, `core/gecko-engine/tests/wasm_tests.rs`,
  `gecko-bin/tests/belief_tier_reconciliation_test.rs` and
  `gecko-bin/tests/mem_recall_e2e_test.rs`.

**Interfaces:**
- Consumes: `EpistemicWriter::retract` from Task 1.
- Produces:
  - the scope `mem:retract`;
  - the host reply `{ "id": <id>, "blast_radius": [<id>, …] }`;
  - the guest `mem.retract(id, reason?)`, returning that object.

- [ ] **Step 1: Write the failing host unit tests:**
  - add `MemHostFn::Retract` to `an_unknown_key_is_refused_for_each_mem_function`;
  - `retract_requires_an_id`: `{}` is refused naming `id`;
  - `retract_refuses_a_non_string_reason`: `{ id: "mem/bel/x", reason: 5 }` is refused and
    `ProbeWriter` records nothing;
  - `retract_forwards_id_and_reason_and_replies_with_the_blast_radius`: `ProbeWriter` records
    `Retract { id: "mem/bel/x", reason: Some("r") }`, and the reply is
    `{ "id": "mem/bel/x", "blast_radius": [] }`. A null `reason` records `None`.
- [ ] **Step 2: Run** `cargo nextest run -p gecko-engine --lib --no-default-features retract`.
  Expected: FAIL.
- [ ] **Step 3: Implement the host arm.** `id` goes through `req_str` and `reason` through
  `opt_str`, so an absent or null reason is `None`.
- [ ] **Step 4: Run the same command.** Expected: PASS.
- [ ] **Step 5: Write the failing guest tests** in `wasm_tests.rs`, using the `recording` and
  `MEM_SCOPES` helpers. `MEM_SCOPES` gains `"mem:retract"`, and its length changes to match.
  - `mem_retract_forwards_id_and_reason`: `mem.retract("mem/bel/x", "r")` sends exactly
    `{ "id": "mem/bel/x", "reason": "r" }`, and `mem.retract("mem/bel/x")` sends
    `{ "id": "mem/bel/x" }`.
  - `mem_retract_forwards_a_non_string_reason_unconverted`: `mem.retract("mem/bel/x", 5)` sends
    `"reason": 5`.
  - `mem_retract_without_an_id_throws_type_error` (use `assert_type_error`).
  - `mem_retract_returns_the_reply`: a callback answering
    `{ "id": "mem/bel/x", "blast_radius": ["mem/bel/y"] }` makes the program's
    `r.blast_radius[0]` equal `"mem/bel/y"`.
  - `mem_retract_needs_its_scope`: with `MEM_SCOPES` minus `mem:retract`, the call fails with
    the not-granted error, and the callback never runs.
- [ ] **Step 6: Run** `cargo nextest run -p gecko-engine --test wasm_tests --no-default-features retract`.
  Expected: FAIL (the guest has no `mem.retract`).
- [ ] **Step 7: Implement `mem_retract`** after `mem_supersede`:
  - `id` goes through `require_str_arg(args, 0, "id")`;
  - `reason` through `opt_arg(args, 1, "reason", context)`;
  - it calls `host_call("mem", "retract", …)` and returns `JsValue::from_json`.

  Run `just build-guest`, then the wasm clippy.
- [ ] **Step 8: Run** the step 6 command, then the whole `wasm_tests` binary. Expected: PASS.
- [ ] **Step 9: The rig.** In `belief_tier_reconciliation_test.rs`:
  - `mem_playbook`'s scopes gain `mem:retract`;
  - a line after the contest runs `const r = mem.retract(b2.id, "rig retraction");`, and `r.id`
    joins the returned object;
  - the reference `MemWriter` sequence (`rig_ref`) gains the matching
    `retract(ctx, b2, Some("rig retraction"))` call.

  `b2` is still asserted. The contest does not change its state, so retracting it after the
  contest is valid. Run `memwriter_is_the_sole_belief_tier_writer` and the rest of that file.
  Expected: PASS.
- [ ] **Step 10: The end-to-end test** in `mem_recall_e2e_test.rs`:
  `a_playbook_retracts_a_root_and_learns_what_rested_on_it`. The playbook, with scopes
  `[mem:derive, mem:retract, mem:recall]`:
  1. `const root = mem.derive(<root text>, [])`;
  2. `const kid = mem.derive(<kid text>, [root.id])`;
  3. `const r = mem.retract(root.id, "wrong")`;
  4. `const after = mem.recall(<root text>, { maxChunks: 5 })`.

  It returns `{ root, kid, radius: r.blast_radius, after: after.map(c => c.id) }`. The
  assertions: `radius == [kid.id]`, and `after` does not contain `root.id`. Run it with
  `-j 1`. Expected: PASS.
- [ ] **Step 11: Run the full suite** (the command from Task 1 step 7). Expected: all pass.
- [ ] **Step 12: Commit** with `quickjs.wasm`: "mem.retract reaches playbooks".

### Task 3: Cyber proof, docs and the ledger

**Files:**
- Test: `gecko-bin/tests/cyber_detect_test.rs`.
- Modify:
  - `docs/refactor/KNOWN-LIMITATIONS.md`: `:9`, `:170-172`, and the visibility section around
    `:175-190`;
  - `README.md:140`, the `mem.*` function list;
  - the comments that enumerate the surface: `gecko-bin/src/main.rs:1268`, and `mem_playbook`'s
    doc comment in `belief_tier_reconciliation_test.rs:179`, if Task 2 did not already change
    it;
  - `docs/refactor/OWED-WORK-LEDGER.md`.
  - `extensions/README.md` and `guest-wasm.md` hold no `mem.*` function list, and are not
    edited.

**Interfaces:**
- Consumes: `MemWriter::retract` (Task 1).

- [ ] **Step 1: Write the test** `retracting_a_claim_removes_it_from_blinded_and_reopens_its_gap`.
  It follows `detect_bridge_claims_disposes_and_reports`'s setup:
  - claim `T1059`;
  - `coverage_gaps`' `undetected_techniques` is empty;
  - retract the returned `belief_id` with a `MemWriter` over the same database and the same
    `RunContext`;
  - `undetected_techniques` has length 1 again.

  For `blinded`, extend the same test or a sibling using the `blinded` rig (`:504-525`): a claim
  `blinded` lists is absent after the retract. `Rig::new` discards the claim replies, so either
  make the claim inline in the new test or have `Rig` keep the claim ids. Retract with a
  `MemWriter::without_index(RouterGraphStore…)` (the pattern at `:199`) and a fresh
  `run_ctx(LOGIC_ID)`. Its actor is `"system"`, which owns the claim, so `visible-to` passes. The test needs no production change, so it
  passes on first run. Before relying on it, confirm it would fail with the retract call
  removed: run it once that way and record the failure.
- [ ] **Step 2: Run** `cargo nextest run -p gecko-bin --test cyber_detect_test --no-default-features --features cyber -j 1 retract`.
  Expected: PASS.
- [ ] **Step 3: Docs.**
  - **`KNOWN-LIMITATIONS.md`:**
    - `:9` lists `retract`;
    - `:170-172` becomes: `supersede` and `retract` both close `valid-to`, so only a
      raw-written state flip with no `valid-to` stays in as-of reads;
    - a sentence says `retract` shares `supersede`'s window rules: it never extends an earlier
      end, it shortens a later one, and a same-run assert and retract leaves `[t, t)`;
    - a **`retract`** bullet in the visibility section names its refusal and team tests. It says
      there is no entrenchment gate, and that the blast radius lists only visible, current
      beliefs.
  - **`README.md:140` and the two comments:** add `retract`, as `mem.retract(id, reason?)` →
    `{ id, blast_radius }` where the signature is shown.
  - **The ledger:**
    - N4 moves to Landed, naming its tests and the design path;
    - "Next up" drops N4 and renumbers;
    - the "Done when" text is corrected to say a retracted claim's technique returns to
      `coverage_gaps`.
- [ ] **Step 4: Run** fmt, both clippy runs and the full suite. Expected: clean, all pass.
- [ ] **Step 5: Commit:** "docs: retract in the limitations, extension docs and ledger (N4)".

## Finish

- Final whole-branch review on Fable, with one fix pass.
- A PR against `main`, with the CI checks watched.
