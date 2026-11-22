# OWED-WORK-LEDGER — owed work across W2 / W3 / W5 / W8

> **Status: living working-surface** (not a ratified spec — it tracks, it does not decide).
> The source of truth for each item is its owning document's cited section; this ledger is the
> index and the sequencing. Last reconciled on 2026-11-20 against PR #57 (cyber seam migration)
> and PR #58 (current ATT&CK import); the evidence for every review-sourced item is in
> [`REVIEW-2026-11.md`](./REVIEW-2026-11.md).
>
> **Ratification provenance:** SPEC-W2-W3 decisions @`11d7ab1`; SPEC-W5 as-design @`95f0d6e`;
> CONTRACT-W8 as-contract (self-anchored, `fdd1dc6`).

**Reading the IDs.** Items are grouped by track: **F** fix-now, **A** seam ①, **B** bitemporal
correctness, **P** populations, **C** consolidation daemon, **S** STIX/ATT&CK ingest,
**O** OKF bundles and OKF v0.2, **U** TypeDB upgrade, **R** TypeDB review fixes, **D-** open decisions. A number in
parentheses — "(was #12)" — is the row number the item had in the previous ledger, kept so
references in specs and commit messages still resolve.

## Where things stand

- **Landed in PR #50:** the W2/W3 schema fold, the as-of functions, `blast-radius-at`, the
  disposition ranker (pure half), the populations writer, function-aware schema re-apply, and
  the §5 acceptance suite (20 tests run by default, 5 evidence probes ignored).
- **Fixed after the review:** a database initialized before PR #50 could not be upgraded (**F1**, PR #51); the rest of track F (**F2–F7**) landed in PR #52.
- **Decided 2026-11-19:** **D-B** (ownership rule) and **D-D** (no write-capable handle for extensions), recorded as SPEC-W5 §3.5.
- **Landed in the belief-edge seam PR (branch `feat/belief-edge-seam`, base `389ac2c`):** the
  keystone `BeliefEdgeDecl` seam (**A1** ✅), write-count enforcement (`GraphWrite.expect_rows`),
  and the `supersede`-of-a-missing-belief fix. The seam unblocks the cyber migration, both W5a
  tests going green, the divergence-exception writer, and the resolved half of the disposition
  writer; it does **not** gate the populations work, the bitemporal track, or the three daemon
  jobs listed under track C as buildable now. The remaining mem writes owe the same count
  (**A11**).
- **Landed in the TypeDB 3.15 upgrade PR (branch `chore/typedb-3.15`):** server and driver at
  3.15.0 (**U1** ✅), and the registry refusing type and role renames.
- **Landed in PR #57 (cyber seam migration):** cyber writes its belief edges through the seam
  (**A2** ✅) over a read-only graph handle, with STIX post-sync writes inside the sync
  transaction (**A9** ✅); the registry refuses extension `undefine`/`redefine` of core and mem
  at schema time (**A12** ✅); the W5a static lint is green (**A3** ✅). **A10** was partial.
- **Found while timing a full ATT&CK import:** the cyber schema and STIX adapter were written
  for ATT&CK's earlier data model and could not import the current Enterprise bundle. The
  import fix landed in PR #58 (**S1** ✅).
- **Landed in the detection-model branch (`feat/attack-detection-model`):** cyber models
  ATT&CK's current detection data (**S2** ✅, **D-L** decided): detection strategies, analytics
  and their log-source requirements are typed, `gecko sync-sensors` loads the platforms and
  sensors a user runs, and `blinded` reports per claim the blind and unassessed platforms.
  The sync engine retypes a concept whose `type_hint` changed. The opt-in full-bundle check
  (**S4** ✅) is validated against Enterprise ATT&CK 20.1.
- **Landed in the multi-bundle databases PR (branch `feat/multi-bundle-databases`):** several
  bundles can share a database (**O1** ✅), which unblocks cyber's detection flow end to end.
  OKF v0.2 adoption is owed (**O2–O5**).
- **Landed in the belief-tier guard PR (branch `feat/belief-tier-guard`):** **A10** ✅ (the OKF
  import types and `ExtensionCallback` moved into `gecko-extension-api`, cyber depends on the API
  crate alone, `scripts/check-extension-deps.sh` guards it in CI, and no extension is offered the
  writer) and **A4** ✅ (a sync refuses to change the belief tier, in production, and retires rather
  than deletes a concept a belief edge references, see O6; the
  `memwriter_is_the_sole_belief_tier_writer` rig and the hostile-write test).
  Owed follow-on: Track M for the belief-tier types no production path writes.
- **Landed in the mem write-counts PR (branch `fix/mem-write-counts`):** **A11** entries 1–3 ✅
  (a derive, contest, `rests_on` or `record_prediction` whose ids match nothing fails and
  commits nothing; the sandbox bridge refuses a malformed id list) and **C8** ✅ (a daemon tick
  is a counted drive in the reconciliation rig).
- **Landed in the TypeDB review's correctness pass (branch `fix/query-correctness`, pass 1):**
  as-of recall is gated by the `visible-to` function and returns newest first; a supersede
  requires an asserted belief; `resolve_prediction`, `informs-synthesis` and `was-used` require
  exactly one row; retrieval tombstoning waits for every informed belief; a memory item requires
  one ownership and at least one source-link; link, citation and SRO rows are deduplicated
  host-side; precision counts only asserted dispositions (**R1**). Pass 2 (schema tightening)
  is owed (**R2**).
- **Landed in the daemon tick PR (branch `feat/daemon-tick-dedup`):** **C1** ✅ (a
  `MemConsolidationDaemon` tick mints one run and its writes share it) and the `dedup_content_hash`
  half of **C2** ✅ (`dedup_episodes` enforces the duplicate rule in one guarded write and records a
  `consolidation-step`); CONTRACT-W8 §3 row 7 ✅. The `daemon tick` drive of the reconciliation rig
  runs the real daemon.

