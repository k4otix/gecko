# W2/W3 schema-probe log

Empirical probe results against a **live TypeDB 3.12** (throwaway, self-deleting
databases). The D3 method: hand-authored raw TypeQL in `#[ignore]` gecko-bin
integration tests, bypassing the stubbed Rust writers, run with:

```text
cargo test --test schema_probe_w2w3 --no-default-features --features cyber \
    -- --ignored --test-threads=1
```

A probe that **fails as expected is a VERDICT, not a bug** (P-DISP-2, P-PREC-5).
Probes prove *expressibility*, not *conformance* — the candidate shapes are the
DECIDED §8.2 constructs; these probes validate them, they do not choose them.

Candidate files probed this batch:
- **C1** `gecko-bin/tests/probes/w2_precedence.tql` — belief-anchored
  `divergence-exception sub belief` + the 4-role `divergence-precedence` relation.
- **C2** `gecko-bin/tests/probes/w2_disposition.tql` — `out-entrenches` entrenchment
  predicate (`return check`).

## W2 batch — verdict summary (8/8)

| Probe | Candidate | Expected | Observed | Verdict |
|---|---|---|---|---|
| P-PREC-1 | C1 `w2_precedence.tql` | pass — define-clean | delta applies clean; full 4-role instance inserts + reads back | **CONFIRMED** |
| P-PREC-2 | C1 `w2_precedence.tql` | pass — pop belief stays asserted; read-back fn | BP stays `"asserted"`; `disposition-of(BI)` → `{E}` | **CONFIRMED** |
| P-PREC-3 | C1 `w2_precedence.tql` | pass — closes T4 problem 3 | contradiction hub over (BP, BI, E) commits; `contradicts(E,BP)` = true | **CONFIRMED** |
| P-PREC-4 | C1 `w2_precedence.tql` | pass — attributed + retractable | supersede E → `is-superseded(E)`=true, live disposition empties, BP stays asserted | **CONFIRMED** |
| P-PREC-5 | C1 `w2_precedence.tql` | pass (asserted-FAIL at commit) | insert accepted; **commit rejected** `@card(1..1)` on `exception-belief` | **CONFIRMED** |
| P-DISP-1 | C2 `w2_disposition.tql` | pass — nested `or` answers | delta applies; `out-entrenches` answers all six orderings | **CONFIRMED** |
| P-DISP-2 | C2 + scaffolding | **EXPECTED FAIL** → host-split | **define REJECTED** `[REP25] unbound concept variable 'winner'` | **CONFIRMED (asserted-fail)** |
| P-DISP-3 | C2 `w2_disposition.tql` | pass — host ranking = §8.2 dispositions | separated → winner A; total tie → contested (None) | **CONFIRMED** |

Task-0 baselines (must stay green): **P-BASE-T2** and **P-BASE-T5** — both still pass
(reproduce the ratified `blast-radius`/`population-members` conflicts).

---

## Per-probe detail

### P-PREC-1 — belief-anchored shape define-clean
- **Delta:** C1 `w2_precedence.tql` applied as a Mode-A schema transaction on top of
  the composed core→mem→cyber schema.
- **Ground state:** a full `divergence-precedence` instance — exception belief E
  dispositions instance BI over population belief BP, scoped to member C1.
- **Observed:** the delta applies with no define error; the instance inserts,
  `(exception-belief:$e, prevailing:$bi, overridden:$bp)` reads back `{prec1/E}`, and
  the `exception-scope` role reads back `{prec1/c1}` (the scoped member).
- **Verdict:** CONFIRMED. `divergence-exception sub belief` + the 4-role relation is
  well-formed AND instantiable, including the `exception-scope` role. Additive `belief
  plays …:prevailing/:overridden` and `concept plays …:exception-scope` in a delta
  transaction are accepted.

### P-PREC-2 — member-scoped exception leaves the population belief asserted (R3)
- **Delta:** C1 + a probe read-back function `disposition-of($inst: belief) ->
  { divergence-exception }`.
