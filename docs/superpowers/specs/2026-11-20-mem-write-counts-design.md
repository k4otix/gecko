# Mem write counts and the daemon as a counted caller (ledger A11 1–3, C8) — short design

Approved by the user 2026-11-20, after a review against the code.

## Intent

Four mem writes match the records they connect by ID, then insert. When an ID
matches nothing the insert writes nothing and the call reports success:
`mem.derive` stores a belief with no derivation link to its evidence,
`mem.contest` returns the ID of an anomaly that was never written, and
`rests_on` / `record_prediction` silently do nothing. Separately, CONTRACT-W8 §3
row 5 reserves `daemon_is_enumerated_by_the_monopoly_test`: a consolidation
daemon tick's writes must be counted by the belief-tier reconciliation rig, not
carved out.

**Goals.** Each of the four writes fails, commits nothing, and names the IDs it
was given when an ID matches nothing. The sandbox bridge stops turning a
malformed ID list into an empty one. A daemon tick that really writes is
reconciled by the rig.

**Non-goals.** `resolve_prediction_ops` (A11 entry 4) waits on a uniqueness
decision for predictions. The guest's own leniency in `core/js-sandbox` (it
drops non-array `evidence` and turns non-array `claims` into `[]`) needs a
`quickjs.wasm` rebuild and is ledgered, not fixed. Other unchecked
match-then-insert writes (`retrieval_event_ops`, `informs_synthesis_op`,
`set_was_used_op`, `set_retrieval_provenance_op`, `tombstone_episode_op`) are
ledgered under A11 as deferred.

**Anti-goals.** No partial commit: a failed derive must not leave its belief.
No vacuous daemon test: a tick that writes nothing proves nothing.

## Approaches

- **Row-count check on each op (chosen).** `GraphWrite.expecting(1)`, the check
  `supersession_ops` already uses; a mismatch rolls back the whole transaction
  before commit. `concept-id` is a key, so each conjunctive match yields 0 or 1
  rows and 1 is the only correct count.
- *A read-before-write existence check* — rejected: a second transaction, racy,
  and duplicates what the count already proves atomically.

## Design

1. **Counts.** `.expecting(1)` on `derivation_op`, the last op of `contest_ops`,
   `rests_on_op`, and `record_prediction_op` (`core/mem-gecko/src/tql.rs`).
2. **Errors.** Each caller rewrites a `RowCountMismatch` on its op into
   `EpistemicError::Storage` that lists the IDs it was given and ends
   `nothing was committed`, the way `supersede` does. The failing op is found by
   index, so other mismatches pass through unchanged.
   - Derive has two callers, `assert_belief` and `assert_belief_with_edges`
     (`writer/edges.rs`), both through `MemWriter::belief_ops`; `belief_ops`
     reports the derivation op's index and one shared mapper serves both. On the
     edge path it runs before `name_failing_edge`.
   - `contest` and `record_prediction` match `isa belief`, so their messages say
     "no belief"; derive and `rests_on` match `memory-item` and say "no memory
     item".
3. **Duplicate claims.** `contest` refuses a claim list with a repeated ID
   (`InvalidInput`), since `[a, a]` passes the two-claim minimum and the count.
4. **Sandbox bridge.** `mem_ids` (`core/gecko-engine/src/sandbox/mem_host.rs`)
   returns an error when the key is present but not an array, or an entry is
   not a string. An absent key stays an empty list.
5. **C8.** The rig (`gecko-bin/tests/belief_tier_reconciliation_test.rs`) gains
   a test `ConsolidationDaemon` whose `tick` dedups a duplicate episode pair
   through the real `MemWriter::dedup_episodes`, driven as its own drive. Its
   delta must be non-empty and pass the rig's existing per-drive checks; a
   `NoopConsolidationDaemon` tick's delta must be empty. The C8 name is that
   drive's test: `memwriter_is_the_sole_belief_tier_writer` keeps the rig and the
   new drive lives inside it, with the reserved name recorded in the ledger as
   where it landed (the rig is one test by construction — it shares one
   reference database across drives).
6. **Docs.** The ledger marks A11 1–3 and C8 done and lists the deferred
   writes and the guest leniency; KNOWN-LIMITATIONS' daemon section says a
   daemon tick is reconciled by the rig; the PR states the behaviour change
   for playbooks.

## Left to the builder

Exact message wording beyond the pinned parts, the shape `belief_ops` uses to
report the index, and the test daemon's fields.
