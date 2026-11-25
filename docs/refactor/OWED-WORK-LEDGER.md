# Owed work

> **Working surface, not a spec.** It tracks and sequences; the owning document of each item
> decides. An item is listed only if something that ships today would use it: a CLI command,
> the `mem.*` sandbox surface, a production code path, or the cyber detection flow.
> Last reconciled 2026-11-24 against `main` @ `e74672d` (after PR #80).
>
> - Landed and closed work, with its test inventories and measurements:
>   [`LEDGER-HISTORY.md`](./LEDGER-HISTORY.md) up to the review, and
>   [Landed since the review](#landed-since-the-review) after it.
> - TypeDB behaviour observed on the live engine: [`TYPEDB-NOTES.md`](./TYPEDB-NOTES.md).
> - Ratification provenance: SPEC-W2-W3 decisions @`11d7ab1`; SPEC-W5 as-design @`95f0d6e`;
>   CONTRACT-W8 as-contract (`fdd1dc6`); INVARIANTS ratified @`0227c2d`. Ratified documents
>   are never edited; a design doc supersedes them, and this ledger points to it.

**IDs.** Rows that survive the 2026-11-24 review keep their old IDs (A, B, C, M, O, S tracks),
so references in specs and commit messages still resolve in the history file. New rows are
`N1`… and the cut pass is `X1`.

## Next up

The only place priority is stated. Sizes: S (a day or less), M (a few days).

| # | ID | Item | Size |
|---|---|---|---|
| 1 | S6 | `coverage_gaps` grouped by tactic | S |
| 2 | N6 | Cyber worked example and `gecko run --args` (N6a) | M |
| 3 | N5 | `timeout-ms` reaches the run | S |

## Open work

### Cyber

| ID | Item | Consumer | Done when | Needs |
|---|---|---|---|---|
| **N8** | `precision.window` accepts a whole-number float (`5.0`), which `Math.floor` returns, reusing the mem bridge's integer rule (`as_u64` or a whole float up to `Number.MAX_SAFE_INTEGER`) | playbooks computing a window | `precision.window: 5.0` is read as 5 and `1e20` is refused | — |
| **S6** | `coverage_gaps` groups live unclaimed techniques by tactic. The tactic read filters liveness, because retired techniques keep their `technique-tactic` edges. S3 imported tactics and nothing reads them yet. The parent roll-up of `blinded` is cut: it changes presentation only, and `coverage_gaps` already reports `sub_techniques` | playbooks calling `coverage_gaps` | The report carries a per-tactic breakdown; a retired technique appears under no tactic | — |
| **N6** | A cyber sample bundle (claim, disposition, coverage, blinded) and README sections for the host imports, with their argument names (`technique_id` is an ATT&CK id, not a concept id). **N6a:** `gecko run <id> --args <json>`, exposed to the guest as a read-only `gecko.args` | the detection flow end to end | The README walkthrough runs against a synced ATT&CK database; a verdict is recorded without editing a playbook | N6a: its own guest rebuild |

### Engine and CLI

| ID | Item | Consumer | Done when | Needs |
|---|---|---|---|---|
| **N7** | A guest whose stdout is not JSON is reported as a failure, not as `success: true` with `raw_output` (`WasmExecutor::evaluate`'s result parsing) | `gecko run` callers | A guest printing non-JSON fails the run with the raw text in the error | — |
| **N5** | `timeout-ms` is persisted and honoured by `gecko run`; `consumes` and `produces` are persisted or no longer parsed | playbook authors | A playbook with `timeout-ms: 500` times out at 500 ms | — |

## Landed since the review

| ID | What landed | Pinned by |
|---|---|---|
| **N1** | The first-run docs say what the code accepts (PR #81) | `documented_config_snippets_are_accepted` |
| **N3** | One semantic index file per database, `<[semantic_index] dir>/<encoded database>.hnsw` (the old `path` key is refused), reconciled with the graph at startup: stale ids are removed and missing ones embedded, so another database's file, a recreated database, items written while the index was off and failed index writes all heal on the next run. The in-memory dirty set is cut. Design: `docs/superpowers/specs/2026-11-24-index-per-database-design.md` | `semantic_index_integration.rs` (`an_index_built_for_another_database_does_not_hide_this_ones_beliefs`, `reconcile_with_an_empty_graph_empties_the_file`, `items_written_while_the_index_was_off_are_recalled`, `a_warm_start_with_matching_ids_embeds_nothing`, `a_failed_persist_during_reconcile_keeps_episode_vectors`, `a_failed_live_upsert_is_repaired_by_the_next_reconcile`); `index_per_database_cli_test.rs`; the `index_dir`/`index_file` unit tests in `config/paths.rs` |
| **X1** | The second cut pass: the divergence mechanism and host disposition ranker (U1), the consolidation daemon and tombstone state (U2), and the fourteen belief-tier types nothing wrote with their attributes and cleanup migrations (U3). `memory-item`'s `@doc` now names what it holds, through `mem/memory-item-doc`. Design: `docs/superpowers/specs/2026-11-24-second-cut-design.md` | the guards `shipped_mem_schema_declares_no_{consolidation,divergence,diet}_labels` and `shipped_cyber_schema_declares_no_cut_labels`; `a_database_holding_the_old_memory_item_doc_upgrades`; the re-derived rig in `belief_tier_reconciliation_test.rs` |
| **B1** | A belief's validity window ends at its supersede: `supersede` closes the old belief's `valid-to` through `tql::close_valid_to_op` (never extending an earlier end), and `believed-at` reads `[valid-from, valid-to)` with no `belief-state` gate, also excluding beliefs a supersession at or before T replaced, so older databases answer correctly. Absorbs B2. Design: `docs/superpowers/specs/2026-11-25-belief-validity-window-design.md` | `believed_at_reads_across_a_supersede`, `a_supersede_chain_gives_the_middle_belief_its_window`, `as_of_recall_reads_across_a_supersede`, `a_same_instant_supersede_leaves_the_old_belief_an_empty_window`, `a_legacy_superseded_belief_without_valid_to_ends_at_its_supersession`, `a_supersede_keeps_an_earlier_valid_to`, `a_supersede_shortens_a_later_valid_to`, `a_refused_supersede_leaves_valid_to_untouched`, `a_backdated_supersede_is_returned_by_no_as_of_read`, `supersession_ops_closes_the_old_window_in_the_same_batch` |
| **N4** | `mem.retract(id, reason?)` withdraws a belief with no replacement. One `visible-to`-gated batch records a `retraction` (the belief, the retracting agent, the run, an optional `retraction-reason`, `created-at`), flips the state to `"retracted"` and closes `valid-to` through `tql::close_valid_to_op`; the vector is removed. The reply is `{ id, blast_radius }`: the beliefs that rested on it that the caller can see and that are still current, read after the commit. No entrenchment guard and no cascade. A retracted claim leaves `blinded`, and its technique returns to `coverage_gaps`. Design: `docs/superpowers/specs/2026-11-25-mem-retract-design.md` | `writer_reader_integration.rs` (`retract_flips_state_closes_the_window_and_records_who_and_why`, `retracting_another_agents_private_belief_is_refused_and_commits_nothing`, `retract_reports_the_visible_current_beliefs_that_rested_on_it`, `as_of_recall_ends_a_retracted_belief_at_its_retraction`, …); `retract_removes_the_vector_from_the_index`; `retracting_a_claim_removes_it_from_blinded_and_reopens_its_gap`; the `mem_retract_*` guest tests; `a_playbook_retracts_a_root_and_learns_what_rested_on_it`; the rig's `mem.retract` step in `memwriter_is_the_sole_belief_tier_writer` |
| **N2** | The guest boundary refuses bad input. A host call runs its callback once and stashes the outcome; the guest reads it with `host_read_reply` into a buffer of exactly its size, so an oversized `mem.recall` mints one `retrieval-event`. The S3 gate runs before the args are read. The guest checks shape only: options bags are objects with known keys, and a function, a non-finite number or an unconvertible value throws; values are forwarded unconverted. The host refuses a non-object payload, an unknown field and every malformed value (`event_time`, `method`, `confidence`, `reason`, `evidence`, `claims`, `max_chunks`, `as_of`), and `claims` is required. `ContextBudget.max_tokens` and `maxTokens` were cut, not validated, because nothing enforced them. Design: `docs/superpowers/specs/2026-11-25-guest-boundary-refuses-bad-input-design.md` | the `mem_host.rs` validation tests (`an_unknown_key_is_refused_for_each_mem_function`, `recall_max_chunks_must_be_a_positive_integer`, `contest_requires_claims`, …); `wasm_tests.rs` (`a_reply_over_one_mib_arrives_intact_and_runs_once`, `ungranted_call_with_oversized_args_reports_not_granted`, `mem_values_must_be_json`, …); `an_oversized_recall_mints_one_retrieval_event` |

## Deferred (reopen on the trigger)

| ID | Item | Reopen when |
|---|---|---|
| **S5** | Objects shared across STIX imports. Today a second ATT&CK domain is refused with one-database-per-domain guidance (history: S5) | A query or belief spans two ATT&CK domains, or MITRE ships differing copies of a shared object |
| **O2–O4** | OKF v0.2 reads: `generated` replaces `timestamp`, `sources` replaces body citations, and the trust and lifecycle fields (history: O2, O3, O4) | Someone needs to sync a v0.2-authored bundle |
| **M1** | `rests-on` and `pivot` reachable from a playbook. `MemWriter::rests_on` exists and nothing calls it; `pivot` has no writer. INVARIANTS 5 keeps both types | N4 lands and a playbook wants to declare an assumption or record an investigative move |
| **M2** | Prediction calibration on a production path. `record_prediction` and `resolve_prediction` exist and nothing calls them | A playbook or cyber flow scores a prediction against an outcome |
| **M3-r** | Entity resolution. `resolution`/`resolves` stay (invariant 9 names them) and keep `canonical-entity`, `canonical-entity-at`, their step functions, `provably-derived` and `MemWriter::canonical_entity` alive with no writer and no caller | A writer that asserts two records denote one entity. If none appears, cutting them supersedes invariant 9 and is its own decision |

## Open decisions

None. D-A and U1–U4 were decided on 2026-11-24 (below). `pivot`, `prediction-resolution`,
`rests-on` and `resolution`/`resolves` are kept because INVARIANTS 5 and 9 name them;
cutting any of them is a new decision.

## Decided 2026-11-24

| ID | Decision | Lands in |
|---|---|---|
| **D-A / U4** | A belief's validity ends by closing `valid-to`: `supersede` and `retract` both set it (a retraction has no supersession edge, so `supersession.created-at` cannot cover it). `believed-at` reads the half-open window and drops its state gate. `asOf` stays on `mem.recall`. Refined 2026-11-25: `believed-at` also excludes beliefs a supersession at or before T replaced, so databases written before B1 answer correctly | B1, N4 |
| **U1** | Cut the shipped divergence mechanism, not only its writer rows: once populations went, `exception-scope` has no group-scoped belief producer, and instance-versus-instance disagreement already has `supersede` and `contest`. Reverses a non-goal of the populations cut. Supersedes SPEC-W2-W3 §2.1–§2.4 and INVARIANTS §8.2 | X1 (a) |
| **U2** | Cut the dedup tick with the ten stubs, rather than add `gecko consolidate`. `mem.remember` defaults `event_time` to the run's clock, so exact duplicates do not arise on the live path. Supersedes CONTRACT-W8's job list (§2, §2a) and its §3 checks | X1 (b) |
| **U3** | Delete the belief-tier labels nothing writes and no invariant names. The `@doc` strings exist for agents introspecting the graph, and a schema that advertises intentions and learned constraints it never holds misleads that reader. Supersedes D-O (2026-11-20). Existing databases keep the types as inert leftovers, as with populations | X1 (c) |

## Cut and superseded

- **Populations,** `scoped`, the cohort branch of `blast-radius` and its `-at` twins:
  A7, B4, P1–P4, D-F; `docs/superpowers/specs/2026-11-24-populations-cut-design.md`.
- **Divergence writers** A5 and A6, with D-C, D-E and D-K: nothing detects a divergence or
  reads a precedence (U1); the mechanism is cut by
  `docs/superpowers/specs/2026-11-24-second-cut-design.md`. Their owed tests go with them:
  `divergence_precedence_write_requires_cominted_anchor`,
  `empty_winner_yields_contested_anomaly_hub_no_exception_minted`, and the writer halves of
  the three partially graduated W2 tests.
- **Consolidation jobs** C2's remainder, C3–C7 and C9, with D-H, D-I and D-J: no scheduler,
  no CLI, no caller (U2); the daemon is cut by the second-cut design.
- **Belief-tier rows with no writer:** M4, M5, M6 and the four non-`rests-on` labels of M1
  (U3), their types cut by the second-cut design (`pivot` folds into M1); M3 as a work row
  (its types are deferred as M3-r).
- **B3** (`canonical-entity-at` past a superseded resolution): no resolution writer.
- **A8** (cyber sighting writer): blocked on a sighting feature that does not exist; its
  types are cut by the second-cut design.
- **O5** (Attested Computation) with D-N: pure design, no consumer, overlaps the sandbox.
- **A11's** remaining entry, `set_retrieval_provenance_op`'s row count: an opportunistic
  one-line fix, not a tracked row. Its guest half landed with N2.

### Superseded and stale passages in ratified text

Ratified documents are not edited; these are the passages a reader should not trust.

- Everything the populations-cut design lists under "Ratified docs are not edited".
- Everything `docs/superpowers/specs/2026-11-24-second-cut-design.md` lists under
  "Ratified and locked docs are not edited", including CONTRACT-W8 whole.
- The former D-G errata, verbatim in the history file: a renamed SPEC-W5 surface test; the
  `writer.rs` line anchors in SPEC-W5, SPEC-W2-W3 and CONTRACT-W8, which predate PR #50;
  CONTRACT-W8's description of the daemon as unbuilt; SPEC-W5's description of the daemon as
  a future caller; and SPEC-W2-W3 §2.1 showing `divergence-precedence`'s `prevailing` and
  `overridden` as optional roles.
