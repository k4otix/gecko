# Owed work

> **Working surface, not a spec.** It tracks and sequences; the owning document of each item
> decides. An item is listed only if something that ships today would use it: a CLI command,
> the `mem.*` sandbox surface, a production code path, or the cyber detection flow.
> Last reconciled 2026-11-28 against `main` @ `7448614` (after PR #102).
>
> - Landed and closed work, with its test inventories and measurements:
>   [`LEDGER-HISTORY.md`](./LEDGER-HISTORY.md) up to the review, and
>   [Landed since the review](#landed-since-the-review) after it.
> - TypeDB behaviour observed on the live engine: [`TYPEDB-NOTES.md`](./TYPEDB-NOTES.md).
> - **OKF spec:** the canonical spec is
>   <https://github.com/open-knowledge-format/open-knowledge-format/blob/main/SPEC.md>, version
>   0.3, released 2026-11-18. The copy under `GoogleCloudPlatform/knowledge-catalog/okf/` is a
>   frozen v0.2 snapshot. Track O in the history file and the O2 + O4 design cite that copy,
>   which matches what they implemented. GECKO reads OKF with v0.2 semantics until **O7**
>   lands.
> - Ratification provenance: SPEC-W2-W3 decisions @`11d7ab1`; SPEC-W5 as-design @`95f0d6e`;
>   CONTRACT-W8 as-contract (`fdd1dc6`); INVARIANTS ratified @`0227c2d`. Ratified documents
>   are never edited; a design doc supersedes them, and this ledger points to it.

## Starting an item from a clear context

This ledger is meant to be enough to resume work with no conversation history. Read it in
this order, then start.

1. **Pick the item.** It is the first row of [Next up](#next-up), unless the user names
   another. The row names its owning documents, what is decided, and what is still open.
2. **Read the row's owning documents,** and the
   [`KNOWN-LIMITATIONS.md`](./KNOWN-LIMITATIONS.md) section it names. `CLAUDE.md` loads by
   itself and holds the code conventions.
3. **Check the row's `file:line` anchors against current `main`** before relying on them.
   Rows age, and the code wins.
4. **Branch from a fresh `main`.** One item per branch and per PR.

### The workflow for each item

The user's working process, in order. Each step waits for the one before.

1. **Design.**
   - Use the brainstorming skill. Lay out the approaches in their own message, and the user
     picks one.
   - Write `docs/superpowers/specs/<date>-<topic>-design.md`. It is short for a small item,
     and full for an M item, which also gets a builder check by a fresh subagent.
   - Wait for the user's approval of the doc. When the user pastes another reviewer's
     findings, verify each one against the code before folding it in.
2. **Plan.**
   - Use the writing-plans skill to write `docs/superpowers/plans/<date>-<topic>.md`.
   - Fable reviews the plan, and its findings are folded in.
   - The plan is committed with the design on the item's branch.
3. **Execute.**
   - The default is inline (the executing-plans skill), one task at a time, with TDD.
   - Each test is watched failing before its code is written. A test that cannot fail first
     is labelled a regression or smoke test.
   - The SDD ledger lives at `.superpowers/sdd/<plan-basename>/progress.md`, which git
     ignores. It records each task's completion and every `Ruling:`.
4. **Final review.**
   - Fable reviews the whole branch from the review package.
   - Re-grade each finding by what a person using the software would get. Critical and
     Important findings get one fix pass, each fix tested failing first, then the full suite.
   - Minors are deferred and listed in the PR body. The user may later ask to "do the
     minors".
5. **PR.**
   - Open it against `main`. The body lists the rulings, the deferred minors and the tests,
     and ends with the session's attribution line.
   - Merge only when the user says so, or by the auto-merge they set.
   - The merge gate is the required checks: "Build + test (Tier 1+2, candle-free,
     cyber-on)" and "Model revision bump gate". Don't wait for CodeQL.
6. **Close the item here.**
   - Move the row to [Landed since the review](#landed-since-the-review), naming its design
     doc and its pinning tests.
   - Update `KNOWN-LIMITATIONS.md`.
   - Do this in the item's own PR.

### Standing rules not in `CLAUDE.md`

- **Gates before every commit.** Check each by its own exit code, never through a pipe:
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`;
  - the same with `--features cyber`;
  - the js-sandbox wasm clippy when the guest changes.
- **The suite runs one test at a time:**
  `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1`.
  TypeDB 3.15 runs on `127.0.0.1:1729`. Never touch the database named `gecko`, and never
  stop a TypeDB this session did not start.
- **Stage named paths only. Never run `git add -A <dir>`:** the user keeps untracked work
  of their own under `docs/`.
- **Commit and push only within the item's workflow.** The user's email is private.
- **After editing `core/js-sandbox/src/main.rs`,** run `just build-guest` and commit
  `quickjs.wasm` in the same commit.
- **Never edit these:**
  - ratified documents;
  - historical documents (`LEDGER-HISTORY.md`, landed design docs);
  - frozen fixtures under `*/tests/fixtures/`.
  A new design doc supersedes; it does not edit.
- **Never loosen the belief-edge registry.** Don't edit `CLAUDE.md` because a subagent
  suggested it.
- **OpenWiki pages under `openwiki/` are regenerated** through OpenWiki's own lifecycle,
  never hand-edited.
- **zsh:** an unquoted `$VAR` doesn't word-split. Use functions or inline commands.

**IDs.** Rows that survive the 2026-11-24 review keep their old IDs (A, B, C, M, O, S tracks),
so references in specs and commit messages still resolve in the history file. New rows are
`N1`… and the cut pass is `X1`.

## Next up

The only place priority is stated. Sizes: S (a day or less), M (a few days).

| ID | Item | Size |
|---|---|---|
| **O7** | **OKF v0.3 reads.** v0.3 makes the v0.2 text precise and changes some rules (spec §13). GECKO's O2 + O4 code follows v0.2. Owning docs: the v0.3 spec (§5, §5.2–§5.5, §8.1, §11, §11.2, §12.1, §13); the O2 + O4 design `docs/superpowers/specs/2026-11-28-okf-v02-provenance-and-trust-design.md`; KNOWN-LIMITATIONS "OKF v0.2 frontmatter". The code is `core/gecko-engine/src/okf/provenance.rs` and `parser.rs` (`parse_concept`, `read_okf_version`, `extract_string_list`). In order of user impact: **(1)** The trust tier counts only *current* events, those whose `at` ≥ `generated.at`, or every event when `generated` is absent: "Consumers MUST calculate the trust tier only from current events" (§5.3, a §13.1 breaking change). Today every event counts, so a human review followed by a later agent rewrite wrongly stays `human-reviewed`. Keep storing every event. **(2)** A `.md` file without frontmatter no longer has to be a concept (§11; read §3.2's concept and asset rules). Today one such file fails the whole sync with `MissingType`. **(3)** An unparseable `stale_after` "MUST NOT" read as fresh (§5.5). Today it is kept raw with no `stale-after`, so a `stale-after <= now` reader takes it as fresh. Its representation in the graph is a design decision. **(4)** A date-only value means the start of that day in UTC (§5), while a date-time without an offset stays invalid. An empty or null value is the same as an absent key (§11.2). Today both warn. **(5)** One item where a list is expected is a one-item list, and a scalar `tags` is one tag that is never split at commas (§11.2). Today `extract_string_list` drops a scalar silently. **(6)** An unknown `status` is not `stable` (§5.4): the `@doc` changes, through a `CORE_MIGRATIONS` redefine. **(7)** New optional fields: `superseded_by`, a path that only has meaning with `status: deprecated` (§5.4); `title`, `description` and `license` in the bundle-root `index.md` (§8.1), to be weighed against `bundle.json`, which GECKO reads today; `x-` extension keys (§4.1), which stay in `metadata-json`. **(8)** Code comments and the KNOWN-LIMITATIONS section say "OKF v0.2". Make them version-neutral, or 0.3 where a rule is 0.3's. **Decided:** the 0.3 rules apply whatever `okf_version` a bundle declares. 0.2 bundles stay readable (§12.1), and the tier change only lowers a tier, which the spec calls "the safe direction". `openwiki/` still reads as `machine-confirmed`, because its `verified.at` equals its `generated.at`. Do not depend on the `okf` crate (see [Decided 2026-11-28](#decided-2026-11-28)); its README's "Upgrading a bundle from OKF v0.2" table is a cross-check | M |
| **O3** | **`sources` and keyed footnotes replace body citations** (history: O3; v0.3 §5.1, §4.2, §6.2). The trigger fired: `openwiki/` has 75 `sources` entries, all `repo://…`. Design it against **v0.3**, after O7. v0.3 says "Do not use a list of citations in the body". `resource` is REQUIRED in an entry, and `id` SHOULD be present when the body cites it. A path without a leading `/` resolves from its file's directory, and for bundles declaring 0.2 or no version, a consumer MAY fall back to the bundle root (§12.1). A path outside the bundle root is external, and consumers "MUST NOT read it from the local file system". A single mapping is a one-item list (§11.2). Notes carried from the O2 + O4 reviews: an explicit `sources: []` means no citations, with no fallback to body links; with no `sources` key, body http(s) links stay the legacy fallback; a `resource` nobody can follow (`repo://…`) is stored as given, keeping `external-resource`'s `target-url` key and widening its `@doc` through a `CORE_MIGRATIONS` redefine; the footnote scan skips code fences and `[^id]:` definition lines; a label that matches no `sources[].id` is warned about, as O2/O4 warn; decide the dedup key when two entries of one concept name the same resource; per-entry signals (`id`, `title`, `author`, `usage_count`, `last_modified`, `usage_window`) sit on the `citation` relation | M |

**Outside this repo,** with no ledger ID, done when the user asks:
- **An OpenWiki update run** that rewrites `openwiki/`'s 67 root-absolute links (`/openwiki/…`) as relative paths. It goes through the lifecycle (`openwiki_begin` → pages → `openwiki_finish`), applying the relative-link rule by hand, because OpenWiki's Claude Code integration doesn't supply it. If `openwiki_begin` returns `noop`, say so rather than forcing a run. Its prerequisite, the bundle-root escape fix (PR #101), has landed.
- **Upstream OpenWiki issues.** Two are already open:
  - the Claude Code integration lacks the relative-link rule;
  - `producerActor: "claude-code"` breaks OKF §7's `<producer>/<version>`.

  One is left to draft, for the user to read before anything is filed, because the text goes public: there is no OKF v0.3 support.
  - `src/okf/index-sync.ts:288` hard-codes `okf_version: "0.2"`, and so do the writer prompts (`src/agent/prompts/code.ts:66`, `personal.ts`).
  - Regenerating the bundle-root `index.md` rewrites it from scratch. v0.3 §8.1 says a producer "MUST keep" its frontmatter (`title`, `description`, `license`).

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
| **O2, O4** | OKF v0.2 provenance, trust and lifecycle are typed facts on the concept, not beliefs. `generated.at` fills `timestamp` (the v0.1 `timestamp` is the fallback), and `generated.by` → `generated-by`; each `verified` event is a unary `verification` relation (`verified-by`, `verified-at`), and the concept's derived `trust-tier` is `unverified` (`verified: []`), `machine-confirmed` or `human-reviewed` (a `human:<id>` actor), absent when `verified` is; `status` → `okf-status` as written; `stale_after` → `stale-after`; the bundle-root `index.md`'s `okf_version` → `okf-version` on `bundle`. A malformed value is never refused or invented: it stays in `metadata-json` and `gecko sync` prints a warning. The deletes naming the new types are guarded by the schema, so `sync_bundle` still syncs against an older schema. Design: `docs/superpowers/specs/2026-11-28-okf-v02-provenance-and-trust-design.md` | the `okf::parser` provenance tests (`trust_tiers_follow_the_verifiers`, `a_malformed_verified_writes_no_tier`, `a_bad_verified_entry_is_skipped`, `generated_wins_over_a_legacy_timestamp`, `a_malformed_timestamp_warns_and_is_kept`, `an_invalid_okf_version_warns`, …); `bundle_sync_test.rs` (`provenance_attributes_round_trip`, `verification_events_round_trip`, `gc_and_retirement_leave_no_verification`, `okf_version_follows_the_root_index`, `a_v01_bundle_gains_no_provenance_attributes`); the edited re-sync in `tightened_schema_applies_to_a_populated_database`; `sync_prints_provenance_warnings`; `okf_v02_test.rs` (`the_v02_fixture_bundle_reads_as_written`, `the_repos_own_openwiki_bundle_parses`) |

## Deferred (reopen on the trigger)

| ID | Item | Reopen when |
|---|---|---|
| **S5** | Objects shared across STIX imports. Today a second ATT&CK domain is refused with one-database-per-domain guidance (history: S5) | A query or belief spans two ATT&CK domains, or MITRE ships differing copies of a shared object |
| **M1** | `pivot` reachable from a playbook. It has no writer and no caller, and nothing defines what its `calibration-score` measures. INVARIANTS 5 keeps the type | A playbook wants to record an investigative move, and a consumer defines its score |
| **M2** | Prediction calibration on a production path. `record_prediction` and `resolve_prediction` exist and nothing calls them | A playbook or cyber flow scores a prediction against an outcome |
| **M3-r** | Entity resolution. `resolution`/`resolves` stay (invariant 9 names them) and keep `canonical-entity`, `canonical-entity-at`, their step functions, `provably-derived` and `MemWriter::canonical_entity` alive with no writer and no caller | A writer that asserts two records denote one entity. If none appears, cutting them supersedes invariant 9 and is its own decision |

## Open decisions

None. D-A and U1–U4 were decided on 2026-11-24 (below). `pivot`, `prediction-resolution`,
`rests-on` and `resolution`/`resolves` are kept because INVARIANTS 5 and 9 name them;
cutting any of them is a new decision.

## Decided 2026-11-28

| ID | Decision | Lands in |
|---|---|---|
| **D-OKF-crate** | Don't depend on the `okf` crate (<https://docs.rs/okf>, W4G1/okf, 0.3.0). It is one person's pre-1.0 project, explicitly not affiliated with Google or the OKF project, and two crates depend on it. Its hand-written YAML subset refuses anchors, aliases, tags and complex keys that GECKO's `serde_yaml` accepts. It skips malformed values silently, where GECKO warns about each one. It would replace only the frontmatter-typing layer: `engine`, `scopes`, `timeout-ms`, one program per concept, link resolution, citations and hierarchy stay GECKO's. It may serve as a test oracle or cross-check. Reopen it if the crate reaches 1.0 or becomes the spec's named reference implementation | O7 (cross-check only) |

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
