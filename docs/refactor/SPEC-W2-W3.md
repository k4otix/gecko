# SPEC-W2-W3 — divergence-exception mechanism (W2) + as-of-t populations/canonical & cohort-belief blast radius (W3)

> Status: **RATIFIABLE** — every [P-*] in §4 is green (Mode A for all; Mode B for every shipped
> construct, via P-COMP), and the four probe-gated decisions (§3.1, §3.4, §1.4, §3.5) are closed.
> Ratification is the user's act — §6 is left for their `RATIFIED at commit <sha>` line.
> Gates: INVARIANTS.md §8.2 (ratified semantics), §8.4 rows W2/W3; resolves CONFLICT-REPORT T2/T4/T5.

This is one spec for two work items because they share a schema surface and a data
dependency, not by convenience. W2 (`INVARIANTS.md` §8.4) builds the divergence-exception
mechanism owed by §8.2; W3 (§8.4) decides and implements as-of-t populations/canonical
plus the cohort-belief blast-radius traversal owed jointly by T2 and T5. W2's disposition
ranking consumes the `valid-from` windows W3 defines on membership, so the two ratify
together on one probe gate — separating them would let one land against a membership
model the other has not yet fixed.

**This spec is CLOSED and RATIFIABLE.** Every design question carries its decision, rationale,
and rejected alternative (or, where a question is an obligation/definition with no either/or
fork, that is stated explicitly); every candidate construct is embedded verbatim; the `[P-*]`
evidence slots in §4 are filled from the probe log
(`docs/refactor/probes/W2-W3-PROBE-LOG.md`); and every probe-gated decision is settled — §3.1
closes to **(A) as-of-t variants**, §3.4 to **b1** (widened `okf-concept` param), §1.4 to
**`[FUN5]`-immutable functions ⇒ a `redefine` migration path**, and §3.5's writer obligations
are written as contract clauses. A green probe proves *expressibility*, never *conformance to
§8.2* (risk R9): each construct was checked against the ratified text below **before** it earned
a probe. Ratification is the user's act; this document does not self-ratify — §6 is left for the
user's `RATIFIED at commit <sha>` line.

---

## §0 — Scope & non-goals

**In scope.** The *schema surface* (new entity/relation/role types, modified owns-clauses)
and the *persisted-function surface* (new TQL functions, one `redefine` of an existing
function) that W2 and W3 require. Concretely: the belief-anchored divergence-exception
shape (C1), the entrenchment-ordering predicate (C2, §2.3 — closed to scaffolding, not
shipped), the cohort-belief blast-radius extension (C3, §3.4 — closed to b1), and the
`population-member` bitemporal window plus its as-of function variants (C4).

**Out of scope (consumers of this spec, not built here).**
- **The resolution/divergence-exception writer** — the Rust `MemWriter` seam that mints a
  `divergence-exception` belief *together with* its `divergence-precedence` relation. This
  spec states the *co-mint-or-reject* obligation the writer owes (§2.2); enforcing it is
  **SPEC-W5 §3.2** (writer contract) and W5b (route through a sanctioned writer).
- **The populations writer** — whatever computes/invalidates `population-member` rows and
  maintains the window. This spec states the writer obligations it creates (§3.5) as
  contract clauses; no writer is built here.
- **The host-side disposition engine** — the four-key ranking `(derivation-method,
  confidence, entrenchment, valid-from) → resolved(head, confidence, reason) | contested`.
  §8.2 item 2 calls for a "disposition/resolution function"; this spec establishes that its
  *ranking* half is host-assembled Rust (R2), not a single persisted TQL function (§2.3),
  and that the persisted surface is at most boolean *predicates* the host composes.

Non-goals also: the consolidation daemon (W8), the MemWriter-monopoly test (W5a), and any
cyber-domain construct. Cross-refs to those work items are §3.6.

---

## §1 — Shared schema surface

Both work items mutate the same four substrate types — `belief`, `scoped`, `population`,
`population-member` — and W2's disposition ranking reads the `valid-from`/`valid-to`
windows W3 adds to `population-member`. This section is the union of everything the two
candidates touch; §2 and §3 give the per-work-item design.

### §1.1 — New-types inventory (name · kind · draft `@doc`)

Every row is a *new* type introduced by a candidate; its authoritative `@doc` is the
one embedded verbatim in §2/§3. Draft `@doc` strings restated here for the inventory —
each is one greenfield sentence, no plan/epic tags, no changelog phrasing.

| Name | Kind | Introduced by | Draft `@doc` | Status |
|---|---|---|---|---|
| `divergence-exception` | entity (`sub belief`) | C1 / §2.1 | A reified disposition belief recording that one belief takes precedence over another for a scoped member, carrying the ranking evidence as provenance. | Shipped — folded (P-COMP green) |
| `divergence-precedence` | relation | C1 / §2.2 | Anchors a divergence-exception belief to the beliefs it disposes between and the member concept the exception is scoped to. | Shipped — folded (P-COMP green) |
| `divergence-precedence:exception-belief` | role | C1 / §2.2 | The disposition belief carrying provenance and retractability for this precedence. | Shipped — folded |
| `divergence-precedence:prevailing` | role | C1 / §2.2 | The belief the disposition resolved toward. | Shipped — folded |
| `divergence-precedence:overridden` | role | C1 / §2.2 | The belief overridden for the scoped member. | Shipped — folded |
| `divergence-precedence:exception-scope` | role | C1 / §2.2 | The member concept the exception is scoped to. | Shipped — folded |
| `out-entrenches` | function (`-> boolean`) | C2 / §2.3 | True if `$a`'s entrenchment tier strictly out-ranks `$b`'s. | Scaffolding — NOT shipped (§2.3; no persisted consumer) |
| `cohort-blast-radius` | function (`-> { belief }`) | C3-b2 / §3.4 | The population-scoped beliefs undermined when this member's cohort standing changes. | Rejected alternative — NOT shipped (b1 is the §3.4 pick) |
| `population-members-at` | function (`-> { concept }`) | C4 / §3.3 | The members of a population as of time `$t`, read from the windowed materialized membership. | Shipped — folded (P-COMP green) |
| `canonical-entity-at` | function (`-> { concept }`) | C4 / §3.3 | The source records resolved to the same entity as `$rec` as of time `$t`, following only provable resolutions valid at `$t`. | Shipped — folded (P-COMP green) |

`valid-from`/`valid-to` are **not** new attributes — they already exist
(`mem_types.tql:19-20`, owned by `belief`). W3 adds them to `population-member` via an
owns-clause (§3.2); the inventory above lists only genuinely new types.

### §1.2 — Modified types & functions

- **`population-member` gains a bitemporal window** — `owns valid-from @card(0..1), owns
  valid-to @card(0..1)` (C4, §3.2), *alongside* its existing `computed-at` (kept — §3.2
  explains why two markers, not one).
- **`concept` (already `plays population-member:member`, `mem_types.tql:298`)** is the
  member player the new window scopes; no change to its role set.
- **`blast-radius` is mutated by `redefine`** — the §3.4 decision closed to **C3-b1** (param
  widened `memory-item` → `okf-concept`, plus a `population-member`→`scoped` branch; b2/b3 are
  the rejected/alternative shapes). b1 is a `redefine` of an existing persisted function — the
  first-ever such mutation in this schema, which is why it carries its own migration probe
  (§1.4, P-REDEF): a persisted function is `[FUN5]`-immutable under a bare `define`.
