# GECKO substrate invariants — reconciled & ratified

> **Status: `RATIFIED` at commit `0227c2d83f2c3317e49ce07951477fe5034baa2b`** (`main`, "docs(schema): self-document every type, role, and function with @doc", #21).
>
> The maintainer has **locked the decisions** in §7 and §8 (divergence-exception semantics, the *attested* derivation class, operational-invariant placement, entrenchment/provability orthogonality, canonical numbering). What is ratified is the **verdicts, findings, and dispositions** — *not* a rewrite of the §1 definitions.
>
> **Two hard rules for anyone editing this file (incl. Claude Code):**
> 1. **§1 definitions stay verbatim.** They are the *claimed intent of record*, transcribed from the corpus. Where a definition is now known to be wrong (invariant 7's binary framing), the **corrected/operative statement lives in §8**, cross-referencing the verbatim it supersedes and the finding that justified it. **Never edit the §1 text in place** — doing so erases the audit trail that the invariant drifted.
> 2. **Ratifying this doc does not change the code.** The schema / function / code / doc changes these decisions imply are **work items (§8.4)**, executed by the conflict report and its fixes — not by this file.
>
> This document **gates** `CONFLICT-REPORT.md`. It is now ratified; the conflict report may proceed, citing §1 (spine) + §8 (operative corrections).

---

## Provenance of the thesis (why this doc has to exist)

`invariant N` is cited ~60× and `design §X.Y` ~8× across the shipped code, but **neither referent is defined anywhere in the repo** (`docs/refactor/` held only validation records before this file). Both live in an untracked, multi-vintage external corpus under `~/Downloads/`. This file lands the reconciled statement in-repo so the citations point at something versioned.

- **`invariant N` authority:** `~/Downloads/gecko-refactor-implementation-plans (1).md` §1 — the 1–9 enumeration, transcribed verbatim below. (The `(1)`-less sibling is a **stale 1–7** numbering; see Drift D-1 — **ratified retired**.)
- **`design §X.Y` referent:** `~/Downloads/gecko_design.md` (Master Spec v1.1) — **half-superseded**; anchor-by-anchor status in §5.

---

## §0 — The lineage partition (the master key; read before anything else)

The corpus splits into **two incompatible schema lineages**. An auditor who reads L1 type names as shipped behavior will produce false findings, so the live lineage is stated first:

| Lineage | Root type | Distinctive markers | Corpus docs | Status |
|---|---|---|---|---|
| **L1 "cognitive-node"** | `cognitive-node sub okf-concept` | `hypothesis` / `deprecation` / `semantic-belief` types; `contextualizes` as an *episode-link*; `execution-id @key`; Rhai playbooks; **delete-on-prune** | `mem-gecko-architecture.md`, `mem-gecko.md`, `mem-gecko-corrected.tql` | **SUPERSEDED — do not test shipped code against these** |
| **L2 "memory-item / four-layer"** | `memory-item sub okf-concept` | four-layer substrate; full provenance ledger; populations + resolution + retrieval-provenance | `mem_gecko_analysis_fable.md` → shipped `mem_types.tql` / `mem_functions.tql` | **LIVE** |

`mem-gecko-corrected.tql` is a *corrected L1* schema, **not** an alternate of shipped — never diff shipped against it as if it were authoritative.

**The live lineage is L2.** Type names an auditor must NOT treat as shipped: `cognitive-node`, `hypothesis`, `deprecation`, `semantic-belief`, and `contextualizes`-as-episode-link. The shipped equivalents are `belief` (with `belief-state`), the `supersession` relation, and `contextualizes` as constraint→context-target (`mem_types.tql:185-187`).

---

## §1 — The nine invariants (verbatim spine + verification) — *DO NOT EDIT THE DEFINITIONS*

Each entry: the corpus text **verbatim** (the *claimed intent of record*), then three reconciliation slots — **verify against** (`file:line`), **verdict** (`honored | drift | unrealized`), and **enforcement locus** (`schema` = the type system forbids the violation · `persisted-function` = a TQL function computes it · `writer` = Rust `MemWriter` guards it · `prose-only` = no mechanism). The enforcement-locus column is load-bearing: a `writer`-enforced invariant holds **only while invariant 2 is airtight**, which converts several "honored" verdicts into "honored *contingent on invariant 2*."

### Invariant 1 — Belief/record boundary
> *"`X-as-belief` applies to the assessment of X, never X itself. **Records** = observed/authoritative, uncontested facts — cheap entities/relations, no provenance apparatus, TTL-eligible, written via the plain engine path. **Beliefs** = contested/derived/revisable claims — fully reified, written **only** via `EpistemicWriter`, kept sparse."*

- **verify against:** `core_schema.tql:23-84` (record tier) vs `mem_types.tql:61-115` (belief tier); record-path bypass proven at `gecko-bin/tests/no_drag_test.rs:5-6,56`.
- **verdict:** **honored.** Disjoint type hierarchies; the syncer writes records directly, never touching belief types.
- **enforcement locus:** **schema** (distinct hierarchies) **+ writer** (the "beliefs only via EpistemicWriter" half → invariant 2; nothing in TypeDB forbids a raw insert into `belief`).

### Invariant 2 — `EpistemicWriter` is the exclusive belief-write path
> *"Every belief-tier write is host-mediated and run-id-stamped automatically. Sandbox/agent code cannot forge or omit provenance. Records bypass it entirely (the syncer writes core records directly — this is what keeps plain markdown→DB drag-free)."*

- **verify against:** `epistemic.rs:1-8`; `writer.rs:745,760,799` (`observe`/`assert_belief`/`supersede` route through `tql::insert_belief_ops`, `writer.rs:769,827`); host stamps `RunContext`, sandbox cannot construct one (`provenance.rs`).
- **verdict:** **honored — but procedurally, not structurally.** No schema constraint prevents a second writer or a raw `db.write` to `belief`/`derivation`/`supersession`. The monopoly is a Rust convention.
- **enforcement locus:** **writer / procedural. THIS IS THE LINCHPIN.** Invariants 1(half), 7(guard), OP-1/3(write)/4/5, and the write-side of the two-ledger firewall inherit their integrity from this being airtight. The conflict report's **enforcement-locus cross-cutting pass** enumerates every belief-tier write path and confirms/denies the monopoly; treat this verdict as *provisional until that pass completes*. **Ratified disposition:** accept as procedural **and guard it with a committed monopoly test** (the inverse of `syncer_stays_on_the_record_path`), i.e. assert no non-`MemWriter` path writes belief-tier types.

### Invariant 3 — Registration is the activation gate
> *"`build_extensions(cfg)` instantiates only config-named extensions, so a disabled extension's types and functions are simply never applied — no second trait. **mem policy:** always registered; applies its schema wholesale for now… Keep mem's schema in two fragments (`mem_types.tql`, `mem_functions.tql`), apply both today."*

- **verify against:** `gecko-bin/src/config.rs` (`build_extensions`, mem forced-on first), `gecko-bin/src/main.rs:601-633` (`apply_all_schemas`, idempotent, per-SchemaInit).
- **verdict:** **honored for activation; over-claims for schema reversibility.** Registration gates *host-fn/write-path activation at runtime*. It does **not** undo `define`d schema: a `plays` clause a fragment added to a substrate type persists after that extension is disabled (`define` is not auto-`undefine`d). "A disabled extension applies neither types nor functions" holds for a *fresh build*, not a re-init on an existing DB. → **T7**.
- **enforcement locus:** **assembler code** (`config.rs`), not schema.
- **Ratified disposition (T7):** **scope invariant 3 explicitly to *activation* gating**; document that schema mutations are per-DB persistent and outside the gate. `undefine`-on-disable is **not** required (single-bundle / one-DB-per-bundle makes the residue a benign unused `plays`). Fix class: **doc**.

### Invariant 4 — Sparse reasoning overlay
> *"GECKO holds decisions and their evidence, referencing bulk data by ID/hash. `|beliefs| ≪ |records| ≪ |refs| ≪ |raw external|`; reification cost touches only the belief tier."*

- **verify against:** design posture; embodied by inv 1's tier split — no single constraint.
- **verdict:** **honored as a design posture; unenforced as a quantity.**
- **enforcement locus:** **prose-only.** A cardinality the schema does not (and cannot cheaply) police.

### Invariant 5 — Cross-cutting primitives live in mem
> *"`rests-on` (assumption graph), `pivot` (investigative move + calibration), `prediction`/`resolution` (calibration flywheel) are substrate types; no domain redefines them locally."*

- **verify against:** `mem_types.tql:205-207` (`rests-on`), `:209-214` (`pivot`), `:216-220` (`prediction-resolution`), `:259-265` (`resolution`/`resolves`). Cyber *adds `plays`* onto these but defines no local copy (`cyber_schema.tql:141,155-156,167-168`).
- **verdict:** **honored.** Defined once in mem; domains extend by role-addition only.
- **enforcement locus:** **schema + convention** (the role-additions are the subject of T7).

### Invariant 6 — Bitemporal is substrate
> *"`event-time` vs `ingest-time`, `valid-from`/`valid-to`, supersession lineage live on mem's belief types. `believed_at(t)` is a substrate capability."*

- **verify against:** `mem_types.tql:17-20`, `:88-89`, `:99-100`, `:153-157`; `mem_functions.tql:20-25` (`believed-at`).
- **verdict:** **honored** at the belief/episode layer. **Caveat (→ T5):** `population-members`/`canonical-entity` are **present-state only** (no time parameter, `mem_functions.tql:122-129,134`) → do **not** compose bitemporally with `believed-at`.
- **enforcement locus:** **schema** (attrs + supersession) **+ persisted-function** (`believed-at`).

### Invariant 7 — Provenance/derivation is graded
> *"`DerivationMethod` distinguishes **provable** (type-enforced join, cardinality, functional-dependency → confidence 1.0 permitted) from **probabilistic** (LLM-synthesis → confidence < 1.0, marked, structurally forbidden from superseding higher-entrenchment beliefs)."*

- **verify against:** `mem_types.tql:10` (7-value `derivation-method`), `epistemic.rs:80-91` (`provable()`/`probabilistic()`), `tql.rs:29-39` (method→entrenchment), `semantic.rs:84-92` (`strength()`), `writer.rs:806-824` (supersede guard), `mem_functions.tql:134-152` (`canonical-entity` provable-only).
- **verdict:** **drift — the binary partition does not classify 3 of the 7 shipped methods** (see §2). The entrenchment-monotonicity guard is **honored** but lives in the writer, not the schema.
- **enforcement locus:** **split.** `derivation-method` values = **schema**; provable/probabilistic = **Rust**; canonical-entity's provable-only closure = **persisted-function**; entrenchment-monotonicity = **writer only** (`writer.rs:817-823`), not a TQL predicate.
- **→ OPERATIVE STATEMENT SUPERSEDED IN §8.1** (the binary framing above is stale; the ratified trinary — provable / probabilistic / **attested** — is §8.1. Retain the verbatim above as claimed-intent-of-record; do not edit it in place.)

### Invariant 8 — The semantic index is a pure retrieval accelerator, never a source of truth
> *"It stores only `(embedding, concept_id, coarse filter metadata)`… Populated only through `EpistemicWriter`. Index failure never fails a graph write… Present-state only. As-of-time-T queries do not touch the index… Retrieval is recorded in a separate ledger from derivation… the similarity score lives only on a retrieval edge in the episodic tier — never on a `derivation`. `derivation-chain()` never traverses the retrieval ledger."*

- **verify against:** `core/gecko-semantic-index/src/lib.rs`; `writer.rs:1090-1194` (index failure marks dirty, never fails the write); `RecallQuery.as_of` bypass; two-ledger firewall in schema: similarity only on `surfaced`/`informs-synthesis` (`mem_types.tql:270-278`), reachable only via `retrieval-provenance-of` (`mem_functions.tql:114-118`); `derivation-chain` follows only `derivation` (`:28-36`).
- **verdict:** **honored at the read layer.** **Latent (→ T1, T8):** nothing *schema-level* stops a future writer from recording a similarity signal as a `derivation` source — the write-side firewall is procedural.
- **enforcement locus:** **schema** (ledger separation for reads) **+ persisted-function** (disjoint walks) **+ writer/procedural** (populate-through-writer, present-state-only, failure-tolerance).

### Invariant 9 — Adapter-neutral identity; cross-source identity is a belief, never a merge
> *"Source-ids are opaque… No component outside its originating adapter may parse a source-id's structure (lint-enforced)… `origin-kind` is an open, adapter-extensible set… Cross-source identity is a reified `resolution` belief, not `owl:sameAs` and not a physical merge… A canonical entity is a query-time view over the transitive closure of provable resolutions… `owl:sameAs` is rejected precisely because it is symmetric-transitive identity-collapse with no provenance and no supersession."*

- **verify against:** `mem_types.tql:259-265` (`resolution sub belief`, `resolves`), `:37` (`origin-kind` OPEN string, not `@values`), `:257-258` (canonical entity IS a population; resolution is a reified belief), `mem_functions.tql:134-152` (`canonical-entity` provable-closure view); `tql.rs:16-25` (`origin_kind` open-set); source-id opacity is **lint/prose** (`tql.rs:17`).
- **verdict:** **honored at the schema/read layer.** **Unrealized (→ T2, T4, T5):** the resolution/population *write & materialization* path is a stub; divergence-exceptions have no mechanism.
- **enforcement locus:** **schema** (resolution-is-a-belief, open `origin-kind`) **+ persisted-function** (`canonical-entity`) **+ lint/prose** (source-id opacity). No writer yet for the ingestion half.

---

## §2 — Derivation-method partition check (RATIFIED as trinary; see §8.1)

The corpus's *validated draft* (`docs/refactor/g1-g2-validated.md:36`) declares **4** methods; the **shipped** schema (`mem_types.tql:10`) declares **7**. The 3 added methods are exactly the ones invariant 7's binary partition fails to classify.

| # | Method (`mem_types.tql:10`) | In 4-value draft? | `provable()` | `probabilistic()` | **Posture (§8.1)** | Entrenchment (`tql.rs:29-39`) | Rank (`semantic.rs:84`) |
|---|---|---|---|---|---|---|---|
| 1 | `type-join` | ✅ | **✅** | — | **provable** | tool-derived | 2 |
| 2 | `cardinality` | ✅ | **✅** | — | **provable** | tool-derived | 2 |
| 3 | `functional-dependency` | ✅ | **✅** | — | **provable** | tool-derived | 2 |
| 4 | `type-db-function` | ❌ | ❌ | ❌ | **attested** | tool-derived | 2 |
| 5 | `external-tool` | ❌ | ❌ | ❌ | **attested** | tool-derived | 2 |
| 6 | `human-assertion` | ❌ | ❌ | ❌ | **attested** | user-stated | 3 |
| 7 | `llm-synthesis` | ✅ | — | **✅** | **probabilistic** | llm | 0 |

**Findings (ratified dispositions):**

- **F-DM1 — RATIFIED as a real trinary, not a gap to collapse.** Methods 4–6 are **neither `provable()` nor `probabilistic()`**; the shipped enum outgrew invariant 7's binary. They form a coherent third class — **`attested`** (§8.1): *truth asserted by a named external authority the system does not itself verify*. This is **invariant-7 text drift** (the code is more correct than the doc), not code-behind-doc. **Fix class: decision (made) + doc + code** — (doc) adopt §8.1's operative wording; (code) make `epistemic.rs` classification **total** by adding an `attested()` predicate so no method returns false-to-all; (decision) attested confidence is **source-bounded** (neither structurally pinned to 1.0 nor forced <1.0).
- **F-DM2 — RATIFIED as intended orthogonality.** Entrenchment-tier (authority to supersede) and provability-posture (contributes to canonical identity) are **orthogonal axes**. `type-db-function`/`external-tool` get **tool-derived (rank 2)** — same tier as the provable trio — yet are **not** counted by `canonical-entity` (provable-only, `mem_functions.tql:139,146`). Intended: a function-derived belief is authoritative enough to supersede an inference but must not auto-merge identities (that wants a *provable* functional-dependency). The reconciled doc states the orthogonality explicitly so no reader conflates "tool-derived entrenchment" with "provable."
- **F-DM3 (negative — no gap):** all 7 methods map to an entrenchment rank (`tql.rs:29-39` is total). No "method with no rank" failure mode.

---

## §3 — Operational invariants (RATIFIED: separate `OP-` section, not renumbered)

The corpus and code treat these as invariants; they are not in `(1)§1`. **Ratified:** keep as a distinct `OP-`-prefixed section — **do not renumber** (renumbering breaks ~60 code citations *and* collides with the 10–17 *mechanism* space in §4). The `OP-` prefix is a permanent, self-documenting namespace ("OP-5" can never be misread as "invariant 5" or "mechanism 15"). Each OP **cross-links** to the numbered invariant it operationalizes, so "separate" reads as *different enforcement tier*, not *second-class*.

| ID | Statement | Operationalizes | verify against | Enforcement locus |
|---|---|---|---|---|
| **OP-1** | **Host-stamped, unforgeable provenance.** `run_id`/`actor`/`source` host-minted; sandbox cannot forge. | **↔ inv 2** (the mechanism behind it) | `provenance.rs` (`RunContext`); `writer.rs` threads `ctx` into every write | writer / host |
| **OP-2** | **The gate runs in TypeDB, never the vector layer.** Authoritative visibility/validity/supersession gating is TQL; the index is a coarse pre-filter only. | ↔ inv 8 | `mem_functions.tql:89` (`gate`) | persisted-function |
| **OP-3** | **Two-ledger firewall.** Retrieval-provenance ⟂ logical derivation; temporal ⟂ index. One query each, never crossed. | **↔ inv 8** (this is the firewall write/read-side that T1/T8 hinge on) | `mem_functions.tql:28-36` vs `:114-118`; `mem_types.tql:270-278` | schema + persisted-function (reads); **writer (writes → T1/T8)** |
| **OP-4** | **Embedding-ownership-transfer on supersession.** Only the supersession *head* owns the embedding (recall-correctness). | ↔ inv 8 | `writer.rs:1171-1194` | writer |
| **OP-5** | **Entrenchment monotonicity.** `llm` may never auto-supersede `user-stated`/`axiom`. | **↔ inv 7** (the guard behind graded provenance) | `writer.rs:817-823`; `fable:477` (prose origin) | **writer only** (not schema) → T3 |
| **OP-6** | **Build/CI invariants.** Model is a runtime asset never a build dep; `auto_fetch=false`; `integration-model` is an un-crossable feature boundary; CI stays candle-free. | ↔ (packaging plan) | `CLAUDE.md`, `gecko-bin/src/config.rs` (`default_model_id`), `.github/workflows/ci.yml` | build-config + CI guard |

> **OP-6 verification is COARSER than 1–9** — file-level, not `file:line`, and it folds **four** distinct invariants into one row (model-not-a-build-dep · `auto_fetch=false` · `integration-model` boundary · candle-free CI). **Before relying on OP-6 to the same standard as 1–9, confirm each at `file:line` and consider splitting into OP-6a…d.**

**OP-1, OP-3(write), OP-4, OP-5 are writer-enforced** → they inherit invariant 2's contingency (the linchpin).

---

## §4 — Numbering-space map (invariants vs mechanisms)

Two numbering spaces; do not conflate:

- **1–9** = the corpus/code **invariants** (§1).
- **10–17** = the audit's tension-list **mechanisms** — *not* additional invariants: `retrieval-provenance` (A2.10), `population-as-view` (A2.9), the concept/belief seam, divergence-exceptions, consolidation ("dreaming"), the population-spec language, the `{ concept }` return-type coupling, the ID scheme.
- **OP-1…6** = operational invariants (§3), a third, prefixed space.

The conflict report cites all three (e.g. "T2 = inv 11 × 6 × 7" reads mechanism-11 × invariant-6 × invariant-7).

---

## §5 — The `design §X.Y` referent (resolution + stale-anchor findings)

The ~8 `design §` doc-comments anchor to `~/Downloads/gecko_design.md` (Master Spec v1.1) — the referent, **half-superseded**:

| Anchor (code) | `gecko_design.md` heading | Status |
|---|---|---|
| `main.rs:7` (§2) | §2 Codebase Organization & Dependency Enforcements | **live** |
| `okf/types.rs:87` (§3.1) | §3.1 OKF Compliance Layer | **live** |
| `registry.rs:3,89` (§3.4) | §3.4 Host-Managed State Registry (RAII) | **live** |
| `lib.rs:81` (§4) | §4 Polymorphic Extension Architecture | **live** |
| `core_schema.tql:21` (§4.1) | §4 (abstract base types) | **live** |
| `pipeline.rs:1,75` (§5) | §5 Execution & Contextual Pipeline | **live** |
| `mem-gecko/src/lib.rs:7` (§4.3) | §4 (`contextualizes` relations) | **STALE** — master spec's `contextualizes` (episode/target-entity) ≠ shipped constraint→context-target (`mem_types.tql:185-187`) |
| — (no code anchor) | §3.2 Dual-Engine Wasm Sandbox (Rhai + QuickJS) | **STALE** — shipped is single-engine **Boa** |

**F-DS1 (doc-drift):** the `design §` citations point at an **out-of-repo, partly-wrong** document. **Fix class: doc** — land a reconciled design/architecture doc in-repo (or fold the live sections here), retire/annotate §3.2 and §4.3 as superseded, and repoint `mem-gecko/src/lib.rs:7`.

---

## §6 — Inter-doc drift findings (self-consistency pass output)

| ID | Drift | Authoritative resolution | Fix class |
|---|---|---|---|
| **D-1** | `gecko-refactor-implementation-plans.md` (no `(1)`) numbers invariants **1–7** (population as #5); `(1)` numbers **1–9** (code-matching). | **RATIFIED: `(1)` is canonical; the 1–7 numbering is retired.** Nothing in code depends on the stale one. | decision (closed) |
| **D-2** | `gecko_design.md` names **Rhai + QuickJS dual engine**; shipped is **Boa** (single, compiled to wasm, checked in as `quickjs.wasm` despite the name). | Shipped Boa authoritative; mark §3.2 superseded. | doc |
| **D-3** | `mem-gecko-research-addendum.md:311` **proposes** provenance-lite (doubts it at :486); `(1):371` **deletes** it (two deployment shapes only; middle mode forbidden by inv 2). | The deletion is authoritative; the addendum is stale. | doc |
| **D-4** | `mem-gecko.md:115` specifies **delete-on-prune**; shipped is **never-delete** (supersession + `consolidation-state` archived/tombstoned, `mem_types.tql:9`). | Never-delete authoritative. | doc |
| **D-5** | Corpus describes `contradiction` variously; shipped shape is an **`anomaly` hub + `claim @card(2..)`** (`mem_types.tql:144-150`), read by `contradicts()` (`mem_functions.tql:56-61`). | Shipped hub-shape authoritative. | doc |
| **D-6** | Decay model: shipped keeps **static activation in TQL** + **host-side continuous half-life decay** (`mem_functions.tql:38-54`) — continuous decay is inexpressible in pure TQL. | Shipped split authoritative; record the TQL limitation. | doc |
| **D-7** | derivation-method **7 shipped vs 4 in the validated draft** — see §2. | Shipped 7 authoritative; **RATIFIED trinary** (§8.1); F-DM1 completes the partition. | decision + doc + code |
| **D-8** | Whole invariants/design corpus is **untracked** (`~/Downloads`); citations point out-of-repo. | Land this file + a design doc in-repo. | doc |

---

## §7 — Ratification record (dispositions locked)

The two divergence axes are now **ratified**, not open. **(a) code-vs-doc:** invariants whose shipped schema/functions do not enforce the §1 text as stated → disposition below. **(b) doc-vs-intent:** the maintainer's decisions on where the verified draft was contestable → answered below.

### (a) code-vs-doc — ratified dispositions

| Drift | Where it should be enforced but isn't | **Ratified disposition** |
|---|---|---|
| **Inv 2** monopoly is procedural, not schema-enforced | no schema constraint on belief-tier writes | **Accept as procedural; guard with a committed MemWriter-monopoly test** (inverse of `syncer_stays_on_the_record_path`). The enforcement-locus pass enumerates and closes the bypass list. |
| **Inv 3** over-claims schema reversibility (T7) | `define`d `plays` persists post-disable | **Scope inv 3 to *activation* gating; document per-DB schema persistence.** No `undefine`-on-disable. |
| **Inv 7** partition is trinary, not binary (F-DM1) | `epistemic.rs:80-91` classify only 4 of 7 | **Ratify the trinary (provable/probabilistic/attested, §8.1); make `epistemic.rs` classification total (add `attested()`).** |
| **Inv 7** monotonicity is writer-only (OP-5) | `entrenchment` is a plain attr; no TQL ordering | **Accept as writer-enforced (contingent on inv 2).** A TQL guard is optional future hardening, not required. |
| **Inv 6** views don't compose bitemporally (T5) | `population-members`/`canonical-entity` take no `$t` | **Decision owed with the resolution writer:** add as-of-t variants (host-side strict re-eval exists, `mem_functions.tql:124`) **or** declare the views present-state-only and forbid combining with `believed_at` (mirroring the index's documented bypass). Fix **jointly with T2**. |
| **Inv 9** ingestion half unrealized (T2/T4) | resolution/population write path is a stub | **Mark `unrealized`; the write-path guardrails + divergence-exception mechanism (§8.2) are owed before the resolution writer.** |

### (b) doc-vs-intent — ratified answers

| Question | **Ratified answer** |
|---|---|
| Operational invariants (OP-1…6) placement | **Separate `OP-` section, not renumbered** (§3). Cross-linked to the invariant each operationalizes. |
| Trinary derivation-method (F-DM1) | **Deliberate third class — `attested`** (§8.1). Make classification total; do not collapse into the binary (collapsing would misrepresent provenance). |
| `type-db-function`/`external-tool` at tool-derived entrenchment yet not provable-for-canonical (F-DM2) | **Intended orthogonality** (entrenchment ⟂ provability). Ratified as-is; stated explicitly so the axes aren't conflated. |
| **`human-assertion` (rank 3) out-entrenches the provable trio (rank 2)** — an analyst claim can supersede a `type-join`-provable belief | **Intended and ratified.** Human judgment overriding machine certainty is the correct default for a security substrate; entrenchment is an *authority* axis orthogonal to *provability*. "Provable/confidence-1.0" and "highest entrenchment" are deliberately decoupled. |
| Divergence-exception semantics (T4) | **See §8.2 — RATIFIED: evidence-ranked resolution, supersede-not-delete.** Neither corpus pole (override / present-both) is canonical alone. |
| Numbering: `(1)` 1–9 vs stale 1–7 (D-1) | **`(1)` canonical; 1–7 retired.** |

**On ratification (scoped to reconciliation — NOT a clean bill of health):** verdicts for invariants **1, 5, 6, 8** stand **honored** (with caveats routed to tensions); **2** is honored-but-procedural (pending the enforcement-locus pass); **3** and **7** carry **drift** (dispositions above); **9** is honored-at-schema / **unrealized**-at-ingestion. **No mutual inconsistency surfaced *during reconciliation*** — the drifts found here are code-vs-doc and doc-vs-doc. **Interaction conflicts between individually-honored invariants remain pending the conflict report** — T2/T4/T5/T8 are precisely the composed-conflict cases (e.g. T2 composes two individually-honored invariants, 11 and blast-radius, into a silently-stale belief). This is the expected pre-audit shape, not a guarantee of consistency.

---

## §8 — Ratified successor statements & decisions (the operative corrections)

*These supersede the specific §1 verbatim they name, as the **operative** statements. The §1 verbatim is retained as claimed-intent-of-record; the successor here is what the conflict report and the code enforce.*

### §8.1 — Invariant 7, operative statement (supersedes the §1 verbatim of invariant 7)

> **`DerivationMethod` classifies each of the 7 shipped methods into one of THREE provenance postures:**
> - **provable** — `type-join`, `cardinality`, `functional-dependency`. Re-derivable from the system's own type-enforced structure. **Confidence 1.0 permitted.** The **only** postures `canonical-entity` counts toward identity closure.
> - **probabilistic** — `llm-synthesis`. Stochastic generation. **Confidence < 1.0, marked.** May not supersede higher-entrenchment beliefs.
> - **attested** — `type-db-function`, `external-tool`, `human-assertion`. **Truth asserted by a named external authority the system does not itself verify.** Confidence is **source-bounded** — neither structurally pinned to 1.0 nor forced below it; it reflects the declared reliability of the source, not a property of the method.
>
> **Provability posture is orthogonal to entrenchment tier** (F-DM2): an `attested` method may out-entrench an `inferred` one (`human-assertion`=user-stated=3 > provable trio=tool-derived=2) **without** counting as provable. Authority-to-supersede and contributes-to-identity are independent axes.

*Reason this supersedes the verbatim: F-DM1 — the verbatim's binary framing does not classify 3 of the 7 shipped methods. The code is more correct than the §1 text.*

### §8.2 — Divergence-exception semantics (T4 / Q1), RATIFIED

**Neither corpus pole is canonical alone.** `(1):185` ("instance **overrides** population") and `research-addendum:165` ("present **both**, contested, nothing silently wins") each wrote down **one end** of a single spectrum as if it were the whole rule. The corpus "contradiction" is that each captured a different endpoint.

**Canonical semantics — evidence-ranked, recorded, retractable resolution:**

- When an instance belief and a population belief disagree about an entity, the divergence-exception writer **consumes the competing beliefs' `derivation-method` + `confidence` + `entrenchment` + `valid-from`** to compute a **disposition** — it does not merely record that a conflict exists, and it **never silently discards a side**.
- **Evidence strongly separates them** → auto-resolve toward the stronger belief with an explicit **confidence and recorded reason**; for most queries that lean is the returned answer. **The losing belief is `superseded`, not deleted** — retained with lineage, so a post-incident query shows *"assessed X at confidence C because [evidence]; here is what misled us."*
- **A human asserts precedence** → a first-class **higher-entrenchment precedence belief**, itself attributed and retractable. **This is what "override" *is*** — the confidence-≈-1.0 / human-precedence end of the spectrum, reached deliberately, still leaving the loser superseded rather than erased.
- **Evidence does NOT separate them** → surface as genuinely **`contested`** for human adjudication. The escape hatch, not the per-divergence default.

**"Override" and "contested" are the two ends of one spectrum, not rival modes.** Auto-*ranking* by evidence is good; auto-*deletion* of the loser is the `owl:sameAs` lossy-collapse invariant 9 rejects — forbidden here for the same reason.

**This maps onto shipped machinery** (the tell it's the right model, not a new subsystem): `derivation-method`+`confidence` grade support strength; `entrenchment` grades authority; bitemporal `valid-from` makes "newer supersedes older, for this entity" a first-class **supersession** (retain-with-lineage); the `contested` belief-state is the genuine-tie output; the `contradiction`→`anomaly` hub models the recorded conflict.

**Mechanism to build (fix class: decision MADE → schema + function), owed BEFORE the resolution writer:**
1. **A precedence relation** so an instance belief can be marked as taking priority over a population belief *with provenance* (the reification the schema currently lacks — `scoped` offers no override construct; this absence *is* why T4 is a conflict).
2. **A disposition/resolution function** that ranks competing beliefs by (`derivation-method`, `confidence`, `entrenchment`, `valid-from`) and returns `resolved(head, confidence, reason)` or `contested`.
3. **Reify divergence-exceptions as beliefs** (not views): a query-time view (`canonical-entity`) can never be a `contradiction` `claim` (`mem_functions.tql:56-61`), so a view-vs-instance conflict is otherwise structurally invisible. The exception must be a belief to participate in the contradiction machinery at all.

### §8.3 — Enforcement-locus summary (the audit's headline, ratified as a first-class concern)

Several "honored" verdicts hold **only because `MemWriter` guards them in Rust**, not because the schema forbids the violating state: invariant 1 (belief-only-via-writer half), invariant 7 (entrenchment monotonicity, OP-5), the write-side two-ledger firewall (OP-3), OP-1, OP-4. **All inherit invariant 2's contingency.** The conflict report's cross-cutting pass must enumerate every belief-tier write path (`begin_write`/`apply_schema`/`query` touching `belief`/`derivation`/`supersession`/`resolution`/`surfaced`) and confirm the `MemWriter` monopoly. **Ratified disposition:** accept procedural enforcement **and** commit a monopoly test asserting no other writer exists; revisit pushing specific guards (e.g. entrenchment ordering) into the schema only if the bypass list is non-empty.

### §8.4 — Work items implied by these decisions (executed by the conflict report + fixes, NOT this doc)

| # | Work item | Fix class | Trigger |
|---|---|---|---|
| W1 | Make `epistemic.rs` derivation-method classification **total**: add `attested()`; adopt §8.1 wording in the ratified invariant-7 statement. | code + doc | F-DM1 / D-7 |
| W2 | Build the **divergence-exception mechanism**: precedence relation + disposition function + reify-as-belief (§8.2). | schema + function | T4 / Q1 — **before the resolution writer** |
| W3 | Decide **as-of-t population/canonical** (variants) vs **present-state-only-and-forbidden-with-`believed_at`**; implement **jointly with T2** (cohort-belief blast-radius). | function + decision | T5 + T2 |
| W4 | **Scope invariant 3 to activation** in the operative doc; document per-DB `plays` persistence. | doc | T7 |
| **W5a** | Commit the **MemWriter-monopoly test over the FULL belief tier** — `belief` **entity + belief-tier relations** (`derivation`, `supersession`, `surfaced`, `informs-synthesis`, and domain belief-referencing relations `technique-detection`/`alert-disposition`). An entity-only test is not meaningful — it can't fail on the live cyber raw-relation breach. | test + audit | inv 2 / §8.3 |
| **W5b** | **Route the cyber belief-referencing relations through a sanctioned writer** (`EpistemicWriter` extension point or mem-owned relation-writer) — the **proof case for W5**; **blocks W5a** (W5a stays red until this lands). | code | inv 2 / cyber `detect/mod.rs:142,196` |
| W6 | Land a **reconciled design/architecture doc** in-repo; retire `gecko_design.md` §3.2/§4.3; repoint `mem-gecko/src/lib.rs:7`. | doc | F-DS1 / D-2 / D-8 |
| W7 | **Split & line-verify OP-6** (4 invariants folded, file-level only). | doc + verify | OP-6 note |
| W8 | Specify the **consolidation ("dreaming") daemon contract** so it obeys the two-ledger firewall (merge-similarity ⇏ derivation), the retrieval-provenance retention rule, and entrenchment (daemon beliefs are `llm`-class, cannot override) — and **writes through `EpistemicWriter`, never raw `graph.write`** (closes the loop with W5: the daemon is a new belief-writer the monopoly must bind). | decision → schema/function | T8 (conflict report) |
| **W-T6** | Guard **observable identity convergence**: fold into the cyber-gecko C2.4 resolution-pass acceptance test — two `observable`s, same normalized `ioc-value`+`ioc-type`, different source ⇒ `canonical-entity` returns **one** entity. Prevents silent two-feed IOC fragmentation. | function + test | T6 residual |