- **Ground state (T4):** population P with members C1, C2; population belief BP scoped
  to P; instance belief BI diverges for C1; exception belief E dispositions BI over BP,
  `exception-scope: C1`.
- **Observed:** `BP.belief-state == "asserted"` (unchanged — no supersession of BP);
  `disposition-of(BI)` → `{prec2/E}`; E's `exception-scope` binds the SPECIFIC member
  `{prec2/c1}` (not C2).
- **Verdict:** CONFIRMED. Member-scoping lives on the exception (which carries the
  scope), never on `supersession` — so overriding BP for C1 leaves it asserted for the
  rest of the cohort. The scope is real per-member (E scopes to C1 alone). Closes risk
  R3. A function taking `($inst: belief)` reads the disposition back.

### P-PREC-3 — the reified exception is a contradiction claim (closes T4 problem 3)
- **Delta:** C1.
- **Ground state:** an `anomaly` hub with `contradiction (hub:$hub, claim:BP, claim:BI,
  claim:E)` where E is a `divergence-exception`.
- **Observed:** the insert commits (E can play `contradiction:claim`); `contradicts(E,
  BP)` = true.
- **Verdict:** CONFIRMED. `divergence-exception sub belief` inherits `plays
  contradiction:claim`, so the reified exception CAN be a contradiction claim — exactly
  what a `canonical-entity` *view* (a `population`, not a belief) never can. This is the
  structural reason the exception is reified as a belief.

### P-PREC-4 — the exception is attributed and retractable (§8.2 clause)
- **Delta:** C1 + a probe function `live-disposition-of($inst: belief)` gated on
  `not { true == is-superseded($e); }`.
- **Ground state:** BP, BI, E, and their `divergence-precedence` (no scope).
- **Observed:** before retraction `live-disposition-of(BI)` → `{prec4/E}`; after a
  superseding belief E2 + `supersession(superseded:E, superseding:E2)`,
  `is-superseded(E)` = true, `live-disposition-of(BI)` → `{}` (empty), and BP is still
  `"asserted"` (the pre-exception state is answerable again).
- **Verdict:** CONFIRMED. Retracting a disposition is a supersession of its anchoring
  belief — no bare-edge deletion. The §8.2 "attributed and retractable" clause holds.

### P-PREC-5 — anchor-less precedence fails at COMMIT (co-mint structural half)
- **Delta:** C1.
- **Ground state:** `divergence-precedence (prevailing:$bi, overridden:$bp)` — NO
  `exception-belief` player.
- **Observed:** the `insert` statement is **accepted** (`insert_result.is_ok()`); the
  **commit is rejected**. Observed error (verbatim):
  ```
  [CNT5] Constraint '@card(1..1)' has been violated: found 0 instances.
  Caused: [DVL11] Instance [...] of relation type 'divergence-precedence' has a
  relates constraint violation for role type 'divergence-precedence:exception-belief'.
  Caused: [COW5] Concept write failed due to a data validation error.
  Caused: [DCT3] Data commit error.
  Caused: [SRV19] Data commit failed.
  Caused: [TSV5] Data transaction commit failed.
  ```
- **Verdict:** CONFIRMED (asserted-fail is the pass). `relates exception-belief @card(1)`
  validates at **commit, not insert** — an anchor-less `divergence-precedence` (the
  forbidden bare-edge override) is structurally rejected. The *procedural* residue —
  that the anchor was minted by the sanctioned writer, not hand-written — is owed to
  SPEC-W5 §3.2 (the schema cannot enforce it).
- **Regression guard:** the probe asserts the insert-ok/commit-err timing AND that the
  commit error string contains `exception-belief`/`@card`/`CNT5` — pinning the specific
  `@card(1)` `exception-belief` violation, not any commit failure.

### P-DISP-1 — nested-disjunction `return check` predicate answers correctly
- **Delta:** C2 `w2_disposition.tql`.
- **Ground state:** beliefs A (`axiom`), M (`tool-derived`), L (`llm`).
- **Observed:** `out-entrenches(A,L)`, `out-entrenches(A,M)`, `out-entrenches(M,L)` all
  true; `out-entrenches(L,A)`, `out-entrenches(M,A)`, `out-entrenches(A,A)` all false.
