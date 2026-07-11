# SPEC-W2-W3 — divergence-exception mechanism (W2) + as-of-t populations/canonical & cohort-belief blast radius (W3)

> Status: DRAFT — ratifiable when every [P-*] in §4 is green (Mode A + Mode B)
> Gates: INVARIANTS.md §8.2 (ratified semantics), §8.4 rows W2/W3; resolves CONFLICT-REPORT T2/T4/T5.

This is one spec for two work items because they share a schema surface and a data
dependency, not by convenience. W2 (`INVARIANTS.md` §8.4) builds the divergence-exception
mechanism owed by §8.2; W3 (§8.4) decides and implements as-of-t populations/canonical
plus the cohort-belief blast-radius traversal owed jointly by T2 and T5. W2's disposition
ranking consumes the `valid-from` windows W3 defines on membership, so the two ratify
together on one probe gate — separating them would let one land against a membership
model the other has not yet fixed.

**This is a DRAFT skeleton.** Every design question below carries its decision (or
recommended direction), rationale, and rejected alternative, and every candidate construct
is embedded verbatim — but the `[P-*]` evidence slots in §4 are empty, and every decision
that a probe verdict is meant to settle (§3.1's (A)-vs-(B) final pick, §3.4's traversal
closure, §1.4's migration posture) is explicitly marked **pending probe verdicts**. A green
probe proves *expressibility*, never *conformance to §8.2* (risk R9): each construct was
checked against the ratified text below **before** it earned a probe. Ratification is the
user's act; this document does not self-ratify.

---

## §0 — Scope & non-goals

**In scope.** The *schema surface* (new entity/relation/role types, modified owns-clauses)
and the *persisted-function surface* (new TQL functions, one `redefine` of an existing
function) that W2 and W3 require. Concretely: the belief-anchored divergence-exception
shape (C1), the entrenchment-ordering predicate under evaluation (C2), the cohort-belief
blast-radius extension under evaluation (C3), and the `population-member` bitemporal window
plus its as-of function variants (C4).

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

| Name | Kind | Introduced by | Draft `@doc` |
|---|---|---|---|
| `divergence-exception` | entity (`sub belief`) | C1 / §2.1 | A reified disposition belief recording that one belief takes precedence over another for a scoped member, carrying the ranking evidence as provenance. |
| `divergence-precedence` | relation | C1 / §2.2 | Anchors a divergence-exception belief to the beliefs it disposes between and the member concept the exception is scoped to. |
| `divergence-precedence:exception-belief` | role | C1 / §2.2 | The disposition belief carrying provenance and retractability for this precedence. |
| `divergence-precedence:prevailing` | role | C1 / §2.2 | The belief the disposition resolved toward. |
| `divergence-precedence:overridden` | role | C1 / §2.2 | The belief overridden for the scoped member. |
| `divergence-precedence:exception-scope` | role | C1 / §2.2 | The member concept the exception is scoped to. |
| `out-entrenches` | function (`-> boolean`) | C2 / §2.3 | True if `$a`'s entrenchment tier strictly out-ranks `$b`'s. **(probe scaffolding by default — §2.3)** |
| `cohort-blast-radius` | function (`-> { belief }`) | C3-b2 / §3.4 | The population-scoped beliefs undermined when this member's cohort standing changes. **(one of three C3 sub-options — §3.4)** |
| `population-members-at` | function (`-> { concept }`) | C4 / §3.3 | The members of a population as of time `$t`, read from the windowed materialized membership. |
| `canonical-entity-at` | function (`-> { concept }`) | C4 / §3.3 | The source records resolved to the same entity as `$rec` as of time `$t`, following only provable resolutions valid at `$t`. |

`valid-from`/`valid-to` are **not** new attributes — they already exist
(`mem_types.tql:19-20`, owned by `belief`). W3 adds them to `population-member` via an
owns-clause (§3.2); the inventory above lists only genuinely new types.

### §1.2 — Modified types & functions

- **`population-member` gains a bitemporal window** — `owns valid-from @card(0..1), owns
  valid-to @card(0..1)` (C4, §3.2), *alongside* its existing `computed-at` (kept — §3.2
  explains why two markers, not one).
- **`concept` (already `plays population-member:member`, `mem_types.tql:298`)** is the
  member player the new window scopes; no change to its role set.
- **`blast-radius` is mutated by `redefine`** (C3-b1) *or* left intact with a companion
  function added (C3-b2/b3). Which of the three is chosen is a §3.4 probe decision. Only
  b1 is a `redefine` of an existing persisted function — the first-ever such mutation in
  this schema, which is why it carries its own migration probe (§1.4, P-REDEF).
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
  before cyber, then cyber applied, define-clean.

### §1.4 — Migration posture (first-ever persisted-function mutation → P-REDEF)

C3-b1 is a `redefine fun blast-radius(…)` — the first time this schema mutates an
*already-persisted* function rather than only adding new ones. TypeDB 3.12's acceptance of
`redefine fun` on a function with a changed **parameter type** (`memory-item` → `okf-concept`)
and unchanged return type is not yet demonstrated in this repo. **Closure pending probe
verdicts (P-REDEF).** If `redefine fun` with a widened param is rejected or forces a
drop-and-recreate, the migration posture falls back to a companion function (C3-b2), which
is a pure `define` with no migration risk. The spec does not pick b1 vs b2/b3 until §3.4's
probes report.

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
> precedence belief**, itself attributed and retractable. **This is what 'override' *is***
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

The relation is `divergence-precedence` (C1). §8.2 item 1 calls for "a precedence relation
so an instance belief can be marked as taking priority over a population belief *with
provenance* (the reification `scoped` lacks)." The relation *survives* — it exists as C1 —
but it is minted only *with* its anchoring exception belief.

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

**Design question — co-mint-or-reject discipline (cross-ref SPEC-W5 §3.2).**

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
host-side, `:122-125`). **P-DISP-2 is expected to FAIL** — a single persisted TQL function
returning `resolved(head, confidence, reason)` is not expressible; that expected-fail is the
*verdict* confirming the host-split, recorded in the probe log, not "fixed" into passing.

