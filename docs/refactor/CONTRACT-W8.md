# CONTRACT-W8 — the consolidation ("dreaming") daemon contract

> **Status: `DRAFT`** — awaiting ratification. Only the user flips this line to
> `RATIFIED at commit <sha>`; this document does not self-ratify (`GLOBAL-CONSTRAINTS.md`).
> Gates: [`CONFLICT-REPORT.md`](./CONFLICT-REPORT.md) T8; [`INVARIANTS.md`](./INVARIANTS.md)
> inv 7/8, OP-1/3/5, mechanisms 10/14.
>
> This document CONSTRAINS a future implementation. It proposes no schema change and no code.
>
> The `ConsolidationDaemon` trait stays stubbed until this is ratified.

---

## §0 — Scope & posture

This contract **proposes constraints; it never overrides** `INVARIANTS.md` or
`CONFLICT-REPORT.md`. Both are LOCKED — cited below one-way, by stable anchor
(§1/§8 invariant numbers, `OP-` numbers, `T8`), never edited.

The daemon described in `core/mem-gecko/src/consolidation.rs` — the
`ConsolidationDaemon` trait's eleven job methods plus the real no-op `tick` and
`NoopConsolidationDaemon` — is, under this contract, **just another belief-writer
bound by the W5 monopoly** (`CONFLICT-REPORT.md` Headline; work items W5/W5a/W5b).
It gets no privileged internal path to the graph. Every constraint in §1 binds
the eventual implementation of the eleven stubbed jobs; none is satisfied by
anything shipped today (confirmed in §2/§2a below), and satisfying none of them
requires shipping anything today — that is the point of ratifying a contract
before the daemon is written (`CONFLICT-REPORT.md` T8: "the maximal
expensive-to-get-wrong-later case").

**Accuracy check performed against the live code** (see §2a footnote): `tick`
and `NoopConsolidationDaemon` are real, callable no-ops; all eleven job methods
(`dedup_content_hash`, `discover_typed_join_links`, `synthesize_links_llm`,
`contradiction_sweep`, `decay_archive_tombstone`, `summarize_archived_provenance`,
`ttl_prune_record_tier`, `rollup_high_volume_episodes`,
`collapse_superseded_provenance`, `precompute_blast_radius`,
`invalidate_blast_radius`) return `EpistemicError::NotYetImplemented` — a
graceful error, never a panic — from their default trait bodies.

---

## §1 — Constraint families (verbatim T8, spine discipline)

Each CF quotes `CONFLICT-REPORT.md` T8's numbered constraint **verbatim**, then
states the operative binding for this contract. Nothing here reinterprets T8;
each CF is the ratified sentence plus an annotation of what it means for the
trait in `consolidation.rs`.

### CF-1 — merge-similarity ⇏ derivation (the two-ledger firewall; inv 8 / OP-3)

> **Verbatim (`CONFLICT-REPORT.md` T8, constraint 1):** "Consolidation uses
> similarity to decide *what* to merge. If that similarity is recorded as, or
> influences, the merged belief's `derivation`, the substrate's own daemon
> commits the exact firewall violation domains are forbidden. **Constraint:**
> content-hash / type-join merges stamp a **provable** `derivation-method`;
> similarity-clustered merges are **probabilistic** (`llm-synthesis`-class),
> marked, and the similarity lives only on a retrieval/episodic edge
> (`surfaced`/`informs-synthesis`), **never** on the merge's `derivation`."

**Binding.** A merge the daemon proposes falls into exactly one of two classes,
never a blend:

- **Provable merges** (content-hash identity, type-enforced join) stamp a
  `derivation-method` of `type-join` / `cardinality` / `functional-dependency`
  — confidence 1.0 permitted (`INVARIANTS.md` §8.1). These carry no similarity
  score at all; identity is structural, not scored.
- **Similarity-clustered merges** stamp `llm-synthesis`, confidence < 1.0,
  marked. The similarity score that justified clustering the pair is recorded
  **only** on a `surfaced`/`informs-synthesis` edge (`mem_types.tql:270-278`),
  never as an attribute of, or input folded into, the merge's `derivation`.
  `derivation-chain()` (`mem_functions.tql:28-36`) must never be able to reach
  a similarity-scored edge from a daemon-synthesized belief — this is the same
  read-side proof `derivation_chain_never_touches_the_retrieval_ledger`
  (`core/mem-gecko/tests/writer_reader_integration.rs`, cited T1) already holds for
  the shipped read path; the daemon's write side must not be the thing that
  breaks it.

### CF-2 — retention: retrieval-provenance never severed; never-delete (D-4)

> **Verbatim (`CONFLICT-REPORT.md` T8, constraint 2):** "The retention rule
> forbids tombstoning a `retrieval-event` while its `informs-synthesis` belief
> is non-superseded. **Constraint:** the daemon's tombstone predicate must
> **exclude** `retrieval-event`s with a live `informs-synthesis` edge, or it
> severs the retrieval axis of live beliefs."

**Binding.** Every daemon job that removes, archives, or tombstones anything
(`decay_archive_tombstone`, `ttl_prune_record_tier`,
`rollup_high_volume_episodes`, `summarize_archived_provenance`,
`collapse_superseded_provenance`) is bound by two rules, not one:

- **Retrieval-provenance retention.** A `retrieval-event` is never tombstoned
  while a non-superseded belief's `informs-synthesis` edge points at it. The
  daemon's candidate set for tombstoning must be computed from (or be no
  broader than) `MemWriter::retrieval_events_safe_to_tombstone`
  (`writer.rs:479`) — the shipped read already encodes exactly this predicate.