- **Verdict:** CONFIRMED. The nested `or`-of-`or` compiles and answers the strict
  entrenchment ordering both directions, including strict irreflexivity.

### P-DISP-2 — pure-TQL 4-key ranker is inexpressible (EXPECTED FAIL → the verdict)
- **Delta:** C2 + probe scaffolding `divergence-winner($a,$b) -> { belief }`, a 4-key
  ranker whose `$winner` is bound **only inside `or`-branches** (`$winner is $a` /
  `$winner is $b`).
- **Expected:** fail — per g1 finding #2 an or-branch-only binding is unbound at
  `return`; either a define error or a zero-row call. Confirms the host-split (risk R2).
- **Observed:** the define is **REJECTED outright** (Branch 1 of the probe). Observed
  error (verbatim):
  ```
  [REP25] Invalid query containing unbound concept variable 'winner'.
  Caused: [FRP4] Function pattern contains an error.
  Caused: [FUN4] Failed to translate TypeQL function into internal representation
  Caused: [DEX30] An error occurred by defining the function
  Caused: [QEX2] Failed to execute define query.
  Near 10:6
  -----
                {
                    $ma == "human-assertion"; $mb == "llm-synthesis";
  -->               $winner is $a;
           ^
  ```
- **Verdict:** CONFIRMED (asserted-fail is the pass). The observed failure is **stronger
  than the g1 prediction**: rather than defining-then-returning-zero-rows, TypeDB 3.12
  refuses to *define* a function that conditionally binds a concept variable inside
  `or`-branches — `$winner is $a` where `$winner` is otherwise unbound is a hard
  `[REP25]` unbound-variable error at define time. Either way the design signal is
  identical and load-bearing: **the 4-key winner cannot be selected in pure TQL**; the
  disposition ranking is host-assembled. C2 stays entrenchment-*predicate*-only.
- **Regression guard:** the probe asserts the define-error string contains
  `winner`/`REP25` (Branch 1) — pinning the specific unbound-variable rejection, not
  merely "some error" — and, should a future TypeDB accept the define instead, asserts
  the call does NOT correctly return the single winner (Branch 2). Either way it stays
  a live certification of the inexpressibility.

### P-DISP-3 — host-side ranking reproduces §8.2 dispositions
- **Delta:** C2.
- **Ground state:** two fixtures. SEP — belief A (`derivation-method
  "human-assertion"`, conf 0.9, `user-stated`, later valid-from) vs B (`llm-synthesis`,
  conf 0.2, `llm`, earlier). TIE — TA and TB identical on all four keys
  (`llm-synthesis`, 0.5, `llm`, same valid-from).
- **Method:** each belief's 4 keys are read from the graph — `derivation-method`
  **joined through the `derivation` relation** (it lives on the relation, not the
  belief), plus `confidence`, `entrenchment`, `valid-from` off the belief — and ranked
  in Rust `(derivation-method, confidence, entrenchment, valid-from)`, most-significant
  first.
- **Observed:** SEP → winner `disp3/sep/A` (decided on the first key); TIE → `None`
  (contested → host writes `belief-state "contested"`, mints no exception).
- **Verdict:** CONFIRMED. The disposition is reproducible host-side over graph
  predicates; separated evidence yields a winner, a total tie yields contested. This is
  the split P-DISP-2 forces.

---

## Notes carried to the spec

1. **P-DISP-2's failure is at define-time, not call-time.** The g1 finding #2 phrasing
   ("type-checks and commits but returns zero rows") describes value-variable cases; a
   *concept* variable conditionally bound via `is` inside `or`-branches is rejected
   earlier, at `define` (`[REP25]`). SPEC-W2-W3 §2.3 should cite the observed `[REP25]`
   as the concrete inexpressibility evidence for the host-split, not merely "zero rows".
2. **C1 additive `plays` in a delta transaction is accepted** — `belief plays
   divergence-precedence:prevailing/:overridden` and `concept plays
   …:exception-scope` compose cleanly on top of the mem substrate (relevant to the
   Mode-B fold and P-COMP).