- **`belief` gains `plays divergence-precedence:prevailing` and `:overridden`**, and its
  subtype `divergence-exception` gains `plays divergence-precedence:exception-belief`
  (C1, §2.2). `concept` gains `plays divergence-precedence:exception-scope`.

### §1.3 — Composition constraints (strip-define assembly; P-COMP)

The shipped schema is applied as sequential transactions
(`core_schema.tql` → `mem_types.tql` → `mem_functions.tql` → `cyber_schema.tql` +
`cyber_functions.tql`), each a single `define`. Two constraints bind the candidates:

- **Strip-define assembly.** A candidate fragment that is *added to* an existing `define`
  block ships as body text with its own `define`/`redefine` header stripped at assembly, or
  as a standalone transaction. C4's `population-member owns …` and C1's new types are
  additive `define` bodies; C3-b1 is a standalone `redefine` transaction (a `redefine` and a
  `define` cannot share one block).
- **Cyber concat stays define-clean → P-COMP.** `cyber_functions.tql` has **no `define` of
  its own** — it is concatenated under `cyber_schema.tql`'s `define` (D3 mechanics note,
  CONFLICT-REPORT Appendix). Any new mem function these candidates add must not break that
  concatenation: the composed four-transaction apply must remain zero-define-error with the
  candidates present. **P-COMP** probes exactly this — the full candidate stack applied
  before cyber, then cyber applied, define-clean. **P-COMP is GREEN** (§4): core→folded-mem→cyber
  composes with zero define errors and the shipped shape delivers all three acceptance behaviors
  with no delta.

### §1.4 — Migration posture (first-ever persisted-function mutation → P-REDEF) — CLOSED

C3-b1 is a `redefine fun blast-radius(…)` — the first time this schema mutates an
*already-persisted* function rather than only adding new ones.

**Decision (P-REDEF, Mode B green — closes risk R1).** A persisted function is
**`[FUN5]`-immutable under the production `define` path** (`apply_all_schemas` → `apply_schema`
→ one `define` schema transaction). Re-applying a function definition on a DB where that
function already exists is **refused outright — for a CHANGED body and for a byte-identical one
alike** — with `[FUN5] A function with name '…' already exists`; the pre-existing body is left
untouched (no silent accept-and-replace, no old-body-kept-`Ok`). Consequences:

- **Types/owns/relations stay `define`-idempotent** — re-`define` of an identical entity /
  `owns` / relation is a no-op (P-PREC-1..5 and P-PM-1/2 re-apply C1 and the window as deltas on
  the folded schema and pass). Only *functions* are non-idempotent under `define`.
- **The first mutation of a shipped function needs `redefine fun` or a fresh DB.** W3's b1
  `blast-radius` body-change is exactly such a first-ever mutation; `redefine fun` is the proven
  working mechanism (P-BR-5: body replaced and in force). `apply_all_schemas` as written would
  `[FUN5]` on re-init of an existing DB once a shipped fn's body changes.
- **Owed migration mechanism (to the implementation task, not built here):** a `redefine`-emitting
  apply path (or a documented drop-and-recreate) so the b1 `blast-radius` body-change lands on an
  already-initialized database. This is the closure of risk R1; §3.5's populations-writer clauses
  and §3.6's W8 cross-ref inherit it.

**Rejected alternative — fall back to a companion function (C3-b2) to sidestep migration.**
Rejected: the §3.4 decision is b1 (the sibling-narrowing risk did not fire — P-BR-1/2), so the
migration is faced, not sidestepped; b2 is the rejected §3.4 alternative, not a migration escape
hatch.

---

## §2 — W2: divergence-exception mechanism (§8.2's three owed constructs)

`INVARIANTS.md` §8.2 owes three constructs "**BEFORE the resolution writer**": (1) a
precedence relation, (2) a disposition/resolution function, (3) reify divergence-exceptions
as beliefs. §2.1–§2.3 supply each; §2.4 checks the interaction with the machinery already
shipped.

### §2.1 — Reify the exception as a belief (§8.2 item 3)

**Design question — stored-vs-computed. DECIDED before candidates were written (blocking).**

**Decision.** The divergence disposition is **computed**, and its human-override /
precedence outcome is **recorded as a first-class belief** — a `divergence-exception sub
belief` (C1) that *anchors* a `divergence-precedence` relation (§2.2). The winner is
*derived from evidence* by the disposition engine; the precedence relation is minted only
*with* its anchoring exception belief, never as a standalone writable edge.

**§8.2 fidelity — verbatim-quote + delta (risk R9).** The ratified text is quoted; the
delta is the operative clarification, stated, never a silent reinterpretation.

> **§8.2 (verbatim):** "When an instance belief and a population belief disagree about an
> entity, the divergence-exception writer **consumes the competing beliefs'
> `derivation-method` + `confidence` + `entrenchment` + `valid-from`** to compute a
> **disposition** — it does not merely record that a conflict exists, and it **never
> silently discards a side**."

> **§8.2 (verbatim):** "**A human asserts precedence** → a first-class **higher-entrenchment
> precedence belief**, itself attributed and retractable. **This is what "override" *is***
> — the confidence-≈-1.0 / human-precedence end of the spectrum, reached deliberately,
> still leaving the loser superseded rather than erased."

> **§8.2 (verbatim):** "Auto-*ranking* by evidence is good; auto-*deletion* of the loser is
> the `owl:sameAs` lossy-collapse invariant 9 rejects — forbidden here for the same reason."

**Delta (operative clarification).** Because the disposition is an *evidence-ranked computed*
outcome and its human end is *a belief* (attributed, retractable), **a bare writable
`prevailing/overridden` precedence edge is FORBIDDEN by §8.2.** A standalone edge that any
holder of the write seam could assert would re-introduce the silent-override pole
(`(1):185` "instance overrides population") that §8.2 explicitly ruled *not canonical
alone*, and it would make the disposition function decorative — the winner would be *asserted
by hand*, not *derived from `(derivation-method, confidence, entrenchment, valid-from)`*. The
shape that satisfies §8.2 **items 1 and 3 together** is therefore the reified belief that
anchors the relation: item 3 (reify as belief) is the `divergence-exception sub belief`; item
1 (the precedence relation) is `divergence-precedence`, which *cannot exist without its
anchor* (§2.2, `@card(1)`). This is a refinement of §8.2's wording, not a change of verdict —
§8.2 already said the override end is "a first-class higher-entrenchment precedence belief";
the delta only states, in schema terms, that the *relation* half must be belief-anchored so
the bare-edge form is structurally impossible.

**Why a belief and not a view (closes T4 problem 3).** A query-time view (`canonical-entity`,
`population-members`) is a `{ concept }` stream, and `contradiction:claim`'s player set is
`memory-item` (`mem_types.tql:148-150`), which type-excludes `concept` ([INF11],
CONFLICT-REPORT T4/D3). So a view can **never** be a contradiction claim. The reified
`divergence-exception sub belief` *inherits* `belief plays contradiction:claim`
(`mem_types.tql:102`) — it **can** be a contradiction claim exactly where a view cannot,
making a view-vs-instance conflict representable in the contradiction machinery for the first
time. This is §8.2 item 3's whole point ("The exception must be a belief to participate in
the contradiction machinery at all").

