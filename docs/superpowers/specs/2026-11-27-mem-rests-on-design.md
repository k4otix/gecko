# `mem.restsOn` — a playbook declares what a belief assumes — design

Closes the `rests-on` half of owed-work ledger item **M1**. `pivot` stays deferred as a
smaller row.

## Intent

A playbook can say "this belief rests on that one". When the assumption is later withdrawn
with `mem.retract`, the reply's `blast_radius` names what depended on it. This turns the
substrate's assumption graph from a schema type nobody writes into a working tool.

In the cyber worked example, an analyst's verdict on an alert rests on the detection claim
it judged. Withdrawing that claim reports the verdict as undermined.

## What exists today

- **The schema has it.** `mem_types.tql:149` defines `relation rests-on` with
  `relates resting @card(1)` and `relates assumption @card(1..)`, and nothing else.
- **`blast-radius` follows it.** The function (`mem_functions.tql:68`) is the closure over
  `rests-on` ∪ `derivation`. `mem.retract`'s reply already lists the visible, current
  beliefs in it (`tql::visible_blast_radius_read`).
- **The writer ignores the caller.** `MemWriter::rests_on`
  (`core/mem-gecko/src/writer/beliefs.rs:345`) takes a `RunContext` it never reads, checks
  no visibility, and writes one unstamped edge (`tql::rests_on_op`, `tql.rs:546`).
- **Nothing reaches it from the guest.** Its only callers are tests.
  `docs/refactor/KNOWN-LIMITATIONS.md:229` says so.

## Goals

- `mem.restsOn(resting, assumptions)` in the guest, behind a `mem:rests_on` scope.
- The write is gated on what the caller can see, in both directions. A playbook cannot hang
  its belief on another agent's private belief, or attach another agent's private belief
  to its own assumptions.
- Every `rests-on` edge records who declared it, in which run, and when (invariant 2).
- The cyber worked example shows a non-empty `blast_radius`.

## Non-goals

- `pivot`. It has no writer, no caller, and no defined meaning for `calibration-score`. It
  stays deferred.
- Retracting dependents. `retract` reports its blast radius and never cascades; that is
  unchanged.
- Making a repeated declaration idempotent. Each call records one edge, with its own stamp.

## Anti-goals

- **A write that reveals what it refuses.** A `rests-on` naming another agent's private
  belief is refused with the same message as a missing id, so the refusal does not confirm
  that the belief exists.
- **Silently fixing bad input.** A duplicate assumption, a self-dependency or an empty
  `assumptions` list is refused, never de-duplicated or dropped.

## Approaches

Two decisions, both made with the user.

