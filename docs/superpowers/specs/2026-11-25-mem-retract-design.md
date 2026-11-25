# `mem.retract` withdraws a belief — design

Closes owed-work ledger item **N4**, under decision **D-A** (ledger, "Decided 2026-11-24").

## Intent

A playbook can assert, derive, supersede and contest beliefs, but it cannot withdraw one. The
only way to take back a cyber detection claim today is to supersede it with a dummy replacement,
which leaves a false belief standing in its place. `retract` withdraws a belief with nothing in
its place, records who withdrew it, in which run and why, and tells the caller what rested on it.

## What exists

- `belief-state` already has a `"retracted"` value. `is-superseded` already counts a retracted
  belief as not current, so `gate`, present-state recall and context selection drop it with no
  change.
- Cyber's `undetected-techniques` and `claim-platforms` read only `belief-state "asserted"`. A
  retracted claim therefore leaves `blinded`, and its technique returns to `coverage_gaps` (which
  lists techniques with no asserted claim) when no other claim covers it. No cyber change is
  needed.
- `tql::close_valid_to_op` closes a validity window without extending it. `believed-at` reads the
  window, so as-of recall ends a retracted belief at its retraction (D-A).
- `EpistemicReader::blast_radius` returns the closure over `rests-on` and `derivation`. It is not
  scoped to the caller.
- `MemWriter::best_effort_remove` drops a vector from the semantic index, as `supersede` does.

## Approaches

The open question was where the reason goes. The schema has nowhere to put it.

- **A `retraction-reason` attribute on the belief.** This is the smallest change. It loses who
  retracted the belief and in which run. Another agent can retract a team or shared belief, so the
  owner is not the retractor. Rejected.
- **No reason.** This drops the trail entirely, and the ledger specifies a reason. Rejected.
- **Chosen by the user: a reified `retraction` relation** linking the belief, the retracting agent
  and the run, with the reason and `created-at`. It mirrors `supersession`. `supersede` leaves its
  actor and run trail through the new belief's `ownership` and `source-link`. `retract` mints no
  belief, so the relation carries that trail itself (invariant 2).

## Goals

1. **Schema** (`core/mem-gecko/schema/mem_types.tql`), additive under `define`:
   - `attribute retraction-reason value string;`
   - `relation retraction`, each with its own `@doc`:
     - `relates retracted @card(1)`, played by `belief`;
     - `relates retractor @card(1)`, played by `agent`;
     - `relates run @card(1)`, played by `doc-run`;
     - `owns retraction-reason @card(0..1)`;
     - `owns created-at @card(1)`.
   - `belief plays retraction:retracted`, `agent plays retraction:retractor` and
     `doc-run plays retraction:run`. A belief is retracted at most once, because only an asserted
     belief can be retracted and the retraction flips its state. No `@card` on the play is needed.
   - No migration: no existing database holds a retraction.
2. **Writer.** `EpistemicWriter::retract(&self, ctx: &RunContext, id: MemId, reason: Option<&str>) -> Result<Vec<MemId>>`.
   - The returned list is the blast radius, as defined in goal 3.
   - **One batch, atomic** (`tql::retraction_ops`):
     1. `ensure_run(ctx)` and `ensure_agent(ctx)`. `supersede` gets these from
        `insert_belief_ops`; `retract` has no insert of its own.
     2. **The link op**, `expecting(1)`. It matches a belief with `belief-state "asserted"` and
        `true == visible-to($b, $ag)`, plus the run and the agent. It inserts
        `retraction (retracted: $b, retractor: $ag, run: $run)` with `created-at == ctx.occurred_at`
        and, when `reason` is `Some`, `retraction-reason`.
     3. **The state flip** to `"retracted"`, `expecting(1)`.
     4. `close_valid_to_op(id, ctx.occurred_at)`.
   - **A refusal commits nothing.** A missing, superseded, already retracted or out-of-scope belief
     matches no row on the link op, and the batch rolls back. The error names the id and says
     "not a current belief visible to the caller; nothing was committed", as `supersede`'s does.
     It never reveals whether the belief exists.
   - **No entrenchment gate.** `supersede` compares two tiers; `retract` has no replacement to
     compare. Visibility plus the `mem:retract` scope is the gate.
   - **No cascade.** Dependents are reported, not retracted.
   - **After the commit:** best-effort removal of the belief's vector (`best_effort_remove`), then
     the blast-radius read. That read runs only after a successful commit, so a refused retract
     never reveals dependents. If the read fails, the error says the retraction committed and the
     blast-radius read failed, so the caller does not retry a retraction that landed.
