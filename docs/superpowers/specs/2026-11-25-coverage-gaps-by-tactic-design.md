# `coverage_gaps` grouped by tactic — design

Closes owed-work ledger item **S6**. This is a small change, so the design is short.

## Intent

`coverage_gaps` lists the live ATT&CK techniques with no current detection claim, as one flat
list. An analyst reads coverage by adversary goal, ATT&CK's tactics ("how much of Persistence do
we see?"). S3 imported tactics and their `technique-tactic` edges, and nothing reads them yet.

## Approaches

- **Replace the flat list with a grouped one.** Rejected: it breaks every current caller and
  test, and it repeats a technique's detail under each tactic it serves.
- **Chosen with the user: additive.** `undetected_techniques` keeps its rows, and each row gains
  `tactics`. A new `by_tactic` gives each live tactic's coverage.

## The report

```
undetected_techniques: [ { concept_id, attack_id, tactics: ["TA0002", …], sub_techniques? } ]
by_tactic: [ { attack_id: "TA0002", concept_id, shortname: "execution", name: "Execution",
               live: 3, undetected: 2, techniques: ["T1059", "T1059.001"] } ]
```

- **`live`** is the number of live techniques serving the tactic. Parents and sub-techniques
  count individually, because each carries its own `technique-tactic` edges.
- **`undetected`** is how many of those are in the gap list. `techniques` holds their ATT&CK
  ids.
- **Liveness** uses `live(..)`: no `retired-at`, no `revoked true`, no `deprecated true`. It
  applies to both the technique and the tactic in the `technique-tactic` read. A retired
  technique keeps its edges, but it counts in no tactic's `live`, appears in no `techniques` list,
  and has no gap row. A dead tactic is absent from `by_tactic` and from every `tactics` list.
- **Which tactics appear.** `by_tactic` is built from the live join, so:
  - a fully covered tactic stays, with `undetected: 0` and `techniques: []`;
  - a live tactic with no live technique is absent, because it says nothing about coverage.
- **Ordering.** `by_tactic` is sorted by tactic `attack_id`. Each `techniques` list is sorted by
  technique `attack_id`, and each `tactics` list by tactic `attack_id`, deduplicated. ATT&CK's
  matrix order is not imported.
- **A technique with no live tactic** gets `tactics: []`.
- **Optional attributes.** A tactic's `title` is optional (`okf-concept`), and it is read through
  `fetch`, so a tactic without one is still grouped, with `name: null`. Nothing is substituted
  for it. The grouping key is the tactic's `attack_id`, which S3's import sets on every tactic.
  A live tactic without one makes `coverage_gaps` fail with an error naming its concept id,
  rather than being dropped silently or keyed by an invented value.
- **A technique with no `attack-id`.** The gap read already tolerates one, since `attack-id` is
  `@card(0..1)`. Counts are keyed by concept id, so it counts in `live` and `undetected`. In a
  `techniques` list it appears by its concept id, which is a real identifier, not an invented one.
- **Reads.** The gap read is unchanged. One more read covers the live `technique-tactic` join:
  it fetches each pair's technique `attack_id`, technique concept id, tactic `attack_id`, concept
  id, shortname and title. Grouping happens host-side, so a technique is undetected when its
  concept id is in the gap set, as for `sub_techniques`.

## Non-goals

- The parent roll-up of `blinded`. The ledger cut it: it changes presentation only.
- ATT&CK matrix ordering, and a per-tactic split of `blinded`.

## Constraints

- All TQL is parameterized. The new read has no values.
- Comments are greenfield. Never fabricate a value.
- Integration tests run with `-j 1`.
- The schema is unchanged, so no migration and no `@doc` work is needed.

## Verification

All in `gecko-bin/tests/cyber_detect_test.rs`, on the structure fixture: two tactics, a parent
in both, `SUB1` in execution, `SUB2` in persistence, and a deprecated sub-technique.

- `coverage_gaps_groups_gaps_by_tactic`. With nothing claimed:
  - execution has `live` 2 (the parent and `SUB1`), `undetected` 2, and the sorted ids;
  - persistence likewise, with the parent and `SUB2`;
  - the parent's row has `tactics` with both, in order.
- `a_fully_covered_tactic_stays_with_no_gaps`. Claiming the parent and `SUB1` leaves execution
  with `live` 2, `undetected` 0 and `techniques: []`.
- `a_retired_technique_counts_in_no_tactic`. The deprecated sub-technique has no kill-chain phase
  in the fixture today, so it gains `execution` (`gecko-bin/tests/common/structure.rs:121`; this is
  shared test code, not a frozen fixture). It must then be in no `live` count and no `techniques`
  list. `cyber_stix_sync_test`'s exact `technique-tactic` pair list gains
  `(RETIRED_SUB, EXECUTION)`.
- `a_dead_tactic_is_not_grouped`. Re-sync the fixture with the persistence tactic deprecated,
  using `edited_structure_bridge` (a raw insert would give the tactic two `deprecated` values).
  It leaves `by_tactic` and every `tactics` list.
- The existing `coverage_gaps_*` tests pass unchanged.
- Docs: the `coverage_gaps` description in `detect/mod.rs` and `extensions/cyber-gecko/README.md`
  names the new fields. The ledger moves S6 to Landed.

## Left to the builder

- Whether the tactic read reuses `live(..)` twice in one match or a small helper.
