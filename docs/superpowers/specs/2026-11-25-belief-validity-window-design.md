# A supersede closes the old belief's validity window — design

Closes owed-work ledger item **B1**, under decision **D-A** (ledger, "Decided 2026-11-24"), and
absorbs B2.

## Intent

As-of recall (`mem.recall(q, { asOf })`, `EpistemicReader::believed_at`) answers "what did the
caller believe at T". Today `believed-at($t)` (`mem_functions.tql:20`) matches only beliefs whose
`belief-state` is `"asserted"` *now*. So a belief superseded after T disappears from every as-of
read, and its successor, whose `valid-from` is after T, does not appear either. As-of recall cannot
see history across a supersession. The failing test `believed_at_reads_across_a_supersede` shows it:
as of a time before the supersede, the read returns nothing.

D-A settled the model. A belief's validity ends by closing `valid-to`: `supersede` sets it now, and
`retract` will set it when N4 lands. `believed-at` reads the half-open window `[valid-from, valid-to)`
and drops its state gate, the same window `canonical-entity-at` already reads.

## Approaches

- **End the window only at the supersession edge's `created-at`.** Rejected by D-A: a retraction
  has no supersession edge.
- **Window only.** `supersede` writes `valid-to`, and `believed-at` reads only the window.
  Rejected by the user: on a database written before this change, as-of recall would return a
  superseded belief next to its successor at every time after the supersession, permanently.
- **Chosen: the window, plus the supersession edge.** `valid-to` is the model, as D-A says.
  `believed-at` also excludes a belief whose `supersession` edge has `created-at <= $t`.
  - For new data the clause is redundant: the writer stamps both `created-at` and `valid-to` with
    `ctx.occurred_at`.
  - On older databases it gives the right answer with no backfill.
  - A retraction still ends the window through `valid-to` alone.

## Goals

1. **`supersede` closes the old belief's window.**
   - In `tql::supersession_ops` (`core/mem-gecko/src/tql.rs:393`), the old belief gets
     `valid-to = created_at`. That is the function's existing parameter, which the writer passes
     as `ctx.occurred_at` (`writer/beliefs.rs:250-256`). It is the instant the successor's
     `valid-from` opens at (`tql.rs:299`), so the successor's window is `[t, ∞)`.
   - The op is `match $o isa belief, has concept-id $oc; $oc == $oid; not { $o has valid-to $vt; $vt <= $t; }; update $o has valid-to == $t;`.
     It is the repo's first parameterized `update`, so **the plan probes this syntax first** on
     live TypeDB 3.15.
   - The write is one more op in the same batch, so it is atomic with the link and the state flip.
   - It never extends a window. A belief whose `valid-to` is already at or before `t` keeps it. A
     `valid-to` later than `t` is shortened to `t`: `valid-to @card(0..1)` lets `update` replace
     it.
   - The op carries no row expectation (`graph_store.rs:125`), so matching nothing commits nothing
     extra. A refused supersede (the link op's `expecting(1)`) rolls back the whole batch.
   - Because the windows are half-open, a belief asserted and superseded at the same `occurred_at`
     gets the empty window `[t, t)`: no as-of read returns it.
2. **`believed-at` reads only the window.**
   - The body becomes:
     ```
     $b isa belief, has valid-from $vf; $vf <= $t;
     not { $b has valid-to $vt; $vt <= $t; };
     not { supersession (superseded: $b), has created-at $ca; $ca <= $t; };
     ```
   - Its `@doc` says it returns the beliefs whose half-open window `[valid-from, valid-to)` contains
     `$t` and that were not superseded at or before `$t`, and that `belief-state` is not read.
   - A changed function body reaches existing databases through the normal `redefine`, as in
     `a_changed_function_doc_reaches_an_existing_database`.
   - Stale text is fixed:
     - the comment over `canonical-entity-at` that says "believed-at does read belief-state";
     - the `believed-at` comment (`mem_functions.tql:19`);
     - `tql.rs:804` ("Supersession is not applied as of `at`");
     - the `supersession_ops` doc comment (`tql.rs:390`).
   - **Resolutions follow.** `resolution sub belief`, so a superseded resolution now leaves
     `canonical-step-at` from its supersede on. That is the window that function already reads,
     and it is correct.
3. **Present-state reads are unchanged.**
   - `gate`, `is-superseded` and recall without `asOf` still exclude superseded beliefs through
     `is-superseded`.
   - `gate` also already excludes a closed window, so a supersede is now excluded by both checks.
4. **Docs.** `KNOWN-LIMITATIONS.md`'s "As-of recall and supersede" section says:
   - as-of recall at T returns the beliefs whose window contains T, across supersessions;
   - **a database written before this change** holds superseded beliefs with no `valid-to`. The
     supersession clause still ends them at the supersede, so as-of recall answers correctly
     there too. Only a belief retracted outside `supersede` (nothing does today) would need
     `valid-to`.
   - The new `believed-at` reaches a database only when `gecko schema-init` (or `sync`) re-applies
     the schema. Until then the writer's `valid-to` lands, and the old state-gated function keeps
     answering.

## Decided with the user (2026-11-25)

- `believed-at` also excludes beliefs superseded at or before `$t`, so older databases answer
  correctly without a backfill. This composes with D-A.

## Non-goals

- `retract` (N4) sets `valid-to` when it lands; nothing retracts today.
- No backfill migration. `check_migrations` admits a range update, not a join-driven one, and D-A
  accepts the limitation.
- Refusing a backdated supersede, where `occurred_at` falls before the old belief's `valid-from`.
  The inverted window matches no as-of read, which is correct for a belief that never held.

## Constraints

- All TQL is parameterized.
- Comments are greenfield.
- Every `@doc` is accurate.
- fmt and both clippy runs are clean.
- Integration tests run with `-j 1` and never touch the database named `gecko`.
- Ratified docs are not edited.

## Verification

- **New live tests** in `writer_reader_integration.rs`:
  - `believed_at_reads_across_a_supersede`: before the supersede, only the old belief; after it, and
    at its instant, only the successor. It fails today.
  - `a_same_instant_supersede_leaves_the_old_belief_an_empty_window`. It passes today only because
    of the state gate, and it pins that the gate's removal does not expose the old belief.
  - `as_of_recall_reads_across_a_supersede`: the same through `recall(… as_of …)`, the `mem.recall`
    path.
  - `a_supersede_keeps_an_earlier_valid_to`: a belief whose window already closed before the
    supersede keeps its earlier end.
  - `a_supersede_shortens_a_later_valid_to`.
  - `a_refused_supersede_leaves_valid_to_untouched`.
  - `a_backdated_supersede_is_returned_by_no_as_of_read`, which pins the non-goal.
  - `a_legacy_superseded_belief_without_valid_to_ends_at_its_supersession`. It supersedes through
    the writer, then deletes the old belief's `valid-to` with a raw write to mimic an older
    database. As-of reads after the supersession return only the successor.
  - The pre-tightening and `95ebac1` upgrade tests confirm that `believed-at` redefines on an
    existing database.
- **Existing tests pass:** the `believed_at_*` tests, the `as_of_recall_*` tests, the gate tests,
  and the schema upgrade and re-apply tests (`believed-at` redefines once, then re-apply plans
  nothing).
- The full candle-free suite passes.

## Left to the builder

- The exact `@doc` sentence for `believed-at`.