**Rationale.** The reified-belief shape reuses shipped machinery rather than adding a
subsystem (§8.2's "tell it's the right model"): `belief-state` gives the exception a
lifecycle; supersession retracts it with lineage; `derivation`+`confidence` carry the ranking
evidence as provenance; entrenchment tiers its authority. Nothing new is needed to retract,
audit, or contradict an exception.

**Rejected alternative — bare `prevailing/overridden` relation.** A two-role
`relation divergence-precedence, relates prevailing, relates overridden;` with no anchoring
belief. Rejected because it is *forbidden by §8.2* (delta above): it is silently assertable,
carries no provenance/attribution/retractability, cannot be a contradiction claim, and makes
the disposition function decorative. It is the exact "silent-override pole" §8.2 rules out.

**C1 — `w2_precedence.tql`** (belief-anchored `resolution`/`resolves` pattern; a bare
`prevailing/overridden` edge is forbidden by §8.2):

```typeql
define
  entity divergence-exception sub belief @doc("A reified disposition belief recording that one belief takes precedence over another for a scoped member, carrying the ranking evidence as provenance.");
  relation divergence-precedence @doc("Anchors a divergence-exception belief to the beliefs it disposes between and the member concept the exception is scoped to."),
    relates exception-belief @card(1) @doc("The disposition belief carrying provenance and retractability for this precedence."),
    relates prevailing @doc("The belief the disposition resolved toward."),
    relates overridden @doc("The belief overridden for the scoped member."),
    relates exception-scope @card(0..1) @doc("The member concept the exception is scoped to.");
  divergence-exception plays divergence-precedence:exception-belief;
  belief plays divergence-precedence:prevailing;
  belief plays divergence-precedence:overridden;
  concept plays divergence-precedence:exception-scope;
```

### §2.2 — The precedence relation (§8.2 item 1)

The relation is `divergence-precedence` (C1). §8.2 item 1 owes, verbatim, "**A precedence
relation** so an instance belief can be marked as taking priority over a population belief
*with provenance* (the reification the schema currently lacks — `scoped` offers no override
construct; this absence *is* why T4 is a conflict)." The relation *survives* — it exists as C1
— but it is minted only *with* its anchoring exception belief.

**Design question — reconcile §8.2's "the loser is superseded" with member-scoping (risk R3).**

**Decision.** Population-vs-instance divergence produces a **member-scoped exception**:
`divergence-precedence` carries `exception-scope` (`@card(0..1)`, the member `concept`) and
leaves the **population belief `"asserted"` for every other member**. Loser-*supersession*
(the §8.2 "the loser is superseded" clause) applies to **instance-vs-instance** divergence,
where whole-belief replacement is correct.

**Rationale.** `supersession` (`mem_types.tql:153-157`) is *whole-belief* replacement — it
has **no member/scope role** (CONFLICT-REPORT T4/D3: "`supersession` is whole-belief
replacement with no member/scope role"). Superseding a population belief because it lost to
one instance would wrongly retract it for the whole cohort. The recorded-exception belief
carries the member scope *on the exception*, not on the population belief, so the population
belief stays asserted for the members it still governs — exactly what §8.2's "never silently
discards a side" requires at cohort granularity.

**Rejected alternative — supersede the population belief on any instance win.** Rejected: it
discards the population belief for members that never diverged, violating §8.2's
no-silent-discard rule and destroying the cohort answer for the majority to record one
member's exception.

**Design question — co-mint-or-reject discipline (cross-ref SPEC-W5 §3.2).** *No alternative
fork — this is an obligation the writer owes, not an either/or choice; the ratifiability gate's
"rejected alternative" requirement is satisfied by that framing (there is no rival shape to
reject, only the requirement to enforce).*

The shape's safety depends on the exception belief and its `divergence-precedence` relation
being **co-minted, never separately** — a relation without its anchor is the bare-edge
override again. This splits into two halves:

- **Structural half — probed here.** `relates exception-belief @card(1)` (C1) means an
  anchor-less `divergence-precedence` **fails at commit**. **P-PREC-5** probes this: insert a
  `divergence-precedence` with no `exception-belief` and assert the write is rejected. A
  green P-PREC-5 is a *fail-as-designed* verdict (the insert must fail), recorded as such.
- **Procedural residue — owed to W5b, not enforceable in schema.** "The anchor was minted by
  the disposition engine (or a human), not fabricated as a hand-written belief" is a
  **writer-contract property**: the sanctioned seam must **refuse a `divergence-precedence`
  write that lacks its co-minted anchor**, and must stamp the anchor's provenance
  (tool/llm-class entrenchment with a recorded reason for engine-minted; human-assertion
  entrenchment for human-minted). This is exactly the guarantee that is structural on paper
  and procedurally violated in code (cf. the cyber raw-relation breach, CONFLICT-REPORT
  Headline). **SPEC-W2-W3 §2.2 states the requirement; SPEC-W5 §3.2 owns its enforcement**
  so it does not fall between the specs.

Retracting a precedence is a **supersession of the exception belief** (not a delete of the
relation) — consistent with §8.2's "retained with lineage."

### §2.3 — The disposition function (persisted-vs-host split)

**Design question — is the four-key ranker a persisted TQL function? (risk R2)**

**Decision.** **No.** §8.2 item 2's "disposition/resolution function that ranks competing
beliefs by (`derivation-method`, `confidence`, `entrenchment`, `valid-from`) and returns
`resolved(head, confidence, reason)` or `contested`" is a **host-assembled ranking** in the
Rust reader/writer. The persisted TQL surface is at most boolean *predicates* the host
composes — not a single function returning a `resolved(head, confidence, reason)` tuple.

**Rationale.** The four-key ranking mixes a categorical evidence ordering
(`derivation-method` provable > attested > probabilistic), a numeric `confidence`, a
categorical `entrenchment` tier order, and a `valid-from` recency tiebreak into a *composite
lexical sort* that emits a `reason` string. TQL persisted functions return a covering type
and a single stream/scalar; assembling a per-pair `reason` and a lexical sort over
heterogeneous keys is not a pure-TQL shape (mirrors the shipped host-split precedents:
continuous decay is host-side, `mem_functions.tql:38-54`; strict spec-text re-eval is
host-side, `:122-125`). **P-DISP-2 confirmed this as an asserted-fail (GREEN):** the single
persisted 4-key ranker — whose `$winner` is bound only inside `or`-branches (`$winner is $a` /
`$winner is $b`) — is **rejected at define** with `[REP25] Invalid query containing unbound
concept variable 'winner'`. That is *stronger* than the g1 finding #2 "defines, returns zero
rows" prediction: a *concept* variable conditionally `is`-bound inside `or`-branches is a hard
define-time error. The rejection **is** the verdict confirming the host-split — recorded in the
probe log, not "fixed" into passing. C2 stays entrenchment-*predicate*-only.

**Ranking-key note.** `derivation-method` lives on the **`derivation` relation**, not on the
belief (`g1-g2-validated.md:498`) — so the ranking join reads the method by going *through*
`derivation`, exactly as `canonical-entity` does (`mem_functions.tql:138`). A persisted
predicate that compared "method on the belief" would be a type error.

**What `contested` returns** *(no alternative fork — a definition of the empty-winner path, not
an either/or choice; evidence GREEN via P-DISP-3).* When evidence does **not** separate the
beliefs, the disposition engine's winner stream is **empty**. On an empty winner: the host writes
`belief-state "contested"` on the divergence via `MemWriter`, mints **no** `divergence-exception`
(there is no precedence to record), and `contest()` mints the `anomaly` hub (`contradiction` +
`claim @card(2..)`) so the tie is a first-class recorded conflict for human adjudication.
`contested` is the escape hatch, not the per-divergence default (§8.2).

**Design question — `out-entrenches` single-source-of-truth (ship C2 or not?).**

**Decision — CLOSED. C2 is probe scaffolding, NOT shipped** (confirmed at the Mode-B fold:
C2 was deliberately not folded into `mem_functions.tql` — no persisted function consumes it).
`out-entrenches` is expressible (P-DISP-1: the nested-`or` predicate answers the strict
entrenchment ordering both directions, incl. irreflexivity), so the exclusion is a design
choice, not an expressibility limit. The entrenchment ordering's source of truth remains
`Entrenchment::strength()` (`core/gecko-extension-api/src/semantic.rs:84-92`: `axiom`=4 >
`user-stated`=3 > `tool-derived`=2 > `inferred`=1 > `llm`=0), writer-enforced via
`entrenchment_rank` (`tql.rs:46`) in the supersede guard (`writer.rs:806-824`, OP-5). Shipping
C2 would encode the *same* ordering in TQL — a **second source of truth** — and because §2.3
decided the ranker is host-side, **nothing persisted consumes it**, so C2 stays scaffolding.

**Conditional — if shipped.** Shipping C2 would be the deliberate OP-5 schema-hardening that
`INVARIANTS.md` §8.3 left open ("revisit pushing specific guards … into the schema only if
the bypass list is non-empty" — and the bypass list *is* non-empty, cyber writes belief-tier
relations raw). If shipped, it **owes a drift-lock test**: a pairwise **5×5** agreement check
between the TQL `out-entrenches` and `Entrenchment::strength()`, so the two sources cannot
silently diverge. **Decide and label — never ship it silently.**

**Rejected alternative — ship C2 unconditionally as the canonical ordering.** Rejected:
duplicating `strength()` in TQL creates two sources of truth for a load-bearing security
ordering (OP-5); with no persisted consumer it is dead schema that can drift from the Rust
guard without any test catching it.

**C2 — `w2_disposition.tql`** (CLOSED to scaffolding, NOT shipped; persisted *predicates* at
most, the 4-key ranking is host-assembled — see the P-DISP verdicts in §4):

```typeql
define
  fun out-entrenches($a: belief, $b: belief) -> boolean @doc("True if $a's entrenchment tier strictly out-ranks $b's."):
      match
          $a has entrenchment $ea; $b has entrenchment $eb;
          { $ea == "axiom"; $eb != "axiom"; }
          or { $ea == "user-stated"; { $eb == "tool-derived"; } or { $eb == "inferred"; } or { $eb == "llm"; }; }
          or { $ea == "tool-derived"; { $eb == "inferred"; } or { $eb == "llm"; }; }
          or { $ea == "inferred"; $eb == "llm"; };
      return check;
```

### §2.4 — Interaction with supersession / contradiction machinery / the entrenchment guard

- **Supersession (`writer.rs:806-824`).** Retracting an exception is a supersession of the
  `divergence-exception` belief; because the exception is `sub belief`, the OP-5 entrenchment
  guard applies unchanged — a lower-entrenchment write cannot supersede a higher-entrenchment
  exception. Instance-vs-instance loser-supersession (§2.2) routes through the same guard.
- **Contradiction machinery.** The reified exception `plays contradiction:claim` (inherited,
  §2.1), so a view-vs-instance conflict is now representable as an `anomaly` hub with the
  exception as one claim — closing T4 problem 3. `contest()` uses this hub for the `contested`
  path (§2.3).
- **Entrenchment guard as the human-override anchor.** §8.2's "human asserts precedence" is a
  `divergence-exception` minted at **human-assertion → user-stated** entrenchment (rank 3),
  which out-entrenches the provable trio (tool-derived, rank 2) by the *ratified* authority-⟂-
  provability decoupling (`INVARIANTS.md` §7(b)/§8.1). The guard does not block this — it is
  the intended override, "reached deliberately, still leaving the loser superseded rather than
  erased."

---

## §3 — W3: as-of-t populations/canonical + cohort-belief blast radius

W3 (`INVARIANTS.md` §8.4) is triggered by **T5** (`believed_at` composes with a present-state-
only population view → time-inconsistent answers) *and* **T2** (cohort belief goes silently
stale because `blast-radius` does not traverse `scoped`/`population-member`). §8.4 binds them
into one work item because a fix to one against an unfixed other doubles the staleness (wrong
members *and* no invalidation signal).

### §3.1 — THE DECISION: (A) as-of-t variants vs (B) present-state-only + forbidden composition — CLOSED

`INVARIANTS.md` §7(a)/T5 owed a decision: **either** add as-of-t population/canonical functions
**or** declare the views present-state-only and *forbid* combining them with `believed_at`
(mirroring the index's documented present-state-only bypass, invariant 8).

**Decision — (A) as-of-t variants.** Add `population-members-at($pop, $t)` and
`canonical-entity-at($rec, $t)` (C4), reading the `valid-from`/`valid-to` window §3.2 adds to
`population-member`. Keep the existing no-`$t` functions as the documented **present-state fast
path**. Both were folded and are define-clean under P-COMP.

**Evidence (all GREEN, Mode A + Mode B).**
- Windows are cheap and additive: `owns valid-from/valid-to` coexists with `computed-at`
  (P-PM-1), and left-and-rejoined epochs coexist as distinct rows (P-PM-2).
- The as-of functions are expressible **and epoch-correct**: `population-members-at` genuinely
  consumes `$t` (P-PM-3, FRP1 ok), the recursive `canonical-entity-at` returns the epoch-correct
  identity closure off validity windows (P-CE-1: `(C1,t₁)`={C1,C2}, `(C1,t₃)`={C1,C2,C3}), and
  the acceptance shape holds — `believed-at(t₀)` ∥ `population-members-at(P, t₀)` agree as-of-t
  (P-T5-FIX).
- **(B) is silently wrong as-of-t.** P-BR-6 shows a present-state traversal is window-BLIND: it
  sweeps a cohort belief `B` even for a member whose window has *closed* (the materialized row
  persists). Only a window-respecting entry discriminates (B for t∈[t₀,t₁), ∅ for t≥t₁). Without
  the window, the blast-radius fix ships present-state-correct and silently wrong as-of-t.

**The (B)-for-`canonical-entity` fallback did NOT trigger.** P-CE-1 is GREEN — the recursive
as-of closure type-checks and returns the epoch-correct set — so `canonical-entity-at` ships
under (A) with no degradation; the present-state-only fallback for the recursive case is not
needed.

**Rationale.** Windows are cheap (two nullable datetimes on a relation that already carries
`computed-at`), and a host-side strict as-of re-eval already exists as precedent
(`mem_functions.tql:122-125`). (A) resolves T5's silent time-inconsistency *and* gives §3.4's
cohort blast-radius a window to respect — which is why W2/W3 share one gate.

**Rejected alternative — (B) wholesale (present-state-only, forbid composition everywhere).**
Rejected: it leaves an as-of-t question a security substrate will ask ("what did this cohort
believe at time t?") permanently unanswerable, when the window that answers it is cheap and
proven correct (P-CE-1, P-T5-FIX). P-BR-6 makes the cost concrete — present-state-only is
*silently* wrong as-of-t, the worst failure mode for a temporal substrate.

### §3.2 — `population-member` bitemporal window

**Design question — window vs the existing `computed-at`: replace or coexist?**

**Decision. Coexist — keep `computed-at` *alongside* `valid-from`/`valid-to`** (P-PM-1 GREEN:
the window `owns` is additive and all three attributes read back on one `population-member`
row). They mean different things: `computed-at` is **cache-row freshness** (when the
materializer last evaluated the spec, `mem_types.tql:255`); `valid-from`/`valid-to` is **when
the membership held** (the interval the concept was a member). An as-of-t read needs the *held*
interval, not the *computed* timestamp; staleness detection (§3.5) needs the *computed*
timestamp. Both are needed.

**Rejected alternative — repurpose `computed-at` as the window start.** Rejected: it
conflates "when we recomputed the cache" with "when membership began," so a re-materialization
that does not change membership would wrongly move the window start.

**C4 (window) — `w3_population_window.tql`:**

```typeql
define
  population-member owns valid-from @card(0..1), owns valid-to @card(0..1);
```

### §3.3 — As-of function variants

`population-members-at` reads the window directly (non-recursive). `canonical-entity-at`
drafts the `canonical-entity` closure with **each resolution constrained by its own validity
window** (`$res has valid-from $vf; $vf <= $t; not { $res has valid-to $vt; $vt <= $t; }`)
*instead of/alongside* the `belief-state "asserted"` filter — because as-of semantics must be
defined **off windows** (belief-state mutates on supersession and is a present-state fact),
mirroring how `believed-at` (`mem_functions.tql:20-25`) reads windows, not state.

**C4 (as-of functions) — `w3_asof_functions.tql`:**

```typeql
define
  fun population-members-at($pop: population, $t: datetime) -> { concept } @doc("The members of a population as of time $t, read from the windowed materialized membership."):
      match
          $pm isa population-member, links (owning-population: $pop, member: $m);
          $pm has valid-from $vf; $vf <= $t;
          not { $pm has valid-to $vt; $vt <= $t; };
      return { $m };
```

Plus a `canonical-entity-at($rec: concept, $t: datetime) -> { concept }` — the
`canonical-entity` provable-closure with each resolution's own validity window replacing the
`belief-state "asserted"` filter (as-of read off windows, per above; belief-state mutates on
supersession, so a resolution valid *at* `$t` but superseded *by now* must still count as-of-t).
Its recursive closure is what **P-CE-1** certified GREEN and epoch-correct (`(C1,t₁)`={C1,C2},
`(C1,t₃)`={C1,C2,C3}) — the recursive as-of walk *does* express, so §3.1's (B) fallback does not
fire; this function ships under (A). **`@doc`:** "The source records resolved to the same entity
as `$rec` as of time `$t`, following only provable resolutions valid at `$t`." *(The symmetric
`resolves` closure self-includes the input `$rec` via the `record-a`/`record-b` back-edge — a
pre-existing property of `canonical-entity`, not new to the as-of variant.)*

### §3.4 — Blast-radius traversal extension (option (b); the param-type problem) — CLOSED to b1

CONFLICT-REPORT T2 established the fix is option **(b)** — add an explicit population-
invalidation traversal to `blast-radius` (walk `scoped`←`population`→`population-member`→
member) — because option (a) ("cohort beliefs `rests-on` their population") is *not directly
expressible*: `population-member` is a relation and `rests-on:assumption` admits only
`memory-item`, and a member `concept` plays no `derivation`/`rests-on` role (T2/D3, doubly
orphaned).

**The param-type problem (central risk — did NOT fire).** `blast-radius($r: memory-item)` today
(`mem_functions.tql:64`). The new branch keys off `population-member:member`, played **only by
`concept`** (`mem_types.tql:298`). And `concept sub okf-concept` (`core_schema.tql:63`) is a
**sibling** of `memory-item sub okf-concept` (`mem_types.tql:61`) — not a subtype
(g1-g2-validated key fact). The risk was that a member `concept` could not reach a
`memory-item`-param function. **P-BR-1/2 (asserted GREEN, Mode A + Mode B) show the risk did not
materialize:** TypeDB 3.12 accepts `redefine fun blast-radius($r: okf-concept)` with per-branch
role narrowing across the `memory-item`/`concept` siblings, admits a `concept` argument, keeps
the return `{ memory-item }`, and reaches the cohort belief — and the recursion through the
scoped belief works (`blast-radius(C1)` = {B, D}).

**Decision — b1 (widened `okf-concept` param).** All three sub-options are viable/expressible
(b1, b2, b3 each reached a working verdict), so §3.4 is a design choice among *working* shapes,
not a forced fallback. b1 is picked and was folded into the shipped `mem_functions.tql`:

- **b1 — widened param** (`$r: okf-concept`, the common supertype of both `concept` and
  `memory-item`). **Chosen.** Single entry point; **backward-compatible** (`memory-item ⊂
  okf-concept`, so existing `$r isa memory-item` callers still type-check — no Rust call-site
  change, `writer.rs:947`); recursion works (P-BR-2); return stays `{ memory-item }`. Widens the
  public contract to any `okf-concept` (`concept` + all `memory-item` subtypes; `external-resource`
  is NOT an `okf-concept`, so it is not admitted). Requires the `redefine` migration §1.4 owes.
- **b2 — companion function, no widening** (`cohort-blast-radius($member: concept) -> { belief }`).
  **Rejected alternative.** Viable (P-BR-3) and migration-free, but leaves the
  `blast-radius ∪ cohort-blast-radius` union to two host-side calls; b1 folds the traversal into
  the one function callers already use.
- **b3 — resolution-belief entry** (keeps the `memory-item` param; a `resolution` *is* a belief).
  **Alternative / optional synthesis** (see the entry-points decision below). Viable (P-BR-4/5)
  but, as a `redefine` of `blast-radius`, mutually exclusive with b1 as written.

**Design question — which entry points count as "membership retraction," and does the new
traversal respect windows? — DECIDED.** Well-posed only once §3.2's window exists (the concrete
reason W2/W3 share one gate).

- **Shipped entry point (b1, probe-backed):** a member `concept` whose `population-member` row is
  closed/superseded — reached via the `population-member:member`→`scoped` branch. This is the
  folded, P-COMP-green shape.
- **The `resolution`-belief entry (b3) — "retract the identity belief binding a member counts as
  a membership-retraction entry point": ADMITTED in principle, DEFERRED as a construct.** It is
  well-motivated (retracting the `resolution` that binds a member to the cohort *does* undermine
  the cohort belief), and b1+b3 *can* compose into one `redefine fun blast-radius($r:
  okf-concept)` carrying both branches (since `okf-concept` admits `concept` *and* `resolution`).
  But the **combined single-fn form was probed separately, never as one function** (R9): the
  synthesized shape is **not probe-backed**, so it is **not ratified here**. It is owed to
  implementation with two obligations before it ships: (1) the `resolves` branch must match
  **both** `record-a` **and** `record-b` (the probe matched only `record-a`, as `canonical-entity`
  matches both); (2) the branch was probed **non-recursively** — implementation must decide
  whether a member bound via a resolution should recurse into downstream cohort beliefs. Captured
  as an acceptance-test-owed row (§5).
- **Window-awareness — owed to implementation.** P-BR-6's windowed entry was **compositional**
  (`population-members-at(P, t)` filtering a present-state traversal), not a function: it proves a
  window-respecting blast is *achievable*, but the **windowed-blast TRAVERSAL function itself** (a
  `blast-radius` variant that gates the `population-member` branch on `valid-from`/`valid-to`
  as-of `$t`) is **not yet built**. The shipped b1 `blast-radius` is present-state (window-blind by
  construction, like `cohort-blast-radius` in P-BR-6); the as-of-correct blast is composed by
  callers via `population-members-at`. Building the windowed-blast traversal fn is
  acceptance-test-owed (§5).

**Rejected alternative — option (a) (cohort belief `rests-on` its population).** Rejected as
**not expressible** (T2/D3): `rests-on:assumption` admits only `memory-item`, `population-member`
is a relation, and the member `concept` plays no assumption role. Option (b) is the tractable
path; b1 realizes it.

**C3 — `w3_blast_radius_redefine.tql`** (b1 is the shipped pick; b2/b3 shown as the
rejected/alternative shapes that were probed):

**b1 — widened param (SHIPPED — folded into `mem_functions.tql`):**

```typeql
redefine
  fun blast-radius($r: okf-concept) -> { memory-item }:
      match
          {
              (assumption: $r, resting: $m) isa rests-on;
          } or {
              (source: $r, derived: $m) isa derivation;
          } or {
              (owning-population: $pop, member: $r) isa population-member;
              (scoped-belief: $m, population: $pop) isa scoped;
          } or {
              { (assumption: $r, resting: $mid) isa rests-on; }
              or { (source: $r, derived: $mid) isa derivation; }
              or { (owning-population: $pop2, member: $r) isa population-member;
                   (scoped-belief: $mid, population: $pop2) isa scoped; };
              let $m in blast-radius($mid);
          };
      return { $m };
```

**b2 — companion function, no widening:**

```typeql
define
  fun cohort-blast-radius($member: concept) -> { belief } @doc("The population-scoped beliefs undermined when this member's cohort standing changes."):
      match
          (owning-population: $pop, member: $member) isa population-member;
          (scoped-belief: $b, population: $pop) isa scoped;
      return { $b };
```

**b3 — resolution-belief entry** (keeps the `memory-item` param; a `resolution` *is* a
belief): add to `blast-radius` an or-branch —

```typeql
{
    (resolution-belief: $r, record-a: $c) isa resolves;
    (owning-population: $pop, member: $c) isa population-member;
    (scoped-belief: $m, population: $pop) isa scoped;
}
```

— captures "retracting the identity belief that binds C1's member undermines the cohort."

### §3.5 — Populations-writer contract clauses (contract-only — no writer built here) — CLOSED

W3 creates writer obligations it does not implement (risk R4). *No alternative fork — these are
obligations the populations writer owes, not either/or design choices; the ratifiability gate's
"rejected alternative" requirement is satisfied by that framing.* Stated as contract clauses the
populations writer must satisfy (enforced by SPEC-W5's allowlist, not here):

- **On member-supersession, close the member's window AND flip `computed-at`.** Set `valid-to` ←
  the supersession time (the membership held until then) **and** advance `computed-at`
  (`mem_types.tql:255`) so downstream staleness detection fires. **Both, not one** — §3.2's two
  markers mean two updates (a window-close without a `computed-at` flip leaves staleness
  undetected; a `computed-at` flip without a window-close leaves the as-of read wrong).
- **Window-maintenance on re-materialization (closes risk R4).** A re-materialization that does
  **not** change membership must **not** move `valid-from` (that is exactly the §3.2 rejected
  "repurpose `computed-at`" conflation); it advances only `computed-at`. A member that leaves and
  later rejoins is written as **two windowed epoch rows**, not a mutated one (P-PM-2 proves
  distinct epoch rows coexist).
- **Who may write `population-member` rows** — only a sanctioned populations writer, never raw
  `graph.write`. This feeds **SPEC-W5's monopoly allowlist** (the belief-tier / cache-tier write
  surface W5a's test asserts is single-writer); `population-member` (a cache-tier relation) and
  the new window attributes join that allowlist.
- The window writes are the *materialization* half; the divergence-exception writes (§2.2) are the
  *disposition* half — both route through sanctioned seams. The §1.4 migration posture (a
  `redefine`-emitting apply path) is a precondition for shipping the b1 `blast-radius` body-change
  the traversal depends on.

### §3.6 — Cross-refs

- **W8 (consolidation daemon).** The daemon precomputes/invalidates population-member rows at
  scale; its contract (W8, CONFLICT-REPORT T8) must respect §3.2's window and §3.5's close-on-
  supersession clause, and write through `EpistemicWriter` (never raw `graph.write`).
- **W5 (monopoly).** §2.2's co-mint procedural residue and §3.5's writer allowlist are
  *enforced* by SPEC-W5 §3.2 / W5a-W5b; SPEC-W2-W3 only states the obligations.
- **W5 surface addition.** The belief-tier write surface W5a's monopoly test covers gains
  `divergence-exception` (a `belief`) and `divergence-precedence` (a belief-referencing
  relation) — both must be added to W5a's asserted population, or the monopoly test checks the
  wrong shape (the same failure mode as the cyber raw-relation breach).

---

## §4 — Probe evidence table ([P-*], D3-style)

Each candidate construct earns probes only *after* the §2/§3 checks against ratified text
above. A probe proves **expressibility**, never **conformance** (risk R9). **Mode A** =
standalone expressibility (the fragment applies/`redefine`s clean against a minimal composed
schema, type-checks, and a fixture inserts/queries as expected). **Mode B** = schema-merge
(the fragment applies against the *full* shipped composed schema —
core→mem→mem-functions→cyber — define-clean, and the scenario manifests live). A probe is
**green** only when both modes agree with its expectation. Rows whose expectation is
**FAIL-as-designed** are green when the write/compile is rejected exactly as predicted; those
are *verdicts*, recorded in the probe log, never "fixed" into passing.

**Evidence is filled from the probe log** (`docs/refactor/probes/W2-W3-PROBE-LOG.md`; probe
worktree `gecko-probe-w2w3`, branch `probe/w2w3-schema`; full suite **23 passed / 0 failed** —
2 baselines + 8 W2 + 11 W3 + P-COMP + P-REDEF, `--no-default-features --features cyber --
--ignored --test-threads=1`). **Probe-ID reconciliation:** the skeleton's provisional `[P-*]`
IDs are replaced here by the actual probe names in the log — the W2 precedence rows realign
(member-scoping is **P-PREC-2**, the contradiction-claim is **P-PREC-3**; the skeleton had them
swapped), the disposition rows match, and the blast-radius rows use the log's **P-BR-1..6** (the
skeleton's `P-BLAST-b*` collapsed b1's two probes and omitted the windowed-seam probe P-BR-6).
**Mode B** is green for every **shipped** construct (C1, the `population-member` window, b1
`blast-radius`, the two as-of fns — all folded and proven define-clean by P-COMP); the
deliberately-unfolded scaffolding/alternatives (C2 `out-entrenches`, b2 `cohort-blast-radius`,
b3) are Mode A green and **Mode-B-N/A by design** (not in the shipped schema), which does not
block ratification because they ship nothing.

**W2 — divergence-exception mechanism**

| Probe | Construct / question | Candidate | Mode A | Mode B | Verdict |
|---|---|---|---|---|---|
| **P-PREC-1** | belief-anchored `divergence-exception` + 4-role `divergence-precedence` define-clean; roles wire onto `belief`/`concept`; full instance inserts + reads back (incl. `exception-scope`) | C1 `w2_precedence.tql` | green | green — folded (P-COMP) | **CONFIRMED** |
| **P-PREC-2** | member-scoped exception via `exception-scope` leaves the population belief `"asserted"` for other members (closes R3); `disposition-of` reads it back to the specific member | C1 `w2_precedence.tql` | green | green — folded (P-COMP) | **CONFIRMED** |
| **P-PREC-3** | the reified `divergence-exception` inserts and **plays `contradiction:claim`** — a view never can (closes T4 problem 3); `contradicts(E,BP)`=true | C1 `w2_precedence.tql` | green | green — folded (P-COMP) | **CONFIRMED** |
| **P-PREC-4** | retracting the exception = supersession of the exception belief (relation retained with lineage; live disposition empties; population stays asserted) | C1 `w2_precedence.tql` | green | green — folded (P-COMP) | **CONFIRMED** |
| **P-PREC-5** | anchor-less `divergence-precedence` (no `exception-belief`) accepted at insert, **rejected at commit** `[CNT5] @card(1..1)` — the bare-edge override is structurally barred | C1 `w2_precedence.tql` | green (commit rejected as predicted) | green — folded (P-COMP) | **CONFIRMED (asserted-fail)** |
| **P-DISP-1** | `out-entrenches` nested-`or` predicate answers the strict entrenchment ordering both directions (incl. irreflexivity) | C2 `w2_disposition.tql` | green | n/a — C2 unfolded scaffolding (§2.3) | **CONFIRMED** |
| **P-DISP-2** | a **single** persisted TQL fn selecting the 4-key `$winner` inside `or`-branches — **define REJECTED** `[REP25]` unbound var ⇒ host-split | C2 + scaffold | green (define rejected as predicted) | n/a — host-side, no schema construct | **CONFIRMED (asserted-fail; R2)** |
| **P-DISP-3** | host-side 4-key ranker reproduces §8.2 dispositions: separated ⇒ winner; total tie ⇒ `contested` (None), no exception minted | C2 `w2_disposition.tql` | green | n/a — host-side | **CONFIRMED** |

**W3 — as-of-t populations/canonical + cohort-belief blast radius**

| Probe | Construct / question | Candidate | Mode A | Mode B | Verdict |
|---|---|---|---|---|---|
| **P-BR-1** | b1 `redefine fun blast-radius($r: okf-concept)` **accepted** (asserted); `blast-radius(C1: concept)` reaches cohort belief `{B}` — the sibling-narrowing risk did NOT fire | C3-b1 `w3_blast_radius_b1.tql` | green | green — folded (P-COMP) | **RESOLVED — b1 accepted** |
| **P-BR-2** | b1 recursion via the population branch: `blast-radius(C1)` = `{B, D}` | C3-b1 `w3_blast_radius_b1.tql` | green | green — folded (P-COMP) | **RESOLVED — recursion works** |
| **P-BR-3** | b2 `cohort-blast-radius($member: concept) -> { belief }` define-clean; `(C1)` = `{B}` | C3-b2 `w3_blast_radius_b2.tql` | green | n/a — b2 unfolded (rejected alt) | **CONFIRMED** |
| **P-BR-4** | b3 `resolves`-entry: `blast-radius(RES)` = `{B}` via `resolves`→member→`scoped` | C3-b3 `w3_blast_radius_b3.tql` | green | n/a — b3 unfolded (alternative) | **RESOLVED — b3 works** |
| **P-BR-5** | `redefine fun blast-radius` accepted; redefined body genuinely in force (no silent old body) | C3-b3 `w3_blast_radius_b3.tql` | green | n/a — schema-tx mechanism (production posture: P-REDEF) | **RESOLVED — redefine works** |
| **P-BR-6** | present-state `cohort-blast-radius` is **window-BLIND** (sweeps `B` for a closed-window member); windowed entry `population-members-at(P,t)` discriminates (B for t∈[t₀,t₁), ∅ for t≥t₁) — **compositional demo** | C3-b2 + C4a + C4b | green | green — windowed entry off shipped fns | **RESOLVED — §3.1 → (A); windowed-blast traversal fn owed (§5)** |
| **P-PM-1** | `population-member owns valid-from/valid-to` applies **alongside** `computed-at`; all three read back | C4a `w3_population_window.tql` | green | green — folded (P-COMP) | **CONFIRMED** |
| **P-PM-2** | two windowed epoch rows for the same (P, C1) coexist (a left-and-rejoined member) | C4a `w3_population_window.tql` | green | green — folded (P-COMP) | **RESOLVED — coexist** |
| **P-PM-3** | `population-members-at($pop, $t)` genuinely consumes `$t` (FRP1 ok): `(P,feb)`={C1,C2}, `(P,may)`={C1} | C4b `w3_asof_functions.tql` | green | green — shipped fn (P-COMP) | **CONFIRMED** |
| **P-CE-1** | recursive `canonical-entity-at($rec, $t)` epoch-correct closure off validity windows: `(C1,t₁)`={C1,C2}, `(C1,t₃)`={C1,C2,C3} | C4b `w3_asof_functions.tql` | green | green — shipped fn (P-COMP) | **RESOLVED — epoch-correct; §3.1 (A) applies to canonical, no (B) degradation** |
| **P-T5-FIX** | `believed-at(t₀)`={B} ∥ `population-members-at(P,t₀)`={C1} agree as-of-t (today={C1,C2}) — the W3 acceptance shape | all W3 deltas | green | green — shipped fns (P-COMP acceptance (b)) | **CONFIRMED** |

**Mode B — the folded shipped shape + migration posture**

| Probe | Construct / question | Candidate | Mode A | Mode B | Verdict |
|---|---|---|---|---|---|
| **P-COMP** | full folded stack composes **core→mem→cyber define-clean**; shipped shape delivers all 3 acceptance behaviors (T2 cohort blast, T5 as-of agreement, W2 reified-exception contradiction claim) with **NO delta** | folded `mem_types.tql`/`mem_functions.tql` | — (Mode-B gate) | green (zero define errors) | **CONFIRMED — ratifiability gate** |
| **P-REDEF** | a persisted fn is `[FUN5]`-immutable under the production `define` path (changed OR identical body refused); mutation needs `redefine fun` / fresh DB | folded schema, production `define` path | — (Mode-B gate) | green | **RESOLVED — `define`-immutable; §1.4 owes a `redefine` migration path** |

**Baselines (regression floor — must stay green post-fold)**

| Probe | Construct / question | Mode A | Mode B | Verdict |
|---|---|---|---|---|
| **P-BASE-T2** | ratified `blast-radius` misses the cohort belief when driven from a non-member (b1's population branch does not fire) | green | green | **GREEN** |
| **P-BASE-T5** | ratified present-state `population-members` ignores time | green | green | **GREEN** |

---

## §5 — Acceptance tests owed by implementation (each probe's graduated name)

When a probe's shape is ratified, it graduates from an `#[ignore]` probe into a shipped
acceptance test the implementation must keep green. With the §4 verdicts in, the surviving
(shipped) rows are the ones whose sub-option was picked; the rejected/alternative rows are
marked **not shipped**, and the two implementation-owed constructs (the windowed-blast traversal
fn and the b3 synthesis) are marked **OWED**. Graduated names (IDs reconciled to the probe log):

| Probe | Graduated acceptance test | Owner | Ships? |
|---|---|---|---|
| P-PREC-1 / P-PREC-3 | `divergence_exception_is_a_belief_and_plays_contradiction_claim` | W2 schema | yes |
| P-PREC-2 | `member_scoped_exception_leaves_population_belief_asserted_for_others` | W2 writer | yes |
| P-PREC-4 | `retracting_a_precedence_supersedes_the_exception_belief_with_lineage` | W2 writer | yes |
| P-PREC-5 | `anchorless_divergence_precedence_rejected_at_commit` | W2 schema | yes |
| P-DISP-1 | `out_entrenches_answers_strict_entrenchment_ordering` (+ conditional 5×5 `…_agrees_with_strength_pairwise` drift-lock **only if** C2 ships) | W2 (conditional) | scaffolding — C2 not shipped |
| P-DISP-2 | `single_persisted_disposition_ranker_is_not_expressible` (expected-fail verdict) | W2 (host-split) | yes (live inexpressibility cert) |
| P-DISP-3 | `empty_winner_yields_contested_anomaly_hub_no_exception_minted` | W2 host engine | yes |
| P-COMP | `full_candidate_stack_composes_define_clean_with_cyber` | assembly | yes |
| P-BR-1 / P-BR-2 | `blast_radius_widens_param_and_reaches_cohort_with_recursion` (b1) | W3 function | yes |
| P-REDEF | `shipped_function_mutation_requires_redefine_not_define` (+ the migration path) | W3 migration | yes |
| **P-BR-6 (OWED)** | `windowed_blast_radius_traversal_respects_membership_window` — the windowed-blast **traversal fn** (P-BR-6 proved it compositionally; the function itself is owed) | W3 function | **OWED (§3.4)** |
| P-BR-3 | `cohort_blast_radius_companion_returns_scoped_beliefs` (b2) | — | not shipped (rejected alt) |
| **P-BR-4 / P-BR-5 (OWED-optional)** | `blast_radius_resolution_branch_matches_both_records_and_reaches_cohort` — b3 synthesized into b1, matching both records + recursion decision, needs its own green probe (R9) before shipping | W3 function | **OWED-optional (§3.4)** |
| P-PM-1 | `population_member_carries_bitemporal_window_and_computed_at` | W3 schema | yes |
| P-PM-2 | `left_and_rejoined_member_is_two_windowed_epoch_rows` | W3 schema | yes |
| P-PM-3 | `population_members_at_returns_windowed_membership` | W3 function | yes |
| P-CE-1 | `canonical_entity_at_recursive_asof_closure` | W3 function | yes |
| P-T5-FIX | `believed_at_and_members_at_agree_as_of_t` | W3 acceptance | yes |

The picked sub-options are settled: **C3 → b1** (§3.4), **C2 → scaffolding, not shipped** (§2.3),
**§3.1 → (A) as-of-t variants** (canonical included, no (B) degradation). The two **OWED** rows —
the windowed-blast traversal fn and the b3 synthesis — are the only constructs this spec names
that are **not** yet probe-backed as shipped shapes; both are handed to implementation, not
ratified here (R9).

---

## §6 — Ratification record

Ratification is the user's act (GLOBAL-CONSTRAINTS: "a spec is at most RATIFIABLE/DRAFT; only
the user flips a status blockquote to RATIFIED at commit `<sha>`"). This spec is **RATIFIABLE**;
this section is left for the user's act.

- **RATIFIED at commit:** _(user sets `<sha>` here)_
- **Probe log reference:** `docs/refactor/probes/W2-W3-PROBE-LOG.md` — the `gecko-bin/tests/probes/`
  suite (probe worktree `gecko-probe-w2w3`, branch `probe/w2w3-schema`) turned every §4 `[P-*]`
  green: **23 passed / 0 failed** (2 baselines + 8 W2 + 11 W3 + P-COMP + P-REDEF), Mode A for all
  and Mode B for every shipped construct (P-COMP is the Mode-B ratifiability gate).
- **Decisions closed (each from its §4 verdict):**
  - **§3.4 → b1** (widened `okf-concept` param) — b2/b3 rejected/alternative; the sibling-narrowing
    risk did not fire (P-BR-1/2).
  - **C2 → scaffolding, NOT shipped** (§2.3) — no persisted fn consumes `out-entrenches`; the
    host-side ranker stands (P-DISP-2/3).
  - **§3.1 → (A) as-of-t variants** — `canonical-entity` included, the (B) fallback did not trigger
    (P-CE-1 epoch-correct).
  - **§1.4 → `[FUN5]`-immutable functions; a `redefine` migration path is owed** (P-REDEF).
  - **§3.4 entry points / window-awareness** — b1's `population-member` entry ships; the b3 synthesis
    and the windowed-blast traversal fn are owed to implementation (§5), not ratified here (R9).

### Ratifiability self-check (recorded per the closure brief)

- **Every design question → decision + rationale + rejected-alternative (or N/A):** §2.1 (reify —
  bare-edge rejected), §2.2 (member-scope — supersede-population rejected; co-mint — *no fork,
  obligation*), §2.3 (host-split — single-fn ranker rejected; C2 — unconditional-ship rejected;
  contested-return — *no fork, definition*), §3.1 ((A) — (B) rejected), §3.2 (coexist — repurpose
  `computed-at` rejected), §3.4 (b1 — b2/b3 + option (a) rejected), §1.4 (`redefine` — b2-sidestep
  rejected), §3.5 (*no fork, obligations*). ✅
- **Every shipped schema construct cites a GREEN probe (Mode A AND Mode B via P-COMP):** C1 →
  P-PREC-1..5 + P-COMP; `population-member` window → P-PM-1/2 + P-COMP; b1 `blast-radius` →
  P-BR-1/2 + P-COMP; `population-members-at`/`canonical-entity-at` → P-PM-3 / P-CE-1 / P-T5-FIX +
  P-COMP. Merged-state evidence for all: **P-COMP**. ✅
- **Every new type/role/fn carries a greenfield `@doc`:** §1.1 inventory (each one sentence, no
  plan/epic tags, no changelog phrasing); status now lives in its own column, never inside a `@doc`. ✅
- **Every §8.2 refinement is verbatim-quote + delta:** §2.1 (three verbatim §8.2 blocks + the
  bare-edge-forbidden delta), §2.2 (full verbatim §8.2 item 1, no compression). §8.2 fidelity
  preserved — nothing here green-lights a construct §8.2 forbids. ✅
- **Not-yet-probe-backed constructs are handed to implementation, not ratified (R9):** the
  windowed-blast traversal fn and the b3 synthesis (§3.4, §5). ✅

_Nothing here green-lights a construct §8.2 forbids: the bare `prevailing/overridden` edge is
rejected in §2.1 by verbatim-quote + delta before any probe (and structurally barred at commit,
P-PREC-5), and the belief-anchored C1 shape is what satisfies §8.2 items 1 and 3 together. This
document is **RATIFIABLE**; the user performs ratification (§6, `RATIFIED at commit <sha>`)._
