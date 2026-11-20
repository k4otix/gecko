# SPEC-W5 — the MemWriter-monopoly test (W5a) + the sanctioned belief-edge writer seam (W5b)

> Status: **RATIFIED at commit `95f0d6e`** (user act) — ratified **as design**: the W5a
> monopoly **test design** (§2), the W5b sanctioned-writer **seam design** (§3), and the
> W5b-blocks-W5a-green **coupling** (§3.3). The anchor is SPEC-W5's **own** evidence commit
> — where **P-W5-1** is green and the introspected surface (**23→24** on the W2 delta) was
> observed and logged — **not** W2-W3's `11d7ab1`: each ratification anchors to the evidence
> state it was made against (§6). **§5's executable checks (the shipped monopoly test + the
> shipped seam) are owed-to-implementation**, not ratified as done — only P-W5-1 is green
> today; this spec ratifies the *design*, and implementation must land §5's named checks
> green. It ships no production code and no shipped-schema change.
> Gates: [`INVARIANTS.md`](./INVARIANTS.md) §8.3 (enforcement-locus / monopoly),
> §8.4 rows W5a/W5b; [`CONFLICT-REPORT.md`](./CONFLICT-REPORT.md) **Headline**
> (the enforcement-locus cross-cutting pass — the bypass-path table, the cyber
> raw-`graph.write` breach, the type-blind seam).

This spec closes the two coupled work items INVARIANTS §8.4 leaves open for the
EpistemicWriter monopoly:

- **W5a** — commit the MemWriter-monopoly test over the **full belief tier**
  (`belief` entity + belief-tier relations), the inverse of the record-side
  `syncer_stays_on_the_record_path`. A test design.
- **W5b** — route the cyber belief-referencing relations
  (`technique-detection`/`alert-disposition`) through a **sanctioned writer**, the
  proof case that lets W5a go green. A seam design.