3. **The blast radius returned to the caller** (`tql::visible_blast_radius_read(id, actor)`):
   - It holds the beliefs in `blast-radius($r)` that are `visible-to` the caller and still
     current (`not is-superseded`).
   - Only beliefs are reported. `visible-to` takes a `belief`, so the query narrows `$m isa belief`.
     A retracted belief's dependents are beliefs in practice: `derive` mints beliefs, and
     `rests-on` has no production writer (M1).
   - Superseded or retracted dependents are left out, because they are not undermined any more.
   - The ids are sorted ascending, so the reply is deterministic.
   - `EpistemicReader::blast_radius` is unchanged. It answers a structural question and has no
     production caller.
4. **Host bridge** (`core/gecko-engine/src/sandbox/mem_host.rs`):
   - `MemHostFn::Retract`, name `"retract"`, scope `mem:retract`, allow-list `id`, `reason`.
   - `id` is required (`req_str`), and `reason` is optional (`opt_str`). An absent or null reason is
     `None`, and a non-string reason is refused.
   - The reply is `{ "id": <retracted id>, "blast_radius": [<ids>] }`.
5. **Guest** (`core/js-sandbox/src/main.rs`), with its own rebuild of `quickjs.wasm`:
   - `mem.retract(id, reason?)` sends `{ id, reason? }`, with `reason` forwarded unconverted through
     `opt_arg`. It returns the host's reply as a JS object: `{ id, blast_radius }`.
   - `id` goes through `require_str_arg`, as `supersede`'s `old` does.
6. **Docs:**
   - `KNOWN-LIMITATIONS.md`, the visibility section: a `retract` bullet beside `supersede`'s, with
     its tests. It also notes that there is no entrenchment gate, and that the blast radius lists
     only visible, current beliefs.
   - `extensions/README.md` and `docs/refactor/guest-wasm.md`, wherever the `mem.*` functions are
     listed.
   - `CLAUDE.md`'s `mem.*` mention, only if it lists the functions.
   - `KNOWN-LIMITATIONS.md` also notes that `occurred_at` is fixed once per run, so a belief
     asserted and retracted in one run gets the empty window `[t, t)` and no as-of read returns
     it, as with a same-run supersede.
   - The ledger: N4 moves to Landed, and its "Done when" is corrected to "its technique returns
     to `coverage_gaps`". M1's trigger ("N4 lands and a playbook wants…") stays, since
     the second half still holds.

## Non-goals

- Cascading a retraction to its dependents.
- Retracting a superseded or contested belief. Nothing sets `"contested"` today.
- Un-retracting.
- Changing `supersede`'s absent-reason default of `""`. `retract` writes no attribute for an
  absent reason, which is the stricter shape; aligning `supersede` is a separate change.
- A `believed-at` change. No pre-N4 database holds a retraction, and `retract` always closes
  `valid-to`.
- Reporting non-belief dependents (episodes, anomalies).

## Constraints