- **Never-delete (`INVARIANTS.md` §6, D-4).** "Summarize" is not a euphemism
  for delete: `mem-gecko.md:115`'s delete-on-prune corpus text is
  ratified-superseded by the shipped model — `consolidation-state` only ever
  **advances forward** (`raw → candidate → consolidated → archived →
  tombstoned`, `mem_types.tql:9`) via supersession-with-lineage, never a row
  deletion. `summarize_archived_provenance` / `collapse_superseded_provenance`
  / `rollup_high_volume_episodes` may collapse *detail*, but the write-once
  `retrieval-provenance` flag (`mem_types.tql:101`) and the derivation/episode
  row itself persist. Both axes — derivation and retrieval — are retained;
  only presentation-detail is allowed to shrink.

### CF-3 — monopoly + entrenchment (writes ONLY through `EpistemicWriter`)

> **Verbatim (`CONFLICT-REPORT.md` T8, constraint 3):** "A daemon-synthesized
> belief is `llm`-class entrenchment (rank 0) → by the T3 guard it can **never
> supersede** a human/tool belief. **But the T3 guard lives in `MemWriter`, not
> the schema (Headline), and cyber already writes some belief-tier state raw**
> — so "the daemon has no privileged write path bypassing the entrenchment
> guard" is only enforceable if the daemon is *held to* the monopoly.
> **Constraint (make it enforceable, not aspirational):** the daemon must
> **write through `EpistemicWriter`, never raw `graph.write`** — every
> consolidation belief/merge/tombstone goes through the same run-stamped,
> entrenchment-guarded path. This closes the loop with the Headline: the
> daemon is exactly the kind of new belief-writer W5 must forbid from
> bypassing `MemWriter`, and its spec should say so in as many words. Confirm
> consolidation *proposes*, never *overrides*."

**Binding.**

