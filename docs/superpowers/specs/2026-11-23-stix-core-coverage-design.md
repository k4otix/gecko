# S3: STIX core coverage — design

Owed-work ledger item **S3**, settling decision **D-M**.

## Intent

The ATT&CK import keeps two kinds of structure only as untyped concept-links or text
attributes:
- a technique's sub-techniques;
- a retired object's replacement.

It also keeps a technique's tactics, only as phase-name strings. Detection reporting
can't use any of these.

S3 makes the three relations queryable as typed relations and puts them to use where
they matter:
- the coverage-gap list stops listing retired techniques, and shows a parent's
  sub-technique coverage;
- a claim on a retired technique names its live replacement.

## Decision D-M: ATT&CK-keyed, add only what has a consumer (option A)

**Considered:**
- **A. Keep the ATT&CK-keyed model.** Add typed `subtechnique-of`, `revoked-by` and a
  `tactic` type with a technique→tactic relation.
- **B. Align with typedb-cti's STIX 2.1 model.** Add entities for kill-chain phases
  and external references; add typed identity, marking, matrix and collection, with
  `created-by` and `object-marking` edges.

**Chosen: A** (the user's lean, endorsed by an independent Fable review). On
Enterprise v20.1, B adds about 35k reference entities and about 53k marking/creator
edges that no reader uses. It would also commit S5's design for sharing objects across
imports early. A is small, and every addition has a reader.

**Non-goals:**
- **Kept as they are:** the matrix, collection, identity, marking definition and
  external references (beyond the ATT&CK id already extracted). They stay bare
  `cyber-object`s, or stay in the raw `stix-object` metadata.
- **Left to S5:** sharing objects across imports.
- **Deferred to follow-ons:** rolling `blinded` up to the parent technique, and
  grouping any report by tactic. Every live technique has its own detection strategy,
  so the roll-up would only change presentation.

**Anti-goals:**
- A claim is never moved to a replacement automatically. 119 of the 149 technique
  revocations replace a technique with a sub-technique, so the claim's scope would
  change. The replacement is a suggestion only.
- A parent's claim never silently covers its sub-techniques, and the reverse never
  happens either.

## Facts it rests on (Enterprise ATT&CK v20.1)

- **subtechnique-of:**
  - 481 relationships, none of them multi-parent.
  - 2 sources are retired.
  - 16 retired sub-techniques have no `subtechnique-of` edge.
  - Every live sub-technique has exactly one live parent.
  - No live sub-technique's tactics differ from its parent's.
- **revoked-by:**
  - 157 relationships, none multi-target, no cycles; the deepest chain is 2.
  - 3 targets are themselves revoked.
  - The types do not always match: one is malware → tool.
  - Up to 5 revoked techniques share one replacement.
- **Deprecated but not revoked:** 289 objects, 12 of them techniques. None has a
  `revoked-by` edge.
- **Retired techniques and their ids:** all 161 still carry their `mitre-attack` id in
  the JSON. The id is not unique among retired objects: 129 retired techniques share
  one with a retired course-of-action.
- **Tactics:** 15 `x-mitre-tactic`s, each with an `x_mitre_shortname` and
  `x_mitre_domains: ["enterprise-attack"]`.
  - All 1,099 technique kill-chain phases have `kill_chain_name: "mitre-attack"` and
    match a tactic shortname.
  - 145 live techniques have more than one tactic.
- **A cross-domain risk:** Mobile and ICS reuse shortnames such as `persistence` under
  the kill-chain names `mitre-mobile-attack` and `mitre-ics-attack`. This was not
  checked against those bundles.

## Design

### Schema (`cyber_schema.tql`)

- **`attribute retired-attack-id value string`**, owned by `cyber-object` as
  `@card(0..1)` and not unique. A retired (revoked or deprecated) object carries its
  ATT&CK id here, in place of the unique `attack-id`.
- **`attribute tactic-shortname value string`.**
- **`entity tactic sub cyber-object`**, owning `tactic-shortname @card(1)` and
  playing `technique-tactic:tactic`.
- **`relation subtechnique-of`**, with roles `subtechnique` and `parent`.
  - Both roles are played by `attack-pattern`.
  - The sub-technique plays its role at most once (`plays … @card(0..1)`), if TypeDB
    3.15 accepts that form; otherwise the adapter enforces the limit. A probe decides.
- **`relation revoked-by`**, with roles `revoked` and `replacement`.
  - Both roles are played by `attack-pattern`, `intrusion-set`, `malware` and `tool`.
  - The revoked object plays its role at most once, by the same rule.
- **`relation technique-tactic`**, with roles `technique` (`attack-pattern`) and
  `tactic` (`tactic`).
- **Each new type and role gets a greenfield `@doc`.**
  - `kill-chain-phase` stays as it is.
  - The new types are additive, so no migration is needed.
  - The changed function bodies reach existing databases through the usual
    function redefine.

### Adapter (`stix/mod.rs`)

- **New subtype mapping:** `x-mitre-tactic` → `tactic`, with
  `tactic-shortname = x_mitre_shortname`.
- **The retired id:** a retired object's `mitre-attack` external id is written as
  `retired-attack-id`.
- **The two relationships become typed:** `subtechnique-of` and `revoked-by` are added
  to `REL_SHAPES`, each with its player lists. A pair whose types the shape doesn't
  admit stays a concept-link, as today.
- **Technique→tactic is resolved in the adapter, within the bundle.**
  - The bundle's tactics are indexed by (kill-chain name, shortname). The kill-chain
    name comes from the tactic's `x_mitre_domains`, through a fixed map:
    - `enterprise-attack` → `mitre-attack`
    - `mobile-attack` → `mitre-mobile-attack`
    - `ics-attack` → `mitre-ics-attack`
  - Each `kill_chain_phases` entry whose `kill_chain_name` is in that map's range must
    resolve to an in-bundle tactic, or `to_okf` errors and names the technique and the
    phase.
  - Phases of any other kill chain, and every phase in a bundle with no tactics (small
    fixtures, non-ATT&CK STIX), are kept only as the `kill-chain-phase` attribute.
- **At most one edge per object:** an object with two `subtechnique-of` parents, or two
  `revoked-by` targets, makes `to_okf` error.
- **Replaced, not appended, on re-import.**
  - All three relations are rewritten like `strategy-detects`: delete each such
    relation whose source is an in-bundle attack-pattern (for `revoked-by`, any
    in-bundle attack-pattern, intrusion-set, malware or tool), then insert the current set.
  - A re-parented or newly revoked object never keeps a stale edge.
  - The orphan sweep covers the three new relations.
- **The mapping version moves from `"2"` to `"3"`,** so every concept refreshes. A
  refresh deletes a concept's outgoing concept-links, so the old untyped
  `subtechnique-of`/`revoked-by` links go away. A test pins this.

### Functions (`cyber_functions.tql`)

- **`undetected-techniques()`** skips retired techniques, using the same liveness
  pattern as the other functions.

### Detection (`detect/mod.rs`)

- **`coverage_gaps`** lists each live technique that has no asserted claim, as today.
  - A listed technique that has live sub-techniques also carries
    `"sub_techniques": { "claimed": n, "total": m }`, the count of its live
    sub-techniques that have an asserted claim.
  - A parent is listed even when all its sub-techniques are claimed. A claimed parent
    is not listed, and doesn't remove its unclaimed sub-techniques from the list.
- **`claim`, when the technique id matches no live technique,** searches
  `retired-attack-id` on attack-patterns.
  - **Revoked:** the error names the replacement, found by following `revoked-by` with
    a visited set to the first live object; for example, "technique 'T1086' is
    revoked; ATT&CK replaces it with 'T1059.001'; claim that technique instead".
  - **Deprecated, or revoked with no live end:** "… is deprecated (or revoked) with no
    replacement".
  - **Otherwise:** the current "no technique" error.
- **`blinded`:** a claim whose technique is retired already reports every platform as
  unassessed. Its row now also carries:
  - `retired_technique`, the `retired-attack-id`;
  - `replaced_by`, the live replacement's `attack-id`, or null.
- **`recommend_sources`** is unchanged.

## Verification

- **Unit tests in `stix/mod.rs`:**
  - tactic resolution, including an unknown phase that errors and a foreign kill chain
    that is skipped;
  - the multi-parent and multi-replacement refusals;
  - `revoked-by` across types;
  - the retired id.
- **Integration tests (`cyber_stix_sync_test.rs`, `cyber_detect_test.rs`):**
  - the typed relations are written;
  - a re-import that re-parents or un-revokes replaces the edge;
  - the old concept-links go away on upgrade from mapping `"2"`;
  - `coverage_gaps` liveness and sub-technique counts;
  - `claim` successor errors, including a chain of 2 and a deprecated technique;
  - the `blinded` replacement fields.
- **The opt-in S4 run on v20.1:**
  - the three new relation counts are stable across a re-sync;
  - the existing counts are unchanged;
  - `blinded` and `recommend_sources` give the same results.

## Left to the builder

- Exact names of the new roles and errors, within this design.
- Whether "at most one" lives in the schema or in the adapter, decided by the probe.