- **Scope: `rests-on` only.** It already has a writer, a schema, a closure and a consumer
  (`retract`'s `blast_radius`). Adding `pivot` was rejected: it would mean inventing what
  `calibration-score` means, with no consumer to check it against.
- **Provenance: stamp the edge like `retraction`.** Leaving it unstamped was rejected:
  "who declared this dependency, and when" is exactly the question a surprising blast
  radius raises. Doing it now, before any playbook has written an edge, means no live data
  ever needs migrating.

## Design

### Schema (`core/mem-gecko/schema/mem_types.tql`)

`rests-on` gains three things:

```
relation rests-on @doc(<unchanged>),
    relates resting @card(1) @doc(<unchanged>),
    relates assumption @card(1..) @doc(<unchanged>),
    relates declarer @card(1) @doc("The agent that declared the dependency."),
    relates run @card(1) @doc("The run during which the dependency was declared."),
    owns created-at @card(1);
```

- `agent` plays `rests-on:declarer`, and `doc-run` plays `rests-on:run`, beside their
  `retraction` lines (`mem_types.tql:138`, `:171`).
- The relation's own `@doc` and the existing roles' `@doc`s don't change, so the edit is
  additive on a database with no `rests-on` rows.
- **An unstamped `rests-on` row fails the upgrade (decided with the user).** There is no
  `SchemaMigration`.
  - Deleting old rows would destroy real edges, and backfilling a declarer would invent
    provenance against invariant 2.
  - In practice no such row exists: the writer was never reachable from a playbook.
  - This follows the existing rule in `KNOWN-LIMITATIONS.md` ("Schema constraints and
    upgrades"): other violating rows fail the upgrade and need a manual cleanup. That
    section gains a line naming `rests-on`.
  - `tightened_schema_applies_to_a_populated_database`
    (`gecko-bin/tests/schema_init_test.rs:1055`) keeps exercising the populated happy path.
    Its population drops the `rests_on` call, which can't write to the pre-tightening schema.
  - A separate test, `an_unstamped_rests_on_fails_the_upgrade`, in the same file:
    - writes a bare `rests-on` with raw TQL on the pre-tightening schema;
    - asserts that applying the shipped schema fails with an error naming `rests-on`.
  - `previous_release_upgrades_to_the_shipped_schema` holds no rows and is unaffected.
- The frozen fixtures under `*/tests/fixtures/` describe earlier releases and are not
  edited.
- The `@card(1)` roles mean a bare `rests-on` insert is refused at commit. Tests that insert
  one by hand change to supply the stamp, or go through the writer:
  - `schema_acceptance_test.rs:132`;
  - `closure_functions_test.rs:134`;
  - `gecko-bin/tests/belief_tier_surface_test.rs:222`, whose `Links 0→2` expectation at `:255`
    may move;
  - `gecko-bin/tests/schema_probe_w3.rs:237`.

  `schema_acceptance_test` gets a case pinning the refusal. Its
  `a_rests_on_missing_its_resting_is_refused_at_commit` (`:623`) keeps the other roles stamped,
  so it is still refused for the one reason it names.

### Writer (`core/mem-gecko`)

- **`tql::rests_on_ops(ctx, resting, assumptions) -> Vec<GraphWrite>`** replaces
  `rests_on_op`. It returns `[ensure_run(ctx), ensure_agent(ctx), link]`, the same shape as
  `retraction_ops` and `contest_ops`.
  - The link matches `$run` and `$ag` from `ctx`, and binds `resting` and every assumption
    as `memory-item`s. Each one must pass the visibility predicate.
  - It inserts
    `rests-on (resting: $r, assumption: …, declarer: $ag, run: $run), has created-at == $ca`,
    with `$ca` set to `ctx.occurred_at`, and is `expecting(1)`.
- **The visibility predicate** is the derivation evidence rule (`tql.rs:367-374`): a belief
  passes through `visible-to`, and any other memory item through its `ownership` (team or
  shared visibility, or owned by the caller). It moves into a shared helper,
  `fn visible_item_clause(var: &str, i: usize) -> String` in `tql.rs`. `derivation_op` and
  `rests_on_ops` both use it, so the rule lives once.
- **`MemWriter::rests_on`** validates before writing:
  - `assumptions` is non-empty: "rests_on requires at least one assumption" (unchanged);
  - no assumption repeats: "rests_on names assumption <id> more than once", as `contest`
    words it;
  - `resting` is not among `assumptions`: "rests_on: <id> cannot rest on itself".
- **On a refused write**, `name_missing_ids` is keyed on `Some(last)`, the link op, with the
  noun "memory item visible to the caller". A missing id and an invisible one get the same
  message.
- **The trait doc** on `EpistemicWriter::rests_on` (`epistemic.rs:221`) says that every id
  must be visible to `ctx.actor`, and that the edge is stamped with the actor, the run and
  `occurred_at`.

### Host bridge (`core/gecko-engine/src/sandbox/mem_host.rs`)

- `MemHostFn::RestsOn`, named `"rests_on"`, joins `from_name` and the dispatch. Its
  accepted keys are `["resting", "assumptions"]`.
- `resting` goes through `req_str`, and `assumptions` through `required_mem_ids`, so a
  missing field, a non-array, or a non-string entry is refused naming the field. An empty
  array reaches the writer, which refuses it.
- An empty string id is refused by the host as "mem call field '<key>' must be a non-empty
  id". Neither `req_str` nor `required_mem_ids` refuses `""` today, so the check is added for
  `rests_on` only, and the ledger gets a note that the other `mem.*` functions accept `""`.
- The reply is `{ "resting": <id>, "assumptions": [<ids>] }`, echoing what was written.
- The S3 scope is `mem:rests_on`, checked before anything else, as for every `mem.*` call.

### Guest (`core/js-sandbox/src/main.rs`)

- `mem.restsOn(resting, assumptions)` builds `{ resting, assumptions }` the way
  `mem_contest` and `mem_retract` do: `require_str_arg` for `resting`, and `opt_arg` for
  `assumptions`, so a missing one is sent as missing and the host refuses it.
- It is registered on the `mem` object beside `retract`.
- Then `just build-guest`, and the regenerated `quickjs.wasm` goes in the same commit.

### Cyber worked example

- **`playbooks/record_verdict.md`:**
  - its scopes become `[cyber-gecko:disposition, mem:rests_on]`;
  - it takes an optional `assumes`. **When present, the playbook checks it before recording
    anything (decided with the user):** it must be a non-empty array of unique, non-empty
    strings, or the playbook throws naming the problem.
    - An explicit `[]` is refused, never dropped.
    - Only then does the playbook call `disposition`, followed by
      `mem.restsOn(verdict.belief_id, assumes)`.
    - Making both writes one transaction, by having `disposition` take `assumes`, was
      rejected. It would couple cyber to `rests-on`, and sequential playbook calls are never
      one transaction anyway.
  - **The one case the check can't catch:** an `assumes` id that is missing or invisible to
    the caller is refused by `mem.restsOn` after the verdict is recorded. The run fails, but
    the verdict stands. The playbook's prose says so;
  - its reply adds `assumes` when given;
  - its prose explains that `assumes` names the beliefs the verdict takes for granted,
    usually the detection claim.
- **`extensions/cyber-gecko/README.md`, the worked example:**
  - Step 6 passes `"assumes":["<powershell belief_id>"]`, the first `belief_id` from step 4.
  - Steps 7 and 8 are unchanged. Step 8 still withdraws the scheduled-task claim, with
    `blast_radius: []`, and nothing is blind afterwards.
  - A new **step 9** withdraws the PowerShell claim. Its `blast_radius` lists the verdict's
    `belief_id`, and `T1059.001` is undetected again: 706, measured on ATT&CK 20.1.
  - The README also says that `evidence` (a derivation) also places a verdict in a blast
    radius, and that `assumes` (`rests-on`) differs in two ways: it links two beliefs that
    both already exist, and it means "assumes", not "was derived from".
- **`gecko-bin/tests/cyber_sample_test.rs`:**
  - it substitutes the PowerShell `belief_id` into steps 6 and 9, as it already does for
    step 8;
  - it asserts that step 6's reply carries `assumes`;
  - it asserts that step 9's `blast_radius` is exactly `[<verdict belief_id>]`;
  - its README-shape guard accepts the new placeholder.

### Docs

- **`KNOWN-LIMITATIONS.md`, "Writes act only on what the caller can see":** a `rests_on`
  bullet joins the gated list, naming its tests, and `rests_on` leaves the "not reachable
  from the guest" line.
- **`docs/refactor/guest-wasm.md`** isn't touched; it doesn't list the `mem.*` functions.
- **`CLAUDE.md`** lists the guest surface ("the `mem.*` namespace") without naming
  functions, so it needs no edit.
- **The ledger:** M1 is replaced by a `pivot`-only deferred row with the same trigger, and a
  Landed row names the tests.

## Tests

- **Writer** (`core/mem-gecko/tests/writer_reader_integration.rs`):
  - `rests_on_stamps_the_declarer_the_run_and_the_time`: the edge reads back with the actor,
    the run id and `occurred_at`.
  - `rests_on_another_agents_private_belief_is_refused_and_commits_nothing`: once with
    another agent's private belief as the assumption, and once as the resting item. The
    message matches the missing-id one, and no edge exists afterwards.
  - `rests_on_a_team_belief_of_another_agent_is_allowed`.
  - `rests_on_refuses_a_repeated_assumption` and `rests_on_refuses_a_self_dependency`,
    both with nothing committed.
  - `rests_on_with_a_missing_assumption_errors_and_commits_nothing` stays, with its message
    updated.
  - The existing `blast_radius_*` tests pass unchanged.
- **Schema:** a bare `rests-on` insert, with no declarer, run or `created-at`, is refused at
  commit (`schema_acceptance_test`).
- **Host bridge** (`mem_host.rs` unit tests): a missing `resting` or `assumptions`, a
  non-array, a non-string entry and an empty id are each refused naming the field. The
  reply echoes both fields, and an unknown extra key is refused.
- **Guest** (`core/gecko-engine/tests/wasm_tests.rs`), with a stub writer:
  - `mem.restsOn("a", ["b"])` reaches the writer with those ids;
  - an ungranted call is refused by S3 before the writer runs;
  - `mem.restsOn("a")` is refused by the host.
- **End to end:** `a_playbook_declares_an_assumption_and_learns_what_rested_on_it`, in
  `gecko-bin/tests/mem_recall_e2e_test.rs`. A playbook declares a dependency, then retracts
  the assumption, and the reply's `blast_radius` lists the resting belief.
  - It is a sibling of `a_playbook_retracts_a_root_and_learns_what_rested_on_it` (`:411`).
  - That test reaches its blast radius through `mem.derive` and stays as it is.
- **Cyber walkthrough:** `the_readme_walkthrough_runs`, updated as above.
- **`record_verdict`'s check:** `record_verdict_refuses_a_bad_assumes_before_recording`, in
  `gecko-bin/tests/cyber_sample_test.rs`. Each of `[]`, `["a", "a"]`, `[""]` and `"x"`:
  - fails the run (non-zero exit);
  - leaves no verdict recorded: `detection_precision` afterwards counts 0 true positives.
- **Rig:** the belief-tier reconciliation rig enumerates the guest-reachable writes.
  - `rig_mem` gains a `mem.restsOn` step, with its scope. The step-5 reference writes mirror
    it.
  - The `rests-on` `WriterOnly` entry in `DISPOSITIONS`
    (`gecko-bin/tests/belief_tier_reconciliation_test.rs:503`) and its `rests_on` drive at
    `:846` are deleted.
  - `pivot` stays `Owed`.

## Calls made in the design

- **Visibility only, as `derive` does.** `rests_on` accepts any visible memory item, as
  either `resting` or an assumption, whatever its belief state. It doesn't also require
  `asserted`, as `retract` and `supersede` do.
  - Declaring that a belief rested on an assumption that has since been superseded is
    legitimate history.
  - `blast_radius` already reports only current beliefs.

## Left to the builder

- The exact text of the TQL, within the shape above.
- Whether `visible_item_clause` takes the actor variable name or assumes `$ag`.
- Test placement inside the named files.