3. **P-PREC-5 proves the structural half only.** `@card(1)` bars an anchor-less relation
   at commit; it does NOT bar a hand-written anchor. The co-mint procedural guarantee
   (SPEC-W2-W3 §2.2) is enforced by the sanctioned writer seam (SPEC-W5 §3.2), not the
   schema.

---

# W3 batch — as-of-t populations/canonical + cohort-belief blast radius

Resolves conflict-report **T2** (blast-radius misses cohort beliefs) and **T5**
(populations ignore time), and closes SPEC-W2-W3 **§3.1** (as-of-t A vs B) and **§3.4**
(blast-radius entry points).

Candidate files probed this batch (`gecko-bin/tests/probes/`):
- **C3-b1** `w3_blast_radius_b1.tql` — `redefine fun blast-radius($r: okf-concept)`,
  param widened `memory-item`→`okf-concept` + a `population-member`→`scoped` branch.
- **C3-b2** `w3_blast_radius_b2.tql` — a NEW additive companion
  `cohort-blast-radius($member: concept) -> { belief }` (no widening; host-side union).
- **C3-b3** `w3_blast_radius_b3.tql` — `redefine fun blast-radius($r: memory-item)`
  keeping the param, adding a `resolves`→`population-member`→`scoped` entry branch.
- **C4a** `w3_population_window.tql` — `population-member owns valid-from, owns valid-to`.
- **C4b** `w3_asof_functions.tql` — `population-members-at($pop, $t)` +
  recursive `canonical-entity-at($rec, $t)` (per-branch validity-window predicates).

> **The T2 baseline drives from assumption A (no path to the cohort); the fix probes
> drive from the MEMBER concept C1** — the only node reaching the cohort belief B via
> `population-member`→`scoped`. b1's point is that `blast-radius(C1: concept)`
> type-check when `concept` and `memory-item` are SIBLINGS under `okf-concept`.

## W3 batch — verdict summary (11/11)

| Probe | Candidate | Expected | Observed | Verdict |
|---|---|---|---|---|
| P-BR-1 | b1 `w3_blast_radius_b1.tql` | open — decides b1 vs b2 | **redefine ACCEPTED**; `blast-radius(C1: concept)` type-checks and returns B | **RESOLVED — b1 VIABLE** |
| P-BR-2 | b1 `w3_blast_radius_b1.tql` | open | recursion through scoped belief: `blast-radius(C1)` = {B, D} | **RESOLVED — recursion works** |
| P-BR-3 | b2 `w3_blast_radius_b2.tql` | pass | `cohort-blast-radius(C1)` = {B} on the T2 fixture | **CONFIRMED** |
| P-BR-4 | b3 `w3_blast_radius_b3.tql` | open | `blast-radius(RES)` = {B} via the `resolves` entry | **RESOLVED — b3 works** |
| P-BR-5 | b3 (redefine) | open — §1.4 posture | `redefine fun blast-radius` accepted; redefined body in force | **RESOLVED — redefine works** |
| P-BR-6 | b2 + C4a + C4b | open — contingent on §3.1 | present-state b2 window-BLIND (returns B); windowed entry discriminates (B for t∈[t₀,t₁), ∅ for t≥t₁) | **RESOLVED — §3.1 → (A)** |
| P-PM-1 | C4a `w3_population_window.tql` | pass | window `owns` additive; valid-from/valid-to/computed-at coexist on one row | **CONFIRMED** |
| P-PM-2 | C4a `w3_population_window.tql` | open | two windowed epoch rows for the same (P, C1) coexist (2 instances) | **RESOLVED — coexist** |
| P-PM-3 | C4b `w3_asof_functions.tql` | pass | `population-members-at(P, feb)`={C1,C2}, `(P, may)`={C1} — `$t` consumed (FRP1 ok) | **CONFIRMED** |
| P-CE-1 | C4b `w3_asof_functions.tql` | open | `canonical-entity-at(C1, t₁)`={C1,C2}; `(C1, t₃)`={C1,C2,C3} — epoch-correct closure | **RESOLVED — epoch-correct** |
| P-T5-FIX | all W3 deltas | pass | `believed-at(t₀)`={B} ∥ `population-members-at(P, t₀)`={C1}; today={C1,C2} | **CONFIRMED** |