## Sequence

```text
F1 ✅ ───────────────────────────► (pre-#50 databases upgrade again)
F2 ✅ F3 ✅ F4 ✅ F5 ✅ F6 ✅ F7 ✅ ─► track F complete

D-B ✅ D-D ✅ ─► A1 ✅ ┬─► A2 ✅ + A9 ✅ ──► A3 ✅ green, A4 ✅ green
                  ├─► A5 (also needs D-A, D-C)
                  ├─► A6 resolved half (also needs D-C)
                  └─► A8 (parked until cyber builds sighting ingestion)
A6 contested half ─► needs D-E only (no seam)
A7 ────────────────► no blocker
A10 ✅ ─────────────► landed with A4
A11 ───────────────► entries 1–3 ✅; entry 4 needs a uniqueness decision
R1 ✅ ──► R2 (schema tightening plan)

D-A ──► B1 ──► B2, B3 ──► B4 (optional; also needs D-F)

P1, P2 (F3 ✅) ─► buildable    P3 ─► no blocker

C1 ✅, C2 (dedup ✅; join-links and contradiction sweep buildable now)
C3 ──► needs D-H       C4–C6 ─► new methods     C7 ─► needs D-I     C8 ✅

U1 ✅ ──────────────► server + driver on 3.15.0
A12 ✅ ────────────► extension undefine/redefine of core/mem refused at schema time

O1 ✅ ─► A4 ✅ + A10 ✅
O2, O3, O4 ─► independent        O5 ─► needs D-N
S5 ─► needs a design (after O1)

S1 ✅ ─► A4 ✅ + A10 ✅ ─► D-L ✅ ─► S2 ✅ ─► S3 (needs D-M)
                                    S4 ✅ (the full-bundle check S2 is accepted against)
```

