# OWED-WORK-LEDGER — owed work across W2 / W3 / W5 / W8

> **Status: living working-surface** (not a ratified spec — it tracks, it does not decide).
> The source of truth for each item is its owning document's cited section; this ledger is the
> index and the sequencing. Last reconciled against `main` @ `8eedf85` (PR #50) on 2026-11-18;
> the evidence for every status and every review-sourced item is in
> [`REVIEW-2026-11.md`](./REVIEW-2026-11.md).
>
> **Ratification provenance:** SPEC-W2-W3 decisions @`11d7ab1`; SPEC-W5 as-design @`95f0d6e`;
> CONTRACT-W8 as-contract (self-anchored, `fdd1dc6`).

**Reading the IDs.** Items are grouped by track: **F** fix-now, **A** seam ①, **B** bitemporal
correctness, **P** populations, **C** consolidation daemon, **D-** open decisions. A number in
parentheses — "(was #12)" — is the row number the item had in the previous ledger, kept so
references in specs and commit messages still resolve.

## Where things stand

- **Landed in PR #50:** the W2/W3 schema fold, the as-of functions, `blast-radius-at`, the
  disposition ranker (pure half), the populations writer, function-aware schema re-apply, and
  the §5 acceptance suite (20 tests run by default, 5 evidence probes ignored).
- **Fixed after the review:** a database initialized before PR #50 could not be upgraded (**F1**, now landed on `review/ledger-2026-11`).
- **Keystone:** the `BeliefEdgeDecl` seam (**A1**). It unblocks the cyber migration, both W5a
  tests going green, the divergence-exception writer, and the resolved half of the disposition
  writer. It does **not** gate the populations work, the bitemporal track, or the three daemon
  jobs listed under track C as buildable now.

## Sequence

```text
F1 ✅ ───────────────────────────► (pre-#50 databases upgrade again)
F2, F3, F4, F5, F6, F7 ──────────► independent; no blockers

D-B, D-D ──► A1 ──┬─► A2 ──► A3 green, A4 green
                  ├─► A5 (also needs D-A, D-C)
                  ├─► A6 resolved half (also needs D-C)
                  └─► A8 (parked until cyber builds sighting ingestion)
A6 contested half ─► needs D-E only (no seam)
A7 ────────────────► no blocker

D-A ──► B1 ──► B2, B3 ──► B4 (optional; also needs D-F)

F3 ──► P1, P2          P3 ─► no blocker

C1, C2 ────────────────► buildable now
C3 ──► needs D-H       C4–C6 ─► new methods     C7 ─► needs D-I     C8 ─► needs A4
```

---

## Track F — fix now (from REVIEW-2026-11; no blockers)

| ID | Item | Severity | Where | Done when |
|---|---|---|---|---|
| **F1** ✅ | **Schema upgrade path for changed type annotations — LANDED.** `SchemaMigration { id, applies_when, tql }` in `gecko-extension-api`, `GeckoExtension::schema_migrations()`, and `TypeDbRouter::apply_schema_with_migrations`: pending migrations run first, in the extension's schema transaction. Migrations are **self-detecting** rather than keyed by a stored version: one runs only while the database's exported schema still contains its `applies_when` text. A stored marker cannot work for the first migration, because databases created by `main` @`8eedf85` already hold the new `@doc` and have no marker, and an identical `redefine` is refused (`[REX25]`). First entry: `mem/population-member-window-doc`. Dropping a shipped function goes through the same list (`undefine fun …`, `applies_when: "fun …("`). (completes #11's type half) | High | `gecko-extension-api/src/lib.rs`, `db/router.rs`, `mem-gecko/src/lib.rs` `MEM_MIGRATIONS` — REVIEW D1 | ✅ `previous_release_upgrades_to_the_shipped_schema` applies the pinned 95ebac1 schema (`tests/fixtures/mem_schema_95ebac1.tql`), asserts plain `define` fails `[DEX15]`, then upgrades twice; `migrations_lead_to_the_shipped_schema` keeps each migration in step with the shipped text; doc comments corrected |
| **F2** | **Harden function-aware re-apply.** (a) Order `redefine`s and new-function `define`s callee-first; report an unsatisfiable mutual signature change as a schema error. (b) End a function block at its `return …;` so a trailing `<type> plays/owns …;` statement or comment is not swallowed into a `redefine`. | Medium | `db/schema_apply.rs:45-120` — REVIEW D2, D3 | Unit tests for caller-before-callee and for a trailing additive statement; a Tier-2 test re-applies such a fragment twice |
| **F3** | **Populations writer atomicity and truthful reporting.** Make open/close/refresh ops self-guarding (no second open row; stale guard in the match), add a per-population lock around read-plan-write, report attempted counts as attempted, bound `at` against the host clock, and error on an unparseable stored datetime. Fix the module doc that says the read happens in the write transaction. | Medium | `writer.rs:403-544`, `populations.rs:14-15, 56-94`, `tql.rs:685-735` — REVIEW D4, D7a/b/e | A Tier-2 test runs two overlapping passes and finds one open row per member and a monotonic `computed-at` |
| **F4** | **Present-state `blast-radius` excludes closed membership rows**, matching `population-members`. Decide and document whether `EpistemicReader::blast_radius` accepts a member concept or the member entry stays `blast_radius_at`-only. | Low–Medium | `mem_functions.tql:67-83`, `writer.rs:1110-1121` — REVIEW D6 | A departed member no longer sweeps its former cohort through `blast-radius`; applied to existing databases through F1's path |
| **F5** | **Confidence validation and a total-order comparator.** `MemWriter::assert_belief` rejects NaN/out-of-range confidence and a probabilistic confidence of 1.0 (invariant 7); the ranker compares with `f64::total_cmp`. | Low | `writer.rs:927-938`, `disposition.rs:128-138` — REVIEW D7c | Unit tests for both |
| **F6** | **Test and comment drift.** Remove the stale first doc block on `p_w5_1_folded_surface_contains_divergence_precedence`; stop the schema comment claiming the as-of functions mirror `believed-at` reading windows (it reads state — see B2). | Low | `schema_probe_w2w3.rs:2035-2045`, `mem_functions.tql:162-166` — REVIEW D7d, D5 | Text matches behavior |
| **F7** | **Split `MemWriter` before the seam lands.** Move the inline reader queries into `tql.rs` and break `writer.rs` into reader / populations / edge-writer submodules. | Maintainability | `writer.rs` (≈1,400 lines; 14 inline read queries) | No query text outside `tql.rs`; no behavior change |

---

## Track A — seam ① and what it unblocks

Owning document: SPEC-W5. Design inputs the review recommends settling first are decisions
**D-B** (reserved labels) and **D-D** (read-only graph handle); see REVIEW §3 for the full
list (label validation for interpolated identifiers, belief-plus-edges in one transaction,
the `GeckoExtension` declaration hook).

| ID | Item | Owning section | Type | Blocked on |
|---|---|---|---|---|
| **A1** | `BeliefEdgeDecl` registry and the sanctioned belief-edge writer method (was #1) | SPEC-W5 §3.2 | writer + registry + extension-API hook | D-B, D-D |
| **A2** | Cyber migration: `technique-detection` / `alert-disposition` written through the seam; raw `graph.write` at `detect/mod.rs:142,196` removed (was #12) | SPEC-W5 §3.3 | writer migration | A1 |
| **A3** | W5a static lint `no_raw_belief_tier_tql_outside_mem_tql` | SPEC-W5 §2.1, §5 row 1 | test | Buildable now. **Red until A2** — the cyber literals at `detect/mod.rs:133,186` are exactly what it flags. Needs a stated scope for test sources |
| **A4** | W5a dynamic rig: `memwriter_is_the_sole_belief_tier_writer` and `hostile_extension_raw_belief_write_is_flagged` (entity and relation) | SPEC-W5 §2.2, §5 rows 2–3, 8 | test rig | Rig buildable now; reconciliation red until A2 |
| **A5** | Divergence-exception writer: co-mints the `divergence-exception` belief and its `divergence-precedence` in one call; refuses an anchor-less or separately-fabricated anchor (was #8 and #13) | SPEC-W2-W3 §2.2; SPEC-W5 §3.2 | writer (mem-owned, dedicated method) | A1's edge-writing core, D-A, D-C |
| **A6** | Disposition writer — acts on the ranker's outcome (was #10, writer half). **Resolved:** mint via A5, or supersede the loser for instance-vs-instance. **Contested:** `contest()` (exists) plus a `belief-state "contested"` write (no writer today). Contract must also say how an evidence-less belief's method is read (REVIEW §2) | SPEC-W2-W3 §2.3, §2.4 | host engine + writer | Resolved half: A5, D-C. Contested half: D-E only |
| **A7** | `scoped` writer (belief → population). No production path writes `scoped`; the cohort branch of `blast-radius` has no input without it. Mem-owned, on the belief-edge surface | SPEC-W5 §1.1 (surface), SPEC-W2-W3 §3.4 | writer (dedicated method) | — |
| **A8** | Cyber episodic-sighting writer (`sighting-of` through the seam) (was #14) | SPEC-W5 §3.3, §3.4 | writer | A1 + a cyber sighting-ingestion feature (parked) |

Tests owed with these items (names reserved by the specs):

| Test | Owning section | Lands with |
|---|---|---|
| `belief_edge_decl_registry_writes_run_stamped` | SPEC-W5 §5 row 5 | A1 |
| `divergence_precedence_write_requires_cominted_anchor` | SPEC-W5 §5 row 6 | A5 |
| `cyber_detect_test` stays green | SPEC-W5 §5 row 7 | A2 |
| `empty_winner_yields_contested_anomaly_hub_no_exception_minted` (the hub-minting half; today's `host_side_ranking_reproduces_dispositions` covers the ranking half) | SPEC-W2-W3 §5 P-DISP-3 | A6 |
| Writer-level halves of the three partially graduated W2 tests (docstrings marked `Graduation scope (owed half)`) | SPEC-W2-W3 §5 | A5, A6 |

---

## Track B — bitemporal correctness of beliefs (new; from REVIEW D5)

No writer ever sets a belief's `valid-to`, and `believed-at` gates on present-state
`belief-state`. The as-of functions are therefore correct only on hand-built fixtures.

| ID | Item | Owning section | Type | Blocked on |
|---|---|---|---|---|
| **B1** | Supersession (and retraction) records when the belief stopped holding, per D-A | SPEC-W2-W3 §3.3; INVARIANTS inv 6 | writer | D-A |
| **B2** | `believed-at` answers as-of-t for a belief superseded after `t` | SPEC-W2-W3 §3.1, §3.3 (T5) | fn (TQL) | B1 |
| **B3** | `canonical-entity-at` stops following a resolution superseded at or before `t`; acceptance fixtures supersede through the writer instead of hand-writing `valid-to` | SPEC-W2-W3 §3.3 | fn (TQL) + tests | B1 |
| **B4** | b3 synthesis: fold the `resolves` branch into `blast-radius`, matching `record-a` and `record-b` (was #7; optional) | SPEC-W2-W3 §3.4, §5 (OWED-optional) | fn (TQL) | Its own green probe (R9), D-F |

---

## Track P — populations follow-ups

The writer landed (`define_population`, `materialize_population`,
`close_population_membership`); `population-member` is cache-tier and off the belief-edge
surface (SPEC-W5 §1.1, allowlisted in §1.2), so none of this is seam-gated.

| ID | Item | Owning section | Type | Blocked on |
|---|---|---|---|---|
| **P1** | A caller that evaluates a population's spec and calls `materialize_population` (daemon job or spec evaluator) | SPEC-W2-W3 §3.5, §3.6 | host code | F3 |
| **P2** | Close-on-supersession trigger: something calls `close_population_membership` when a member record is superseded | SPEC-W2-W3 §3.5 (first clause) | host code | F3; a definition of "member-supersession" for record-tier concepts |
| **P3** | Enforce "only the populations writer writes `population-member`". No test is named for this anywhere: SPEC-W5 §1.2 exempts the relation from accounting and §5's lint scans belief-edge types only. Smallest form: extend A3's lint with an explicit cache-tier list | SPEC-W2-W3 §3.5 (third clause) | test | — (shares A3's scanner) |
| **P4** | Rows written before windows existed: backfill `valid-from` or document that as-of reads skip them. Attribution: the writer takes no `RunContext`, so populations carry no run/actor stamp | — (REVIEW §2) | writer | — |

---

## Track C — CONTRACT-W8 consolidation daemon

All eleven job methods return `NotYetImplemented` today. CONTRACT-W8 §2 marks three write
surfaces as existing, so the track is not uniformly blocked.

| ID | Item | Owning section | Blocked on |
|---|---|---|---|
| **C1** | `tick` with one host-minted `RunContext` per tick and a reconciling `ConsolidationReport` | CONTRACT-W8 CF-3, §2a | — |
| **C2** | `dedup_content_hash`, `discover_typed_join_links`, `contradiction_sweep` (write surfaces exist: `dedup_episodes`, `assert_belief`, `contest`) | CONTRACT-W8 §2, §2a | C1 |
| **C3** | Similarity recorded on `surfaced`/`informs-synthesis` for an offline batch scan, then `synthesize_links_llm` (was #15) | CONTRACT-W8 §2 (CF-1) | New sanctioned method; D-H |
| **C4** | General `consolidation-state` transition writer, then `decay_archive_tombstone` and `ttl_prune_record_tier` (was #16) | CONTRACT-W8 §2 (CF-2) | New sanctioned method |
| **C5** | Provenance summarize/collapse writer, then `summarize_archived_provenance` and `collapse_superseded_provenance` (was #17) | CONTRACT-W8 §2, §5 | New method; D-J |
| **C6** | Episode rollup writer, then `rollup_high_volume_episodes` (was #18) | CONTRACT-W8 §2 | New sanctioned method |
| **C7** | Blast-radius cache `precompute`/`invalidate` (was #19). The windowed traversal it depended on has landed (`blast-radius-at`) | CONTRACT-W8 §2a, §4 | D-I |
| **C8** | `daemon_is_enumerated_by_the_monopoly_test` | CONTRACT-W8 §3 row 5 | A4 |

The remaining six checks in CONTRACT-W8 §3 (rows 1–4, 6, 7) land with the job each one names:
rows 1, 6, 7 with C1/C2; row 2 with C3; row 3 with C4; row 4 with C5/C6.

---

## Open decisions

| ID | Decision | Owning section | Gates |
|---|---|---|---|
| **D-A** | How a belief's validity ends: `supersede` closes `valid-to` and as-of reads use the window, **or** as-of functions read `supersession.created-at`. Includes whether `believed-at` keeps any state gate | SPEC-W2-W3 §3.3 (its description of `believed-at` does not match the shipped function — REVIEW D5) | B1–B3, A5 |
| **D-B** | Registry refuses relations defined by the core or mem fragments (mem-owned edges get dedicated writers only) | SPEC-W5 §3.1, §3.2 | A1 |
| **D-C** | What the disposition writer does when the ranker's winner cannot supersede the loser under the entrenchment guard (provable winner, human-asserted loser), and how an evidence-less belief is graded on the method key | SPEC-W2-W3 §2.3, §2.4; INVARIANTS §8.1, §8.2 | A5, A6 |
| **D-D** | Give extensions a read-only graph handle so a raw belief-tier write is impossible rather than merely detected | SPEC-W5 §2.1 (residual), §3.3 | A1, A2 |
| **D-E** | Which entity carries `belief-state "contested"` on a tie (each competing belief, or only the anomaly hub records it) | SPEC-W2-W3 §2.3 ("What `contested` returns") | A6 contested half |
| **D-F** | b3 recursion: does a resolution-bound member recurse into downstream cohort beliefs? | SPEC-W2-W3 §3.4 | B4 |
| **D-G** | Errata in ratified text, for the spec owner: SPEC-W5 §2.3/§4/§5 name a test that is now `p_w5_1_folded_surface_contains_divergence_precedence`; `writer.rs` line anchors in SPEC-W5, SPEC-W2-W3 and CONTRACT-W8 pre-date PR #50 (guard `806-824` → ≈`970-990`; `dedup_episodes` `442` → `609`) | SPEC-W5 §6; CONTRACT-W8 §2a footnote | — |
| **D-H** | Shape of the offline similarity-recording method (which edge, which run identity) | CONTRACT-W8 §2, §5 | C3 |
| **D-I** | Whether blast-radius caching stays a daemon job or becomes a separate `SemanticIndex`-style mechanism | CONTRACT-W8 §5 | C7 |
| **D-J** | Collapse granularity: which provenance is summarizable and which must survive | CONTRACT-W8 §5 / CF-2 | C5 |
| **D-K** | C2 `out-entrenches` schema hardening (OP-5). The §8.3 trigger has fired; shipping it owes a 5×5 drift-lock test against `Entrenchment::strength()` | SPEC-W2-W3 §2.3 (conditional); SPEC-W5 §2; INVARIANTS §8.3 | — (deliberate decision; D-D may make it unnecessary) |

---

## Landed

All of the following are on `main` via the squash **PR #50 (`8eedf85`)**. The per-item SHAs are
on `origin/conflict_resolution` only and will dangle if that branch is deleted.

| Was | Item | Owning section | Commit | Residual |
|---|---|---|---|---|
| #2 | C1 `divergence-exception` + `divergence-precedence` (4 roles) | SPEC-W2-W3 §2.1, §2.2 | `319f808` | Writer: A5 |
| #3 | `population-member` window (`valid-from`/`valid-to`) | SPEC-W2-W3 §3.2 | `319f808` | Pre-#50 databases: F1 |
| #4 | `blast-radius` widened to `okf-concept` (b1) | SPEC-W2-W3 §3.4 | `319f808` | Closed rows: F4 |
| #5 | `population-members-at`, `canonical-entity-at` | SPEC-W2-W3 §3.3 | `319f808` | B3 |
| #6 | `blast-radius-at` + `MemWriter::blast_radius_at`; `windowed_blast_radius_traversal_respects_membership_window` | SPEC-W2-W3 §3.4, §5 | `ba25e0b` | — |
| #9 | Populations writer + pure planner | SPEC-W2-W3 §3.5 | `4c72a95` | F3, P1–P4 |
| #10 (ranker) | `mem_gecko::disposition::rank`, `DerivationMethod::rank` | SPEC-W2-W3 §2.3 | `3dbb393` | F5; writer half: A6 |
| #11 | Function-aware schema re-apply (`db::schema_apply`, `TypeDbRouter::apply_schema`) — fixes `[FUN5]` on re-run | SPEC-W2-W3 §1.4 | `ba25e0b` | **Partial:** F2 (F1 landed) |
| — | §5 acceptance suite: 20 tests run by default, 5 evidence probes `#[ignore]` (C2, b2, b3 ×2, compositional windowed) | SPEC-W2-W3 §5 | `09467b9`, names `4163fa8` | F6 |
| — | W5a surface probe (membership form) | SPEC-W5 §2.3, §5 row 4 | `09467b9` | D-G |

Test-naming decision carried from the previous ledger: shipped tests use SPEC-W2-W3 §5's
graduated names; the folded §5 rows stay split (`…_claim_defines` / `…_claim_holds`,
`…_reaches_cohort` / `…_reaches_cohort_recursive`); P-DISP-3 keeps
`host_side_ranking_reproduces_dispositions` until A6 lands the hub-minting half.

---

## Implementation notes (empirical, TypeDB 3.12)

Observed on the live engine; recorded so they are not re-derived.

- **`[DEX15]` — a changed type annotation is not re-`define`-able.** Re-`define` is a no-op
  only for an identical definition; a different `@doc` on an existing type is refused.
  `redefine relation <name> @doc("…");` replaces it. (REVIEW D1.)
- **`[REP3]`/`[REX38]` — functions are validated one at a time on `redefine`,** against the
  stored versions of their callees, even inside a single `redefine` block. Callee-first order
  is required for a coordinated signature change. (REVIEW D2.)
- **`[REX27]` — `redefine` of an identical `owns … @card(…)` is an error,** so a type statement
  carried into a function's `redefine` text fails on re-apply. (REVIEW D3.)
- **`[FUN5]` — a persisted function is immutable under `define`,** for a changed and for a
  byte-identical body alike. (SPEC-W2-W3 §1.4, P-REDEF.)
- **`[REP1]` value-into-attribute.** `insert $x has attr $v` where `$v` is a function- or
  `reduce`-returned value fails at compile ("cannot be declared as both a 'Attribute' and as
  a 'Value'"). Writers that persist a computed score must bind an attribute instance; a
  `given` parameter with `has attr == $p` is fine (the populations writer uses that form).
- **`let $x in <single-valued fn>` yields one row.** A `return first` scalar is coerced to a
  one-row stream, so `select-for-context`'s `let $score in retrieval-score($b, $now)` is
  well-formed. Settled.

---

## Outside this ledger

- **Cyber-gecko C1/C2/C3** is a separate, mostly unblocked domain track. Its only touchpoint
  here is the sighting writer (**A8**).
- **Not owed by W2/W3/W5/W8:** W1 `attested()`, W4 (scope invariant 3 to activation), W6
  (reconciled design doc), W7 (split OP-6), W-T6 (IOC-convergence test) — INVARIANTS §8.4.