- **No raw `graph.write`, ever.** Every daemon-authored belief, merge,
  tombstone, or anomaly goes through `EpistemicWriter`
  (`core/gecko-extension-api/src/epistemic.rs:192-235`) — the same seam every
  other belief-writer uses. The daemon gets no bypass, exactly as cyber's raw
  `technique-detection`/`alert-disposition` writes (`detect/mod.rs:142,196`)
  are the ratified counter-example of what NOT to do (`CONFLICT-REPORT.md`
  Headline, path #3).
- **Rank-0 entrenchment, no exceptions.** Every belief the daemon mints or
  supersedes carries `entrenchment = "llm"` (`tql.rs:32`), rank 0
  (`semantic.rs:84-92`). The daemon can never construct a belief with a higher
  entrenchment tier to route around the guard — `MemWriter::supersede`'s
  entrenchment check (`writer.rs:806-824`:
  `entrenchment_rank(new) < entrenchment_rank(old) ⇒ EntrenchmentViolation`)
  must reject every daemon-authored supersession of a `user-stated` / `axiom`
  / `tool-derived` belief. Consolidation **proposes; it never overrides.**
- **One host-minted `RunContext` per tick (OP-1).** All writes issued inside a
  single `tick` invocation share one `RunContext`, so a `ConsolidationReport`'s
  counts (`episodes_deduped`, `links_discovered`, `state_transitions`,
  `vectors_removed`) reconcile against writer-side accounting for that run —
  the same correlation discipline `assert_belief`'s same-run retrieval
  correlation already relies on (`writer.rs:773-790`). Because the daemon is
  host-side Rust (a scheduled job, not sandboxed guest code reached through
  `host_call_extension`), OP-1's "sandbox cannot forge" framing does not apply
  literally — but the discipline it protects (single, attributable,
  audit-linkable provenance per unit of work) does, and is what this clause
  requires.

> **SATISFIABILITY NOTE.** CF-3 is **ratifiable now** — it states a constraint
> on the eventual implementation, not a construct that must exist today. It is
> **only IMPLEMENTABLE once the sanctioned write surface exists**:
> belief-tier **relation** writes beyond the seven methods already on
> `EpistemicWriter` (`observe`/`assert_belief`/`supersede`/`contest`/
> `rests_on`/`record_prediction`/`resolve_prediction`) need W5b's proposed
> `BeliefEdgeDecl` seam (`CONFLICT-REPORT.md` W5b) — which does not exist yet
> (zero hits for `BeliefEdgeDecl` anywhere in the repo as of this writing) —
> and **lifecycle transitions** (`consolidation-state` flips for
> `decay_archive_tombstone`/`ttl_prune_record_tier`/`rollup_high_volume_episodes`)
> have **no writer method today** on the trait or on `MemWriter` (the one
> existing `consolidation-state` writer, `tombstone_episode_op`
> (`tql.rs:537`), is reachable only through `dedup_episodes`'s pairwise
> keeper/loser path, not a general lifecycle advance). Without stating this
> flag explicitly, "how does the daemon write an anomaly hub or flip a
> lifecycle state without raw TQL" surfaces as a blocker discovered
> mid-implementation rather than a known, named, pre-ratified dependency. See
> §2 for the full write-surface breakdown and §4 for the cross-reference to
> where each gap is owed.

*Reconciliation with §2:* the NOTE's "write an anomaly hub" is **illustrative**
of the raw-TQL temptation, not one of the actual owed gaps — anomaly-hub
minting already **EXISTS** via `EpistemicWriter::contest` (§2), so the daemon's
`contradiction_sweep` needs no new method. The two genuine
owed-new-sanctioned-method gaps are (i) recording similarity on a
`surfaced`/`informs-synthesis` edge for an offline pairwise scan and (ii) the
lifecycle `consolidation-state` transitions — both itemized in §2.

---

## §2 — Required write-surface inventory

Every write one of the eleven stubbed jobs needs, classified **exists** /
**owed-by-W5b** / **owed-new-sanctioned-method** / **not belief-tier (N/A)**.

| Write needed | Used by | Status | Evidence |
|---|---|---|---|
| Mint a belief + **provable** derivation (`type-join`/`cardinality`/`functional-dependency`, confidence 1.0 permitted) | `discover_typed_join_links` | **EXISTS** | `EpistemicWriter::assert_belief` (`epistemic.rs:197-203`) |
| Mint a belief + **probabilistic** derivation (`llm-synthesis`, confidence < 1.0, entrenchment `llm`/rank 0) | `synthesize_links_llm` | **EXISTS** (belief-mint half only — see next row) | `assert_belief` with `DerivationMethod::LlmSynthesis` |
| Record a similarity score on a `surfaced`/`informs-synthesis` edge for a pair found by an **offline batch scan** (not an interactive `recall`) | `synthesize_links_llm` (the CF-1 firewall half) | **OWED — new sanctioned method** | the two edges are written by two different private paths, neither reachable by an offline daemon: the `surfaced` edge is minted by the private `record_retrieval` (`writer.rs:493`), called **only** from `recall_indexed` (`writer.rs:614`); the `informs-synthesis` edge is minted **inside `assert_belief`** (`writer.rs:779`), gated on `record_retrieval_provenance` and correlated to the run's prior retrievals via `ctx.matched_retrievals` (`writer.rs:777`). Both are welded to the interactive recall→assert flow of a single run; **no public method mints either edge for a daemon-driven, non-recall pairwise comparison** |
| Tombstone a content-hash-identical episode + best-effort de-vector | `dedup_content_hash` | **EXISTS** | `MemWriter::dedup_episodes` (`writer.rs:442`) |
| Mint an anomaly hub over ≥2 claims | `contradiction_sweep` | **EXISTS** | `EpistemicWriter::contest` (`epistemic.rs:215`; impl `writer.rs:844`) |
| Read: which `retrieval-event`s are safe to tombstone | `ttl_prune_record_tier` | **EXISTS** (read only) | `MemWriter::retrieval_events_safe_to_tombstone` (`writer.rs:479`) |
| Advance `consolidation-state` (`raw→candidate→consolidated→archived→tombstoned`) for an **arbitrary** memory-item, outside a dedup pair | `decay_archive_tombstone`, `ttl_prune_record_tier` | **OWED — new sanctioned method** | the only shipped `consolidation-state` writer is `tombstone_episode_op` (`tql.rs:537`), reachable exclusively through `dedup_episodes`'s keeper/loser pair; nothing advances an item through the full lifecycle or tombstones a record-tier item standing alone |
| Collapse a belief's/episode's provenance detail into a summary while the write-once `retrieval-provenance` flag (`mem_types.tql:101`) and the row itself persist | `summarize_archived_provenance`, `collapse_superseded_provenance` | **OWED — new sanctioned method** | no summarization/collapse writer exists on `EpistemicWriter` or `MemWriter` today |
| Aggregate high-volume episodes into a rollup entity | `rollup_high_volume_episodes` | **OWED — new sanctioned method** | no rollup/aggregate writer exists today |
| Materialize / invalidate a `blast-radius` cache entry for an assumption | `precompute_blast_radius`, `invalidate_blast_radius` | **N/A — not a belief-tier write** | this is cache materialization over an already-derived computation (inv 8's "derived, rebuildable" pattern, the same posture as `SemanticIndex`), not a new belief — CF-3's `EpistemicWriter`-only clause governs belief-tier writes; the cache is out of that scope in the same way the index is, but still inherits invariant 8's failure-tolerance discipline (a cache-materialization failure must never fail the authoritative graph op) |
| Any **new belief-tier relation type** beyond the seven already on `EpistemicWriter` (`observe`/`assert_belief`/`supersede`/`contest`/`rests_on`/`record_prediction`/`resolve_prediction`) that a future daemon job might need | none of the eleven shipped jobs strictly requires one today; flagged for any job added later | **OWED-BY-W5b** | W5b's proposed `BeliefEdgeDecl` seam (`CONFLICT-REPORT.md` W5b) is the intended generic mechanism; it does not exist yet (confirmed: zero repo hits for `BeliefEdgeDecl`). Until it lands, a new relation type needs its own bespoke, individually-reviewed `EpistemicWriter` method — the same way `assert_belief`/`supersede`/`contest` were each added one at a time |

**Reading the table against CF-1/CF-2/CF-3:** the two rows marked **owed** for
CF-1 (similarity-on-surfaced) and the lifecycle/summarization rows for CF-2 are
precisely the SATISFIABILITY NOTE's two named gaps. Nothing in this table is a
proposal to build any of them — it is the inventory this contract requires
exist, in some sanctioned shape TBD, before the corresponding job can move past
`NotYetImplemented`.

### §2a — Method-binding table (every trait method → its CFs → clause)

Verified against the live `core/mem-gecko/src/consolidation.rs` — **no line
drift found**; every anchor below matches the brief exactly.

| Method | Binds | Clause |
|---|---|---|
| `tick` (`consolidation.rs:65`) | CF-3, OP-1 | one daemon `RunContext` per tick; `ConsolidationReport` counts reconcile with writer-side counts |
| `dedup_content_hash` (`consolidation.rs:76`) | CF-1, CF-3 | hash-identity is provable; loser tombstoned via `MemWriter::dedup_episodes`, never raw |
| `discover_typed_join_links` (`consolidation.rs:85`) | CF-1, CF-3 | `type-join`, confidence 1.0 permitted; through `assert_belief` |
| `synthesize_links_llm` (`consolidation.rs:95`) | CF-1, CF-3 | `llm-synthesis`, confidence < 1.0, marked; similarity never on the merge's `derivation`; rank-0 ⇒ never supersedes |
| `contradiction_sweep` (`consolidation.rs:103`) | CF-3 | mints anomaly hubs via `contest()`; proposes, never resolves |
| `decay_archive_tombstone` (`consolidation.rs:112`) | CF-2, inv 8, CF-3 | tombstone predicate excludes live-`informs-synthesis` retrieval-events; index remove best-effort, never fails the graph op |
| `summarize_archived_provenance` (`consolidation.rs:120`) / `collapse_superseded_provenance` (`consolidation.rs:150`) | CF-2, D-4 | both axes retained; write-once `retrieval-provenance` flag persists |
| `ttl_prune_record_tier` (`consolidation.rs:133`) | CF-2 | must call `retrieval_events_safe_to_tombstone` |
| `rollup_high_volume_episodes` (`consolidation.rs:141`) | CF-2 | aggregates retain the episodic axis |
| `precompute_blast_radius` (`consolidation.rs:158`) / `invalidate_blast_radius` (`consolidation.rs:166`) | inv 8, W3 | cache derived, graph SoR; invalidation triggers include membership-epoch changes once W3 ships |

**Anchor-verification footnote.** All twelve line numbers above (the real
no-op `tick` plus the eleven stubbed job methods) were re-derived by `grep -n`
against `core/mem-gecko/src/consolidation.rs` on branch `conflict_resolution`
at the time of writing and match the task brief's anchors exactly (`tick:65`,
`dedup_content_hash:76`,
`discover_typed_join_links:85`, `synthesize_links_llm:95`,
`contradiction_sweep:103`, `decay_archive_tombstone:112`,
`summarize_archived_provenance:120`, `ttl_prune_record_tier:133`,
`rollup_high_volume_episodes:141`, `collapse_superseded_provenance:150`,
`precompute_blast_radius:158`, `invalidate_blast_radius:166`) — no correction
was needed. `MemWriter::dedup_episodes` (`writer.rs:442`) and
`MemWriter::retrieval_events_safe_to_tombstone` (`writer.rs:479`) likewise
match exactly. The entrenchment guard (`writer.rs:806-824`) matches exactly —
the comment block starts at `writer.rs:806` and the guarded `return Err(...)`
closes at `writer.rs:824`.

---

## §3 — Executable-checks-owed (future test per CF, named now)

None of these run today (T8: "none runnable today (stub)"). Each is named so
the implementation that finally builds a job cannot land without also landing
its check.

| # | CF | Test (owed, name reserved) | What it must assert |
|---|---|---|---|
| 1 | CF-1 | `daemon_provable_merge_stamps_provable_derivation` | a `dedup_content_hash`/`discover_typed_join_links` merge's `derivation-method` ∈ {`type-join`, `cardinality`, `functional-dependency`}; confidence may be 1.0 |
| 2 | CF-1 | `daemon_similarity_merge_never_touches_derivation` | a `synthesize_links_llm` merge's `derivation-method` is `llm-synthesis`, confidence < 1.0, AND the similarity score used to cluster the pair appears only on a `surfaced`/`informs-synthesis` edge — `derivation_chain()` over the merge belief never reaches a similarity-scored edge (extends the shipped read-side proof `derivation_chain_never_touches_the_retrieval_ledger` to a daemon-authored belief) |
| 3 | CF-2 | `daemon_never_tombstones_live_informs_synthesis_retrieval_event` | after `decay_archive_tombstone`/`ttl_prune_record_tier` runs, every `retrieval-event` with a non-superseded `informs-synthesis` belief is still present (not `tombstoned`) — the daemon's tombstone set is a subset of `retrieval_events_safe_to_tombstone`'s output |
| 4 | CF-2 | `daemon_summarize_retains_both_axes` | after `summarize_archived_provenance`/`collapse_superseded_provenance`/`rollup_high_volume_episodes`, the belief/episode row still exists (never deleted) and its write-once `retrieval-provenance` flag is unchanged |
| 5 | CF-3 | `daemon_is_enumerated_by_the_monopoly_test` | once W5a's belief-tier monopoly test lands (`CONFLICT-REPORT.md` W5a), a daemon tick's writes are included in the population it accounts for — the daemon is a counted caller, not a carve-out |
| 6 | CF-3 | `daemon_belief_never_supersedes_higher_entrenchment` | every belief the daemon mints or supersedes carries `entrenchment = "llm"` (rank 0); a daemon `supersede` of a `user-stated`/`axiom`/`tool-derived` belief returns `EntrenchmentViolation` (`writer.rs:806-824`) |
| 7 | CF-3 | `daemon_tick_writes_share_one_run_context` | every write issued inside one `tick` call carries the same host-minted `run_id` (OP-1) |

---

## §4 — Cross-references

### To SPEC-W5 (the monopoly harness)

`docs/refactor/SPEC-W5.md` does not exist yet at the time of writing this
contract (it is sequenced to be **ratified after W2+W3**, per the
spec-production plan). This is expected — the dependency is flagged here, not
resolved by borrowing its content.

`docs/refactor/SPEC-W2-W3.md` (already drafted) forward-cites the same not-yet-written
document at **§3.6**: "§2.2's co-mint procedural residue and §3.5's writer
allowlist are *enforced* by **SPEC-W5 §3.2** / W5a-W5b; SPEC-W2-W3 only states
the obligations." This contract makes the identical forward-reference for the
same reason: **SPEC-W5 §3.2** is where the daemon's writes must be folded into
W5a's asserted belief-tier population once SPEC-W5 lands. Re-confirm this
anchor when SPEC-W5 is actually drafted — §3.2 is a forward-guess inherited
from SPEC-W2-W3, not a citation into existing text.

When implemented, the daemon "joins the monopoly harness" in exactly the sense
any other belief-writer does: the fix for the Headline's finding is one
**committed test asserting no non-`MemWriter` path writes belief-tier state**
(work item W5a), gated on W5b landing a sanctioned relation-writer seam first.
The daemon must be an enumerated caller inside that assertion, never a
carve-out — this is CF-3 / T8 constraint 3's "closes the loop with the
Headline," stated here in as many words.

### To SPEC-W2-W3 (blast-radius traversal + membership epochs)

- **SPEC-W2-W3 §3.4** ("Blast-radius traversal extension") — `precompute_blast_radius`/
  `invalidate_blast_radius` must track whatever traversal set W3 lands (the
  extended `scoped`→`population`→`population-member` walk fixing T2), not only
  today's `rests-on ∪ derivation` (`mem_functions.tql:64-78`). If W3 adds
  membership-epoch windows (**SPEC-W2-W3 §3.2**, `population-member`
  bitemporal window), a membership-epoch change becomes an invalidation
  trigger the cache must react to, not only direct `rests-on`/`derivation`
  writes.
- **SPEC-W2-W3 §3.5** ("Populations-writer contract clauses") already names
  this contract's daemon explicitly, in its own §3.6: "**W8 (consolidation
  daemon).** The daemon precomputes/invalidates population-member rows at
  scale; its contract (W8, CONFLICT-REPORT T8) must respect §3.2's window and
  §3.5's close-on-supersession clause, and write through `EpistemicWriter`
  (never raw `graph.write`)." This contract reciprocates: CF-3's
  "`EpistemicWriter`, never raw `graph.write`" clause is the same obligation
  SPEC-W2-W3 §3.6 already anticipates for the daemon — the two documents agree
  without either amending the other.