**Ranking-key note.** `derivation-method` lives on the **`derivation` relation**, not on the
belief (`g1-g2-validated.md:498`) — so the ranking join reads the method by going *through*
`derivation`, exactly as `canonical-entity` does (`mem_functions.tql:138`). A persisted
predicate that compared "method on the belief" would be a type error.

**What `contested` returns.** When evidence does **not** separate the beliefs, the disposition
engine's winner stream is **empty**. On an empty winner: the host writes `belief-state
"contested"` on the divergence via `MemWriter`, mints **no** `divergence-exception` (there is
no precedence to record), and `contest()` mints the `anomaly` hub (`contradiction` +
`claim @card(2..)`) so the tie is a first-class recorded conflict for human adjudication.
`contested` is the escape hatch, not the per-divergence default (§8.2).

**Design question — `out-entrenches` single-source-of-truth (ship C2 or not?).**

**Decision (default posture).** **C2 is probe scaffolding, not shipped.** The entrenchment
ordering's source of truth today is `Entrenchment::strength()`
(`core/gecko-extension-api/src/semantic.rs:84-92`: `axiom`=4 > `user-stated`=3 >
`tool-derived`=2 > `inferred`=1 > `llm`=0), writer-enforced via `entrenchment_rank`
(`tql.rs:46`) in the supersede guard (`writer.rs:806-824`, OP-5). C2 encodes the *same*
ordering in TQL — a **second source of truth**. Ship it only if a *persisted* function
actually consumes it; because §2.3 decided the ranker is host-side, likely **nothing does**,
so C2 stays scaffolding.

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

**C2 — `w2_disposition.tql`** (probe scaffolding by default; persisted *predicates* at most,
the 4-key ranking is host-assembled — see P-DISP expectations in §4):

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

### §3.1 — THE DECISION: (A) as-of-t variants vs (B) present-state-only + forbidden composition

`INVARIANTS.md` §7(a)/T5 owes a decision: **either** add as-of-t population/canonical
functions **or** declare the views present-state-only and *forbid* combining them with
`believed_at` (mirroring the index's documented present-state-only bypass, invariant 8).

**Recommended direction — (A) as-of-t variants.** Add `population-members-at($pop, $t)` and
`canonical-entity-at($rec, $t)` (C4), reading the `valid-from`/`valid-to` window §3.2 adds to
`population-member`. Keep the existing no-`$t` functions as the documented **present-state
fast path**.

**Rationale.** Windows are cheap (two nullable datetimes on a relation that already carries
`computed-at`), and a host-side strict as-of re-eval already exists as precedent
(`mem_functions.tql:122-125` — the strict spec-text variant runs host-side). Choosing (A)
resolves T5's silent time-inconsistency *and* gives §3.4's cohort blast-radius a window to
respect, which is why W2/W3 share one gate.

**Fallback — (B) for `canonical-entity` only.** If the recursive as-of closure
(`canonical-entity-at`, per-resolution validity windows over a recursive walk) fails to
type-check or express (**P-CE-1** red), fall back to present-state-only *for canonical-entity
only* and forbid its composition with `believed_at`; `population-members-at` (non-recursive)
can still ship under (A) even then.

**FINAL PICK: pending probe verdicts.** (A) is adopted iff **P-PM-*** and **P-CE-1** are
green (Mode A + Mode B); the `canonical-entity` half degrades to (B) iff P-CE-1 is red. This
skeleton does not close the pick.

**Rejected alternative — (B) wholesale (present-state-only, forbid composition everywhere).**
Rejected as the *default* because it leaves an as-of-t question a security substrate will ask
("what did this cohort believe at time t?") permanently unanswerable, when the window that
answers it is cheap. Retained only as the P-CE-1-red fallback for the recursive case.

### §3.2 — `population-member` bitemporal window

**Design question — window vs the existing `computed-at`: replace or coexist?**

**Decision. Coexist — keep `computed-at` *alongside* `valid-from`/`valid-to`.** They mean
different things: `computed-at` is **cache-row freshness** (when the materializer last
evaluated the spec, `mem_types.tql:255`); `valid-from`/`valid-to` is **when the membership
held** (the interval the concept was a member). An as-of-t read needs the *held* interval, not
the *computed* timestamp; staleness detection (§3.5) needs the *computed* timestamp. Both are
needed.

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

Plus a `canonical-entity-at($rec: concept, $t: datetime) -> { concept }` draft — the
`canonical-entity` provable-closure with each resolution's own validity window replacing the
`belief-state "asserted"` filter (as-of read off windows, per above). Its recursive closure is
what **P-CE-1** probes; if the recursive as-of walk does not express, §3.1's (B) fallback
applies to this function only. **Draft `@doc`:** "The source records resolved to the same
entity as `$rec` as of time `$t`, following only provable resolutions valid at `$t`."

### §3.4 — Blast-radius traversal extension (option (b); the param-type problem)

CONFLICT-REPORT T2 established the fix is option **(b)** — add an explicit population-
invalidation traversal to `blast-radius` (walk `scoped`←`population`→`population-member`→
member) — because option (a) ("cohort beliefs `rests-on` their population") is *not directly
expressible*: `population-member` is a relation and `rests-on:assumption` admits only
`memory-item`, and a member `concept` plays no `derivation`/`rests-on` role (T2/D3, doubly
orphaned).

**The param-type problem (central risk).** `blast-radius($r: memory-item)` today
(`mem_functions.tql:64`). The new branch keys off `population-member:member`, which is played
**only by `concept`** (`mem_types.tql:298`). And `concept sub okf-concept` (`core_schema.tql:63`)
is a **sibling** of `memory-item sub okf-concept` (`mem_types.tql:61`) — not a subtype
(g1-g2-validated key fact). So a member `concept` cannot be passed to a `memory-item` param.
Three sub-options widen or sidestep this; **the spec picks one only after the probes report
(pending probe verdicts).**

- **b1 — widened param** (`$r: okf-concept`, the common supertype of both `concept` and
  `memory-item`). Central risk: does the return stay `{ memory-item }` and does `redefine fun`
  accept the widened param (§1.4, P-REDEF)?
- **b2 — companion function, no widening** (`cohort-blast-radius($member: concept) -> { belief }`);
  the union with `rests-on ∪ derivation` happens host-side in `EpistemicReader::blast_radius`.
  Pure `define`, no migration risk.
- **b3 — resolution-belief entry** (keeps the `memory-item` param; a `resolution` *is* a
  belief). Adds an or-branch capturing "retracting the identity belief that binds C1's member
  undermines the cohort."

**Design question — which entry points count as "membership retraction," and does the new
traversal respect windows?** *Well-posed only once §3.2's window exists* — which is the
concrete reason W2/W3 share one ratification gate. Candidates: (i) a `population-member` row
superseded/closed; (ii) the `resolution`/identity belief binding a member retracted; (iii) an
epoch closed. And: does a **past** member's supersession blast a cohort belief that was valid
*then* (window-aware traversal) or only present cohort beliefs? **Closure pending probe
verdicts** — the traversal shape is chosen with §3.2's window in hand (P-PM-*), not before.

**Rejected alternative — option (a) (cohort belief `rests-on` its population).** Rejected as
**not expressible** (T2/D3): `rests-on:assumption` admits only `memory-item`, `population-member`
is a relation, and the member `concept` plays no assumption role. Option (b) is the tractable
path; which of b1/b2/b3 realizes it is the probe decision above.

**C3 — `w3_blast_radius_redefine.tql`** (three sub-options; probe all, spec picks one):

**b1 — widened param:**

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

### §3.5 — Populations-writer contract clauses (contract-only — no writer built here)

W3 creates writer obligations it does not implement (risk R4). Stated as contract clauses the
populations writer must satisfy (enforced by SPEC-W5's allowlist, not here):

- **On member-supersession, close the member's window** (`valid-to` ← the supersession time)
  **and flip `computed-at`** (`mem_types.tql:255`) so downstream staleness detection fires.
  Both, not one — §3.2's two markers mean two updates.
- **Who may write `population-member` rows** — only a sanctioned populations writer, never raw
  `graph.write`. This feeds **SPEC-W5's monopoly allowlist** (the belief-tier / cache-tier
  write surface W5a's test asserts is single-writer).
- The window writes are the *materialization* half; the divergence-exception writes (§2.2) are
  the *disposition* half — both route through sanctioned seams.

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

Evidence slots are **empty/pending** — filled in a later task once probes run against the
probe worktree (`gecko-bin/tests/probes/`).

| Probe | Construct / question | Candidate file | Expectation | Mode A | Mode B | Verdict |
|---|---|---|---|---|---|---|
| **P-PREC-1** | `divergence-exception` + `divergence-precedence` define clean; roles wire onto `belief`/`concept` | `w2_precedence.tql` | PASS (define-clean) | _pending_ | _pending_ | _pending_ |
| **P-PREC-2** | reified `divergence-exception` inserts as a belief and **plays `contradiction:claim`** (closes T4 #3) | `w2_precedence.tql` | PASS | _pending_ | _pending_ | _pending_ |
| **P-PREC-3** | member-scoped exception via `exception-scope`; population belief stays `"asserted"` for other members | `w2_precedence.tql` | PASS | _pending_ | _pending_ | _pending_ |
| **P-PREC-4** | retracting the exception = supersession of the exception belief (relation retained with lineage) | `w2_precedence.tql` | PASS | _pending_ | _pending_ | _pending_ |
| **P-PREC-5** | anchor-less `divergence-precedence` (no `exception-belief`) rejected at commit (`@card(1)`) | `w2_precedence.tql` | **FAIL-as-designed** | _pending_ | _pending_ | _pending_ |
| **P-DISP-1** | `out-entrenches` type-checks as persisted `boolean`; agrees pairwise 5×5 with `strength()` (drift-lock) | `w2_disposition.tql` | PASS *(only if C2 shipped)* | _pending_ | _pending_ | _pending_ |
| **P-DISP-2** | a **single** persisted TQL fn returning `resolved(head, confidence, reason)` over the 4 keys | `w2_disposition.tql` | **FAIL-as-designed** (confirms host-split, R2) | _pending_ | _pending_ | _pending_ |
| **P-DISP-3** | empty-winner ⇒ `contested` path is host-detectable (no exception minted; anomaly hub) | `w2_disposition.tql` | _pending_ | _pending_ | _pending_ | _pending_ |
| **P-COMP** | full candidate stack applies before cyber, then cyber concats — define-clean end-to-end | all | PASS (zero define errors) | _pending_ | _pending_ | _pending_ |
| **P-REDEF** | `redefine fun blast-radius` with widened param accepted (first persisted-fn mutation) | `w3_blast_radius_redefine.tql` (b1) | _pending_ (migration posture, §1.4) | _pending_ | _pending_ | _pending_ |
| **P-BLAST-b1** | `blast-radius($r: okf-concept)` type-checks, returns `{ memory-item }`, reaches cohort belief via `scoped` | `w3_blast_radius_redefine.tql` (b1) | _pending_ (param-type risk) | _pending_ | _pending_ | _pending_ |
| **P-BLAST-b2** | `cohort-blast-radius($member: concept) -> { belief }` define-clean; host-side union | `w3_blast_radius_redefine.tql` (b2) | PASS | _pending_ | _pending_ | _pending_ |
| **P-BLAST-b3** | resolution-belief or-branch keeps `memory-item` param; reaches cohort via `resolves`→member | `w3_blast_radius_redefine.tql` (b3) | _pending_ | _pending_ | _pending_ | _pending_ |
| **P-PM-1** | `population-member owns valid-from/valid-to` applies alongside `computed-at` | `w3_population_window.tql` | PASS | _pending_ | _pending_ | _pending_ |
| **P-PM-2** | `population-members-at($pop, $t)` type-checks; past-only member excluded from present, present as-of past | `w3_asof_functions.tql` | PASS | _pending_ | _pending_ | _pending_ |
| **P-CE-1** | `canonical-entity-at($rec, $t)` recursive as-of closure (per-resolution windows) type-checks & returns | `w3_asof_functions.tql` | _pending_ (gates §3.1 (A)-vs-(B) for canonical) | _pending_ | _pending_ | _pending_ |

---

## §5 — Acceptance tests owed by implementation (each probe's graduated name)

When a probe's shape is ratified, it graduates from an `#[ignore]` probe into a shipped
acceptance test the implementation must keep green. Draft graduated names:

| Probe | Graduated acceptance test | Owner |
|---|---|---|
| P-PREC-1/2 | `divergence_exception_is_a_belief_and_plays_contradiction_claim` | W2 schema |
| P-PREC-3 | `member_scoped_exception_leaves_population_belief_asserted_for_others` | W2 writer |
| P-PREC-4 | `retracting_a_precedence_supersedes_the_exception_belief_with_lineage` | W2 writer |
| P-PREC-5 | `anchorless_divergence_precedence_rejected_at_commit` | W2 schema |
| P-DISP-1 | `out_entrenches_agrees_with_strength_pairwise` (drift-lock; only if C2 ships) | W2 (conditional) |
| P-DISP-2 | `single_persisted_disposition_ranker_is_not_expressible` (expected-fail verdict) | W2 (host-split) |
| P-DISP-3 | `empty_winner_yields_contested_anomaly_hub_no_exception_minted` | W2 host engine |
| P-COMP | `full_candidate_stack_composes_define_clean_with_cyber` | assembly |
| P-REDEF / P-BLAST-b1 | `blast_radius_redefine_widens_param_and_reaches_cohort` | W3 function |
| P-BLAST-b2 | `cohort_blast_radius_companion_returns_scoped_beliefs` | W3 function |
| P-BLAST-b3 | `blast_radius_resolution_branch_reaches_cohort_via_member` | W3 function |
| P-PM-1 | `population_member_carries_bitemporal_window_and_computed_at` | W3 schema |
| P-PM-2 | `population_members_at_returns_windowed_membership` | W3 function |
| P-CE-1 | `canonical_entity_at_recursive_asof_closure` | W3 function |

