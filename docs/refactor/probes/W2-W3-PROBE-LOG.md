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