---

## §5 — What this contract does NOT decide

Explicitly out of scope — implementation freedom, not silence:

- **Scheduling and cadence** of `tick` (cron, continuous, triggered-on-write).
- **Batching strategy** within a tick (per-pair vs whole-graph passes; how much
  of the graph one `tick` covers).
- **Ordering among the eleven jobs** (e.g. whether `dedup_content_hash` must run
  before `discover_typed_join_links` in the same tick).
- **The concrete shape** of the owed writer methods identified in §2 (the
  gap is named; the method signature, its schema-fragment implications if
  any, and whether it lives on `EpistemicWriter` vs a daemon-scoped extension
  point are future decisions, made when W5b/the owed methods are actually
  built).
- **The `BeliefEdgeDecl` seam's design** — that is W5b's / SPEC-W5's decision,
  not this contract's.
- **Whether `precompute_blast_radius`/`invalidate_blast_radius` remain daemon
  jobs at all**, versus being split into a separate cache-maintenance
  mechanism analogous to `SemanticIndex`.
- **Performance/SLA** of a tick, or of any individual job.
- **Whether `NoopConsolidationDaemon` is ever replaced by a different default**
  versus staying the shipped default with a real daemon wired in as an
  opt-in extension.

---

## §6 — Ratification record

- **Status:** `DRAFT`. Not ratified. Ratification is the user's act
  (`GLOBAL-CONSTRAINTS.md`); this contract does not flip its own status.
