# OWED-WORK-LEDGER — owed work across W2 / W3 / W5 / W8

> **Status: living working-surface** (not a ratified spec — it tracks, it does not decide).
> The source of truth for each item is its owning document's cited section; this ledger is the
> index and the sequencing. Last reconciled against `main` @ `43fcb70` (PR #52) on 2026-11-19;
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
- **Fixed after the review:** a database initialized before PR #50 could not be upgraded (**F1**, PR #51); the rest of track F (**F2–F7**) landed in PR #52.
- **Decided 2026-11-19:** **D-B** (ownership rule) and **D-D** (no write-capable handle for extensions), recorded as SPEC-W5 §3.5. A1 is unblocked.
- **Keystone:** the `BeliefEdgeDecl` seam (**A1**). It unblocks the cyber migration, both W5a
  tests going green, the divergence-exception writer, and the resolved half of the disposition
  writer. It does **not** gate the populations work, the bitemporal track, or the three daemon
  jobs listed under track C as buildable now.

## Sequence

```text
F1 ✅ ───────────────────────────► (pre-#50 databases upgrade again)
F2 ✅ F3 ✅ F4 ✅ F5 ✅ F6 ✅ F7 ✅ ─► track F complete

D-B ✅ D-D ✅ ─► A1 ──┬─► A2 + A9 ──► A3 green, A4 green
                  ├─► A5 (also needs D-A, D-C)
                  ├─► A6 resolved half (also needs D-C)
                  └─► A8 (parked until cyber builds sighting ingestion)
A6 contested half ─► needs D-E only (no seam)
A7 ────────────────► no blocker
A10 ───────────────► no blocker; must land before a second extension

D-A ──► B1 ──► B2, B3 ──► B4 (optional; also needs D-F)

P1, P2 (F3 ✅) ─► buildable    P3 ─► no blocker

C1, C2 ────────────────► buildable now
C3 ──► needs D-H       C4–C6 ─► new methods     C7 ─► needs D-I     C8 ─► needs A4
```

---

## Track F — fix now (from REVIEW-2026-11; no blockers)

| ID | Item | Severity | Where | Done when |
|---|---|---|---|---|
| **F1** ✅ | **Schema upgrade path for changed type annotations — LANDED.** `SchemaMigration { id, applies_when, tql }` in `gecko-extension-api`, `GeckoExtension::schema_migrations()`, and `TypeDbRouter::apply_schema_with_migrations`: pending migrations run first, in the extension's schema transaction. Migrations are **self-detecting** rather than keyed by a stored version: one runs only while the database's exported schema still contains its `applies_when` text. A stored marker cannot work for the first migration, because databases created by `main` @`8eedf85` already hold the new `@doc` and have no marker, and an identical `redefine` is refused (`[REX25]`). First entry: `mem/population-member-window-doc`. Dropping a shipped function goes through the same list (`undefine fun …`, `applies_when: "fun …("`). (completes #11's type half) | High | `gecko-extension-api/src/lib.rs`, `db/router.rs`, `mem-gecko/src/lib.rs` `MEM_MIGRATIONS` — REVIEW D1 | ✅ `previous_release_upgrades_to_the_shipped_schema` applies the pinned 95ebac1 schema (`tests/fixtures/mem_schema_95ebac1.tql`), asserts plain `define` fails `[DEX15]`, then upgrades twice; `migrations_lead_to_the_shipped_schema` keeps each migration in step with the shipped text; doc comments corrected |
| **F2** ✅ | **Function-aware re-apply hardened — LANDED.** The planner compares each fragment function with the database's exported text (which preserves function bodies verbatim): unchanged functions are skipped, new ones `define`d and changed ones `redefine`d, ordered callee-first by strongly connected components of the call graph. Mutually recursive functions are written together; if one of them already exists with a different signature the plan is refused with a schema error. A function block ends at the `;` closing its `return`, so a later statement goes to the type `define` and a later comment stays harmlessly with the function. | Medium | `db/schema_apply.rs`, `db/router.rs` — REVIEW D2, D3 | ✅ 13 planner unit tests (callee-first, new caller after changed callee, mutual signature change refused, multi-line `return`, trailing statement); Tier-2 `trailing_statement_after_a_function_reapplies` (×3) and `callee_signature_change_lands_before_its_caller` (caller first in source, callee gains a parameter, applied twice) |
| **F3** ✅ | **Populations writer guarded — LANDED.** The open op refuses a second open row and a row computed after `at`; close and refresh refuse a row computed after `at` (all in the write's `match`). A per-population lock in `MemWriter` serializes one writer's read-plan-write; `define_population` takes the same lock and reports a lost create race as "exists" when the specs agree. `MaterializeReport` fields are `opens_attempted` / `closes_attempted` / `refreshes_attempted`. A pass more than 5 minutes ahead of the host clock is rejected; an unparseable stored datetime is a `Storage` error. Module doc corrected. Cross-process passes still read in a separate transaction; the guards keep them from corrupting rows, but a stale pass can still open a member the fresher pass did not include. | Medium | `writer/populations.rs`, `populations.rs`, `tql.rs` — REVIEW D4, D7a/b/e | ✅ `populations_writer_overlapping_passes_keep_one_open_row`: two writers with separate locks race four rounds; one open row per member, no window closing before it opens, `computed-at` monotonic. Fails without the open guard (checked by removing it). Clock bound covered in `populations_writer_rejects_bad_input_and_writes_nothing` |
| **F4** ✅ | **Present-state `blast-radius` follows open membership rows only — LANDED.** Both population branches carry `not { $pm has valid-to $vt; }`. Decision: `EpistemicReader::blast_radius` stays memory-item-keyed; a member concept goes through `MemWriter::blast_radius_at` (stated in the trait doc). Reaches existing databases as a body-only `redefine`. | Low–Medium | `mem_functions.tql`, `epistemic.rs` — REVIEW D6 | ✅ `blast_radius_at_follows_the_materialized_window` also asserts present-state `blast-radius` sweeps the cohort while the member is in and nothing after it leaves |
| **F5** ✅ | **Confidence validated; ranker totally ordered — LANDED.** `assert_belief` and `supersede` reject a confidence that is NaN or outside `[0, 1]`; `assert_belief` rejects 1.0 from a probabilistic method (invariant 7). The ranker reads a NaN confidence as absent (method-defaulted) and compares with `f64::total_cmp`. | Low | `writer/beliefs.rs`, `disposition.rs` — REVIEW D7c | ✅ `confidence_validation_enforces_range_and_invariant_7`, `nan_confidence_is_method_defaulted_and_order_independent` |
| **F6** ✅ | **Test and comment drift — LANDED.** Stale P-W5-1 doc block removed; the as-of comment now says `believed-at` reads state. | Low | `schema_probe_w2w3.rs`, `mem_functions.tql` — REVIEW D7d, D5 | ✅ Text matches behavior |
| **F7** ✅ | **`MemWriter` split — LANDED.** `writer.rs` is now `writer/{mod, index, reader, beliefs, populations, consolidation, tests}.rs`; the 14 inline read queries are builders in `tql.rs`. The edge-writer submodule is created with A1, since no edge writer exists yet. | Maintainability | `core/mem-gecko/src/writer/` | ✅ No query text under `writer/`; no behavior change (full suite green) |

---

## Track A — seam ① and what it unblocks

Owning document: SPEC-W5, as amended by §3.5 (decisions **D-B** and **D-D**). The other
design inputs in REVIEW §3 — label validation for interpolated identifiers, belief-plus-edges
in one transaction, the `GeckoExtension` declaration hook — are A1's to settle.

| ID | Item | Owning section | Type | Blocked on |
|---|---|---|---|---|
| **A1** | `BeliefEdgeDecl` registry and the sanctioned belief-edge writer method (was #1). The registry accepts only relations the declaring extension's fragment introduces; mem-owned edges are dedicated methods, not declarations | SPEC-W5 §3.2, §3.5 (D-B) | writer + registry + extension-API hook | — |
| **A2** | Cyber migration: `technique-detection` / `alert-disposition` written through the seam; raw `graph.write` at `detect/mod.rs:142,196` removed (was #12) | SPEC-W5 §3.3 | writer migration | A1 |
| **A3** | W5a static lint `no_raw_belief_tier_tql_outside_mem_tql` | SPEC-W5 §2.1, §5 row 1 | test | Buildable now. **Red until A2** — the cyber literals at `detect/mod.rs:133,186` are exactly what it flags. Needs a stated scope for test sources |
| **A4** | W5a dynamic rig: `memwriter_is_the_sole_belief_tier_writer` and `hostile_extension_raw_belief_write_is_flagged` (entity and relation) | SPEC-W5 §2.2, §5 rows 2–3, 8 | test rig | Rig buildable now; reconciliation red until A2 |
| **A5** | Divergence-exception writer: co-mints the `divergence-exception` belief and its `divergence-precedence` in one call; refuses an anchor-less or separately-fabricated anchor (was #8 and #13) | SPEC-W2-W3 §2.2; SPEC-W5 §3.2 | writer (mem-owned, dedicated method) | A1's edge-writing core, D-A, D-C |
| **A6** | Disposition writer — acts on the ranker's outcome (was #10, writer half). **Resolved:** mint via A5, or supersede the loser for instance-vs-instance. **Contested:** `contest()` (exists) plus a `belief-state "contested"` write (no writer today). Contract must also say how an evidence-less belief's method is read (REVIEW §2) | SPEC-W2-W3 §2.3, §2.4 | host engine + writer | Resolved half: A5, D-C. Contested half: D-E only |
| **A7** | `scoped` writer (belief → population). No production path writes `scoped`; the cohort branch of `blast-radius` has no input without it. Mem-owned, on the belief-edge surface | SPEC-W5 §1.1 (surface), SPEC-W2-W3 §3.4 | writer (dedicated method) | — |
| **A9** | Read-only graph handle and neutral sync writes: `GraphReader` split out of `GraphStore`; extension bridges get `Arc<dyn GraphReader>`; the post-sync hook returns `GraphWrite`s instead of taking a driver transaction | SPEC-W5 §3.5 (D-D steps 1–2) | extension-API + engine + cyber | A1; lands with A2 |
| **A10** | Extensions depend only on `gecko-extension-api`: move `ExtensionCallback` and the OKF import types into the API crate; CI guard fails if a crate under `extensions/` depends on `typedb-driver` or `gecko-engine` | SPEC-W5 §3.5 (D-D step 3) | API refactor + CI | — (before a second extension) |
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
| **D-B** ✅ | **Decided 2026-11-19 — ownership rule.** The registry accepts only a relation the declaring extension's own fragment introduces (not core, mem, another extension, or a role-addition); mem-owned edges get dedicated `MemWriter` methods | SPEC-W5 §3.5 | A1 |
| **D-C** | What the disposition writer does when the ranker's winner cannot supersede the loser under the entrenchment guard (provable winner, human-asserted loser), and how an evidence-less belief is graded on the method key | SPEC-W2-W3 §2.3, §2.4; INVARIANTS §8.1, §8.2 | A5, A6 |
| **D-D** ✅ | **Decided 2026-11-19 — read-only handle and no driver route.** Extensions get `GraphReader` only, return neutral writes at sync time, and depend only on `gecko-extension-api` (CI-guarded). W5a stays the backstop | SPEC-W5 §3.5 | A9, A10 |
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
| #4 | `blast-radius` widened to `okf-concept` (b1) | SPEC-W2-W3 §3.4 | `319f808` | — (F4 ✅) |
| #5 | `population-members-at`, `canonical-entity-at` | SPEC-W2-W3 §3.3 | `319f808` | B3 |
| #6 | `blast-radius-at` + `MemWriter::blast_radius_at`; `windowed_blast_radius_traversal_respects_membership_window` | SPEC-W2-W3 §3.4, §5 | `ba25e0b` | — |
| #9 | Populations writer + pure planner | SPEC-W2-W3 §3.5 | `4c72a95` | P1–P4 (F3 ✅) |
| #10 (ranker) | `mem_gecko::disposition::rank`, `DerivationMethod::rank` | SPEC-W2-W3 §2.3 | `3dbb393` | Writer half: A6 (F5 ✅) |
| #11 | Function-aware schema re-apply (`db::schema_apply`, `TypeDbRouter::apply_schema`) — fixes `[FUN5]` on re-run | SPEC-W2-W3 §1.4 | `ba25e0b` | — (F1, F2 ✅) |
| — | §5 acceptance suite: 20 tests run by default, 5 evidence probes `#[ignore]` (C2, b2, b3 ×2, compositional windowed) | SPEC-W2-W3 §5 | `09467b9`, names `4163fa8` | — (F6 ✅) |
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
- **`database.schema()` exports each function's source text verbatim** (unindented, comments
  dropped), so the schema planner can compare a fragment's function with the stored one after
  normalizing whitespace and comments.
- **Nested negation works in a write's `match`** — `not { $o isa population-member, links (…);
  not { $o has valid-to $vt; }; };` guards an `insert` against a second open row.

---

## Outside this ledger

- **Cyber-gecko C1/C2/C3** is a separate, mostly unblocked domain track. Its only touchpoint
  here is the sighting writer (**A8**).
- **Not owed by W2/W3/W5/W8:** W1 `attested()`, W4 (scope invariant 3 to activation), W6
  (reconciled design doc), W7 (split OP-6), W-T6 (IOC-convergence test) — INVARIANTS §8.4.