Task-0 baselines (must stay green): **P-BASE-T2** and **P-BASE-T5** — both still pass.
Full run: **21 passed; 0 failed** (2 baselines + 8 W2 + 11 W3), `--test-threads=1`.

## §3.1 decision evidence: (A) as-of-t variants — SUPPORTED

- The bitemporal window is **cheap and clean**: `owns valid-from/valid-to` is additive
  and coexists with `computed-at` (P-PM-1); left-and-rejoined epochs coexist as distinct
  rows (P-PM-2).
- The as-of-t functions are **fully expressible and answer correctly**:
  `population-members-at` genuinely consumes `$t` (P-PM-3, FRP1 ok); the recursive
  `canonical-entity-at` returns the epoch-correct identity closure off validity windows
  (P-CE-1); and the acceptance shape holds — `believed-at(t)` and
  `population-members-at(P, t)` agree as-of-t (P-T5-FIX).
- **Option B (present-state only) is silently wrong as-of-t.** P-BR-6 shows a present-state
  traversal (`cohort-blast-radius`) is window-BLIND: it sweeps a cohort belief B even for a
  member C1 whose window has *closed* (the materialized row still exists). Only a
  window-respecting entry (`population-members-at(P, t)`) discriminates — reaching B for
  t∈[t₀,t₁) and excluding it for t≥t₁. Without the window the blast-radius fix can ship
  present-state-correct and silently wrong as-of-t.

→ **§3.1 leans (A) as-of-t variants.** Existing no-`$t` functions remain the documented
present-state fast path.

## §3.4 blast-radius entry: b1/b2/b3 all VIABLE — spec author's design choice

The central type-inference risk **did not materialize**: TypeDB 3.12 accepts b1's
`redefine fun blast-radius($r: okf-concept)` with per-branch role narrowing across the
`memory-item`/`concept` siblings, and `blast-radius(C1: concept)` type-checks and returns
the cohort belief (P-BR-1). All three sub-options reach a working verdict:

- **b1 (widened param):** single entry point; **backward-compatible** (existing
  `memory-item` callers still type-check — `memory-item ⊂ okf-concept`); recursion through
  the scoped belief works (P-BR-2). Widens the public contract to any `okf-concept`
  (`concept` + all `memory-item` subtypes; `external-resource` is NOT an `okf-concept`, so
  it is not admitted). Requires a `redefine` migration (see the later P-REDEF probe for the
  production define-path posture).
- **b2 (companion):** additive `define`, no redefine, cleanest separation; the union with
  `blast-radius` is two host-side calls (P-BR-3).
- **b3 (resolution entry):** keeps the `memory-item` param; models "retract the *identity
  belief* that binds the member" rather than "retract the member concept" (P-BR-4). Also a
  `redefine`, so **mutually exclusive with b1**.

Because b1 and b3 both `redefine blast-radius`, they cannot coexist as written — but their
branches are compatible: a single `redefine fun blast-radius($r: okf-concept)` could carry
BOTH the `population-member` branch (b1, member-concept entry) AND the `resolves` branch
(b3, resolution-belief entry), since `okf-concept` admits `concept` *and* `resolution`
(`resolution sub belief sub memory-item sub okf-concept`). Offered as a synthesis for the
spec author; probed separately here per R9 fidelity.

---

## Per-probe detail (W3)

### P-BR-1 — b1 widened param type-checks (decides b1 vs b2)
- **Delta:** composed schema, then `redefine fun blast-radius($r: okf-concept)` (b1).
- **Ground state:** population P, member concept C1, belief B scoped to P (driven from C1).
- **Observed:** the `redefine` is **ACCEPTED** (no type-inference error). `blast-radius(C1)`
  where C1 `isa concept` returns `{br1/B}` — the widened `okf-concept` param admits a
  `concept` argument and the per-branch narrowing (`rests-on:assumption`/`derivation:source`
  → `memory-item`; `population-member:member` → `concept`) type-checks.
