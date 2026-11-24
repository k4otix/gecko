# Owed work

> **Working surface, not a spec.** It tracks and sequences; the owning document of each item
> decides. An item is listed only if something that ships today would use it: a CLI command,
> the `mem.*` sandbox surface, a production code path, or the cyber detection flow.
> Last reconciled 2026-11-24 against `main` @ `e74672d` (after PR #80).
>
> - Landed and closed work, with its test inventories and measurements:
>   [`LEDGER-HISTORY.md`](./LEDGER-HISTORY.md).
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
| 1 | N1 | Docs match the code on first-run paths | S |
| 2 | N3 | One semantic index per database | S–M |
| 3 | X1 | Second cut pass (design doc first) | M, mostly deletion |
| 4 | B1 | Beliefs end: supersede closes `valid-to`, as-of recall reads the window | M |
| 5 | N2 + N4 + N6a | One guest rebuild: the boundary refuses bad input, `mem.retract`, `gecko.args` | M |
| 6 | S6 | `coverage_gaps` grouped by tactic | S |
| 7 | N6 | Cyber worked example and `gecko run --args` | M |
| 8 | N5 | `timeout-ms` reaches the run | S |

## Open work

### Memory

| ID | Item | Consumer | Done when | Needs |
|---|---|---|---|---|
| **N3** | The index file is keyed by database, and a snapshot built for another database is rebuilt, not reused. Today the path is `cache_dir/index/gecko.hnsw` or one explicit `path` (`gecko-bin/src/config/paths.rs`), and a rebuild runs only when the index is empty or its model id differs, so database B's run reuses A's index and indexed recall misses B's beliefs (found by reading; the first step is a failing test). Also: the dirty set is in-process only, so in a one-shot `gecko run` "marked dirty for repair" repairs nothing | `mem.recall` with the index enabled (the shipped `gecko.toml`); one database per bundle and per ATT&CK domain is the default | Two databases, alternating runs: each recalls its own pre-existing beliefs | — |
| **B1** | Per D-A: `supersede` sets `valid-to` on the belief it replaces, `retract` (N4) sets it on the belief it withdraws, and `believed-at` reads the half-open window `[valid-from, valid-to)` and drops its `belief-state` gate (the same window `canonical-entity-at` reads). Absorbs B2 | `mem.recall(q, { asOf })` | As-of recall at T returns a belief superseded after T and not its successor; a supersede within one run at a single `occurred_at` gives the old belief an empty window and the new one `[t, ∞)` (one test); the as-of fixtures supersede through the writer. `KNOWN-LIMITATIONS.md` says a database written before B1 holds superseded beliefs with no `valid-to`, which as-of recall then returns | — |
| **N4** | `retract(ctx, id, reason)`: sets `belief-state "retracted"` and `valid-to`, visible-to gated, exactly one row; removes the belief's vector from the index, as supersede does; the reply lists the `blast-radius` of the retracted belief | `mem.retract`; withdrawing a cyber claim without a dummy replacement | A retracted claim leaves `blinded` and `coverage_gaps`; present-state indexed recall no longer surfaces it; the reply names what rested on it | B1; the guest rebuild (N2) |
| **N2** | The guest and host refuse malformed arguments instead of defaulting them: `evidence` and `claims` (A11's guest half), `asOf`, `maxChunks`, `maxTokens`, `confidence`, an unparseable `event_time` (today "now"), an unknown `method` (today `llm-synthesis`; cyber's `claim` already refuses), and `callExtension`'s extension and function names (today `""`). A reply over the guest buffer reports the size needed without running the callback twice, so an oversized `mem.recall` mints one `retrieval-event` | every `mem.*` and `callExtension` call | One host test per field; the guest tests are green after `just build-guest`; an oversized recall mints exactly one retrieval-event | One `quickjs.wasm` rebuild, shared with N4 and N6a |

### Cyber

| ID | Item | Consumer | Done when | Needs |
|---|---|---|---|---|
| **S6** | `coverage_gaps` groups live unclaimed techniques by tactic. The tactic read filters liveness, because retired techniques keep their `technique-tactic` edges. S3 imported tactics and nothing reads them yet. The parent roll-up of `blinded` is cut: it changes presentation only, and `coverage_gaps` already reports `sub_techniques` | playbooks calling `coverage_gaps` | The report carries a per-tactic breakdown; a retired technique appears under no tactic | — |
| **N6** | A cyber sample bundle (claim, disposition, coverage, blinded) and README sections for the host imports, with their argument names (`technique_id` is an ATT&CK id, not a concept id). **N6a:** `gecko run <id> --args <json>`, exposed to the guest as a read-only `gecko.args`, built in N2's guest rebuild | the detection flow end to end | The README walkthrough runs against a synced ATT&CK database; a verdict is recorded without editing a playbook | N6a: the guest rebuild (N2) |

### Engine and CLI

| ID | Item | Consumer | Done when | Needs |
|---|---|---|---|---|
| **N1** | The README, `docs/INSTALL.md` and the cyber README say what the code accepts: `embedder = "candle"`, `enabled = ["cyber"]`, the `BAAI/` model-id prefix, Tier-3 on manual dispatch, `gecko status` output, `sync-stix` and `sync-sensors` in the command table | first-run users | Every `gecko.toml` snippet in the user docs is accepted by the binary (`documented_config_snippets_are_accepted`) | — |
| **N5** | `timeout-ms` is persisted and honoured by `gecko run`; `consumes` and `produces` are persisted or no longer parsed | playbook authors | A playbook with `timeout-ms: 500` times out at 500 ms | — |
| **X1** | **Second cut pass,** with a design doc in the shape of `2026-11-24-populations-cut-design.md`, listing the superseded passages of CONTRACT-W8, SPEC-W2-W3, SPEC-W5 and INVARIANTS. Scope, per decisions U1–U3: **(a) divergence** — `mem_gecko::disposition` (538 lines), `divergence-exception`, `divergence-precedence` and its roles, the `mem/divergence-precedence-required-roles` migration, the W2 halves of `schema_probe_w2w3.rs`, and `DerivationMethod::rank()` if nothing else calls it; **(b) consolidation** — the ten stub jobs, `NoopConsolidationDaemon`, `MemConsolidationDaemon` and its dedup tick, `dedup_episodes`, and the labels left with no writer: `consolidation-step`, `consolidation-action`, `consolidation-state`, the `mem/consolidation-step-required-roles` migration and the `not { … consolidation-state "tombstoned"; }` guards in `tql.rs`; **(c) schema diet** — the twelve mem labels no production path writes and no invariant names (`intention`, `goal-hierarchy`, `working-set`, mem's `playbook`, `execution`, `tool-invocation`, `learned-constraint`, `contextualizes`, `evidence`, `justification`, `support`, `attack`) and cyber's `sighting` and `sighting-of`; **(d)** the uncalled public methods those leave behind, and plan-tag comments in `mem_types.tql` and `mem_functions.tql`. Audit shared attributes and supertypes before deleting: `condition` is owned by `anomaly` (kept) and `learned-constraint` (cut); `execution-episode` stays as the supertype of `retrieval-event` | an introspected schema whose `@doc`s describe only what GECKO holds; a smaller belief-tier surface for the sync fingerprint | A guard test: the shipped schemas name none of the deleted labels; the rig's `DISPOSITIONS` re-derived; the CONTRACT-W8 §3 rows and `daemon tick` drive go with the daemon | — |

## Deferred (reopen on the trigger)

| ID | Item | Reopen when |
|---|---|---|
| **S5** | Objects shared across STIX imports. Today a second ATT&CK domain is refused with one-database-per-domain guidance (history: S5) | A query or belief spans two ATT&CK domains, or MITRE ships differing copies of a shared object |
| **O2–O4** | OKF v0.2 reads: `generated` replaces `timestamp`, `sources` replaces body citations, and the trust and lifecycle fields (history: O2, O3, O4) | Someone needs to sync a v0.2-authored bundle |
| **M1** | `rests-on` reachable from a playbook. `MemWriter::rests_on` exists and nothing calls it | N4 lands and a playbook wants to declare an assumption |
| **M2** | Prediction calibration on a production path. `record_prediction` and `resolve_prediction` exist and nothing calls them | A playbook or cyber flow scores a prediction against an outcome |
| **M3-r** | Entity resolution. `resolution`/`resolves` stay (invariant 9 names them) and keep `canonical-entity`, `canonical-entity-at`, their step functions, `provably-derived` and `MemWriter::canonical_entity` alive with no writer and no caller | A writer that asserts two records denote one entity. If none appears, cutting them supersedes invariant 9 and is its own decision |

## Open decisions

None. D-A and U1–U4 were decided on 2026-11-24 (below). `pivot`, `prediction-resolution`,
`rests-on` and `resolution`/`resolves` are kept because INVARIANTS 5 and 9 name them;
cutting any of them is a new decision.

## Decided 2026-11-24

| ID | Decision | Lands in |
|---|---|---|
| **D-A / U4** | A belief's validity ends by closing `valid-to`: `supersede` and `retract` both set it (a retraction has no supersession edge, so `supersession.created-at` cannot cover it). `believed-at` reads the half-open window and drops its state gate. `asOf` stays on `mem.recall` | B1, N4 |
| **U1** | Cut the shipped divergence mechanism, not only its writer rows: once populations went, `exception-scope` has no group-scoped belief producer, and instance-versus-instance disagreement already has `supersede` and `contest`. Reverses a non-goal of the populations cut. Supersedes SPEC-W2-W3 §2.1–§2.4 and INVARIANTS §8.2 | X1 (a) |
| **U2** | Cut the dedup tick with the ten stubs, rather than add `gecko consolidate`. `mem.remember` defaults `event_time` to the run's clock, so exact duplicates do not arise on the live path. Supersedes CONTRACT-W8's job list (§2, §2a) and its §3 checks | X1 (b) |
| **U3** | Delete the belief-tier labels nothing writes and no invariant names. The `@doc` strings exist for agents introspecting the graph, and a schema that advertises intentions and learned constraints it never holds misleads that reader. Supersedes D-O (2026-11-20). Existing databases keep the types as inert leftovers, as with populations | X1 (c) |

## Cut and superseded

- **Populations,** `scoped`, the cohort branch of `blast-radius` and its `-at` twins:
  A7, B4, P1–P4, D-F; `docs/superpowers/specs/2026-11-24-populations-cut-design.md`.
- **Divergence writers** A5 and A6, with D-C, D-E and D-K: nothing detects a divergence or
  reads a precedence (U1). Their owed tests go with them:
  `divergence_precedence_write_requires_cominted_anchor`,
  `empty_winner_yields_contested_anomaly_hub_no_exception_minted`, and the writer halves of
  the three partially graduated W2 tests.
- **Consolidation jobs** C2's remainder, C3–C7 and C9, with D-H, D-I and D-J: no scheduler,
  no CLI, no caller (U2).
- **Belief-tier rows with no writer:** M4, M5, M6 and the four non-`rests-on` labels of M1
  (U3); M3 as a work row (its types are deferred as M3-r).
- **B3** (`canonical-entity-at` past a superseded resolution): no resolution writer.
- **A8** (cyber sighting writer): blocked on a sighting feature that does not exist; its
  types go in X1.
- **O5** (Attested Computation) with D-N: pure design, no consumer, overlaps the sandbox.
- **A11's** remaining entry, `set_retrieval_provenance_op`'s row count: an opportunistic
  one-line fix, not a tracked row. Its guest half is N2.

### Superseded and stale passages in ratified text

Ratified documents are not edited; these are the passages a reader should not trust.

- Everything the populations-cut design lists under "Ratified docs are not edited".
- What X1's design will list for U1–U3.
- The former D-G errata, verbatim in the history file: a renamed SPEC-W5 surface test; the
  `writer.rs` line anchors in SPEC-W5, SPEC-W2-W3 and CONTRACT-W8, which predate PR #50;
  CONTRACT-W8's description of the daemon as unbuilt; SPEC-W5's description of the daemon as
  a future caller; and SPEC-W2-W3 §2.1 showing `divergence-precedence`'s `prevailing` and
  `overridden` as optional roles.