- **Gates:** `CONFLICT-REPORT.md` T8 (all three constraints, quoted verbatim
  in §1); `INVARIANTS.md` inv 7 (entrenchment/provenance grading, §8.1), inv 8
  (semantic index / two-ledger firewall), OP-1 (host-stamped provenance),
  OP-3 (two-ledger firewall write-side), OP-5 (entrenchment monotonicity);
  `INVARIANTS.md` §4 mechanisms 10 (retrieval-provenance) and 14
  (consolidation/"dreaming").
- **Proposes no schema change:** confirmed — no `.tql` file is touched by this
  document or its authoring.
- **Proposes no code:** confirmed — no `.rs` file is touched;
  `core/mem-gecko/src/consolidation.rs` stays exactly as read in §2a, all eleven
  jobs still `NotYetImplemented`.
- **Verbatim + delta discipline:** each of CF-1/CF-2/CF-3 quotes
  `CONFLICT-REPORT.md` T8's numbered constraint verbatim, then states only an
  operative binding — no clause reinterprets or narrows the ratified T8 text.
- **Ratifiability checklist** (`GLOBAL-CONSTRAINTS.md`, adapted — this task
  carries no probes): every constraint traces to a decision already ratified
  in T8/inv 7/inv 8/OP-1/OP-3/OP-5 (no open design question is left
  unanswered by this contract — T8 made the decisions; this contract
  operationalizes them against the real trait shape); the CF-3
  SATISFIABILITY NOTE states the implementation dependency on W5b + the owed
  lifecycle/summarization writer methods explicitly, rather than presenting
  the daemon as independently buildable today.
- **On ratification:** replace the status line above with
  `RATIFIED at commit <sha>`, citing the commit that lands this file (per
  `INVARIANTS.md`'s own status-blockquote convention). No other edit to this
  document is implied by ratification — ratifying locks the constraints, it
  does not change them.
