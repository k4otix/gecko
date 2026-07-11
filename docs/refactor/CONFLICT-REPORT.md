# GECKO substrate — conflict report

> **Gated by [`INVARIANTS.md`](./INVARIANTS.md)** (`RATIFIED` at `0227c2d`). This report cites the invariant spine (§1) and the ratified operative corrections (§8). It audits **tensions between decisions held simultaneously** — not code bugs — at the **design level** (schema + persisted functions + doc-comment contracts + the reconciled corpus).
>
> **Reconciled against commit `0227c2d83f2c3317e49ce07951477fe5034baa2b`.**

## How to read this

- A **valid counterexample is a graph state the type system permits**, regardless of whether a Rust writer to reach it exists today. Reachability = "the schema does not forbid this state," not "a current code path constructs it."
- **Tags:** `shipped` (behavior is live) · `schema-only` (the conflict is in the type system; no writer yet) · `stub` (targets a `NotYetImplemented` mechanism) · `doc-drift` (docs disagree with code or each other) · `undocumented`.
- **Fix class:** `schema` (change the type system) · `function` (change a persisted TQL function or a writer guard) · `decision` (a design choice must be ratified) · `doc`.
- **Verification discipline:** every *consistent* verdict cites an executable check (a query that should return empty, or a schema constraint that structurally forbids the counterexample); every *conflict* ships a concrete permitted scenario, either instantiated against a live throwaway TypeDB (Deliverable #3) or explicitly marked inexpressible-because-stub.
- **Live-DB confirmations** (T2/T4/T5 counterexample construction, T6/T7 schema composition) are produced by the Deliverable #3 schema-probing worktree and folded into the `[D3]` slots below.

---

## Headline — Enforcement-locus cross-cutting pass (inv 2 is the linchpin)

**The finding CC discovered inside T3, generalized and confirmed.** Several "honored" invariant verdicts hold **only because `MemWriter` guards them in Rust**, not because the schema forbids the violating state. Everything writer-enforced inherits the integrity of **invariant 2** (EpistemicWriter-exclusivity). This pass enumerates every code path that can issue a belief-tier write and asks whether the monopoly is real.

### The write seam is type-blind

`GraphStore::write(&[GraphWrite])` is the single production write coupling (`graph.rs:97-102`). `GraphWrite.query` is a **raw TypeQL string** — `"a given … match? insert|put|delete|update"` (`graph.rs:62-71`). The seam accepts **any** write, including `insert $b isa belief, has belief-state "asserted", …`. **Nothing at the seam distinguishes record-tier from belief-tier writes.** The monopoly is therefore not a property of the seam; it is a property of *who holds a handle to it* and *what TQL they choose to author*.

### The bypass-capable path list

Holders of `Arc<dyn GraphStore>` (grep: `dyn GraphStore` across `core/`, `gecko-bin/src/`, `extensions/`):

| # | Path | What it writes | Belief-tier? | Status |
|---|---|---|---|---|
| 1 | **`MemWriter`** (`writer.rs:55`) via `tql::insert_belief_ops` (`writer.rs:769,827`) | `belief`/`derivation`/`supersession`/`episode`/`resolution` | **yes — the sanctioned path** | run-stamped; the intended monopoly holder |
| 2 | **Syncer / record path** (`syncer/bundle.rs:131`, direct `db.begin_write()`) | `bundle`/`concept`/`external-resource`/`containment`/`hierarchy`/`concept-link`/`citation` | **no** — record tier only | ✅ proven clean by `no_drag_test.rs:87` + `syncer_stays_on_the_record_path` (`bundle.rs:741`) |
| 3 | **cyber detect bridge** (`extensions/cyber-gecko/src/detect/mod.rs:50`) | belief **entities** via `writer.assert_belief` (`mod.rs:119,177`); but the linking **relations** `technique-detection` / `alert-disposition` via **raw `graph.write`** (`mod.rs:142,196`) | **partially** — entities routed through the writer; belief-*referencing relations* written raw | ⚠️ disciplined-but-unenforced |
| 4 | **Any future extension** handed the graph in `build_host_bridge` (`gecko-bin/src/main.rs:778,1016`) | anything it authors | **potentially yes** | ⚠️ structurally unrestricted |

### Verdict — **CONFLICT (tag: `shipped`, structural). Fix class: decision + test (+ optional schema).**

The monopoly is **procedural, not structural**, exactly as `INVARIANTS.md` §1/inv-2 and §8.3 ratified:

1. **cyber path (#3) is the live proof the monopoly is a convention, not a constraint.** It does the right thing for belief *entities* (through `assert_belief`, run-stamped) but writes belief-tier *relations* raw. It *could* equally have written `insert $b isa belief …` directly — nothing stops it. The discipline is in the author's head, not the type system.
2. **The only monopoly tests are record-side.** `no_drag`/`syncer_stays_on_the_record_path` prove the *syncer* writes zero beliefs. There is **no inverse test** asserting *nothing but `MemWriter`* writes belief entities. So the guarantee is: "we checked one known-clean path," not "the belief tier has one writer."
3. **Consequence for the thesis.** The claim "the type system enforces the invariants" is **weaker than stated**: invariants 1(half), 7(entrenchment monotonicity / OP-5), the write-side two-ledger firewall (OP-3), OP-1, OP-4 are all *writer-enforced*. If a second belief-writer is ever added (a migration, a domain extension, a bulk importer) without going through `MemWriter`, every one of those guarantees silently lapses.

**Ratified disposition (`INVARIANTS.md` §8.3, work item W5):** accept procedural enforcement **and** commit a `MemWriter`-monopoly test.

**W5 scope must cover belief-tier RELATIONS, not just entities — this is blocking, not secondary.** The live breach this pass found (cyber `claim`/`disposition` writing `technique-detection`/`alert-disposition` via raw `graph.write`, `mod.rs:142,196`) is a **relation** write, not an entity write. A monopoly test that only asserts "no non-`MemWriter` path mints a belief *entity*" would **pass while that breach stands** — it checks the wrong shape. So W5 has two coupled sub-items, and **neither is optional**:
- **W5a — broaden the test surface to the full belief tier:** the monopoly assertion must cover belief-tier *relations* — `derivation`, `supersession`, `surfaced`, `informs-synthesis`, and the domain belief-referencing relations `technique-detection` / `alert-disposition` — not only the `belief` entity. Until it does, the test is not meaningful (it can't fail on the one breach we have).
- **W5b — route the cyber belief-referencing relations through a sanctioned writer** (an `EpistemicWriter` extension point, or a mem-owned relation-writer) *before* W5a can go green. This is the **proof case for W5**, not a nice-to-have: today's discipline is one careless `graph.write` from a forged provenance stamp, and W5a will (correctly) fail until W5b lands.

Consider pushing individual guards into the schema only if the bypass list grows.

**Executable check (owed, W5a):** a committed test that, after every non-`MemWriter` code path runs, asserts the population of **every belief-tier type** (`belief` + the relations above) equals what `MemWriter` accounts for. The record-side half already exists (`no_drag`/`syncer_stays_on_the_record_path`); the inverse — over the full belief tier — does not.

---

## T1 — Resolution firewall under belief-about-belief recursion (inv 8 × 9 × mechanism 10)

**Question.** Resolution-beliefs are beliefs about beliefs; does the two-ledger firewall survive the recursion?

**Evidence (confirmed).** `canonical-entity` joins **only** through `derivation` to read `derivation-method` (`mem_functions.tql:134-152`); `derivation-chain` follows **only** `derivation` (`:28-36`); similarity lives only on `surfaced`/`informs-synthesis` (`mem_types.tql:270-278`), reachable only via `retrieval-provenance-of` (`:114-118`). `resolution sub belief` (`mem_types.tql:259`), so a resolution participates in the derivation ledger like any belief; nothing routes it into the retrieval ledger.

**Verdict — CONSISTENT at the read layer. Latent finding (tag: `stub`).** No persisted function crosses ledgers; the resolution recursion stays inside the derivation ledger. But nothing *schema-level* forces a similarity-surfaced candidate onto `surfaced` rather than being fabricated as a `derivation` source with an llm method — and the resolution *write* path is unimplemented. This is an instance of the **enforcement-locus gap**: the read layer can't cross ledgers; a future writer could.

**Fix class: function** (a write-path guardrail + test once the resolution writer is built: a similarity signal must land on `surfaced`, never on a `derivation`).

**Executable check (consistent-verdict evidence):** `derivation-chain(<any resolution>)` ∩ `retrieval-event`s = ∅ — the return type of `derivation-chain` is `{ memory-item }` reached only via `derivation`, and `retrieval-event`s are reachable only via `informs-synthesis`/`surfaced`; the two function bodies share no relation. **`[D3] CONFIRMED:** the shipped integration test `derivation_chain_never_touches_the_retrieval_ledger` (`writer_reader_integration.rs`) passes against the real `MemWriter`/`RouterGraphStore` — the firewall holds at the read layer end-to-end. The latent write-side gap is unchanged (no writer to guard yet).

---

## T2 — Member-record supersession vs cohort-belief blast-radius (mechanism 11 × inv 6 × inv 7-blast) — **strongest finding**

**Evidence (confirmed).** `blast-radius` traverses **only** `rests-on ∪ derivation` (`mem_functions.tql:64-78`). A cohort belief attaches to a population via `scoped` (`mem_types.tql:247-249`); membership is `population-member` (`:252-255`). **Neither `scoped` nor `population-member` is traversed by `blast-radius`.** Membership (re)materialization/invalidation has no writer — `population-member` is a "MATERIALIZED … derived cache" (`mem_types.tql:251`) with a `computed-at` staleness marker but nothing computes it; consolidation is `NotYetImplemented` (`consolidation.rs`).

**Verdict — CONFLICT (tag: `schema-only`, schema-real — *not* a mere stub gap; see D3 reconciliation).**

**Counterexample the design permits.** Population `P` (spec matches `{C1, C2}`); belief `B` `scoped` to `P`; supersede `C1`'s underlying record (or a belief resting on `C1`). `blast-radius(C1)` returns only items reachable via `rests-on`/`derivation` from `C1`; **`B` is not among them** — it rests on `P` via `scoped`, an untraversed edge. The cohort belief silently goes stale: the thing that would tell a caller "reconsider `B`, its cohort changed" never fires.

**`[D3] CONFIRMED live.** Ground state fully constructible in raw TQL against the composed schema; `blast-radius(A)` returned `{t2/bel/D}` **only** — cohort belief `B` **absent**. This is a **traversal wall, not a construction wall**: `B` reaches the cohort solely via `scoped`→P→`population-member`→C1, neither traversed by `blast-radius`.

**D3 sharpening — the conflict is doubly reinforced and survives any writer.** (i) The failure lives in the *persisted function body* (`blast-radius`'s traversal set), so **writing the missing Rust populations writer would not fix it** — it is `schema-only`, not `stub`. (ii) Fix option (a) below is **itself blocked by the type system**: a cohort belief **cannot** be made to `rests-on` its membership, because `population-member` is a **relation** (confirmed via introspection) and `rests-on:assumption` admits only `memory-item`; and the member record `C1` (a `concept`) plays **no** `derivation`/`rests-on` role at all. The cohort belief is therefore **doubly disconnected** from membership-provenance propagation — orphaned by construction.

**Interaction with T5 (ratified: fix jointly).** If membership is *also* not as-of-t-correct (T5), the staleness is **doubled**: wrong members *and* no invalidation signal. `INVARIANTS.md` §8.4/W3 binds these into one work item.

**Fix class: schema + function.** Option (a) "cohort beliefs `rests-on` their population" is **not directly expressible** (D3 (ii)) — it would require either making `population`/`population-member` a `memory-item`-playing assumption target, or a new belief↔population dependency relation. Option (b) — **add an explicit population-invalidation traversal to `blast-radius`** (walk `scoped`←`population`→`population-member`→member, plus flip `computed-at` on member-supersession) — is the tractable path. Either way the schema expresses the stale state but no function detects it.

---

## T3 — Entrenchment ordering × resolution-supersession (inv 7 × 9) — demoted to consistent; caveat graduated to the headline

**Evidence (confirmed).** `resolution sub belief` (`mem_types.tql:259`) → `MemWriter::supersede` applies the entrenchment guard (`writer.rs:806-824`: `entrenchment_rank(new) < entrenchment_rank(old) ⇒ EntrenchmentViolation`, `tql.rs:46-50`, ranks in `semantic.rs:84-92`). A provable resolution maps to entrenchment `tool-derived` (rank 2, `tql.rs:29-39`); `canonical-entity` counts **only** provable resolutions (`mem_functions.tql:139,146`).

**Verdict — CONSISTENT, but contingent on invariant 2.** The scenario (a provable resolution in the canonical closure retracted by a lower-entrenchment method) is exactly what the guard blocks — an `llm` supersede (rank 0) never lands, so no cycle. **But the guard is host-side in the Rust writer, not a TQL predicate:** `entrenchment` is a plain `@card(0..1)` attribute (`mem_types.tql:98`) with no ordering function in `mem_functions.tql`. So T3's safety **depends on invariant 2 being airtight** — a raw TQL supersession write (which the type-blind seam permits — see Headline) bypasses it entirely.

**This caveat is not T3-specific — it graduated to the Headline (enforcement-locus pass).** T3 itself resolves **consistent** (tag: `shipped`). **Residual (ratified intended):** `human-assertion` (rank 3) out-entrenches the provable trio (rank 2), so an analyst claim *can* supersede a `type-join`-provable belief — this is the authority-⟂-provability decoupling ratified in `INVARIANTS.md` §7(b)/§8.1, intended, not a hole. Equal-tier (provable-supersedes-provable) is intended re-derivation.

**Executable checks:** (1) `supersede(<provable resolution>, <llm belief>)` ⇒ `EntrenchmentViolation` (unit-tested in `writer.rs`). (2) The Headline's W5 monopoly test doubles as T3's second check — "no non-`MemWriter` path writes a `supersession`."

---

## T4 — Canonical-entity view vs divergence-exception belief (inv 9 / mechanism 12 × mechanism 13)

**Evidence (confirmed).** The divergence-exception rule (an instance belief overrides its population belief) is **design intent** in the corpus (`gecko-refactor-implementation-plans (1).md:185`) with **no schema or function mechanism shipped** — the schema offers only `scoped` ("belief scopes to the population, not each member", `mem_types.tql:247`) with **no precedence relation and no override-resolution function**. `contradicts($b1,$b2)` requires both operands be `claim`s of a `contradiction` hub (`mem_functions.tql:57-61`; `contradiction` relates `hub` + `claim @card(2..)`, `mem_types.tql:148-150`); a query-time **view** (`canonical-entity`) is not a belief and cannot be a `claim`.

**Verdict — CONFLICT (tag: `doc-drift` + `schema-only`).** Three distinct problems:
1. **Documented-but-unrealized:** no schema construct lets an instance belief override a population belief; no precedence function exists. The corpus claims a capability the schema can't express.
2. **Doc-vs-doc contradiction (now RATIFIED-resolved):** `(1):185` said instance **overrides** (one wins); `research-addendum:165` said **present both, flagged `contested`, nothing silently wins**. **`INVARIANTS.md` §8.2 ratified that neither pole is canonical** — the mechanism is **evidence-ranked, recorded, retractable resolution**: consume competing beliefs' `derivation-method`+`confidence`+`entrenchment`+`valid-from`, auto-resolve toward the stronger with a recorded reason (the loser `superseded`, **never deleted**), escalate to `contested` only when evidence doesn't separate. "Override" = the confidence-≈-1.0 / human-precedence end of that one spectrum. Auto-*deletion* of the loser is the `owl:sameAs` lossy-collapse invariant 9 forbids.
3. **View-vs-belief invisibility:** even if realized, a `canonical-entity` **view** can never be a `contradiction` `claim`, so a view-vs-instance-belief conflict is structurally undetectable by `contradicts()`.

**Fix class: decision (MADE, §8.2) → schema + function** (work item W2, owed **before** the resolution writer): (1) a **precedence relation** (the reification `scoped` lacks); (2) a **disposition function** ranking by (`derivation-method`,`confidence`,`entrenchment`,`valid-from`) → `resolved(head, confidence, reason)` | `contested`; (3) **reify divergence-exceptions as beliefs** (a view can't participate in the contradiction machinery).

**`[D3] CONFIRMED — inexpressible-by-construction (schema-real, stronger than a stub).** The disagreeing beliefs insert cleanly, but (a) the **only** relation between a belief and a population is `scoped` (belief→population, no precedence); no relation type takes `(population-scoped-belief, instance-belief)` with override semantics, and `supersession` is whole-belief replacement with no member/scope role; (b) `contradiction:claim`'s player set is `memory-item`, which **type-excludes** `concept` and every cyber-object subtype ([INF11] verified) — so a query-time `canonical-entity`/`population-members` **view** (a `{ concept }` stream, not even a persisted entity) can never be entered as a contradiction claim. A view-vs-instance conflict is therefore **not representable** as a contradiction and cannot be surfaced by `contradicts()` at all.

---

## T5 — `believed_at(t)` vs the materialized population-member view (inv 6 × mechanism 11 × inv 8)

**Evidence (confirmed).** `believed-at($t)` is temporal (`mem_functions.tql:20-25`). `population-members($pop)` (`:126-129`) and `canonical-entity($rec)` (`:134`) take **no time parameter** — the comment at `:122-125` records that 3.12 rejected the unused `$now` ([FRP1]), so membership resolves **present-state only**. The index has an explicit as-of bypass (`RecallQuery.as_of` skips the index); populations/canonical have **no** as-of variant.

**Verdict — CONFLICT (tag: `schema-only`, specified gap).**

**Counterexample.** `B` `scoped` to `P`, valid at `t`; at `t` the cohort was `{C1}`, today it is `{C1, C2}`. `believed_at(t)` returns `B` (temporally correct), but `population-members(P)` returns **today's** `{C1, C2}` — the cohort belief is reconstructed against the **wrong membership**. Unlike the index (which has a *documented* present-state-only bypass, inv 8), this composition is **unspecified** — a caller combining `believed_at` with a population view gets a silently time-inconsistent answer.

**Fix class: function + decision** (work item W3, jointly with T2). Either add as-of-t population/canonical functions (host-side strict re-eval of `spec-text` already exists per `mem_functions.tql:124`, so an as-of-t strict variant is feasible), **or** declare the views present-state-only and *forbid* combining them with `believed_at`, mirroring the index's explicit documented bypass.

**`[D3] CONFIRMED live.** Inserted `B` `scoped` to `P` with `valid-from` t; `population-member` P→C1 (`computed-at` t), then P→C2 (`computed-at` today). `believed-at(2026-01-01)` returned `{t5/bel/B}`; `population-members(P)` returned `{t5/c1, t5/c2}` — B is re-scoped against **today's** cohort. **D3 sharpening:** `population-member` carries only a single `computed-at` staleness marker — **no `valid-from`/`valid-to` window** — so, like T2, the gap is in the schema/function shape and **survives any writer** (`schema-only`, not stub).

---

## T6 — The `{ concept }` return-type coupling vs cyber subtyping (mechanism 12 × mechanism 17) — sharpened

**Evidence (confirmed).** `cyber-object sub concept` (`cyber_schema.tql:18`) inherits `plays resolves:record-*` and `plays population-member:member` (mem adds these to `concept`, `mem_types.tql:296-300`) → `canonical-entity($rec: concept)` accepts cyber-objects and still returns `{ concept }` (covariant; subtypes collapse to `concept`, no widening). `external-resource` is deliberately excluded from `resolves`/`population-member` (`mem_types.tql:302-304`, rationale `:290-293`) to preserve the single-type return. `sighting sub execution-episode` (`cyber_schema.tql:143`) — **not** a `concept`, so it cannot play `resolves`.

**Verdict — CONSISTENT / INTENDED for the subtyping mechanics.** Subtyping `concept` collapses to `concept` on return — no widening. `blinded-detections() -> { concept }` (`cyber_functions.tql:14`) returns detecting-logic scoped to `concept` — OK.

**Sharpened residual — the actual finding (tag: `schema-only`, cyber ingestion partial).** Sightings-can't-resolve is *deliberate* (a sighting is an observation episode, not an entity). But **two feeds will report the same observable, and observables *do* resolve.** So the real question is not "is sighting-excluded-from-resolution deliberate" (it is) but **"does the sighting→observable decomposition route cross-feed identity onto the `observable` (which resolves), so two sightings of the same IOC converge via their shared observable rather than being lost?"** The schema wires `sighting -sighting-of-> observable` (`cyber_schema.tql:147-149`) and `observable owns ioc-value @card(1), ioc-type @card(1)` (`:71-75`) and `observable plays resolves:record-*` (via `cyber-object sub concept`). **Whether two feeds' observables of the same IOC actually converge depends on the (partial) cyber ingestion keying observables by normalized `ioc-value`+`ioc-type`** — if ingestion mints a fresh `observable` per sighting without resolving on the normalized IOC, sightings of the same IOC from two feeds **never converge** — a silent identity-fragmentation bug. Verify against the cyber plan's C1.6/C2.4 resolution pass.

**Fix class: function** (ensure cyber ingestion resolves observables on normalized `ioc-value`+`ioc-type`, or emits `resolution` beliefs between them) — or **decision/doc** if convergence is intentionally deferred.

**This must be a named work item, not a pointer to a plan — silent identity-fragmentation evaporates otherwise. → W-T6** (fold into the cyber-gecko C2.4 resolution-pass acceptance test). **Concrete gating check:** insert two `observable`s with the **same** normalized `ioc-value`+`ioc-type` from **different** sources (`source-adapter`/`origin-kind`); assert `canonical-entity` resolves them to **one** canonical entity (i.e. `canonical-entity($obsA)` contains `$obsB`). If they don't converge, two feeds' sightings of the same IOC never merge — the fragmentation bug. Owner: cyber-gecko ingestion (C1.6/C2.4).

**`[D3] CONFIRMED (mechanics) — residual OPEN.** Composed schema is **define-clean** (all four transactions committed, zero define errors). `canonical-entity($o: observable)` **type-checks, compiles, and runs** (returns an empty closure for an unresolved observable) — confirming `observable sub cyber-object sub concept` is admitted. `sighting` **cannot** play `resolves`: inserting `resolves(record-a: $sighting …)` fails `[QUA1] Type inference error` (sighting is not a `concept`). **The residual IOC-convergence question is NOT closed by D3** — it depends on the *partial cyber ingestion* keying observables by normalized IOC (plan C1.6/C2.4), which the schema-probe does not exercise. Remains a **verify item**: confirm two feeds' observables of the same IOC converge (resolve) rather than minting duplicate `observable`s.

---

## T7 — Registration-is-the-gate vs cross-fragment `plays` additions (inv 3)

**Evidence (confirmed).** The cyber fragment adds `plays` **onto substrate types it does not own** — `population plays sensor-coverage:coverage-scope` (`cyber_schema.tql:141`), `concept plays technique-detection:detecting-logic` + `belief plays technique-detection:detection-claim` (`:155-156`), and `concept plays alert-disposition:dispositioned-logic` + `belief plays alert-disposition:disposition-belief` (`:167-168`). This is additive **mutation** of substrate type definitions, not just subtyping. Registration order (mem before cyber) guarantees define-time resolution; `apply_all_schemas` is idempotent, per-SchemaInit (`main.rs:601-633`).

**Verdict — PARTIAL CONFLICT (tag: `shipped`, latent).** Registration gates **activation** (host fns / write paths at runtime) but **not schema reversibility**: a `plays` clause defined onto `belief`/`population` **persists** in a database after cyber is later disabled — TypeDB `define` is not auto-`undefine`d; disabling only stops host-fn activation. The doc "a disabled extension applies neither types nor functions" holds for a **fresh build**, not for re-init-with-different-config on an existing DB. Given single-bundle / one-DB-per-bundle + SchemaInit-then-Run, the residue is benign (an unused `plays` with no writer) — but **the invariant statement over-claims**.

**This is a second instance of the enforcement-locus theme** (the invariant over-claims what the mechanism enforces) — cross-referenced to the Headline.

**Fix class: decision / doc (RATIFIED, W4).** `INVARIANTS.md` §1/inv-3 ratified: **scope invariant 3 explicitly to *activation* gating**, document that schema mutations are per-DB persistent, and do **not** require `undefine`-on-disable.

**`[D3] CONFIRMED live.** Schema introspection (`match $t label <X>; $t plays $r;`) confirms **all four** additive `plays` clauses land on core/mem substrate types cyber does not own: `belief`→`technique-detection:detection-claim` **and** `alert-disposition:disposition-belief`; `population`→`sensor-coverage:coverage-scope`; `concept`→`technique-detection:detecting-logic` **and** `alert-disposition:dispositioned-logic`. The mutation lands and persists on the substrate types — anchor confirmed.

---

## T8 — Consolidation ("dreaming") as a firewall-crosser and retention-rule interactor (inv 8 × 7 × mechanism 10 × mechanism 14)

**Why this is a tension, not just a stub.** Consolidation is the one mechanism that **writes beliefs derived from other beliefs/episodes at scale**, and its contract is undefined — `consolidation.rs` is `NotYetImplemented` throughout (`dedup_content_hash`, `discover_typed_join_links`, `synthesize_links_llm`, `contradiction_sweep`, `decay_archive_tombstone`, `summarize_archived_provenance` all return `NotYetImplemented`). Designing it against firewalls it could violate is the maximal "expensive-to-get-wrong-later" case — the reason this audit runs before more code commits.

**Evidence (confirmed).** The lifecycle `raw → candidate → consolidated → archived → tombstoned` is the `consolidation-state` enum (`mem_types.tql:9`). `decay_archive_tombstone` is scaffolded to call the best-effort index-remove on archive/tombstone (`consolidation.rs:109-116`). Merge-similarity would use the same embeddings the two-ledger firewall keeps off `derivation`.

**Verdict — TENSION → design constraints on the unwritten daemon (tag: `stub`). Fix class: decision → schema/function** (work item W8, ratify **before** the daemon is written). The three constraints:

1. **Merge-similarity vs the two-ledger firewall (inv 8 / OP-3).** Consolidation uses similarity to decide *what* to merge. If that similarity is recorded as, or influences, the merged belief's `derivation`, the substrate's own daemon commits the exact firewall violation domains are forbidden. **Constraint:** content-hash / type-join merges stamp a **provable** `derivation-method`; similarity-clustered merges are **probabilistic** (`llm-synthesis`-class), marked, and the similarity lives only on a retrieval/episodic edge (`surfaced`/`informs-synthesis`), **never** on the merge's `derivation`.
2. **Tombstoning vs retrieval-provenance retention (mechanism 14 × 10).** The retention rule forbids tombstoning a `retrieval-event` while its `informs-synthesis` belief is non-superseded. **Constraint:** the daemon's tombstone predicate must **exclude** `retrieval-event`s with a live `informs-synthesis` edge, or it severs the retrieval axis of live beliefs.
3. **Consolidation-created beliefs vs entrenchment (inv 7) — and this is the SAME linchpin, not a future-only concern.** A daemon-synthesized belief is `llm`-class entrenchment (rank 0) → by the T3 guard it can **never supersede** a human/tool belief. **But the T3 guard lives in `MemWriter`, not the schema (Headline), and cyber already writes some belief-tier state raw** — so "the daemon has no privileged write path bypassing the entrenchment guard" is only enforceable if the daemon is *held to* the monopoly. **Constraint (make it enforceable, not aspirational):** the daemon must **write through `EpistemicWriter`, never raw `graph.write`** — every consolidation belief/merge/tombstone goes through the same run-stamped, entrenchment-guarded path. This closes the loop with the Headline: the daemon is exactly the kind of new belief-writer W5 must forbid from bypassing `MemWriter`, and its spec should say so in as many words. Confirm consolidation *proposes*, never *overrides*.

**Executable check:** none runnable today (stub). The deliverable is a **written daemon-spec contract** the implementation must satisfy, cross-checked against invariants 7/8/10/14. Recorded here as W8.

---

## Self-consistency pass (from `INVARIANTS.md` §6; restated as findings)

These first-order conflicts surfaced during reconciliation and are ratified-resolved in `INVARIANTS.md`:

| ID | Drift | Resolution | Fix class |
|---|---|---|---|
| D-1 | stale 1–7 numbering vs code-matching 1–9 | `(1)` canonical, 1–7 retired | decision (closed) |
| D-2 | `gecko_design.md` Rhai+QuickJS vs shipped Boa | Boa authoritative; §3.2 superseded | doc |
| D-3 | provenance-lite proposed (`research-addendum:311`) then deleted (`(1):371`) | deletion authoritative | doc |
| D-4 | delete-on-prune (`mem-gecko.md:115`) vs never-delete | never-delete authoritative | doc |
| D-5 | contradiction shape → `anomaly` hub + `claim @card(2..)` | shipped hub-shape authoritative | doc |
| D-6 | decay: static TQL activation + host-side continuous decay | shipped split authoritative | doc |
| D-7 | derivation-method 7 shipped vs 4 in draft → **trinary** (provable/probabilistic/**attested**) | ratified trinary; F-DM1 completes the partition (W1) | decision + doc + code |
| D-8 | corpus untracked; citations point out-of-repo | land INVARIANTS.md + design doc in-repo (W6) | doc |
| F-DS1 | `design §` anchors point at half-superseded `gecko_design.md` (§3.2, §4.3 stale) | reconciled design doc; repoint `mem-gecko/src/lib.rs:7` (W6) | doc |

---

## Summary

| # | Tension | Verdict | Tag | Fix class | Work item |
|---|---|---|---|---|---|
| **Headline** | EpistemicWriter monopoly (enforcement locus) | **CONFLICT — procedural not structural** | shipped | decision + test (+ schema) | W5 |
| T1 | Resolution firewall recursion | CONSISTENT (read); latent write gap | stub | function | (W-T1) |
| T2 | Cohort-belief blast-radius | **CONFLICT** — silently stale (D3-confirmed; doubly orphaned) | schema-only *(survives any writer)* | schema + function | W3 (with T5) |
| T3 | Entrenchment × resolution-supersession | CONSISTENT, contingent on inv 2 | shipped | — (guard exists) | — |
| T4 | Divergence-exception vs canonical view | **CONFLICT** — unrealized + view-invisible (D3: inexpressible-by-construction) | doc-drift + schema-only | decision(made §8.2) + schema + function | W2 |
| T5 | `believed_at` vs population view | **CONFLICT** — time-inconsistent (D3-confirmed) | schema-only *(survives any writer)* | function + decision | W3 (with T2) |
| T6 | `{concept}` coupling vs cyber subtyping | CONSISTENT mechanics; **residual** IOC-convergence risk | schema-only | function (or doc) | **W-T6** |
| T7 | Registration gate vs cross-fragment `plays` | **PARTIAL CONFLICT** — over-claims reversibility | shipped | decision/doc(ratified) | W4 |
| T8 | Consolidation firewall/entrenchment contract | **TENSION** — daemon-spec owed | stub | decision → schema/function | W8 |

**The three findings expected to dominate (reviewer's prior, borne out):**
1. **The enforcement-locus pass** — the monopoly is procedural; the bypass list is non-trivial (cyber writes belief-tier relations raw; the seam is type-blind; no inverse test). Highest structural stakes.
2. **T2 + T5 together** — population-scoped beliefs go stale under member supersession *and* reconstruct against wrong-time membership; the two compound (W3).
3. **T4 + T8** — the two places a *decision* was owed before code: divergence-exception semantics (ratified §8.2) and the dreaming-daemon contract (W8). Both cheap now, expensive after implementation.

---

## Appendix — Deliverable #3: sample-bundle schema-probing (live TypeDB 3.12)

**Method.** Applied `core_schema.tql` → `mem_types.tql` → `mem_functions.tql` → `cyber_schema.tql`+`cyber_functions.tql` as four sequential schema transactions against throwaway DBs on live TypeDB 3.12, then hand-authored raw TypeQL (bypassing the stubbed Rust writers). Shipped-half claims additionally confirmed by running the real `mem-gecko` integration tests. Throwaway DBs (`probe_5349e8db`, `probe_731d8b13`) deleted; `gecko` never touched.

### Scenario → constructible? → manifests? → wall

| scenario | constructible? | manifests? | wall-location |
|---|---|---|---|
| **T2** cohort-belief blast-radius | **YES** — P, C1, B `scoped`→P, `population-member` P→C1, assumption A + resting belief D all insert cleanly | **YES** — `blast-radius(A)` = `{t2/bel/D}` only; **B absent** | *Traversal* wall, not construction: `blast-radius` recurses `rests-on ∪ derivation` only; B reaches the cohort solely via `scoped`→P→`population-member`→C1 |
| **T4** divergence-exception | ground state **YES**; override/precedence mechanism **NO** | **YES** — conflict structurally invisible: `contradicts(B_pop,B_inst)=false`; a view/record cannot be a `claim` | (a) only belief↔population relation is `scoped` (no precedence); (b) `contradiction:claim` player set is `memory-item`, type-excludes `concept`/cyber-object ([INF11]) |
| **T5** `believed_at` vs population view | **YES** — B `scoped`→P `valid-from` t; `population-member` P→C1 (computed-at t) then P→C2 (computed-at today) | **YES** — `believed-at(2026-01-01)` = `{B}`; `population-members(P)` = `{C1, C2}` | `population-member` has a single `computed-at`, **no `valid-from`/`valid-to`**; `population-members` takes **no `$t`** |

### T6 / T7 live confirmations
- **Composition define-clean:** all four transactions committed, zero define errors. *Mechanics note:* `cyber_functions.tql` has no `define` of its own (concatenated under `cyber_schema.tql`'s `define`) — applied standalone it fails `[TQL03]`. Not a defect; a build-assembly constraint worth documenting.
- **T6:** `canonical-entity($o: observable)` type-checks, compiles, runs (empty closure for an unresolved observable) — `observable sub cyber-object sub concept` admitted. `sighting` playing `resolves:record-a` fails `[QUA1] Type inference error` (not a `concept`).
- **T7:** all four cross-fragment `plays` clauses confirmed present on substrate types via introspection.
- **Record/belief boundary (Task B):** `concept` (record tier) plays **13 roles, none provenance apparatus** (no `derivation`/`rests-on`/`evidence`/`support`/`attack`/`justification`/`supersession`/`ownership`/`contradiction`/`scoped`); `belief` plays **25** including the full provenance suite. Records carry no provenance — confirmed. Shipped functions confirmed green: `blast_radius_propagates_transitively`, `derivation_chain_never_touches_the_retrieval_ledger`, `population_members_and_canonical_entity` (3 passed).

### D3 tag-reconciliation findings (feed back into the report)
1. **T2 and T5 are `schema-only`, not `stub`.** Both ground states are constructible *and* manifest live; the failure is in the persisted function bodies (`blast-radius` traversal set; `population-members` arity) and relation shapes (`scoped`/`population-member` without a bitemporal window). **Writing the missing Rust populations writer would not fix them** — tags corrected above.
2. **T2 is doubly orphaned (structural reinforcement).** A cohort belief cannot be made to `rests-on` its membership: `population-member` is a *relation* and `rests-on:assumption` admits only `memory-item`; and the member record C1 (a `concept`) plays no `derivation`/`rests-on` role. Fix option (a) is not directly expressible — pushed the fix analysis to option (b).
3. **T4 is inexpressible-by-construction** (stronger than a writer stub): no precedence relation, and the contradiction machinery type-excludes the view. A view-vs-instance conflict is not representable as a contradiction at all.