Recommended next, now that **S2** has landed: settle **D-M** so **S3** starts.

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
| **A1** ✅ | **`BeliefEdgeDecl` registry and the sanctioned belief-edge writer — LANDED in the belief-edge seam PR (branch `feat/belief-edge-seam`, base `389ac2c`)** (was #1). Extensions declare edges through `GeckoExtension::belief_edges()`; `BeliefEdgeRegistry::from_extensions` accepts only relations the declaring extension's fragment introduces, and `gecko run` builds it in `build_host_bridge` (a bad declaration fails the run with `belief-edge declarations`). `MemWriter::edge_writer_for(ext)` returns a `BeliefEdgeWriter` bound to that extension, whose `assert_belief_with_edges` writes the belief and its edges in one transaction, every edge op `expect_rows = Some(1)`. The scanner fails closed: ownership is refused when any other fragment mentions the relation or one of its subtypes, and a non-belief player is refused unless its whole supertype chain resolves outside `memory-item`. Mem-owned edges stay dedicated methods. No production extension declares edges yet (cyber does in A2) | SPEC-W5 §3.2, §3.5 (D-B) | writer + registry + extension-API hook | ✅ Tier 2 (`core/mem-gecko/tests/belief_edge_integration.rs`): `belief_edge_decl_registry_writes_run_stamped`, `missing_player_commits_no_belief`, `value_outside_values_commits_no_belief`, `non_unique_key_commits_no_belief`, `live_schema_mismatch_is_refused_before_writing`, `two_edges_of_one_relation_both_land`, `expect_rows_accepts_an_exact_insert_match`, `expect_rows_rolls_back_a_zero_match`, `expect_rows_rolls_back_a_double_match`, `expect_rows_counts_an_update`; `supersede_of_a_missing_belief_errors_and_commits_nothing`. Unit: the registry suite in `belief_edge/registry.rs` (incl. `refuses_a_mem_relation`, `refuses_a_relation_another_extension_subtypes`, `refuses_memory_item_players`, `refuses_an_extension_fragment_with_an_alias`), the scanner suite in `belief_edge/scan.rs`, `bound_writer_writes_belief_and_edges_in_one_call`, `edge_writer_for_refuses_an_extension_with_no_declarations`, `edge_op_keeps_values_out_of_query_text`, `ownership_is_refused_before_confidence_is_checked`, `a_row_count_mismatch_names_the_failing_edge`, and `graph::tests::row_count_mismatch_*` in the API crate. Shipped set: `from_extensions_accepts_the_shipped_extensions` (`gecko-bin/tests/belief_edge_registry_test.rs`, `cyber` feature) |
| **A2** ✅ | **Cyber migration — LANDED in PR #57.** `technique-detection` and `alert-disposition` are declared through `CyberGecko::belief_edges()` and written with their belief in one transaction by the bound `BeliefEdgeWriter`; the raw `graph.write` literals in `detect/mod.rs` are gone (was #12) | SPEC-W5 §3.3 | writer migration | ✅ `detect_bridge_claims_disposes_and_reports` and `edge_to_an_unknown_technique_commits_no_belief` (`gecko-bin/tests/cyber_detect_test.rs`); `cyber_bridge_requires_cyber_belief_edges` (`gecko-bin/src/main.rs`) |
| **A3** ✅ | **W5a static lint `no_raw_belief_tier_tql_outside_mem_tql` — LANDED in PR #57.** The belief-tier surface (the `memory-item` subtype closure plus every belief-edge relation) is introspected from the composed core→mem→cyber schema. Scope is production sources: `.rs` under each crate's `src/` with inline `#[cfg(test)]` modules blanked, and `.tql` files, outside `tests`/`target`. A hit is a write verb (`insert`/`put`/`update`/`delete`) plus a surface label after `isa`/`isa!` or directly before `(`; the positive control asserts hits in `core/mem-gecko/src/tql.rs` | SPEC-W5 §2.1, §5 row 1 | test | ✅ `gecko-bin/tests/belief_tier_monopoly_test.rs` (`cyber` feature, Tier 2), plus 9 serviceless detector unit tests including `flags_the_pre_seam_cyber_literals` and `flags_an_isa_bang_delete` |
| **A4** ✅ | **W5a dynamic rig and belief-tier guard — LANDED in the belief-tier guard PR.** `memwriter_is_the_sole_belief_tier_writer` (`gecko-bin/tests/belief_tier_reconciliation_test.rs`, `cyber`, live TypeDB) reconciles every production path against an in-process `MemWriter` reference database by the production fingerprint; `hostile_extension_raw_belief_write_is_flagged` covers entity and relation. `sync_bundle` refuses a belief-tier change (`DbError::BeliefTierWrite`) and retires, rather than deletes, a concept a belief edge references (O6). Residual: a same-count swap passes the fingerprint. Labels with no other ledger owner are owned by Track M (**M1**–**M6**) | SPEC-W5 §2.2, §5 rows 2–3, 8 | test rig + engine guard | O1 |
| **A5** | Divergence-exception writer: co-mints the `divergence-exception` belief and its `divergence-precedence` in one call; refuses an anchor-less or separately-fabricated anchor (was #8 and #13) | SPEC-W2-W3 §2.2; SPEC-W5 §3.2 | writer (mem-owned, dedicated method) | A1's edge-writing core, D-A, D-C. Before A5 reuses that core, `tql::belief_edge_op` must return `Result` instead of `debug_assert!` + `continue` on a missing role value |
| **A6** | Disposition writer — acts on the ranker's outcome (was #10, writer half). **Resolved:** mint via A5, or supersede the loser for instance-vs-instance. **Contested:** `contest()` (exists) plus a `belief-state "contested"` write (no writer today). Contract must also say how an evidence-less belief's method is read (REVIEW §2) | SPEC-W2-W3 §2.3, §2.4 | host engine + writer | Resolved half: A5, D-C. Contested half: D-E only |
| **A7** | `scoped` writer (belief → population). No production path writes `scoped`; the cohort branch of `blast-radius` has no input without it. Mem-owned, on the belief-edge surface | SPEC-W5 §1.1 (surface), SPEC-W2-W3 §3.4 | writer (dedicated method) | — |
| **A9** ✅ | **Read-only graph handle and neutral sync writes — LANDED in PR #57.** Cyber's bridge holds an `Arc<dyn GraphReader>` and a bound `BeliefEdgeWriter`; the post-sync hook returns `GraphWrite`s that run inside the sync transaction | SPEC-W5 §3.5 (D-D steps 1–2) | extension-API + engine + cyber | ✅ `failing_extension_write_rolls_back_the_whole_sync` (`gecko-bin/tests/bundle_sync_test.rs`), `post_sync_groups_relations_into_one_write_per_shape` (`extensions/cyber-gecko/src/stix/mod.rs`) |
| **A10** ✅ | **Extensions depend only on `gecko-extension-api` — LANDED in the belief-tier guard PR.** `ExtensionCallback` and the OKF import types live in the API crate; `scripts/check-extension-deps.sh` fails CI if a crate under `extensions/` depends on `typedb-driver` or `gecko-engine`; no extension is offered the `EpistemicWriter` (`inject_epistemic_host_fns` is removed) | SPEC-W5 §3.5 (D-D step 3) | API refactor + CI | — |
| **A8** | Cyber episodic-sighting writer (`sighting-of` through the seam) (was #14) | SPEC-W5 §3.3, §3.4 | writer | A1 + a cyber sighting-ingestion feature (parked) |
| **A11** | **Write-count audit of the remaining mem writes.** A `match … insert` that matches nothing commits as a silent no-op. Entries 1–3 ✅ (mem write-counts PR): `derivation_op` (via `mem.derive`), the last op of `contest_ops` (via `mem.contest`), `rests_on_op` and `record_prediction_op` each require exactly one row, and the writer names the given ids in a `Storage` error ending "nothing was committed"; `contest` refuses a repeated claim, and the sandbox bridge and cyber's detect bridge refuse a non-array id list or a non-string entry. Still owed: (4) `resolve_prediction_ops`: `record_prediction` has no uniqueness guard (`core/mem-gecko/src/writer/beliefs.rs`), so the right count needs a decision first. `informs_synthesis_op` and `set_was_used_op` are now counted (`ProvenanceOp`), as are `resolve_prediction` and supersede (`BeliefOps`). Deferred, same shape, not guest-supplied ids: `retrieval_event_ops` (no expecting(1)) and `set_retrieval_provenance_op`. The dedup loser-tombstone is counted inside `dedup_episodes_ops`, whose guarded op requires exactly one row. Deferred, needs a `quickjs.wasm` rebuild: the guest drops a non-array `evidence` and turns a non-array `claims` into `[]` (`core/js-sandbox/src/main.rs`) before the host sees them. The populations ops match zero rows by design, as their guards, and get no count | Belief-edge seam design §2 | writer correctness | Entries 1–3 ✅ `derive_with_a_missing_evidence_id_errors_and_commits_nothing`, `contest_with_a_missing_claim_errors_and_commits_nothing`, `rests_on_with_a_missing_assumption_errors_and_commits_nothing`, `record_prediction_on_a_missing_belief_errors_and_commits_nothing` (`core/mem-gecko/tests/writer_reader_integration.rs`), `belief_with_edges_and_a_missing_evidence_id_names_the_evidence` (`belief_edge_integration.rs`), `a_malformed_id_list_is_refused_before_the_writer_runs` (`mem_host.rs`); entry 4: a uniqueness decision for predictions |
| **A12** ✅ | **Registry coverage of `undefine` and `redefine` — LANDED in PR #57.** The registry refuses an extension fragment or migration that `undefine`s or `redefine`s a type or function the core or mem schema introduces, including an own function whose redefined body names a base type. `check_extension_schemas` runs the check at schema time, before any schema is applied | SPEC-W5 §3.5 (D-B) | registry | ✅ `refuses_an_extension_that_modifies_a_base_type_or_function`, `accepts_an_extension_that_modifies_only_its_own_types`, `refuses_an_explicit_redefine_of_an_own_function_whose_body_names_a_base_type` (`belief_edge/registry.rs`); `check_refuses_an_extension_that_undefines_a_mem_type` (`gecko-bin/src/config.rs`); `apply_all_schemas_refuses_before_touching_the_database` (`gecko-bin/src/main.rs`) |

Tests owed with these items (names reserved by the specs):

| Test | Owning section | Lands with |
|---|---|---|
| `belief_edge_decl_registry_writes_run_stamped` ✅ (`core/mem-gecko/tests/belief_edge_integration.rs`) | SPEC-W5 §5 row 5 | A1 |
| `divergence_precedence_write_requires_cominted_anchor` | SPEC-W5 §5 row 6 | A5 |
| `cyber_detect_test` stays green ✅ | SPEC-W5 §5 row 7 | A2 |
| `empty_winner_yields_contested_anomaly_hub_no_exception_minted` (the hub-minting half; today's `host_side_ranking_reproduces_dispositions` covers the ranking half) | SPEC-W2-W3 §5 P-DISP-3 | A6 |
| Writer-level halves of the three partially graduated W2 tests (docstrings marked `Graduation scope (owed half)`) | SPEC-W2-W3 §5 | A5, A6 |

---

## Track S — STIX/ATT&CK ingest (cyber-gecko)

**Design of record:** the maintainer's *cyber-gecko Implementation Plan — detect-first, against
the shipped substrate* (2026-07-10, kept outside the repo), implemented in PR #11. Its
relationship typing is an extraction of the STIX 2.1 spec's Appendix B (relationship
summary), cut to what detect consumes; its §C1.3 data-source plane is ATT&CK's earlier data
model: data components that `detects` a technique and point at their data source through
`x_mitre_data_source_ref`. It cites no ATT&CK release and no upstream TypeQL schema. Its C2
acceptance criterion — syncing MITRE's enterprise-attack bundle yields matching
`attack-pattern`/`data-source`/`data-component` counts with `requires-evidence` edges, and an
SRO outside the typed subset lands as a `concept-link` — was never run against the real bundle
(the tests used a hand-built fixture); running it surfaced S1. Current Enterprise ATT&CK instead links a
technique to data components through a detection strategy (`detects`) and its analytics
(`x_mitre_analytic_refs`, then each analytic's `x_mitre_log_source_references[].x_mitre_data_component_ref`),
and its data components no longer reference a data source. Against a current import the
plan's `requires-evidence` and `evidence-of` stayed empty, so its `blinded` (the
`blinded-detections()` function, joining `requires-evidence` to `sensor-coverage`) could
never report anything; S2 replaced that model and removed those types. `coverage_gaps` and
`precision` read detection claims and dispositions and were unaffected. [typedb-cti](https://github.com/typedb-osi/typedb-cti) was compared as a
reference: it models STIX 2.1 core faithfully but has none of ATT&CK's `x-mitre-*`
extensions, so it informs S3, not S2.

| ID | Item | Owning section | Type | Blocked on |
|---|---|---|---|---|
| **S1** ✅ | **The current Enterprise ATT&CK bundle imports — LANDED in PR #58.** `data-source` owns `platform` and the adapter writes platforms only on subtypes that own it; deprecated and revoked objects (224 reused ids) are ingested without the unique `attack-id`; `tool` plays `uses:user`; each relation shape lists its role-player subtypes and a relation whose ends cannot play its roles is kept as a concept-link (706 strategy→`detects` today). Full bundle: 4,877 concepts and 20,478 typed relations in one transaction, 148 s first sync and 1.2 s re-sync, counts unchanged (debug build, local 3.15.0) | — | adapter + schema | ✅ `stix_sync_persists_typed_attributes_and_backbone`, `stix_sync_links_tool_uses_and_records_unmodeled_shapes_as_links`, `typed_relation_players_match_the_schema` |
| **S2** ✅ | **ATT&CK's current detection data is modelled — LANDED (branch `feat/attack-detection-model`, D-L).** `detection-strategy`, `analytic` and `analytic-requirement` are cyber types (analytics own `platform`; a requirement carries the log-source name, key and channels); the adapter returns `(OkfBundle, StixRelations)` and `cyber_post_sync` replaces a bundle's strategy→technique (`detects`), strategy→analytic and analytic→requirement relations. `gecko sync-sensors` loads a sensor file (platforms, sensors, log sources, fallback data components). `blinded` reports per claim `blind_platforms` and `unassessed_platforms`, built on the functions `requirement-covered`, `analytic-gaps` and `claim-platforms`; a platform with several analytics counts covered if any can run. `evidence-of`, `requires-evidence`, `sensor-coverage` and `blinded-detections` are removed by schema migrations, and pre-current bundles are no longer supported | — | schema + adapter + sensors + cyber functions | ✅ `stix_sync_persists_typed_attributes_and_backbone`, `cyber_detect_test`, `stix_full_bundle_resync_is_stable` |
| **S3** | **STIX core coverage.** Relationship types dropped today: `subtechnique-of` (481), `revoked-by` (157); object types ingested as bare `cyber-object`: `x-mitre-tactic`, `x-mitre-matrix`, `x-mitre-collection`, `identity`, `marking-definition`; `kill_chain_phases` and `external_references` flattened to attributes. typedb-cti is the reference for these shapes. The sync engine already retypes a concept whose `type_hint` changed (refused when a belief edge references it or another bundle contains it), so promoting a bare `cyber-object` to a subtype re-syncs in place | — (design owed, D-M) | schema + adapter | D-M; after S2 |
| **S4** ✅ | **Real-bundle regression check — LANDED.** Validated against **Enterprise ATT&CK 20.1**. The opt-in `stix_full_bundle_resync_is_stable` (skips unless `GECKO_ATTACK_BUNDLE` names the bundle, which stays a runtime asset, never in the build tree) imports the bundle twice into one database and asserts the concept count and every typed relation count are unchanged. 20.1 result: 4,877 concepts, 4,203 analytic requirements, 1,771 strategy-analytic, 706 strategy-detects, 1,477 mitigates, 18,975 uses and 26 attributed-to, identical across both syncs; first sync 156 s and re-sync 1.7 s (debug build, local 3.15.0; S1 baseline 148 s and 1.2 s). It then syncs a sensor file (Windows, Linux, macOS), claims three techniques and times `blinded` on the full graph: 1.25 s first call, 83 ms second, the two results equal and non-empty. Its first run failed: attaching channels by matching each requirement back by name took 147 s and the sync passed TypeDB's 300 s transaction timeout, so the adapter now inserts a requirement with its first channel and attaches only the further ones. The upgrade path was timed once by hand against 20.1: a sync with main's binary (`027f36a`, 150 s, 4,877 inserted), then a re-sync with this branch's binary, which applies the cyber schema migrations and updates all 4,877 concepts in place in one transaction (retypes included): 31.3 s in all, the sync transaction about 30 s, well inside the 300 s limit; a further re-sync skipped all 4,877 in 2.5 s (debug builds, local 3.15.0) | — | test (opt-in) + adapter | ✅ `stix_full_bundle_resync_is_stable` |
| **S5** | **Objects shared across STIX imports.** Two ATT&CK domains (Enterprise and Mobile) cannot share a database. They ship the same MITRE `identity` and `marking-definition` objects, and some groups and software, under the same STIX ids, so the second import is refused as a concept-ID collision (O1). Under the default bundle name `stix-sync`, a second domain replaces the first instead, and a record the new import drops that a belief edge references is retired, not deleted (O6). Sharing objects across imports needs a design: an object held by several imports (multi-containment ownership, collected only when no import holds it) and `attack-id` uniqueness across domains. | — (design owed) | design + engine + adapter | O1; the design |

---

## Track O — OKF bundles and OKF v0.2

[OKF v0.2](https://raw.githubusercontent.com/GoogleCloudPlatform/knowledge-catalog/refs/heads/main/okf/SPEC.md) supersedes v0.1, which GECKO's parser implements. Concept IDs (file paths), links (§6.1, broken links tolerated) and bundle layout already match. The items are what v0.2 renames or adds (§13), plus O1, which v0.2 does not require but cyber's detection flow does. A v0.1 bundle stays consumable under v0.2's fallbacks.

| ID | Item | Owning section | Type | Blocked on |
|---|---|---|---|---|
| **O1** ✅ | **Several bundles in one database — LANDS in the multi-bundle databases PR (branch `feat/multi-bundle-databases`).** A sync acts only on the concepts its bundle contains (GC, change detection, containment, hierarchy, links). GC releases, rather than deletes, a concept another bundle also contains. An incoming concept ID held by another bundle or by none is refused with `DbError::ConceptIdCollision`, and nothing is written. Links and hierarchy resolve within the bundle. `sync-stix --bundle-name` lets several imports share a database. One bundle per database stays the default. It also adds `CORE_MIGRATIONS`, whose first entry, `core/bundle-shared-database-doc`, brings the `bundle` `@doc` forward on existing databases. Done when: in `multi_bundle_test.rs`, `two_bundles_share_a_database`, `resync_garbage_collects_only_its_own_bundle`, `colliding_concept_id_is_refused`, `uncontained_concept_counts_as_a_collision`, `renamed_bundle_is_refused_with_guidance`, `empty_bundle_deletes_nothing_from_others`, `shared_legacy_concept_is_released_not_deleted`, `link_does_not_cross_bundles`, `link_does_not_cross_bundles_in_either_order`, `updated_concept_relinks_within_its_bundle`, and in `cli::` `sync_stix_bundle_name_override`, `second_attack_domain_is_refused`, `bundle_name_override_selects_the_default_database`, `playbooks_and_attack_share_a_database`; `core_migrations_lead_to_the_shipped_schema` and `previous_core_schema_upgrades_to_the_shipped_schema`; and `cyber_detect_test` seeding through a synced bundle. Links and hierarchy are scoped to the bundle by filtering on the manifest's concept IDs, not by containment joins in the write. Full Enterprise ATT&CK v20.1 import (4,877 concepts, 20,478 typed relations, debug build, local TypeDB 3.15): 143–152 s first sync over 3 runs, 1 s re-sync | `docs/superpowers/specs/2026-11-20-multi-bundle-databases-design.md` | engine + CLI | — |
| **O2** | **`generated: {by, at}` replaces `timestamp`** (§5.2, §13.1; breaking). GECKO reads `timestamp` at `core/gecko-engine/src/okf/parser.rs:172` into the `timestamp` attribute. Read `generated.at` (and `generated.by`, §7 actor convention), with the v0.1 `timestamp` fallback v0.2 allows | OKF v0.2 §5.2, §13.1 | parser + schema | — |
| **O3** | **`sources` frontmatter and keyed footnotes replace body citations** (§5.1, §13.1; breaking). GECKO treats every http(s) body link as a citation (`core/gecko-engine/src/okf/linker.rs:35`). Read `sources` (and its credibility signals) into `external-resource`/`citation`, keeping the body parse as the v0.1 fallback | OKF v0.2 §5.1, §13.1 | parser + schema | — |
| **O4** | **Trust and lifecycle fields:** `verified` (§5.2; trust tiers keyed on `human:` actors, §5.3, §7), `status` (§5.4), `stale_after` (§5.5), and `okf_version` in the root `index.md` (§8, §12). Today the concept-level keys land only in `metadata-json`, and `index.md` is a reserved file the parser skips. Decide how they map to GECKO and to mem's belief and provenance model: a human-verified record versus a belief's confidence and entrenchment, and `stale_after` versus validity windows | OKF v0.2 §5.2–5.5, §7, §8, §12 | design + parser | — (the mapping is the item's first step) |
| **O5** | **`Attested Computation` concept type** (§10): `runtime`, `parameters`, `computation`, `executor` with its `receipt`, and a deterministic `attester`. It overlaps GECKO's playbooks, `engine`, `scopes` and wasm sandbox. OKF fixes the interface, not the packaging, and defers attester sandboxing to the consumer | OKF v0.2 §10 | design + engine | D-N |
| **O6** ✅ | **Retire, not delete — decided 2026-11-20 (design `docs/superpowers/specs/2026-11-20-retire-not-delete-design.md`).** When a concept that a belief-tier relation references leaves its bundle, the sync retires it: it keeps the entity, its `concept-id` and the attributes its type requires, removes its other attributes (including `code-block` and `scope`, so a removed playbook cannot run) and every relation it plays except containment and belief-tier relations, sets `retired-at`, and counts it in `SyncResult.concepts_retired`. `gecko run` refuses a retired concept. This mirrors STIX's revoked objects (`extensions/cyber-gecko/src/stix/mod.rs:155-175`) | Belief-tier guard spec, pre-GC amendment | engine + schema | A4 ✅ |

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
| **C1** ✅ | LANDED in the daemon tick PR: `MemConsolidationDaemon::tick(now)` mints one `RunContext` (actor and source `consolidation-daemon`) and every job method takes it; `ConsolidationReport` carries `run_id` (set only when the tick deduped something), `vectors_removed` and `vectors_marked_dirty` (`core/mem-gecko/tests/consolidation_daemon_test.rs`: `tick_dedups_a_cohort_and_reports_its_run`, `tick_reports_index_outcomes`, `daemon_tick_writes_share_one_run_context`). No CLI or scheduler calls `tick` | CONTRACT-W8 CF-3, §2a | ✅ |
| **C2** | `dedup_content_hash` ✅ (LANDED in the daemon tick PR: the tick groups plain episodes by owner set, title and event-time and tombstones each loser through `dedup_episodes`, which records a `consolidation-step`; `a_second_tick_does_nothing`, `mixed_owner_cohort_dedups_only_identical_owner_sets`). Still owed: `discover_typed_join_links` and `contradiction_sweep` (write surfaces exist: `assert_belief`, `contest`) | CONTRACT-W8 §2, §2a | C1 ✅ |
| **C3** | Similarity recorded on `surfaced`/`informs-synthesis` for an offline batch scan, then `synthesize_links_llm` (was #15) | CONTRACT-W8 §2 (CF-1) | New sanctioned method; D-H |
| **C4** | General `consolidation-state` transition writer, then `decay_archive_tombstone` and `ttl_prune_record_tier` (was #16) | CONTRACT-W8 §2 (CF-2) | New sanctioned method |
| **C5** | Provenance summarize/collapse writer, then `summarize_archived_provenance` and `collapse_superseded_provenance` (was #17) | CONTRACT-W8 §2, §5 | New method; D-J |
| **C6** | Episode rollup writer, then `rollup_high_volume_episodes` (was #18) | CONTRACT-W8 §2 | New sanctioned method |
| **C7** | Blast-radius cache `precompute`/`invalidate` (was #19). The windowed traversal it depended on has landed (`blast-radius-at`) | CONTRACT-W8 §2a, §4 | D-I |
| **C8** ✅ | `daemon_is_enumerated_by_the_monopoly_test` — LANDED in the mem write-counts PR as the `daemon tick` drive of `memwriter_is_the_sole_belief_tier_writer` (`gecko-bin/tests/belief_tier_reconciliation_test.rs`): the real `MemConsolidationDaemon` tick dedups an episode pair, its delta must be non-empty, and every belief-tier label it changes must be accounted for or a writer-only entry naming it, as for any other drive | CONTRACT-W8 §3 row 5 | ✅ |

Row 7 (`daemon_tick_writes_share_one_run_context`) landed with C1. The remaining five checks in
CONTRACT-W8 §3 (rows 1–4, 6) land with the job each one names: rows 1, 6 with C2 (row 1,
`daemon_provable_merge_stamps_provable_derivation`, stays owed because dedup mints no belief or
derivation and so has nothing to check yet); row 2 with C3; row 3 with C4; row 4 with C5/C6.

---

## Track R — TypeDB review fixes

| ID | Item | Source | Kind | Done when |
|---|---|---|---|---|
| **R1** ✅ | **Correctness pass (pass 1).** As-of recall returns only beliefs visible to the caller (`visible-to`), newest first, with no supersession applied as of T; supersede requires an asserted belief; resolve_prediction, informs-synthesis and was-used refuse a write that matches nothing; retrieval tombstoning waits for every informed belief; the schema requires one ownership and at least one source-link per memory item; one link per (source, target) per sync; precision counts only current verdicts | `docs/superpowers/specs/2026-11-22-typedb-review-fixes-design.md` | queries and writers | ✅ `as_of_recall_hides_another_agents_private_belief`, `as_of_recall_returns_a_team_belief_to_another_agent`, `as_of_recall_returns_the_owners_private_belief`, `as_of_recall_is_newest_first_and_truncates_deterministically`, `superseding_an_already_superseded_belief_is_refused_and_commits_nothing`, `resolve_prediction_without_a_recorded_prediction_is_refused_and_commits_nothing`, `a_resolve_prediction_mismatch_names_the_belief`, `a_retrieval_event_feeding_a_live_belief_is_not_safe_to_tombstone`, `a_memory_item_with_two_ownerships_is_refused_at_commit`, `a_memory_item_with_no_source_link_is_refused_at_commit`, `precision_ignores_a_superseded_disposition`, `precision_with_no_asserted_dispositions_is_zero` |
| **R2** | **Schema tightening (pass 2).** Owed; plan `docs/superpowers/plans/2026-11-22-schema-tightening.md` | same spec | schema | the plan's tasks landed |
| **R3** | **Follow-ons from pass 1.** Batched multi-row reads (`GraphReader::read_rows`); deprecated relation syntax; function-body dedup; `EpistemicReader::believed_at` (a host trait method with no production caller) is still unscoped by agent; the retrieval-event write has no expecting(1). Declined: verb-named relations | same spec | cleanup | each item landed or dropped |

## Track M — mem belief-tier types with no production writer

Decided 2026-11-20 (D-O). Each label below is in mem's schema and on the belief-tier surface, but no
production path writes it. Each item owns its labels in the reconciliation rig's disposition table
(`DISPOSITIONS` in `gecko-bin/tests/belief_tier_reconciliation_test.rs`), and is done when a
production path writes them through `MemWriter` and the rig accounts for them.

| ID | Item | Labels | Type | Blocked on |
|---|---|---|---|---|
| **M1** | **Argument and assumption structure.** Record what a belief rests on and what supports or argues against it, and propagate a retracted assumption through `rests-on` | `rests-on` (writer-only: `rests_on`), `evidence`, `justification`, `support`, `attack` | writer + recall | Propagation: B1 (retraction) |
| **M2** | **Prediction calibration.** A production path that records a prediction and resolves it against the observing episode | `prediction-resolution` (writer-only: `record_prediction`, `resolve_prediction`) | writer + run path | — |
| **M3** | **Entity-resolution writer.** Assert, through the writer, that two source records denote one entity; B3 and B4 read these | `resolution`, `resolves` | writer | — |
| **M4** | **Prospective memory.** Intentions, their decomposition into sub-goals, and a task's working set | `intention`, `goal-hierarchy`, `working-set` | writer + run path | — |
| **M5** | **Procedural memory and run trace.** Remembered playbooks and their corroboration; the episode steps of a doc-run, its tool calls, and investigative pivots | `playbook`, `execution`, `tool-invocation`, `pivot` | writer + run path | — |
| **M6** | **Context suppression.** Learned anti-context rules applied to the concept whose recall they narrow | `learned-constraint`, `contextualizes` | writer + recall | — |

---

## Track U — TypeDB upgrade

Server and driver move together (dependency policy: lockstep). The pin lives in `Cargo.toml`,
`PINNED_TYPEDB` (`gecko-bin/src/orchestrator.rs`, which `gecko doctor` also reads), the CI
images and `docker-compose.yml`.

| ID | Item | Why | Done when | Blocked on |
|---|---|---|---|---|
| **U1** ✅ | **Upgrade TypeDB server and driver to 3.15.0 — LANDED in the TypeDB 3.15 upgrade PR.** Bump `typedb-driver = "~3.15"` (`Cargo.toml`), the `typedb/typedb:3.15.0` image in `.github/workflows/ci.yml` and `model-integration.yml`, the version `gecko doctor` expects (`gecko-bin/src/doctor.rs`), and the local server `just up` starts | Fixes on our paths: the `reduce … count` server panic recorded in the implementation notes (typedb#8021, fixed 3.15.0); string-attribute comparisons dropping answers (3.12.1) and value comparisons skipping valid answers, incl. long strings and decimals (3.14.1); a server crash when a database is deleted right after a transaction on it commits or closes, the integration tests' create/drop pattern (3.14.2); schema-transaction opens stuck behind a timed-out request or the statistics updater (3.14.1); parse/translation caching for parameterized queries (3.12.2). Risk: 3.15 reworks type inference, and the live check relies on unmet type patterns raising `INF2`/`INF3`/`FCP1` | Full Tier-2 suite green on 3.15.0 with `--test-threads=1`, `belief_edge_integration.rs` in particular. Each 3.12 implementation note below re-checked: the `reduce` panic note closed (the memory-item backstop may then use the direct count form, optional), the others kept or updated. 3.15 deprecation warnings checked for `try` in write stages and unchecked optional variables (`writer_reader_integration.rs:1131` uses a match-stage `try`). A registry test that `redefine <type> label <new>;` (type rename, since 3.12.2) in an extension fragment is refused for core and mem types. ✅ Suite green on 3.15.0 (349 passed, 5 skipped); notes below re-checked; the one deprecation warning (an unchecked optional variable in a test fetch) fixed with `$s?`; `refuses_an_extension_fragment_that_renames_a_type_or_role` and `scanner_recognizes_type_and_role_renames`. A 3.15.0 server opens a 3.12.0 data directory (databases and schemas listed and readable; the copies tried held no instances) | — |

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
| **D-G** | Errata in ratified text, for the spec owner: (1) SPEC-W5 §2.3/§4/§5 name a test that is now `p_w5_1_folded_surface_contains_divergence_precedence`; (2) `writer.rs` line anchors in SPEC-W5, SPEC-W2-W3 and CONTRACT-W8 pre-date PR #50 (guard `806-824` → ≈`970-990`; `dedup_episodes` `442` → `609`); (3) CONTRACT-W8 describes the daemon as unbuilt, and these statements are stale: (3a) the preamble (~17) says the trait "stays stubbed" and builds no code, but `MemConsolidationDaemon::tick` ships and runs `dedup_content_hash` (the other ten jobs are stubs); (3b) the accuracy check (~40) says `tick` is a no-op; (3c) §2a (~175-176) says `tombstone_episode_op` is reachable only through `dedup_episodes`'s pairwise path, but that op was folded into the guarded `dedup_episodes_ops` write, which also records a `consolidation-step`; (3d) the §2a row (~204) says `dedup_content_hash` rests on a content-hash-identical episode at `writer.rs:442`, but the rule is same owner set, title and event-time, in `writer/consolidation.rs`; (3e) the signature table (~227-228) cites `consolidation.rs:65`/`:76`, and the job methods take `&RunContext`; (4) SPEC-W5 (~552-559) calls the daemon a future caller, and it joins the rig as the `daemon tick` drive | SPEC-W5 §6; CONTRACT-W8 §2a footnote | — |
| **D-H** | Shape of the offline similarity-recording method (which edge, which run identity) | CONTRACT-W8 §2, §5 | C3 |
| **D-I** | Whether blast-radius caching stays a daemon job or becomes a separate `SemanticIndex`-style mechanism | CONTRACT-W8 §5 | C7 |
| **D-J** | Collapse granularity: which provenance is summarizable and which must survive | CONTRACT-W8 §5 / CF-2 | C5 |
| **D-L** ✅ | **Decided 2026-11-21 — detection model.** Strategies, analytics and analytic requirements are cyber types; `requires-evidence` and `evidence-of` are dropped (not derived, not re-sourced) and pre-current bundles are unsupported; blindness is computed from analytic log-source requirements against a user-declared sensor file | docs/superpowers/specs/2026-11-21-attack-detection-model-design.md | S2 |
| **D-M** | STIX core shape: align the cyber schema with typedb-cti's STIX 2.1 model (kill-chain-phase and external-reference entities, the full relationship set), or keep the ATT&CK-keyed model and add only what cyber queries. `attack-id` stays the unique key either way, since detection edges resolve techniques by it | — (Track S) | S3 |
| **D-K** | C2 `out-entrenches` schema hardening (OP-5). The §8.3 trigger has fired; shipping it owes a 5×5 drift-lock test against `Entrenchment::strength()` | SPEC-W2-W3 §2.3 (conditional); SPEC-W5 §2; INVARIANTS §8.3 | — (deliberate decision; D-D may make it unnecessary) |
| **D-N** | How GECKO treats an Attested Computation: as a playbook (engine, scopes and sandbox run), with its attester as a second sandboxed program, or as its own concept kind; where receipts and verdicts live (the record tier, or beliefs with provenance); how the attester is sandboxed | OKF v0.2 §10 | O5 |
| **D-O** ✅ | **Decided 2026-11-20 — Track M.** Owners for schema-only belief-tier types: the 17 labels no other ledger item owns and no production path writes are assigned to M1–M6 by function | Track M | A4 |

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

## Implementation notes (empirical, TypeDB 3.12; re-checked on 3.15.0)

Observed on the live engine; recorded so they are not re-derived. Unless a note says
otherwise, it holds on 3.15.0.

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
- **Write answers are countable.** Under `query_with_rows`, `match … insert` and
  `match … update` answers are row streams with one row per match (counts of 0, 1 and 2 were
  all observed). `GraphWrite.expect_rows` relies on this.
- **The live check dropped its `@key`/`@unique` condition.** Matching on annotations returns
  `[REP254]` (not implemented; still so on 3.15.0). An ambiguous key is still refused at commit by the expect-1
  row count.
- **Unknown labels error; unmet conditions return empty.** On 3.15.0 a type query naming a
  label the schema lacks raises `INF2`/`INF3`, and a known type that fails the condition
  (`$t label belief; $t plays x:y;` with no such `plays`) returns no rows. 3.12 also raised
  `FCP1` for some unsatisfiable patterns. The live check passes only on a returned row and
  reads any error as a failed condition (`InvalidInput`), so it refuses in both cases; a DB
  outage during the check is therefore labelled `InvalidInput`, with the storage cause kept
  in the message.
- **The memory-item backstop uses a negation.** It is written as
  `$m label memory-item; not { $s sub $m; $s sub P; };` and passes when it returns one
  document. On 3.12 the pass-on-empty form raised `FCP1`; on 3.15.0 it returns no rows.
- **`reduce … count` over an unsatisfiable pattern panicked the 3.12 server**
  (`compiler/executable/reduce.rs:132`). Fixed in 3.15.0 (typedb#8021): the same query returns
  a count of 0.
- **Alias definitions are unimplemented** (`[DEX1]`, still so on 3.15.0). The registry refuses any `alias` in
  extension fragments regardless.
- **Types and roles can be renamed** (since 3.12.2): `redefine <type> label <new>;` and
  `redefine <relation>:<role> label <new>;` both commit on 3.15.0. The registry refuses
  either in an extension fragment.
- **Optional variables are dereferenced explicitly** (3.15.0). Fetching a `try`-bound
  variable needs `$v?`; the bare form still works but logs a deprecation warning that says
  it will fail in the next version. `try` in a write stage is deprecated in favor of
  `if { isset $v; } then { … }`.

---

## Outside this ledger

- **Cyber-gecko C1/C2/C3** is a separate, mostly unblocked domain track. Its touchpoints here
  are the sighting writer (**A8**) and the ingest track (**S**).
- **Not owed by W2/W3/W5/W8:** W1 `attested()`, W4 (scope invariant 3 to activation), W6
  (reconciled design doc), W7 (split OP-6), W-T6 (IOC-convergence test) — INVARIANTS §8.4.