- **Verdict:** RESOLVED — **b1 is VIABLE.** The sibling-narrowing risk did not materialize.

### P-BR-2 — b1 recursion through the scoped belief
- **Delta:** b1. **Ground state:** C1 member of P; B scoped to P; D `rests-on` B.
- **Observed:** `blast-radius(C1)` = `{br2/B, br2/D}` — B via the direct population branch,
  D via recursion (the scoped belief B re-enters `blast-radius`, whose `rests-on` branch
  reaches D).
- **Verdict:** RESOLVED — recursion feeds back through the population branch correctly.

### P-BR-3 — b2 companion returns the cohort belief
- **Delta:** b2 (`define fun cohort-blast-radius($member: concept)`).
- **Observed:** `cohort-blast-radius(C1)` = `{br3/B}` on the T2 fixture.
- **Verdict:** CONFIRMED — the additive companion resolves T2 without widening `blast-radius`.

### P-BR-4 — b3 resolution-entry returns the cohort belief
- **Delta:** b3. **Ground state:** RES `resolves (record-a: C1, record-b: C2)`; C1 member of
  P; B scoped to P.
- **Observed:** `blast-radius(RES)` = `{br4/B}` — retracting the identity belief RES that
  binds C1 undermines the cohort belief B via `resolves`→`population-member`→`scoped`.
- **Verdict:** RESOLVED — b3's `resolves` entry branch works.
- **Note for spec author:** the branch as given (brief snippet) matches only
  `record-a: $c`; a production version should also match `record-b` (as `canonical-entity`
  does) so a member bound via either record fires the blast.

### P-BR-5 — redefine fun blast-radius accepted, body in force
- **Delta:** b3 applied as a `redefine` on a DB where `blast-radius` already exists.
- **Observed:** the schema transaction is **accepted**; the redefined body is genuinely in
  force — `blast-radius(RES)` reaches B via the new `resolves` branch, which the original
  (rests-on ∪ derivation only) body could never reach.
- **Verdict:** RESOLVED — `redefine fun` works and replaces the body (no silent old body).
  (The production define-path migration posture is the separate later probe P-REDEF.)

### P-BR-6 — windowed blast propagation (the T2×T5 seam)
- **Delta:** b2 + C4a + C4b. **Ground state:** member C1 with a CLOSED window
  [2026-01-01, 2026-04-01); belief B scoped to P, valid-from 2026-01-01.
- **Observed (both branches):**
  - Present-state `cohort-blast-radius(C1)` = `{br6/B}` — **window-blind**: the closed-window
    row still exists, so B is swept even though C1 has left the cohort (over-broad as-of-now).
  - Window-respecting `population-members-at(P, 2026-02-01)` contains C1 → windowed blast
    reaches B (t∈[t₀,t₁)); `population-members-at(P, 2026-05-01)` excludes C1 → windowed
    blast is empty (t≥t₁).
- **Verdict:** RESOLVED — the window-respecting entry gives the as-of-correct blast; the
  present-state entry is silently window-blind. **This is the concrete evidence for §3.1 → (A).**

### P-PM-1 — window owns additive, coexists with computed-at
- **Delta:** C4a. **Observed:** the delta defines clean; one `population-member` row carries
  `valid-from`, `valid-to` AND `computed-at` (each reads back once). Two meanings coexist:
  `computed-at` = cache-row freshness; `valid-from`/`valid-to` = when membership held.
- **Verdict:** CONFIRMED.

### P-PM-2 — two windowed epochs coexist
- **Delta:** C4a. **Ground state:** two `population-member` instances for the same (P, C1) —
  window [t₀,t₁) then a rejoin window from t₂.
- **Observed:** both rows coexist as distinct instances (2 `valid-from` rows;
  `valid-from` is `@card(0..1)`, so two rows ⇒ two instances). A left-and-rejoined member
  is representable as two windowed epoch rows.