The picked sub-options (C3 b1-vs-b2-vs-b3; C2 ship-or-not; §3.1 (A)-vs-(B)) determine which
of these rows survive to ship — decided with the §4 verdicts, not here.

---

## §6 — Ratification record

Ratification is the user's act (GLOBAL-CONSTRAINTS: "a spec is at most RATIFIABLE/DRAFT; only
the user flips a status blockquote to RATIFIED at commit `<sha>`"). This section is left for
that act.

- **RATIFIED at commit:** _(user sets `<sha>` here)_
- **Probe log reference:** _(the `gecko-bin/tests/probes/` run that turned every §4 [P-*]
  green in Mode A + Mode B — pending)_
- **Decisions closed at ratification:** C3 sub-option (b1/b2/b3); C2 ship-or-scaffold; §3.1
  (A)-vs-(B) final pick for `canonical-entity`; §3.4 traversal window-awareness — each set
  from its §4 verdict.

_This document remains **DRAFT** until every [P-*] in §4 is green (Mode A + Mode B) and the
user ratifies. Nothing here green-lights a construct §8.2 forbids: the bare
`prevailing/overridden` edge is rejected in §2.1 by verbatim-quote + delta before any probe,
and the belief-anchored C1 shape is what satisfies §8.2 items 1 and 3 together._
