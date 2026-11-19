# OWED-WORK-LEDGER — named-but-unbuilt work across W2 / W3 / W5 / W8

> **Status: living working-surface** (not a ratified spec — it tracks, it does not decide).
> Consolidates every construct, method, test, and decision that the four ratified
> conflict-resolution documents name but leave unbuilt. The source of truth for each item is
> its owning doc's cited section; this ledger is the index, kept in sync as items land.
> **Ratification provenance:** SPEC-W2-W3 decisions @`11d7ab1` (commit `3a10d7f`); SPEC-W5
> as-design @`95f0d6e` (commit `f132214`); CONTRACT-W8 as-contract, self-anchored (commit
> `fdd1dc6`).

**Two keystones gate almost everything downstream:**

- **① W5b `BeliefEdgeDecl` seam** — unblocks every sanctioned belief-edge/relation write (cyber
  migration, the divergence-exception co-mint writer, all of W8's owed writer methods, the
  `sighting-of` writer, and W5a going green).
- **② The W2/W3 schema fold** — **✅ LANDED @`319f808`**. The C1 constructs, the
  `population-member` window, b1 `blast-radius`, and the two as-of fns are folded onto
  `conflict_resolution` byte-identical to the P-COMP-green shape (159 serviceless tests green;
  live Tier-2 apply proven by P-COMP). This unblocks the W2/W3 writers (#8–#10), the disposition
  engine (#10), and the windowed-blast fn (#6).

## Ledger A — constructs & methods (named, unbuilt)

| # | Item | Owning doc | Type | Blocked on |
|---|---|---|---|---|
| 1 | `BeliefEdgeDecl` registry / sanctioned relation-writer seam | SPEC-W5 §3.2 | writer + registry | — (**keystone ①**) |
| 2 | C1 divergence constructs (`divergence-exception` entity, `divergence-precedence` relation + 4 roles) | SPEC-W2-W3 §2.1/§2.2 | schema | ✅ **LANDED** @`319f808` |
| 3 | `population-member` bitemporal window (`valid-from`/`valid-to`) | SPEC-W2-W3 §3.2 | schema | ✅ **LANDED** @`319f808` |
| 4 | `blast-radius` b1 redefine (widened `okf-concept` param) | SPEC-W2-W3 §3.4 | fn (TQL) | ✅ **LANDED** @`319f808` (fresh-init; #11 redefine still owed for already-initialized DBs) |
| 5 | `population-members-at` / `canonical-entity-at` as-of fns | SPEC-W2-W3 §3.3 | fn (TQL) | ✅ **LANDED** @`319f808` |
| 6 | **Windowed-blast traversal fn** (gates `population-member` branch on the window) | SPEC-W2-W3 §3.4/§5 **OWED** | fn (TQL) | window (#3) + b1 (#4); proven only compositionally (P-BR-6) |
| 7 | **b3 synthesis** (fold `resolves` branch into b1; match record-a **and** record-b) | SPEC-W2-W3 §3.4/§5 **OWED-opt** | fn (TQL) | needs own green probe (R9) + recursion decision (#24) |
| 8 | resolution / divergence-exception writer (co-mint belief+relation) | SPEC-W2-W3 §0/§2.2 | writer | seam ① + C1 (#2) |
| 9 | populations writer (materialize/invalidate rows, maintain window, close-on-supersession) | SPEC-W2-W3 §3.5 | writer | ✅ **LANDED** — `MemWriter::{define_population, materialize_population, close_population_membership}` (pure planner `mem_gecko::populations::plan`, Tier-1 + Tier-2 tested). Caller supplies the evaluated member set; the writer never executes `spec-text`. Not seam-gated: `population-member` is cache-tier, off the belief-edge surface (SPEC-W5 §3.4). Present-state `population-members` now excludes closed rows — fresh-init only; an already-initialized DB needs #11. **Still owed:** the W5a allowlist test that asserts it is the sole writer, and any caller (daemon / spec evaluator) |
| 10 | host disposition engine (4-key ranker → `resolved`\|`contested`) | SPEC-W2-W3 §2.3 | host-engine | **ranker ✅ LANDED @`3dbb393`** (`mem_gecko::disposition::rank` + `DerivationMethod::rank`, pure, Tier-1 tested); **writer half still owed** — acting on `Contested` (anomaly-hub mint, P-DISP-3's owed assertion) needs seam ① |
| 11 | `redefine`-emitting migration path (or documented drop-and-recreate) | SPEC-W2-W3 §1.4 | migration | — (precondition for #4 on an initialized DB) |
| 12 | cyber migration: `technique-detection`/`alert-disposition` → seam (drop raw `graph.write`) | SPEC-W5 §3.3 | writer (migration) | seam ① |
| 13 | `divergence-precedence` co-mint **enforcement** (seam refuses anchor-less write) | SPEC-W5 §3.2 | writer (seam) | seam ① + C1 (#2) |
| 14 | cyber-gecko episodic-**sighting** writer (`sighting-of` through seam) | SPEC-W5 §3.3/§3.4 | writer | seam ① + a future cyber sighting-ingestion feature |
| 15 | similarity-on-`surfaced`/`informs-synthesis` writer for **offline** batch scan | CONTRACT-W8 §2 | writer (new) | new sanctioned method |
| 16 | lifecycle `consolidation-state` transition writer (arbitrary item advance/tombstone) | CONTRACT-W8 §2 | writer (new) | new sanctioned method |
| 17 | provenance summarize/collapse writer | CONTRACT-W8 §2 | writer (new) | new method + collapse-granularity decision (#22) |
| 18 | high-volume episode rollup writer | CONTRACT-W8 §2 | writer (new) | new sanctioned method |
| 19 | blast-radius cache materialize/invalidate (`precompute`/`invalidate_blast_radius`) | CONTRACT-W8 §2a/§4 | cache | windowed-blast fn (#6) |
| 20 | the eleven daemon jobs + `tick` (all `NotYetImplemented`) | CONTRACT-W8 §2a | host code | #15–#19 + seam ① |

## Ledger B — owed tests / landing-gates

| Test | Owning doc | Blocked on |
|---|---|---|
| `no_raw_belief_tier_tql_outside_mem_tql` (W5a static lint) | SPEC-W5 §5.1 | **buildable now; complete only after fold ②** — its introspected surface is short `divergence-precedence` until ② lands C1 (23 → 24 relations) |
| `memwriter_is_the_sole_belief_tier_writer` (W5a dynamic) | SPEC-W5 §5.2/§5.8 | RED until cyber migration (#12) |
| `hostile_extension_raw_belief_write_is_flagged` (entity **+ relation**) | SPEC-W5 §5.3 | the rig |
| `belief_edge_decl_registry_writes_run_stamped` | SPEC-W5 §5.5 | seam ① |
| `divergence_precedence_write_requires_cominted_anchor` | SPEC-W5 §5.6 | #13 |
| `cyber_detect_test` stays green across migration | SPEC-W5 §5.7 | #12 |
| W2/W3 §5 graduated acceptance suite (P-PREC-*, P-PM-*, P-BR-1/2, P-CE-1, P-T5-FIX, P-REDEF, P-DISP-*) | SPEC-W2-W3 §5 | ✅ **LANDED @`09467b9`** — 19 acceptance tests run by default / CI's TypeDB job; 5 probes stay `#[ignore]` (non-shipped b2/b3/C2/windowed). Writer-level assertions (co-mint, contested-hub) still owed with #8/#10/#13 (the three partially-graduated tests carry a `Graduation scope (owed half)` docstring). Shipped tests now carry §5's graduated names (reconciled — Ledger C **RESOLVED**); the still-`#[ignore]` probes, the baselines, and the W5 probe keep their probe names |
| CONTRACT-W8 §3 seven checks (rows 1–7, incl. `daemon_is_enumerated_by_the_monopoly_test`) | CONTRACT-W8 §3 | each with its job (#20); row 5 also needs W5a |

## Ledger C — owed decisions (not constructs)

| Decision | Owning doc | Depends on |
|---|---|---|
| C2 `out-entrenches` / OP-5 schema-hardening (ship a schema guard?) | SPEC-W2-W3 §2.3 (conditional) + SPEC-W5 §2 (trigger fired; INVARIANTS §8.3) | a deliberate OP-5 decision — if yes, owes a 5×5 drift-lock test |
| collapse-granularity (summarizable detail vs load-bearing provenance) | CONTRACT-W8 §5/CF-2 | the summarize/collapse method design (#17) |
| whether blast-radius jobs stay daemon jobs vs a `SemanticIndex`-style mechanism | CONTRACT-W8 §5 | — |
| b3 recursion (should a resolution-bound member recurse into downstream cohort beliefs?) | SPEC-W2-W3 §3.4 | part of #7 |
| graduated-test-name reconciliation | SPEC-W2-W3 §5 | ✅ **RESOLVED (this commit)** — decision: rename the artifact to match ratified §5 (moving code to spec, not reinterpreting spec). Shipped tests renamed to §5's graduated names, code-side only (§5 doc untouched). Three carried deviations: (a) the folded §5 rows are **kept split** with distinct-word single-underscore tails (snake-case-clean, no lint waiver) — P-PREC-1/3 as `…_plays_contradiction_claim_defines` / `…_plays_contradiction_claim_holds`, P-BR-1/2 as `…_widens_param_and_reaches_cohort` / `…_widens_param_and_reaches_cohort_recursive`; (b) **P-DISP-3 is the exception** — kept `host_side_ranking_reproduces_dispositions`, NOT §5's `empty_winner_yields_contested_anomaly_hub_no_exception_minted`, which asserts the hub-minting half owed to #10 (parked in-docstring until #10 lands); (c) baselines, the W5 probe (`p_w5_1`), and the still-`#[ignore]` probes retain probe names (no adoptable §5 name / not yet graduated). No assertion changed |

## Implementation notes (empirical, TypeDB 3.12)

Findings from probing the live 3.12 engine; recorded so they are not re-derived or re-litigated when the writers land.

- **`[REP1]` value-into-attribute — affects the disposition/score writers (#9, #10).** Binding a
  function- or `reduce`-returned *value* straight into an attribute slot — `insert $x has attr $v`
  where `$v` came from `let $v in <fn>` or a `reduce` — trips `[REP1] The variable '…' cannot be
  declared as both a 'Attribute' and as a 'Value'` at query compile. The writers that materialize a
  computed disposition/score must bind an explicit attribute instance, not pass the bare value var
  into `has`. (Surfaced while empirically confirming the item below.)
- **`let $x in <single-valued fn>` coercion — validates `select-for-context`.** TypeDB 3.12 coerces a
  `return first` scalar (e.g. `retrieval-score -> double`) into a one-row stream and *yields the row*
  (confirmed on 3.12 with a positive insert-count probe + a zero-row negative control; not the
  duration-comparison silent-empty failure mode). So `select-for-context`'s
  `let $score in retrieval-score($b, $now)` is well-formed — each gated belief yields exactly one
  score. Settled; not to be re-litigated.

## Zero-blocker starting points & chosen order

Three fronts have no blocker today:

- **① W5b `BeliefEdgeDecl` seam** (keystone; unblocks all sanctioned belief-edge writes).
- **② W2/W3 schema fold onto `conflict_resolution`** (pure make-real, no design risk; unblocks all of W2/W3).
- **cyber-gecko C1/C2** (separate domain track; see the parallel-front note below).

**Chosen: build ② first** — **✅ schema fold LANDED @`319f808`**; the §5 acceptance suite it triggered is **graduated & green @`09467b9`** (the rig is re-synced — 19 acceptance gates + 5 owed/rejected probes). Next fronts: the **disposition engine (#10)** — **ranker ✅ LANDED @`3dbb393`**, writer half owed to seam ① — and **populations writer (#9)** — the self-contained W2/W3 writers to take first (the co-mint writer #8 / enforcement #13 want the seam ①); then windowed-blast fn (#6). ① the seam and cyber-gecko C1/C2 remain the parallel tracks.

## Parallel front — cyber-gecko C1/C2/C3 (separate domain track)

Cyber-gecko's own **C1/C2/C3** roadmap is a distinct, **mostly-unblocked** front, **not owed by W2/W3/W5/W8** and not gated on either keystone. The **only** touchpoint with this ledger is the `sighting-of` writer (**#14**): if/when cyber-gecko builds sighting ingestion, that writer must route through the W5b seam (①). Recorded here so the ledger's scope boundary does not hide that a whole parallel cyber front runs alongside the conflict-resolution work.

## Out-of-ledger

Excluded from the spec plan, **not owed by these four items**: W1 `attested()`, W4 (scope inv 3 to activation), W6 (reconciled design doc), W7 (split OP-6), W-T6 (IOC-convergence test).