- All TQL is parameterized.
- Comments are greenfield, and every new type and role carries a one-sentence `@doc`.
- The guest is untrusted, and the host is the single authority on argument values.
- Never fabricate success over bad input.
- After editing `core/js-sandbox/src/main.rs`, run `just build-guest` and commit `quickjs.wasm`.
- fmt, both clippy runs (with and without `cyber`) and the `js-sandbox` wasm clippy are clean.
- Integration tests run with `-j 1` and never touch the database named `gecko`.
- Ratified and historical docs are not edited, and frozen fixtures keep their text.

## Verification

- **Live writer tests**, in `core/mem-gecko/tests/writer_reader_integration.rs`:
  - `retract_flips_state_closes_the_window_and_records_who_and_why`: state `"retracted"`;
    `valid-to == occurred_at`; one `retraction` edge to the caller's agent and run, with the reason
    and `created-at`.
  - `retract_without_a_reason_writes_no_reason`.
  - `a_retracted_belief_leaves_present_state_recall_and_gate`.
  - `as_of_recall_ends_a_retracted_belief_at_its_retraction`: as of before the retraction it is
    returned; at and after the retraction it is not.
  - `retracting_another_agents_private_belief_is_refused_and_commits_nothing`.
  - `retracting_a_team_belief_of_another_agent_is_allowed`, recording the other agent as retractor.
  - `retracting_a_superseded_belief_is_refused`.
  - `retracting_twice_is_refused`.
  - `retracting_a_missing_belief_is_refused`.
  - `retract_reports_the_visible_current_beliefs_that_rested_on_it`: a derived dependent and a
    transitive one are listed, sorted. Another agent's private dependent and a superseded
    dependent are not.
  - `retract_removes_the_vector_from_the_index`, in `semantic_index_integration.rs`.
- **Cyber**, in `gecko-bin/tests/cyber_detect_test.rs`: a claim `blinded` reports is gone from
  it after `retract`, and its technique, absent from `coverage_gaps` while claimed, is listed there
  again.
- **Host**, in `mem_host.rs` unit tests: an unknown key is refused (the existing loop gains
  `Retract`); `id` is required; a non-string `reason` is refused; the reply carries the id and the
  blast radius.
- **Guest**, in `wasm_tests.rs`: `mem.retract` forwards `{ id, reason }`, needs the `mem:retract`
  scope, and returns `{ id, blast_radius }`. A missing `id` throws `TypeError`.
- **End to end**, in `gecko-bin/tests/mem_recall_e2e_test.rs` or a sibling: a playbook derives an
  evidence-less root belief, derives a second belief from it, then retracts the root. The reply
  names the second belief, and a later recall does not surface the root. (`mem.remember` mints an
  episode, which `retract` does not take.)
- **The belief-tier census.** `retraction` joins the belief-tier surface automatically
  (`gecko_engine::db::surface`: relations whose roles `memory-item` subtypes play).
  - `memwriter_is_the_sole_belief_tier_writer` (`belief_tier_reconciliation_test.rs`) requires
    each label to be reached by a production step. The rig's mem playbook gains a real
    `mem.retract` step with its reference `MemWriter::retract` call, not a `DISPOSITIONS` entry,
    which would be false now that retract has a production path. That step needs the rebuilt
    `quickjs.wasm`, so the guest lands before the rig change.
  - The label lists in `belief_tier_surface_test.rs`, `belief_tier_monopoly_test.rs` and
    `no_drag_test.rs` gain `retraction` and `retraction-reason`.
- Schema guards: re-apply plans nothing, and the shipped-schema label tests pass with the new
  labels.
- The full candle-free suite passes.

## Settled after the builder check

- `coverage_gaps` lists techniques with no asserted claim, so a retraction puts the technique
  back on it; the ledger's "leaves `coverage_gaps`" was backwards.
- `retract` takes beliefs only. The end-to-end root is an evidence-less `mem.derive` belief.
- The belief-tier rig gains a real `mem.retract` step.

## Left to the builder

- The exact `@doc` sentences.
- Whether `retraction_ops` shares a helper with `supersession_ops` for the refusal message.
- Where the e2e test lives.
