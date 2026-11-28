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

Nothing queued.

## Open work

Nothing open.

## Landed since the review

| ID | What landed | Pinned by |
|---|---|---|
| **N1** | The first-run docs say what the code accepts (PR #81) | `documented_config_snippets_are_accepted` |
| **N3** | One semantic index file per database, `<[semantic_index] dir>/<encoded database>.hnsw` (the old `path` key is refused), reconciled with the graph at startup: stale ids are removed and missing ones embedded, so another database's file, a recreated database, items written while the index was off and failed index writes all heal on the next run. The in-memory dirty set is cut. Design: `docs/superpowers/specs/2026-11-24-index-per-database-design.md` | `semantic_index_integration.rs` (`an_index_built_for_another_database_does_not_hide_this_ones_beliefs`, `reconcile_with_an_empty_graph_empties_the_file`, `items_written_while_the_index_was_off_are_recalled`, `a_warm_start_with_matching_ids_embeds_nothing`, `a_failed_persist_during_reconcile_keeps_episode_vectors`, `a_failed_live_upsert_is_repaired_by_the_next_reconcile`); `index_per_database_cli_test.rs`; the `index_dir`/`index_file` unit tests in `config/paths.rs` |
| **X1** | The second cut pass: the divergence mechanism and host disposition ranker (U1), the consolidation daemon and tombstone state (U2), and the fourteen belief-tier types nothing wrote with their attributes and cleanup migrations (U3). `memory-item`'s `@doc` now names what it holds, through `mem/memory-item-doc`. Design: `docs/superpowers/specs/2026-11-24-second-cut-design.md` | the guards `shipped_mem_schema_declares_no_{consolidation,divergence,diet}_labels` and `shipped_cyber_schema_declares_no_cut_labels`; `a_database_holding_the_old_memory_item_doc_upgrades`; the re-derived rig in `belief_tier_reconciliation_test.rs` |
| **B1** | A belief's validity window ends at its supersede: `supersede` closes the old belief's `valid-to` through `tql::close_valid_to_op` (never extending an earlier end), and `believed-at` reads `[valid-from, valid-to)` with no `belief-state` gate, also excluding beliefs a supersession at or before T replaced, so older databases answer correctly. Absorbs B2. Design: `docs/superpowers/specs/2026-11-25-belief-validity-window-design.md` | `believed_at_reads_across_a_supersede`, `a_supersede_chain_gives_the_middle_belief_its_window`, `as_of_recall_reads_across_a_supersede`, `a_same_instant_supersede_leaves_the_old_belief_an_empty_window`, `a_legacy_superseded_belief_without_valid_to_ends_at_its_supersession`, `a_supersede_keeps_an_earlier_valid_to`, `a_supersede_shortens_a_later_valid_to`, `a_refused_supersede_leaves_valid_to_untouched`, `a_backdated_supersede_is_returned_by_no_as_of_read`, `supersession_ops_closes_the_old_window_in_the_same_batch` |
| **N2** | The guest boundary refuses bad input. A host call runs its callback once and stashes the outcome; the guest reads it with `host_read_reply` into a buffer of exactly its size, so an oversized `mem.recall` mints one `retrieval-event`. The S3 gate runs before the args are read. The guest checks shape only: options bags are objects with known keys, and a function, a non-finite number or an unconvertible value throws; values are forwarded unconverted. The host refuses a non-object payload, an unknown field and every malformed value (`event_time`, `method`, `confidence`, `reason`, `evidence`, `claims`, `max_chunks`, `as_of`), and `claims` is required. `ContextBudget.max_tokens` and `maxTokens` were cut, not validated, because nothing enforced them. Design: `docs/superpowers/specs/2026-11-25-guest-boundary-refuses-bad-input-design.md` | the `mem_host.rs` validation tests (`an_unknown_key_is_refused_for_each_mem_function`, `recall_max_chunks_must_be_a_positive_integer`, `contest_requires_claims`, …); `wasm_tests.rs` (`a_reply_over_one_mib_arrives_intact_and_runs_once`, `ungranted_call_with_oversized_args_reports_not_granted`, `mem_values_must_be_json`, …); `an_oversized_recall_mints_one_retrieval_event` |
| **N4** | `mem.retract(id, reason?)` withdraws a belief with no replacement. One `visible-to`-gated batch records a `retraction` (the belief, the retracting agent, the run, an optional `retraction-reason`, `created-at`), flips the state to `"retracted"` and closes `valid-to` through `tql::close_valid_to_op`; the vector is removed. The reply is `{ id, blast_radius }`: the beliefs that rested on it that the caller can see and that are still current, read after the commit. No entrenchment guard and no cascade. A retracted claim leaves `blinded`, and its technique returns to `coverage_gaps`. Design: `docs/superpowers/specs/2026-11-25-mem-retract-design.md` | `writer_reader_integration.rs` (`retract_flips_state_closes_the_window_and_records_who_and_why`, `retracting_another_agents_private_belief_is_refused_and_commits_nothing`, `retract_reports_the_visible_current_beliefs_that_rested_on_it`, `as_of_recall_ends_a_retracted_belief_at_its_retraction`, …); `retract_removes_the_vector_from_the_index`; `retracting_a_claim_removes_it_from_blinded_and_reopens_its_gap`; the `mem_retract_*` guest tests; `a_playbook_retracts_a_root_and_learns_what_rested_on_it`; the rig's `mem.retract` step in `memwriter_is_the_sole_belief_tier_writer` |
| **S6** | `coverage_gaps` groups its gaps by tactic: each gap row carries its live `tactics`, and `by_tactic` gives each live tactic its `live` technique count, `undetected` count and gap `techniques`, sorted by ATT&CK id. Dead techniques and dead tactics are excluded from the `technique-tactic` read (retired techniques keep their edges). The parent roll-up of `blinded` was cut: it changes presentation only. Design: `docs/superpowers/specs/2026-11-25-coverage-gaps-by-tactic-design.md` | `coverage_gaps_groups_gaps_by_tactic`, `a_fully_covered_tactic_stays_with_no_gaps`, `a_retired_technique_counts_in_no_tactic`, `a_dead_tactic_is_not_grouped`; the `group_by_tactic_*` unit tests |
| **N6** | A cyber worked example. `extensions/cyber-gecko/sample-bundle/` holds two detections (`T1059.001`, `T1053.005`) and five playbooks that read their inputs from `gecko.args`: claim, verdict, coverage report, precision and withdraw. `sample-sensors.yaml` is written from ATT&CK 20.1's analytics, so one claim is blind and `WinEventLog:Security` is recommended. The cyber README gains the walkthrough, with outputs from a real Enterprise 20.1 run, and `claim`, `disposition` and `precision` reference sections. **N6a:** `gecko run <id> --args <json>` gives a playbook a deep-frozen, non-writable `gecko.args`, delivered as the WASI variable `GECKO_ARGS` and capped at 1 MiB on the host and in the executor. Design: `docs/superpowers/specs/2026-11-25-cyber-worked-example-design.md` | `the_readme_walkthrough_runs` (runs the README's own `gecko run` commands); `run_args_reach_the_playbook`, `non_object_args_fail_before_connecting`; the `run_args_*` guest and `parse_run_args` unit tests |
| **N5** | A playbook's `timeout-ms` bounds its run. `MAX_TIMEOUT_MS` (600000 ms) is enforced by the parser (refusing `0`, negatives, floats, strings, `null` and a blank value, naming the file), by `sync_bundle`, by `gecko run`'s read of the stored value, and by the executor before the guest starts. The core schema's `timeout-ms` attribute (`value integer @range(1..)`) is written by sync and read by `gecko run` only where the schema defines it, so an older database still runs. A timed-out run reports `execution timed out after <ms> ms`. `consumes` and `produces` are no longer special fields; they land in `metadata-json`. Design: `docs/superpowers/specs/2026-11-26-timeout-ms-design.md` | `a_playbook_times_out_at_its_timeout_ms`, `a_stored_timeout_out_of_range_fails_the_run`, `a_playbook_runs_on_a_schema_without_timeout_ms`, `timeout_ms_round_trips`, `an_out_of_range_timeout_is_refused_before_writing`; the `timeout_ms_*` parser tests; `a_zero_timeout_is_refused`, `a_timeout_over_the_ceiling_is_refused`, `the_ceiling_itself_is_accepted` |
| **N8, N9** | `precision` reads `window` through the bridges' shared whole-number rule (`gecko_extension_api::whole_number`: a JSON integer, or a whole float up to `Number.MAX_SAFE_INTEGER`), so `5.0` is read as 5. It refuses anything outside `1..=MAX_WINDOW_SECONDS` (3155760000 s, 100 Julian years) with an error naming the limit, and computes its cutoff with checked chrono math, so a huge window is an error, not a panic. Design: `docs/superpowers/specs/2026-11-27-precision-window-design.md` | `precision_reads_a_whole_float_window_and_refuses_one_past_the_ceiling`; `a_whole_float_window_is_read_as_an_integer`, `a_window_outside_the_range_is_refused_naming_the_limit`, `a_refused_window_is_shown_without_echoing_a_large_value`, `window_cutoff_at_the_ceiling_is_100_julian_years_earlier`, `window_cutoff_chrono_cannot_represent_is_an_error`; the `guest_value` tests |
| **N7** | A run fails, never reports a fabricated success. The executor reads guest stdout through `read_guest_output`: empty output fails with `guest wrote no output`, non-JSON output with `guest output is not JSON: <text>` (at most 1 KiB quoted), and a top-level `"success": false` always carries a message. A failed `gecko run` exits non-zero with the same stdout, and the pipeline logs the failure at `warn`, not `error`. Design: `docs/superpowers/specs/2026-11-27-guest-output-fails-run-design.md` | `a_failed_playbook_exits_non_zero`, `a_playbook_times_out_at_its_timeout_ms`, `a_guest_failure_without_a_message_is_reported`; the `read_guest_output` unit tests (`empty_stdout_is_no_output`, `non_json_stdout_fails_with_a_quote`, `a_long_non_json_quote_is_cut_on_a_char_boundary`, `a_quote_of_exactly_the_limit_is_not_marked_cut`, `a_failure_without_a_message_gets_one`, `a_failure_keeps_its_message_and_output`, `json_that_is_not_a_failure_succeeds`) |
| **M1 (`rests-on`)** | `mem.restsOn(resting, assumptions)` declares that a belief rests on others, behind `mem:rests_on`. The edge is stamped with its declarer, run and `created-at` (invariant 2), and every id must be visible to the caller in either position; a refusal looks like a missing id. A repeated assumption, a self-dependency and an empty list are refused. A `rests-on` written before the stamp fails the upgrade (no migration invents provenance). In the cyber worked example, `record_verdict`'s `assumes` makes the verdict rest on its claim, and withdrawing the claim lists the verdict. The other `mem.*` functions still accept an empty-string id; `rests_on` refuses one. Design: `docs/superpowers/specs/2026-11-27-mem-rests-on-design.md` | `rests_on_stamps_the_declarer_the_run_and_the_time`, `rests_on_another_agents_private_belief_is_refused_and_commits_nothing`, `rests_on_gates_the_resting_item_and_every_assumption_in_one_call`, `rests_on_a_team_belief_of_another_agent_is_allowed`, `rests_on_accepts_the_callers_own_episode_and_refuses_another_agents`, `rests_on_twice_lists_the_dependent_once_in_the_blast_radius`; `an_unstamped_rests_on_is_refused_at_commit`, `an_unstamped_rests_on_fails_the_upgrade`; `rests_on_refuses_bad_fields`; the `mem_rests_on_*` guest tests; `a_playbook_declares_an_assumption_and_learns_what_rested_on_it`; the rig's `mem.restsOn` step; `the_readme_walkthrough_runs` step 9, `record_verdict_refuses_a_bad_assumes_before_recording`, `record_verdict_with_a_missing_assumption_keeps_the_verdict` |
| **N10** | A concept whose `engine` is present but unsupported fails to parse with `InvalidEngine`, naming the file and the value as written; a declared engine with no non-blank code in a `js`/`javascript`/`quickjs` fence fails with `MissingProgram`. The accepted spellings are one constant (`SCRIPT_ENGINE_LANGS`) shared by `from_lang` and both messages. A zero-line fence is its own empty fence instead of matching across into the next. Design: `docs/superpowers/specs/2026-11-28-engine-refused-at-parse-design.md` | `an_unsupported_engine_is_refused_as_written`, `engine_spellings_from_lang_accepts_parse`, `a_declared_engine_without_a_program_is_refused`, `a_blank_fence_beside_a_real_one_is_skipped`, `a_fence_language_is_matched_like_engine`, `a_fence_without_an_engine_stays_documentation`; `a_zero_line_fence_does_not_swallow_the_next`, `a_lone_zero_line_fence_is_an_empty_fence`, `extract_program_skips_a_blank_matching_fence`; `a_bundle_with_an_unsupported_engine_fails_to_sync` |

## Deferred (reopen on the trigger)

| ID | Item | Reopen when |
|---|---|---|
| **S5** | Objects shared across STIX imports. Today a second ATT&CK domain is refused with one-database-per-domain guidance (history: S5) | A query or belief spans two ATT&CK domains, or MITRE ships differing copies of a shared object |
| **O2–O4** | OKF v0.2 reads: `generated` replaces `timestamp`, `sources` replaces body citations, and the trust and lifecycle fields (history: O2, O3, O4) | Someone needs to sync a v0.2-authored bundle |
| **M1** | `pivot` reachable from a playbook. It has no writer and no caller, and nothing defines what its `calibration-score` measures. INVARIANTS 5 keeps the type | A playbook wants to record an investigative move, and a consumer defines its score |
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