They are one spec because **W5b blocks W5a-green** (§0, §3.3): the monopoly test
stays red until the live cyber raw-relation breach is routed through the seam. The
test surface is not a hand list — it is **introspection-derived** and proven live by
**P-W5-1** (§4), so it catches every belief-edge relation the schema owns today
(cyber's three — the two raw-written breaches `technique-detection`/`alert-disposition`
plus the defined-but-unwritten `sighting-of` (§3.3) — plus the ones INVARIANTS §8.4's
enumeration omits) and any that land tomorrow (W2's `divergence-precedence`).

Every design question below carries a **decision + rationale + rejected alternative**
(or, where a question is an obligation with no either/or fork, that is stated). Every
refinement of INVARIANTS §8 text uses **verbatim-quote + delta discipline** — the
ratified sentence is quoted, then the operative clarification stated; §8 is never
silently reinterpreted. INVARIANTS.md and CONFLICT-REPORT.md are LOCKED and cited
one-way by stable anchor. Ratification is the user's act — §6 is left for their line.

**Line-drift discipline.** Every code anchor below was re-derived by `grep`/`Read`
against branch `conflict_resolution` at the time of writing. Where the
CONFLICT-REPORT Headline's anchor has since drifted, the current line is cited and the
drift noted — most notably `build_host_bridge` (the Headline's `main.rs:778,1016` is
now `gecko-bin/src/main.rs:957`, with the cyber bridge composed at `:1018` off the
graph handle captured at `:1016`, and `build_host_bridge` itself wired at `:1068`).
The cyber raw-`graph.write` anchors (`detect/mod.rs:142,196`) have **not** drifted.

---

## §0 — Scope & non-goals

**In scope.**
- **W5a — a test design (not the test).** The two halves of the MemWriter-monopoly
  guarantee: a **static** source lint (no raw belief-tier TQL outside mem's `tql.rs`)
  and a **dynamic** live test (every *enumerated* bypass-capable path runs; per-type
  counts over the introspected belief-edge surface equal what MemWriter accounts for —
  see the §2.1 residual for what a finite path-enumeration cannot reach), plus the
  introspection **ratchet** that keeps the surface honest (P-W5-1, already green).
- **W5b — a seam design (not the seam).** The `BeliefEdgeDecl` registry: the
  mem-owned, declaration-gated writer method through which an extension's belief-edge
  relations (cyber's `technique-detection`/`alert-disposition`, and future domains')
  are written run-stamped, replacing the raw `graph.write`. The registry is the **same
  source of truth** the W5a ratchet reads.

**The proof case — W5b blocks W5a-green.** W5a's dynamic half asserts the population
of every belief-tier type equals what MemWriter accounts for. The live cyber breach
writes `technique-detection`/`alert-disposition` via raw `graph.write`
(`extensions/cyber-gecko/src/detect/mod.rs:142,196`), *outside* MemWriter's
accounting — so those instances are **unaccounted**, and W5a's dynamic assertion is
**RED until W5b routes them through the seam**. This is stated again, in as many
words, in §3.3. It is the reason the two work items are one spec and ratify together.

**Out of scope (consumers of this spec, not built here).**
- **The shipped test code and the shipped seam code.** This spec designs both; the
  implementation task builds them. §5 names the owed executable checks.
- **Pushing entrenchment ordering (or any writer guard) into the schema.** INVARIANTS
  §8.3 left this open, conditional on the bypass list being non-empty — it *is*
  non-empty (§2.2). This spec records the trigger fired but does **not** design the
  schema guard; it commits the monopoly test §8.3 ratified as the primary disposition.
- **The consolidation daemon** (CONTRACT-W8) — a *future* belief-writer the monopoly
  must bind. Cross-referenced (§3.4), not designed here.
- **The populations writer / disposition engine / resolution writer** — SPEC-W2-W3
  §3.5/§2.3 own their contracts; this spec only records where their write surfaces
  join the W5a allowlist (§1.2).

---

## §1 — The belief-tier (memory-item-substrate) write surface

### §1.1 — Enumeration strategy: introspection-derived, NOT a hand list

**Design question — how is the belief-tier surface the monopoly test asserts over
enumerated? A committed hand-list, or derived from the schema?**

**Decision.** **Introspection-derived, backed by a committed allowlist of deliberate
exclusions.** The surface is computed from the composed schema at test time:

> **Surface rule.** A relation is on the belief-edge surface **iff it owns at least
> one role played by ≥1 type and *exclusively* by `memory-item` subtypes.** A relation
> qualifies on one exclusive role even if its other roles admit record/cache-tier
> players.

The introspection is exactly P-W5-1's recipe (§4): the memory-item subtype closure
from `match $t sub memory-item; let $l = label($t);`; every `(player-type, role,
owning-relation)` triple from `match $t plays $r; $rel relates $r;` with the relation
label decoded host-side via `label($rel)`; a relation kept iff some role's full player
set is non-empty and ⊆ the memory-item closure. `divergence-precedence` is caught via
`belief plays divergence-precedence:prevailing`/`:overridden` even though its
`exception-scope` role is played by the record-tier `concept`
(`gecko-bin/tests/probes/w2_precedence.tql`); `resolves`/`scoped` are caught via their
belief role though `record-*`/`population` are record/cache tier.

**Rationale.** A hand list reproduces the exact failure mode the Headline names —
*"it checks the wrong shape."* CONFLICT-REPORT Headline, W5a bullet:

> **Verbatim (`CONFLICT-REPORT.md` Headline, W5a):** "broaden the test surface to the
> full belief tier: the monopoly assertion must cover belief-tier *relations* —
> `derivation`, `supersession`, `surfaced`, `informs-synthesis`, and the domain
> belief-referencing relations `technique-detection` / `alert-disposition` — not only
> the `belief` entity. Until it does, the test is not meaningful (it can't fail on the
> one breach we have)."

That enumeration is **illustrative, not exhaustive** — and W2 adds
`divergence-precedence` the day it lands. A hand list would need a human edit for each
new belief-edge relation and would silently miss the ones nobody remembered. An
introspection-derived surface needs no edit: **P-W5-1 proves it gains
`divergence-precedence` the instant the W2 candidate schema applies, with no test
change** (§4).

**§8.4 fidelity — verbatim-quote + delta (the locked doc has a latent gap).**
INVARIANTS §8.4 row W5a enumerates the belief-tier surface as:

> **Verbatim (`INVARIANTS.md` §8.4, W5a):** "the **MemWriter-monopoly test over the
> FULL belief tier** — `belief` **entity + belief-tier relations** (`derivation`,
> `supersession`, `surfaced`, `informs-synthesis`, and domain belief-referencing
> relations `technique-detection`/`alert-disposition`)."

> **Delta (operative clarification).** **§8.4's enumeration is INCOMPLETE.** It names
> six relations as the "full belief tier," but the introspected surface on the live
> composed schema is **23 relations** without W2 and **24 with it** (P-W5-1, §4). §8.4
> omits at minimum `resolves` and `scoped` (both have an exclusively-`belief` role),
> the general substrate edges `evidence`, `justification`, `support`, `attack`,
> `contradiction`, `ownership`, `source-link`, `rests-on`, `pivot`,
> `prediction-resolution`, `contextualizes`, `execution`, `goal-hierarchy`,
> `tool-invocation`, and cyber's episodic `sighting-of` — each a genuine
> memory-item-substrate write. **This is a latent gap in a LOCKED document; it is NOT
> edited here** (GLOBAL-CONSTRAINTS: §8 is never edited). Instead, this spec records
> that **the introspection-derived surface is AUTHORITATIVE over §8.4's enumeration**:
> §8.4's list is the *seed illustration*; the surface the W5a test asserts over is the
> introspected one. This is a refinement of what "full belief tier" *means*, not a
> change of §8.4's verdict — §8.4 already said "an entity-only test is not meaningful,"
> and the introspection is how "full" is made complete and self-maintaining.

**Rejected alternative — a committed hand-list of belief-tier types (the §8.4 six).**
Rejected: it is the Headline's own failure mode. It omits `resolves`/`scoped` today
(a monopoly test built from it would pass while an unaccounted `resolves`/`scoped`
write stood), and it needs a human edit the day W2 lands — the day after W5 would
otherwise ship — reintroducing exactly the "checks the wrong shape" defect for
`divergence-precedence`.

### §1.2 — Boundary rule + explicit exclusions (the committed allowlist)

**The ratchet rule.** For every relation on the introspected belief-edge surface, it
must be **either** *accounted* (every instance was written through MemWriter / the W5b
seam and reconciles against writer-side counts, §2.2), **or** on the **committed
allowlist** of deliberate exclusions, each with a named owner, **or** — for a
**defined-but-unwritten** surface relation (zero instances today) — explicitly
**dispositioned as owed-to-seam-when-written** with a named owner (cyber's `sighting-of`,
§3.3): its zero count passes the §2.2 reconciliation trivially, but the disposition is
recorded so it is a deliberate decision, not a silent gap. Any introspected belief-edge
relation ∈ **none** of these sets **fails** the monopoly test — forcing a deliberate
decision (route it through the seam now, allowlist it with an owner, or record it
owed-to-seam-when-written), never a silent gap.

**The committed allowlist (deliberate exclusions, with owners).** *No either/or fork —
these are the enumerated deliberate carve-outs the design records so nothing is
silently out of scope; the ratifiability gate's rejected-alternative requirement is
satisfied by that framing.*

| Type | Why excluded from MemWriter-accounting | Owner | Reference |
|---|---|---|---|
| `population-member` | **Not on the introspected belief-edge surface** — its roles `owning-population` (played by `population`) and `member` (played by `concept`) carry **no memory-item role**, so the surface rule never selects it. But it is a **sanctioned-write-only cache relation**, not free-for-all: it is materialized only by the populations writer. Recorded here so it is *not silently out of scope* rather than merely absent. | populations writer (mem-materialization) | SPEC-W2-W3 §3.5 ("who may write `population-member` rows — only a sanctioned populations writer, never raw `graph.write` … feeds SPEC-W5's monopoly allowlist"); §3.2 window attrs join it |
| `population-member` window attrs (`valid-from`/`valid-to`) | Same cache-tier ownership as the relation. | populations writer | SPEC-W2-W3 §3.5 |

**Explicit inclusions the surface already catches that §8.4 omits** (not exclusions —
listed so the reader sees the surface is broader than the locked doc's seed): `resolves`,
`scoped` (belief roles), and the substrate/episodic edges enumerated in the §1.1 delta.
All are **accounted** (MemWriter authors them, or they are owed to a sanctioned writer
per SPEC-W2-W3), never allowlisted-out — **with one exception the surface caught: cyber's
`sighting-of`, which no code path writes today** (defined `cyber_schema.tql:147`; `sighting
sub execution-episode sub episode sub memory-item`, so its `sighting-instance` role is
exclusively memory-item). It is neither a live breach nor an allowlist carve-out but
**owed-to-seam-when-written** (§3.3): zero instances today (the §2.2 reconciliation passes),
but if a sighting-ingestion path is ever built it must route `sighting-of` through the W5b
registry, never raw.

**Record-tier boundary (unchanged, cited).** The record tier owns none of this: the
existing `no_drag` source lint proves `concept`/`external-resource`/`bundle` own no
belief attribute and play no epistemic **bearer** role
(`gecko-bin/tests/no_drag_test.rs:160`,
`record_tier_owns_no_belief_apparatus_and_plays_no_bearer_role`); being a
`source-link:origin` (cited) is record-safe, being the `source-link:memory` (the belief
that cites) is not (`no_drag_test.rs:117-119`). W5a is the **inverse** guarantee over
the belief tier — the direction §8.3 says is missing.

---

## §2 — W5a: the MemWriter-monopoly test design

INVARIANTS §8.3 ratifies the disposition this section designs:

> **Verbatim (`INVARIANTS.md` §8.3):** "the conflict report's cross-cutting pass must
> enumerate every belief-tier write path (`begin_write`/`apply_schema`/`query` touching
> `belief`/`derivation`/`supersession`/`resolution`/`surfaced`) and confirm the
> `MemWriter` monopoly. **Ratified disposition:** accept procedural enforcement **and**
> commit a monopoly test asserting no other writer exists; revisit pushing specific
> guards (e.g. entrenchment ordering) into the schema only if the bypass list is
> non-empty."

> **Delta (operative clarification).** §8.3's parenthetical list of belief-tier types
> (`belief`/`derivation`/`supersession`/`resolution`/`surfaced`) is, like §8.4's, a
> *seed illustration*; §2 asserts over the **introspection-derived surface** (§1.1). Two
> complementary tests deliver "no other writer exists": a **static** half (no raw
> belief-tier TQL can even be authored outside mem's `tql.rs`, §2.1) and a **dynamic**
> half (having run every bypass-capable path, the belief-tier population reconciles with
> MemWriter's accounting, §2.2). §8.3's "revisit pushing specific guards into the schema
> only if the bypass list is non-empty" — the bypass list **is** non-empty (the cyber
> breach, §2.2), so the trigger has fired; the ratified *primary* disposition is still
> the committed monopoly test, and schema-hardening is left as owed future work
> (SPEC-W2-W3 §2.3's conditional C2 `out-entrenches` is one such candidate), not
> designed here.

### §2.1 — Static half: a workspace source lint (generalizing `syncer_stays_on_the_record_path`)

**Design question — how do we prove no code path *authors* raw belief-tier TQL outside
the sanctioned writer?**

**Decision.** A **workspace source lint** that generalizes the record-side
`syncer_stays_on_the_record_path` (`core/gecko-engine/src/syncer/bundle.rs:741`) into
its inverse: **no raw TQL string that `insert`/`put`/`update`s a belief-tier type may
exist outside `core/mem-gecko/src/tql.rs`.** It lives beside `no_drag_test.rs` and
reuses its scanning/`count()` idiom (`no_drag_test.rs:139` `wiring_for`,
`:28` `count`). The lint scans workspace `.rs` sources (and any `.tql` fragment shipped
by an extension) for a write verb (`insert`/`put`/`update`) applied to any name on the
introspected belief-edge surface (§1.1), and asserts every hit resolves to
`core/mem-gecko/src/tql.rs` — where mem authors all belief writes
(`insert_belief_ops`, `tql.rs:196`, the sole authoring point routed from `writer.rs:769`
and `:827`).

**Rationale.** The write seam is **type-blind** — this is the Headline's structural
finding, and the reason a purely dynamic test is insufficient:

> **Verbatim (`CONFLICT-REPORT.md` Headline, "The write seam is type-blind"):**
> "`GraphStore::write(&[GraphWrite])` is the single production write coupling
> (`graph.rs:97-102`). `GraphWrite.query` is a **raw TypeQL string** … The seam accepts
> **any** write, including `insert $b isa belief, … `. **Nothing at the seam
> distinguishes record-tier from belief-tier writes.** The monopoly is therefore not a
> property of the seam; it is a property of *who holds a handle to it* and *what TQL
> they choose to author*." *(Anchors verified current: the `GraphStore` trait's `write`
> is declared at `core/gecko-extension-api/src/graph.rs:97`; `GraphWrite.query` is the
> raw `String` at `graph.rs:62-64`; the production impl is `RouterGraphStore::write`,
> `core/gecko-engine/src/db/graph_store.rs:71`.)*

Because the seam cannot type-check the write, the guarantee "belief writes are authored
only in one place" must be checked at the **source** level — exactly what the static
lint does. It is the *authoring* half; §2.2 is the *runtime* half. Neither alone
suffices: the lint cannot see a write assembled dynamically
(`format!("insert $x isa {}", relation_label)` carries no literal belief-tier type name
for a grep-style scan to catch), and the dynamic test only exercises the paths the
harness drives.

**Residual coverage — stated honestly; the two halves are NOT jointly complete.**
Together they cover **(literal belief-tier writes anywhere in source)** ∪ **(any write —
literal or dynamically-composed — in the four enumerated bypass-capable paths of §2.2)**.
That union is **not** *(any belief-tier write anywhere)*: a **dynamically-composed raw
belief-tier write in a fifth, non-enumerated path** is caught by **neither** half — the
static lint sees no literal type name, and the dynamic test never drives that path. This
residual is the best achievable against a **type-blind seam** (the seam cannot reject the
write, and no finite path-enumeration is provably exhaustive), and it is precisely what
W5b removes: the `BeliefEdgeDecl` registry (§3.2) gives belief-edge writes a sanctioned,
type-checked path, so a domain has **no reason to author a raw dynamic write at all** —
making the residual *unnecessary*, not merely *uncaught*. **Until W5b lands the residual
is real.** The two halves therefore *reduce* the unguarded surface to "dynamically-composed
writes in non-enumerated paths"; they do not eliminate it. The monopoly test is honestly
"complete **modulo** that residual," and the registry is what retires it.

**Rejected alternative — rely on the dynamic count-reconciliation alone.** Rejected: a
runtime count test only exercises the paths the harness happens to drive; it cannot
prove a dormant or rarely-taken code path never authors a raw belief insert. The static
lint closes that gap by forbidding the *string* from existing outside `tql.rs`. (And a
purely static lint alone is also insufficient — it cannot follow a dynamically-composed
query string; hence both halves.)

### §2.2 — Dynamic half: run every bypass-capable path, then reconcile counts

**Design question — how do we prove, at runtime, that nothing but MemWriter populated
the belief tier?**

**Decision.** A live test (throwaway TypeDB, like the Deliverable-3 rig) that, on the
composed schema, **drives every bypass-capable path from the Headline's table**, then
asserts: **for every relation/entity on the introspected belief-edge surface (§1.1), the
graph population equals what the harness's own MemWriter calls account for**, modulo the
committed allowlist (§1.2). The paths, at their **current** anchors:

| # | Bypass-capable path | Current anchor | What the test drives | Expected accounting |
|---|---|---|---|---|
| 1 | **MemWriter** (the sanctioned holder) | `core/mem-gecko/src/writer.rs:769,827` → `tql::insert_belief_ops` (`tql.rs:196`) | the harness itself mints beliefs/edges through MemWriter | every instance is accounted (this *is* the accounting baseline) |
| 2 | **Syncer / record path** | `core/gecko-engine/src/syncer/bundle.rs:131` (`sync_bundle`, direct `db.begin_write()`) | `sync` the sample bundle | **zero** belief-tier instances (already proven by `no_drag_sync_creates_records_and_zero_epistemic_apparatus`, `no_drag_test.rs:43`, and `syncer_stays_on_the_record_path`, `bundle.rs:741`) — the record-side half W5a's dynamic half generalizes |
| 3 | **cyber detect bridge** | `extensions/cyber-gecko/src/detect/mod.rs:49` (`cyber_extension_callback`); `claim` (`:90`) and `disposition` (`:153`) | a real `claim`/`disposition` call | beliefs via `writer.assert_belief` (`:119`, `:177`) **are** accounted; the linking `technique-detection`/`alert-disposition` written **raw** (`:142`, `:196`) are **UNACCOUNTED** → the assertion FAILS (the whole point — §3.3) until W5b routes them through the seam |
| 4 | **A hostile future-extension stub** | composed through `build_host_bridge` (`gecko-bin/src/main.rs:957`, wired `:1068`), handed an `Arc<dyn GraphStore>` exactly as cyber's bridge is at `main.rs:1016-1018` | a test-only stub whose host-import handler authors, on the captured graph handle, **both** a raw `insert … isa belief …` **and a raw belief-referencing _relation_** (e.g. `insert $d isa technique-detection, links (…, detection-claim: $b);` against a belief it did not mint) | **both** the raw belief **and the raw relation** are unaccounted → the reconciliation **flags each**. The **relation** injection is load-bearing: the live breach is a *relation* write, so path #4 must exercise the relation case **structurally** (from a non-cyber adversary), not lean on cyber path #3 as the only relation-case teeth |

The reconciliation is per-type over the introspected surface: `count(<type>) ==
memwriter_accounted(<type>) + allowlisted(<type>)`. Path #2 contributes zero to the
belief tier; path #1 contributes the accounted baseline; paths #3 and #4 are the
adversarial injections the assertion must catch.

**Rationale.** This is the direct inverse of the record-side monopoly test §8.3 calls
for, and it is the executable check CONFLICT-REPORT owes:

> **Verbatim (`CONFLICT-REPORT.md` Headline, "Executable check (owed, W5a)"):** "a
> committed test that, after every non-`MemWriter` code path runs, asserts the
> population of **every belief-tier type** (`belief` + the relations above) equals what
> `MemWriter` accounts for. The record-side half already exists
> (`no_drag`/`syncer_stays_on_the_record_path`); the inverse — over the full belief
> tier — does not."

Driving path #4 (the hostile stub through `build_host_bridge`) is what makes the test a
*structural* monopoly check rather than a cyber-specific one: the Headline's path #4 is
"any future extension handed the graph … structurally unrestricted." Because path #4
injects a raw belief-referencing **relation** (not only a belief entity), the
"structural, not cyber-specific" claim is exercised for the **relation** case — the
actual breach shape — by a non-cyber adversary, so the relation-case teeth no longer rest
solely on cyber path #3. A *new* extension that bypasses the seam **on a path the harness
drives** is caught by count-reconciliation, not by hoping its author read this spec. What
path #4 does **not** prove is the §2.1 residual: a dynamically-composed raw write in a
path the harness does **not** enumerate is caught by neither half — reduced by W5a,
retired by W5b, never claimed complete here.

**Rejected alternative — assert only "no `belief` entity has a non-MemWriter
provenance stamp."** Rejected: it is the entity-only test §8.4/the Headline explicitly
call not-meaningful — "it can't fail on the live cyber raw-relation breach"
(`INVARIANTS.md` §8.4, W5a). The breach is a **relation** write; an entity-only
assertion passes while it stands. The count-reconciliation must cover the belief-tier
*relations*, which is why the surface (§1.1) is relation-valued.

### §2.3 — The ratchet: P-W5-1 keeps the surface honest

**Design question — what stops the introspected surface from silently ceasing to cover
a new belief-edge relation (the failure mode a hand list has)?**

**Decision.** The surface is **re-derived by introspection on every run** (§1.1), and a
committed ratchet probe — **P-W5-1** (§4, `gecko-bin/tests/schema_probe_w2w3.rs`,
`p_w5_1_introspection_ratchet_catches_divergence_precedence`) — proves the derivation
*grows* when a new belief-edge relation lands, with **no test edit**. P-W5-1 is green on
`conflict_resolution`: on the unfolded mem schema the surface already contains cyber's
`technique-detection`/`alert-disposition` and excludes `divergence-precedence`; applying
the W2 candidate delta makes `divergence-precedence` appear. The ratchet rule (§1.2) —
introspected ∉ (accounted ∪ allowlist) fails — turns that growth into a forcing
function: a newly-landed belief-edge relation that nobody routed through the seam and
nobody allowlisted **fails W5a** until someone decides which it is.

**Rationale.** This is the live demonstration that the introspection strategy (§1.1)
does what a hand list cannot: **catch `divergence-precedence` the instant W2 lands.**
Because W5 ratifies *after* W2+W3 (the spec-production sequence), its surface must
already reflect W2's new relations — and P-W5-1 proves the mechanism does so
automatically. See §4 for the observed surfaces.

**Rejected alternative — a snapshot assertion pinning the surface to an exact expected
set.** Rejected: it is a hand list wearing an introspection costume — it would need an
edit for every new belief-edge relation and would fail *closed* (a spurious red) rather
than *forcing a decision* the day W2 lands. The ratchet asserts *membership and growth*
(cyber's two present; `divergence-precedence` gained on the delta; the surface only
grows), never an exact frozen set.

---

## §3 — W5b: the sanctioned belief-edge writer seam

W5b is the **proof case** for W5 (CONFLICT-REPORT Headline): today's discipline is
"one careless `graph.write` from a forged provenance stamp," and W5a stays red until the
cyber belief-referencing relations are routed through a sanctioned writer.

### §3.1 — Options weighed (two rejected)

**Design question — what is the sanctioned path for an *extension's* belief-edge
relation write (the write `EpistemicWriter`'s seven methods —
`observe`/`assert_belief`/`supersede`/`contest`/`rests_on`/`record_prediction`/
`resolve_prediction`, `epistemic.rs:194-225` — do not cover)?**

**Rejected alternative (i) — a generic free-form `link_belief(ctx, relation_spec)` on
`EpistemicWriter`.** A single method taking an arbitrary relation specification (label +
role→player map). **Rejected:** a free-form relation spec *is the raw seam wearing a run
stamp*. Nothing stops `relation_spec = supersession{ superseded: X, superseding: Y }`
from bypassing the OP-5 entrenchment-monotonicity guard (`writer.rs:806-824`), or
`resolves{…}` from minting an identity belief-edge without the resolution writer's
disposition, or `divergence-precedence{…}` without its co-minted anchor (SPEC-W2-W3
§2.2). It moves the type-blindness (§2.1) inside the writer — the monopoly test would
pass (the write went "through MemWriter") while every belief-tier guard silently lapsed.
It violates invariant 2's *whole point*: the sanctioned path exists so provenance and
guards **cannot** be omitted.

**Rejected alternative (ii) — domain-specific trait methods on the core API.** Add
`write_technique_detection(…)`, `write_alert_disposition(…)`, and one method per future
domain relation, onto `EpistemicWriter`. **Rejected:** it violates invariants 3 and 5.
Inv 3 (registration is the activation gate) and inv 5 (cross-cutting primitives live in
mem, domains extend by role-addition only) mean the *core substrate API must not grow a
method per domain relation* — every new domain would force a core-API change, and the
core would accrete cyber-specific (then next-domain-specific) surface. `assert_belief`
is substrate-general; `write_technique_detection` is not.

### §3.2 — Chosen shape: the mem-owned `BeliefEdgeDecl` registry

**Decision.** At **registration** (the invariant-3 gate), an extension **declares** its
belief-edge relations; `MemWriter` exposes **one** method that accepts **only registered
declarations**, authors the TQL in mem's `tql.rs`, run-stamps it, and validates the
belief player. A declaration is:

```
BeliefEdgeDecl {
    relation:        <relation label>,              // e.g. "technique-detection"
    roles:           [ RoleDecl { role, belief: bool, players: [<type label>] }, … ],
    owned_attrs:     [ AttrDecl { attr, card, /* value supplied at write */ }, … ],
}
```

- **the belief role is marked** — exactly one (or more) role carries `belief: true`;
  the writer validates that the player handed for that role is a `belief`/`memory-item`
  the *current run* is entitled to reference, and stamps run identity as `assert_belief`
  already does. `technique-detection` marks `detection-claim`; `alert-disposition` marks
  `disposition-belief` (`cyber_schema.tql:154,156` and `:166,168`).
- **allowed player types per role** — the writer refuses a player whose type is not in
  the role's declared `players` set (e.g. `detecting-logic` admits `concept`,
  `detection-claim` admits `belief`).
- **owned attributes are mandatory, not optional.** The seam **must** carry
  relation-owned attributes, or it cannot express `alert-disposition`, which
  `owns detection-verdict @card(1)` (`cyber_schema.tql:164`; written raw today at
  `detect/mod.rs:186-188` as `has detection-verdict == $v`). A link-only seam is
  therefore ruled out — it could not replace the disposition write. `AttrDecl` declares
  the attr and its cardinality; the value is supplied per write.

`BeliefEdgeDecl` does not exist in the repo today (**confirmed: zero hits for
`BeliefEdgeDecl`**, consistent with CONTRACT-W8's §1 SATISFIABILITY NOTE). Building it is
the implementation task; this section fixes its shape.

**Rationale.** The registry threads the needle the two rejected options miss: it is
**general** (no core method per domain — one method, data-driven by declarations, so inv
3/5 hold; a domain extends by *declaring*, exactly as it extends the schema by
role-addition) **and** it is **not free-form** (only *registered, reviewed*
declarations are writable, so the writer can validate the belief player, mark the
provenance, and refuse an unregistered or malformed edge — closing option (i)'s hole).
The declaration is the seam's type system where the raw `GraphStore::write` has none.

**The registry is the SAME source of truth the W5a ratchet reads.** The set of
belief-edge relations the seam will write is exactly the set the W5a monopoly test
asserts is single-writer. A relation that is **registered-and-writer-routed** is
*accounted* (§2.2); one that is introspected as a belief-edge but neither registered nor
allowlisted **fails** the ratchet (§1.2/§2.3). Registration and monopoly-coverage are
the same list — an extension cannot register a belief-edge relation it does not route
through the seam without W5a going red, and cannot route one it did not register.

**§3.2 also owns the co-mint enforcement SPEC-W2-W3 §2.2 requires.** SPEC-W2-W3 §2.2
splits the `divergence-precedence` co-mint discipline into a *structural* half (the
schema's `relates exception-belief @card(1)` bars an anchor-less relation at commit —
P-PREC-5, already green) and a *procedural* half it explicitly hands to this spec:

> **Verbatim (`SPEC-W2-W3.md` §2.2):** "**Procedural residue — owed to W5b, not
> enforceable in schema.** … the sanctioned seam must **refuse a `divergence-precedence`
> write that lacks its co-minted anchor**, and must stamp the anchor's provenance … This
> is exactly the guarantee that is structural on paper and procedurally violated in code
> (cf. the cyber raw-relation breach). **SPEC-W2-W3 §2.2 states the requirement; SPEC-W5
> §3.2 owns its enforcement** so it does not fall between the specs."

**Binding (this spec, owning the enforcement).** The seam **refuses a
`divergence-precedence` write whose anchoring `divergence-exception` belief is not
co-minted in the same sanctioned call.** Concretely: the `divergence-precedence`
`BeliefEdgeDecl` marks `exception-belief` as the belief role, and the writer method that
writes it **mints (or is handed, within the same run-stamped call) the
`divergence-exception` belief and the relation together, atomically** — never the
relation against a caller-supplied, separately-fabricated anchor. This is the procedural
half the `@card(1)` schema constraint cannot enforce (the schema bars *zero* anchors, not
a *hand-written* one); the seam supplies it. The same co-mint discipline generalizes to
any registered belief-edge relation whose belief role is a freshly-minted disposition
belief.

*(This spec does **not** re-open the divergence-exception *shape* — that is ratified in
SPEC-W2-W3 §2.1/§2.2. W5b owns only the *writer-seam enforcement* of the co-mint
obligation SPEC-W2-W3 assigned it.)*

### §3.3 — Cyber migration contract; W5b BLOCKS W5a-green

**The migration.** cyber's `claim` and `disposition` (`detect/mod.rs:90,153`) each do
two writes today: a `writer.assert_belief` (`:119`, `:177` — already sanctioned,
run-stamped) **and** a raw `graph.write` of the linking relation (`:142` for
`technique-detection`, `:196` for `alert-disposition`). **The migration drops the raw
`graph.write` for the seam:** cyber registers `BeliefEdgeDecl`s for `technique-detection`
and `alert-disposition` (marking `detection-claim`/`disposition-belief` as the belief
roles, declaring the `detection-verdict` owned attr on `alert-disposition`), and both
functions write their linking relation through the registered seam instead of raw TQL.
The `cyber_extension_callback` bridge (`detect/mod.rs:54`) holds a `GraphReader` for
the *read* paths (`coverage_gaps`/`blinded`/`precision`, `:205,220,235`) and a
`BeliefEdgeWriter` bound to cyber for its belief-tier *writes*. After migration, cyber
holds no raw belief-tier write.

**A third cyber belief-edge relation the surface caught — `sighting-of` (disposition).**
P-W5-1's 23-relation surface (§4) includes cyber's `sighting-of` (`cyber_schema.tql:143-149`;
`sighting sub execution-episode sub episode sub memory-item`, so its `sighting-instance`
role is played *exclusively* by a memory-item subtype and the §1.1 surface rule selects it).
Unlike `technique-detection`/`alert-disposition`, **`sighting-of` is defined-but-never-written**:
no code path — cyber or otherwise — inserts a `sighting` entity or a `sighting-of` relation
today (the sole Rust reference, `detect/mod.rs:166`, uses supplied alert/sighting ids as
belief *evidence* for the disposition belief, not as a `sighting-of` write). Two consequences.
**(1) Not a live breach; does NOT block W5a-green.** The dynamic count-reconciliation (§2.2)
sees `count(sighting-of) == 0 == accounted + allowlisted` and passes; only the two
*written-raw* relations gate W5a. **(2) Owed to the seam the moment sighting-ingestion is
built.** `sighting-of` is a belief-edge relation; any future episodic-sighting writer must
author it through the §3.2 `BeliefEdgeDecl` registry (its `sighting-instance` role marked
belief-tier), **never raw `graph.write`** — exactly the discipline this section imposes on
`technique-detection`/`alert-disposition`. It is recorded here as **owed-to-seam-when-written**
(owner: cyber), *not* a current migration item (there is nothing to migrate — zero instances),
so the surface-caught relation has a stated accounting rather than an
introspected-∉-(accounted ∪ allowlist) silent gap (§1.2). The surface did its job: it found
the relation the migration list omitted, and the honest answer is "latent, owed to the seam
if ever written," not "silently unaccounted."

**The acceptance constraint.** `gecko-bin/tests/cyber_detect_test.rs` must **stay
green** across the migration — the observable behavior of `claim`/`disposition` (belief +
correctly-linked `technique-detection`/`alert-disposition` with its verdict, read back
by `coverage_gaps`/`precision`) is unchanged; only the *write path* changes from raw to
sanctioned. The migration is behavior-preserving by construction.

**W5b BLOCKS W5a-green — stated in as many words.** **W5a's dynamic half (§2.2) stays
RED until W5b lands.** While cyber writes `technique-detection`/`alert-disposition` via
raw `graph.write` (`detect/mod.rs:142,196`), those relation instances are written
*outside* MemWriter's accounting, so the §2.2 count-reconciliation finds unaccounted
belief-tier instances and **fails** — correctly. W5a **cannot go green** until the cyber
breach is routed through the W5b seam and MemWriter accounts for those writes. This is
not incidental sequencing: it is CONFLICT-REPORT's ratified coupling —

> **Verbatim (`CONFLICT-REPORT.md` Headline, W5b):** "route the cyber
> belief-referencing relations through a sanctioned writer … *before* W5a can go green.
> This is the **proof case for W5**, not a nice-to-have: … W5a will (correctly) fail
> until W5b lands."

So the two work items ship together: W5b routes the breach, and only then does the W5a
monopoly test the same effort commits actually pass. A green W5a is the *proof* the
seam closed the last raw belief-tier write.

### §3.4 — Cross-references

- **CONTRACT-W8 (the consolidation daemon) — a future belief-writer the monopoly must
  bind.** CONTRACT-W8 CF-3 requires the daemon "write through `EpistemicWriter`, never
  raw `graph.write`," and its §2 write-surface inventory flags that any *new belief-tier
  relation* a daemon job needs is **owed-by-W5b** — the `BeliefEdgeDecl` seam is the
  intended generic mechanism (CONTRACT-W8 §2, last row; §1 SATISFIABILITY NOTE). This
  spec reciprocates: when implemented, the daemon **joins the W5a monopoly harness** as an
  enumerated caller, never a carve-out (CONTRACT-W8 §3 check 5,
  `daemon_is_enumerated_by_the_monopoly_test`; §4). The daemon's belief-edge writes route
  through §3.2's registry exactly as cyber's do.
- **cyber-gecko episodic-sighting writer — a future belief-edge writer owed to the seam.**
  `sighting-of` is on the introspected surface but **unwritten today** (§3.3); the moment
  cyber-gecko builds sighting ingestion, its episodic-sighting writer **must** author
  `sighting-of` through §3.2's `BeliefEdgeDecl` registry (declaring `sighting-instance` as
  the belief-tier role), **never raw `graph.write`** — and thereby **joins the W5a monopoly
  harness** as an enumerated, accounted caller, exactly as cyber's
  `technique-detection`/`alert-disposition` do post-migration (§3.3) and the daemon does
  above. Until then it stays **owed-to-seam-when-written** on the §1.2 ratchet (zero count,
  §2.2 passes). This cross-ref is the standing pointer so the obligation is **discharged
  when cyber-gecko reaches sighting ingestion, not rediscovered** — the surface caught it
  (§3.3); this line makes sure the writer is built through the seam.
- **SPEC-W2-W3 §2.2 (co-mint discipline).** The procedural co-mint residue for
  `divergence-precedence` is *enforced* here (§3.2), *stated* there. W2's surface
  additions (`divergence-exception` a `belief`, `divergence-precedence` a
  belief-referencing relation) join the W5a surface automatically via introspection
  (§1.1/§2.3, P-W5-1) — SPEC-W2-W3 §3.6 already anticipates this ("both must be added to
  W5a's asserted population, or the monopoly test checks the wrong shape").
- **SPEC-W2-W3 §3.5 (populations-writer / allowlist).** `population-member` and its window
  attrs are the committed allowlist entries §1.2 records, owned by the populations writer,
  "never raw `graph.write`."

### §3.5 — Amendments decided after ratification (D-B, D-D, pre-GC check)

> **Status: decided by the user on 2026-11-19** (ledger D-B and D-D, from
> `REVIEW-2026-11.md` §3 points 1, 2 and 5). These amend §3.2 and §3.3; the ratified text above is
> left as it stood at `95f0d6e`, and where the two disagree this section governs. Ratifying the
> amendment is a separate act for the user.

**D-B — the registry accepts only extension-owned relations.** §3.2 protects the registry
procedurally ("registered, reviewed"). That leaves the free-form hole of §3.1 (i) open one step
removed: an extension could declare `supersession` and then write it past the entrenchment guard.
The rule is therefore structural:

- A `BeliefEdgeDecl` is accepted only for a relation that the **declaring extension's own schema
  fragment introduces**. A relation that core, mem or another extension defines, or one the
  declaring fragment only extends by role-addition (invariant 5), is refused.
- The check runs when the assembler builds the registry. It reads the fragments' text, so it is
  testable without a live TypeDB. Before first use, the seam also validates every declared label
  against a strict identifier pattern and against the live schema, because labels are
  interpolated (they cannot be `given` parameters) and are the seam's one exception to "all TQL
  is parameterized".
- **Mem-owned belief edges are not registry clients.** `supersession`, `resolves`, `scoped` and
  `divergence-precedence` each carry their own guard (entrenchment, disposition, scope, co-mint),
  so each is written by a dedicated `MemWriter` method. This supersedes §3.2's description of a
  `divergence-precedence` `BeliefEdgeDecl`. The co-mint obligation of SPEC-W2-W3 §2.2 is
  unchanged and is enforced by that dedicated method. The consolidation daemon (§3.4) writes
  mem-owned edges through the same dedicated methods.

**D-D — extensions get no write-capable graph handle.** §3.3 keeps an `Arc<dyn GraphStore>` in
the cyber bridge for reads, which is also a raw write path, so the §2.1/§2.2 residual is only
detected. The decision is to retire it structurally, in three steps:

1. **Read-only handle at run time.** `gecko-extension-api` splits a read-only `GraphReader`
   (`read`) out of `GraphStore`, which keeps `write` and extends `GraphReader`. Extension bridges
   are built with `Arc<dyn GraphReader>`; only mem holds a `GraphStore`. Lands with the cyber
   migration (§3.3).
2. **Neutral writes at sync time.** An extension's post-sync hook returns `GraphWrite`s for the
   engine to run in its sync transaction, instead of receiving a driver transaction. Lands with
   step 1.
3. **Extensions depend only on `gecko-extension-api`.** The types extensions use from
   `gecko-engine` move into the API crate: the sandbox `ExtensionCallback` and the OKF bundle
   types used by import. A CI guard (the same shape as the `candle` guard) fails the build if
   any crate under `extensions/` depends on `typedb-driver` or `gecko-engine`. Lands before a
   second extension is started.

**Limit.** Extensions are compiled-in, trusted code. These steps prevent an author from reaching
a raw write path by accident; they do not defend against a deliberately hostile extension. W5a
(§2) remains the backstop for every belief-tier write, mem's own included.

**Pre-GC check — ratified by the user on 2026-11-20.** A sync refuses to garbage-collect a concept
that plays a role in a belief-tier relation, with `DbError::BeliefEdgeReferencesRemovedConcept`
naming each concept ID and the relation that references it, and nothing commits. The fingerprint
of §2 stays the backstop. The recovery is to restore the removed record or to sync into a fresh
database; retiring such a record instead of refusing is ledger item O6.

---

## §4 — Probe evidence (P-W5-1)

**P-W5-1** is green on `conflict_resolution`
(`gecko-bin/tests/schema_probe_w2w3.rs::p_w5_1_introspection_ratchet_catches_divergence_precedence`;
full detail in `docs/refactor/probes/W2-W3-PROBE-LOG.md`, "W5 batch"). Run in ISOLATION —
the folded-schema probes in that file expect the Mode-B fold and fail on this branch's
unfolded mem schema; only P-W5-1 goes green here:

```text
cargo test --test schema_probe_w2w3 --no-default-features --features cyber p_w5_1 \
    -- --ignored --test-threads=1
```

| Probe | Question | Expected | Observed | Verdict |
|---|---|---|---|---|
| **P-W5-1** | Does an introspection-derived belief-edge surface catch cyber's relations without a hand list, and gain `divergence-precedence` the instant the W2 candidate schema lands? | surface includes `technique-detection`/`alert-disposition` always; excludes `divergence-precedence` until the W2 delta, includes it after | before: **23** relations incl. both cyber relations + `resolves`/`scoped`, NO `divergence-precedence`; after the delta: **24**, `divergence-precedence` added; surface only grows | **CONFIRMED** |

**Observed surface (core + unfolded-mem + cyber, no W2), 23 relations:**
`{alert-disposition, attack, contextualizes, contradiction, derivation, evidence,
execution, goal-hierarchy, informs-synthesis, justification, ownership, pivot,
prediction-resolution, resolves, rests-on, scoped, sighting-of, source-link,
supersession, support, surfaced, technique-detection, tool-invocation}`.

**After `apply_delta(w2_precedence.tql)`, 24 relations:** the same set **+
`divergence-precedence`**.

The observed set is **broader than INVARIANTS §8.4's six-relation enumeration** — direct
live evidence for the §1.1 delta (the surface catches `resolves`/`scoped` and the
substrate/episodic edges §8.4 omits) and for the §2.3 ratchet (the surface gains
`divergence-precedence` with no test edit). This is the derivation the W5a monopoly test
asserts over (§2.2) and the same source of truth the W5b registry populates (§3.2).

---

## §5 — Executable checks owed (test names, paths)

None run today beyond P-W5-1 (the surface-derivation ratchet); the monopoly test and the
seam are owed to implementation. Each is named so the implementation cannot land without
its check.

| # | Half | Test (owed, name reserved) | Path | What it must assert |
|---|---|---|---|---|
| 1 | W5a static | `no_raw_belief_tier_tql_outside_mem_tql` | beside `gecko-bin/tests/no_drag_test.rs` | every `insert`/`put`/`update` of an introspected belief-edge type in workspace sources resolves to `core/mem-gecko/src/tql.rs` |
| 2 | W5a dynamic | `memwriter_is_the_sole_belief_tier_writer` | a Deliverable-3-style live rig | after paths #1–#4 (§2.2) run, `count(<type>) == memwriter_accounted(<type>) + allowlisted(<type>)` for every type on the introspected surface |
| 3 | W5a dynamic | `hostile_extension_raw_belief_write_is_flagged` | same rig | a stub extension composed through `build_host_bridge` (`main.rs:957/:1068`) that authors **both** a raw `insert … isa belief …` **and a raw belief-referencing relation** (e.g. `technique-detection` against a belief it did not mint) is caught by count-reconciliation — the **relation** case (the actual breach shape) tested structurally by a non-cyber adversary |
| 4 | W5a ratchet | **P-W5-1** (`p_w5_1_introspection_ratchet_catches_divergence_precedence`) | `gecko-bin/tests/schema_probe_w2w3.rs` | **SHIPPED-GREEN** — introspected surface gains `divergence-precedence` on the W2 delta, no test edit |
| 5 | W5b seam | `belief_edge_decl_registry_writes_run_stamped` | mem-gecko + a test extension | a registered `BeliefEdgeDecl` writes the relation run-stamped, with owned attrs; an unregistered/malformed edge is refused |
| 6 | W5b seam | `divergence_precedence_write_requires_cominted_anchor` | mem-gecko | the seam refuses a `divergence-precedence` write whose `divergence-exception` anchor is not co-minted in the same call (SPEC-W2-W3 §2.2) |
| 7 | W5b migration | `cyber_detect_test` (unchanged) | `gecko-bin/tests/cyber_detect_test.rs` | **stays green** across the migration from raw `graph.write` (`detect/mod.rs:142,196`) to the seam |
| 8 | W5b→W5a | `memwriter_is_the_sole_belief_tier_writer` (row 2) turns GREEN | — | after row 7's migration lands, the §2.2 reconciliation finds zero unaccounted belief-tier instances — the W5b-blocks-W5a proof (§3.3) discharged |

> **Implementation note (2026-11-19, not ratified text) — row 5.** `belief_edge_decl_registry_writes_run_stamped`
> ships in `core/mem-gecko/tests/belief_edge_integration.rs`, against a test-only extension
> fixture shaped like cyber. The refusal half is covered beside it (`missing_player_commits_no_belief`,
> `value_outside_values_commits_no_belief`, `non_unique_key_commits_no_belief`,
> `live_schema_mismatch_is_refused_before_writing`) and by the registry unit tests in
> `core/mem-gecko/src/belief_edge/registry.rs`. Tracked as ledger item A1.
>
> **Implementation note (2026-11-19, not ratified text) — row 1.** `no_raw_belief_tier_tql_outside_mem_tql`
> ships in `gecko-bin/tests/belief_tier_monopoly_test.rs` (`cyber` feature, live TypeDB). The
> surface is introspected from the composed core→mem→cyber schema: the `memory-item` subtype
> closure plus the §1.1 belief-edge relations. The scope is production sources — `.rs` under
> each `core/*`, `extensions/*` and `gecko-bin` crate's `src/`, with inline `#[cfg(test)]`
> modules blanked, and `.tql` under `core/`, `extensions/` and `gecko-bin/`, outside `tests`
> and `target`. A hit is a unit (a Rust string literal, or a `.tql` file with comments and
> quoted strings removed) holding a write verb (`insert`/`put`/`update`/`delete`) and a surface
> label after `isa` or directly before `(`, which also catches anonymous-relation inserts. The
> positive control asserts that `core/mem-gecko/src/tql.rs` shows `belief` and `supersession`
> hits; every other file must show none. Known limit: the unit is a single string literal, so a write assembled across `concat!`/`format!` pieces is not seen. Tracked as ledger item A3.
>
> **Implementation note (2026-11-20, not ratified text) — rows 2, 3 and 8.**
>
> - **Row 2** ships in `gecko-bin/tests/belief_tier_reconciliation_test.rs` (`cyber`, live TypeDB). The
>   production database is driven only through the `gecko` binary. The expectation is a reference
>   database driven by in-process `MemWriter` and `edge_writer_for("cyber-gecko")` calls, which keeps
>   §2.2 row 1 literally: the harness's own MemWriter calls are the accounting. The measure is the
>   production fingerprint: per label, instance, attribute-ownership and role-player counts. On
>   TypeDB 3.15 a direct `has $a` count and a per-label `owns` read both error on a label that owns
>   nothing (INF11/INF6), so the attribute measure reads the schema's owner set once and counts `has`
>   only for labels in it. The §1.2 disposition table has two classes, *writer-only* and *owed*; an
>   *allowlisted* class was dropped because it had no entries.
>   Labels with no other ledger owner are owned by Track M (M1–M6). The mem path derives b2 before the recall, because
>   recall surfaces beliefs only and would otherwise return no chunks.
> - **Row 3 deviation.** Since D-D step 1 a bridge holds only `Arc<dyn GraphReader>`, so a stub composed
>   through `build_host_bridge` cannot write. The remaining raw route is sync-time output
>   (`extension_writes`, an importer's `type_hint`), which the test drives, through the guard and around
>   it. This applies D-D and needs no separate ratification.
> - **Row 8** is discharged: the reconciliation is green with cyber on the seam.
> - **Detection became prevention.** `sync_bundle` refuses a belief-tier change in production
>   (`DbError::BeliefTierWrite`). The residual is a same-count swap, which the fingerprint cannot see.
>   Each fingerprint is at most three grouped reads, one per measure. Measured guard time with mem and
>   cyber composed (debug build, near-empty databases, 23 syncs): a median of about 4.3 ms per sync.
> - **The pre-GC check** ships as `DbError::BeliefEdgeReferencesRemovedConcept`; it is ratified in §3.5.
>
> Tracked as ledger items A4 and A10.

---

## §6 — Ratification record

Ratification is the user's act (GLOBAL-CONSTRAINTS: "a spec is at most RATIFIABLE/DRAFT;
only the user flips a status blockquote to RATIFIED at commit `<sha>`"). This spec is
**RATIFIED as design** — it was sequenced to ratify **after** W2+W3 so its surface reflects
their new relations (P-W5-1 proves it does), and the user performed the act at SPEC-W5's own
evidence commit.

- **RATIFIED at commit:** `95f0d6e` (full `95f0d6e14bc91af3e2bdc755236f680c6c2e6c95`) — the
  commit where **P-W5-1 is green** and the introspected surface (**23 relations, 24 after
  the W2 delta**) was observed and logged, i.e. SPEC-W5's own evidence state. **This is a
  different anchor than the W2-W3 ratifications' `11d7ab1`**, by design: each ratification
  anchors to the evidence it was made against, and bundling them would muddy which evidence
  state each was ratified against. Ratified **as design** (W5a test-design + W5b seam-design
  + the W5b-blocks-W5a coupling); **§5's executable checks are owed-to-implementation** —
  only P-W5-1 (§5 row 4) is shipped-green, the rest (rows 1–3, 5–8) are named-but-owed.
- **Probe evidence:** P-W5-1 green (`docs/refactor/probes/W2-W3-PROBE-LOG.md`, "W5 batch";
  `gecko-bin/tests/schema_probe_w2w3.rs`), run in isolation on `conflict_resolution`.
- **Gates discharged (at ratification):**
  - INVARIANTS §8.3 (verbatim-quoted §2) — the monopoly test is committed as the primary
    disposition; the "bypass list non-empty → revisit schema guards" trigger is recorded
    as fired, schema-hardening left owed (not designed here).
  - INVARIANTS §8.4 W5a (verbatim-quoted §1.1) — the enumeration is refined to the
    introspection-derived surface; §8.4's incompleteness is recorded, the locked doc
    unedited.
  - INVARIANTS §8.4 W5b (verbatim-quoted §3.3) — the sanctioned-writer seam is designed;
    the cyber migration contract and the W5b-blocks-W5a coupling are stated in as many
    words.
  - CONFLICT-REPORT Headline (verbatim-quoted §1.1/§2.1/§2.2/§3.3) — the type-blind seam,
    the bypass-path table (at current anchors), the cyber breach, and the owed executable
    check are all addressed.
- **Cross-refs confirmed:** CONTRACT-W8 §1/§2/§3 (the daemon joins the harness),
  SPEC-W2-W3 §2.2 (co-mint enforcement owned here), §3.5/§3.6 (populations-writer
  allowlist; the W2 surface additions).

### Ratifiability self-check (recorded per the closure discipline)

- **Every design question → decision + rationale + rejected alternative (or N/A):** §1.1
  (introspection-derived — hand-list rejected), §1.2 (*allowlist obligation, no fork*),
  §2.1 (source lint — dynamic-only rejected), §2.2 (count-reconciliation over the surface
  — entity-only rejected), §2.3 (ratchet — snapshot-set rejected), §3.1/§3.2
  (`BeliefEdgeDecl` registry — free-form `link_belief` and per-domain core methods both
  rejected), §3.2 co-mint (*obligation owned from SPEC-W2-W3, no fork*). ✅
- **Every §8 refinement is verbatim-quote + delta:** §1.1 (§8.4 W5a verbatim + the
  incompleteness delta), §2 (§8.3 verbatim + the surface/two-halves delta), §3.2/§3.3
  (SPEC-W2-W3 §2.2 and CONFLICT-REPORT Headline W5a/W5b verbatim). No §8 text is edited or
  silently reinterpreted. ✅
- **The one shipped artifact (P-W5-1) is green and cited** (§4); the rest are owed
  executable checks named in §5, not claimed as done. ✅
- **Completeness claims are honest, not overstated:** §2.1/§2.2 state the **residual** —
  the two monopoly halves cover (literal writes anywhere) ∪ (any write in the four
  enumerated paths), *not* (any write anywhere); a dynamically-composed raw write in a
  non-enumerated path is reduced by W5a and **retired by W5b** (the registry), never
  claimed caught. Path #4 injects a raw belief-edge **relation** so the relation-case teeth
  are structural, not cyber-only. The surface-caught `sighting-of` is dispositioned
  (owed-to-seam-when-written, §3.3/§1.2), not left silently unaccounted. ✅
- **RATIFIED as design at `95f0d6e`** (SPEC-W5's own evidence commit, ≠ W2-W3's `11d7ab1`);
  §5's executable checks are owed-to-implementation, only P-W5-1 is shipped-green. ✅

_This document is **RATIFIED as design** at `95f0d6e` (§6); §5's executable checks remain
owed to implementation. Nothing here edits a LOCKED document or green-lights a construct
INVARIANTS/CONFLICT-REPORT forbids: the
introspection-derived surface only *widens* coverage over §8.4's seed, the seam is
strictly more restrictive than the raw `graph.write` it replaces, and W5a is designed to
stay red until W5b closes the last raw belief-tier write._