- **Verdict:** RESOLVED — distinct windowed epochs coexist (no dedup collapse).

### P-PM-3 — population-members-at consumes $t (FRP1 ok)
- **Delta:** C4a + C4b. **Ground state:** C1 open-ended from t₀; C2 closed at t₁.
- **Observed:** `population-members-at(P, 2026-02-01)` = {C1, C2}; `(P, 2026-05-01)` = {C1}.
  Different answers for different `$t` prove the parameter is genuinely consumed; the
  relation-variable `has` + negation-over-relation-attrs body defines cleanly.
- **Verdict:** CONFIRMED.

### P-CE-1 — canonical-entity-at epoch closure
- **Delta:** C4a + C4b. **Ground state:** RES1 `resolves (C1, C2)` valid-from t₀; RES2
  `resolves (C2, C3)` valid-from a LATER t₂; each a provable (`type-join`) resolution.
- **Observed:** `canonical-entity-at(C1, 2026-03-01)` = `{C1, C2}` (only RES1 valid — C3
  absent); `canonical-entity-at(C1, 2026-08-01)` = `{C1, C2, C3}` (both epochs valid — C3
  enters via recursion). The closure is read off the resolution's validity **window**, never
  its (mutable) belief-state, mirroring `believed-at`.
- **Verdict:** RESOLVED — the recursive as-of closure is epoch-correct.
- **Note:** the symmetric `resolves` closure includes the input C1 itself (the
  `record-a`/`record-b` back-edge) — a pre-existing property of `canonical-entity`, not new
  to the as-of variant; the epoch-load-bearing fact is C3's absence at t₁ / presence at t₃.

### P-T5-FIX — believed-at and members-at agree as-of-t (the W3 acceptance shape)
- **Delta:** all W3 deltas. **Ground state:** B scoped to P, valid-from t₀; C1 a member from
  t₀; C2 joined today.
- **Observed:** `believed-at(2026-01-01)` = {B}; `population-members-at(P, 2026-01-01)` =
  {C1} (the t₀ cohort — C2 not yet a member); `population-members-at(P, 2026-07-11)` =
  {C1, C2} (today's full cohort). The as-of-t₀ belief view and membership view are mutually
  consistent — B is reconstructed against the t₀ cohort, not today's (the T5 bug: the
  time-blind `population-members(P)` = {C1, C2} at all times).
- **Verdict:** CONFIRMED — the fix is time-varying, so the t₀ agreement is a genuine
  as-of-t result, not a constant.

---

## Notes carried to the spec (W3)

1. **b1 type-checks — the sibling-narrowing risk did not materialize.** TypeDB 3.12 accepts
   a function param typed at a supertype (`okf-concept`) whose branches narrow it to
   incompatible subtypes (`memory-item` vs `concept`), and admits a `concept` argument.
   §3.4 is a design choice among three *working* shapes, not a forced fallback.
2. **§3.1 → (A) as-of-t variants** is empirically supported: windows are cheap, the as-of
   functions are expressible and correct, and present-state traversal is silently
   window-blind (P-BR-6). Keep the existing no-`$t` functions as the present-state fast path.
3. **The as-of semantics read off validity windows, never belief-state.** `canonical-entity-at`
   drops the `belief-state "asserted"` filter that `canonical-entity` carries and gates on
   `valid-from`/`valid-to` instead — belief-state mutates on supersession, so a resolution
   valid *at* t but superseded *by now* must still count as-of-t (mirrors `believed-at`).
4. **b1/b3 are mutually exclusive (both `redefine blast-radius`), but their branches
   compose** into one `redefine fun blast-radius($r: okf-concept)` carrying both the
   population-member and the `resolves` branch (§3.4 synthesis) — probed separately per R9.
5. **W3 requires `redefine fun` for b1/b3.** P-BR-5 proves the mechanism at the schema-tx
   level (body replaced, in force). The production define-path migration posture is owed to
   the later P-REDEF probe / SPEC-W2-W3 §1.4.
