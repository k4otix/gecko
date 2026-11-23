# STIX Core Coverage (S3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Import ATT&CK's sub-technique, revocation and tactic structure as typed relations. Then use them so that:
- `coverage_gaps` skips retired techniques and shows a parent's sub-technique coverage;
- `claim` and `blinded` name a retired technique's live replacement.

**Architecture:**
- **The adapter, `to_okf`:**
  - adds a `tactic` subtype and a `retired-attack-id`;
  - adds `subtechnique-of` and `revoked-by` to `REL_SHAPES`;
  - resolves each technique's tactics within the bundle.
- **`cyber_post_sync`** replaces the three relations on each import, as it does
  `strategy-detects`.
- **The detection layer** reads the new relations.

**Tech Stack:** Rust, TypeDB 3.15 (TypeQL), serde_json, tokio integration tests.

**Spec:** `docs/superpowers/specs/2026-11-23-stix-core-coverage-design.md`

## Global Constraints

- **TypeQL:**
  - All TQL is parameterized. Only fixed schema labels are interpolated.
  - New query text uses the relation form `rel (roles)`, or
    `$r isa rel, links (roles)`. The workspace guard `relation_syntax_guard_test`
    enforces this.
- **Schema `@doc`s:** every new entity, relation, role and attribute-bearing type gets
  a one-sentence greenfield `@doc`.
- **Comments are greenfield.**
- **Never fabricate success:** bad input errors, and never defaults.
- **Never `#[allow(dead_code)]`.**
- **Off limits:**
  - Ratified docs (CONTRACT-W8, SPEC-W5, SPEC-W2-W3) are never edited.
  - Frozen fixtures under `*/tests/fixtures/` keep their text.
  - The belief-edge registry is never loosened.
- **Integration tests:**
  - They need TypeDB on `localhost:1729` and run with `--test-threads=1` (nextest
    `-j 1`).
  - Never touch the `gecko` database.
- **Before each commit:**
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`;
  - the same clippy with `--features cyber`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **The mapping version** `STIX_MAPPING_VERSION` becomes `"3"`.
- **The kill-chain map** (tactic `x_mitre_domains` → `kill_chain_name`):
  - `enterprise-attack` → `mitre-attack`
  - `mobile-attack` → `mitre-mobile-attack`
  - `ics-attack` → `mitre-ics-attack`

- **Schema changes are additions only.**
  - Never edit an existing `@doc`, which is refused with `DEX16`. `cyber-object`'s
    `@doc` cannot be migrated.
  - The new roles carry `@card(1)` and join `REQUIRED_ROLES`
    (`gecko-bin/tests/cyber_stix_sync_test.rs:1435`).
- **The shared fixture stays as it is.** `attack::fixture_json()` keeps its objects,
  because counted assertions in five suites depend on it. New tests use a separate
  structure fixture.

## Review Focus

1. **A re-import after ATT&CK re-parents or un-revokes an object, including a malware
   or group with no other typed relation.** No stale `subtechnique-of` or `revoked-by`
   edge survives. Tested in Task 2.
2. **A database synced under mapping `"2"`.** After upgrading and re-syncing, the old
   untyped `subtechnique-of` and `revoked-by` concept-links are gone, only the typed
   relations remain, and bare `cyber-object` tactics are retyped. Tested in Task 2.
3. **A revoked-by chain** (A → B → C, where C is live) names C, and a cycle doesn't
   hang. `replacement_of` is unit-tested for the cycle; the chain is tested through
   `claim`. Tested in Task 4.
4. **A bundle with tactics whose technique phase names no tactic** fails the sync, and
   so does a tactic with no shortname or an unmapped domain. A bundle with no tactics
   still imports. Tested in Task 2.
5. **A claimed parent with unclaimed sub-techniques.** The parent is not listed, and
   each unclaimed sub-technique still is. A retired sub-technique is excluded from
   `total`. Tested in Task 3.

## Execution

Tasks run strictly in order 1 → 5, with no parallel implementers. All five touch the
cyber crate.

---

### Task 1: Probe and schema

**Files:**
- Modify: `extensions/cyber-gecko/schema/cyber_schema.tql`
- Modify: `extensions/cyber-gecko/src/stix/mod.rs`. Declare the shapes only;
  `to_okf` and `cyber_post_sync` don't change in this task.
- Test: `gecko-bin/tests/cyber_stix_sync_test.rs`:
  - `typed_relation_players_match_the_schema` (`:380`)
  - `required_attributes_of_the_live_cyber_schema` (`:411`)
  - `REQUIRED_ROLES` (`:1435`)

**Interfaces:**
- **Produces, in the schema:**
  - `attribute retired-attack-id value string`, owned by `cyber-object` as
    `@card(0..1)` and not unique. This is an added `owns` line; `cyber-object`'s
    `@doc` is not touched.
  - `attribute tactic-shortname value string`.
  - `entity tactic sub cyber-object`, which owns `tactic-shortname @card(1)` and plays
    `technique-tactic:tactic`.
  - `relation subtechnique-of` with `relates subtechnique @card(1)` and
    `relates parent @card(1)`. `attack-pattern` plays both roles.
  - `relation revoked-by` with `relates revoked @card(1)` and
    `relates replacement @card(1)`. `attack-pattern`, `intrusion-set`, `malware` and
    `tool` play both roles.
  - `relation technique-tactic` with `relates technique @card(1)` and
    `relates tactic @card(1)`. `attack-pattern` plays `technique`.
  - A greenfield `@doc` on each new type and role.
- **Produces, in `stix/mod.rs`:**
  - the `pub(crate)` consts `SUBTECHNIQUE_OF_REL = "subtechnique-of"`,
    `REVOKED_BY_REL = "revoked-by"` and `TECHNIQUE_TACTIC_REL = "technique-tactic"`;
  - three standalone `RelShape` consts, `SUBTECHNIQUE_OF`, `REVOKED_BY` and
    `TECHNIQUE_TACTIC`, modelled on `STRATEGY_ANALYTIC` (`:578`);
  - `typed_shapes()` (`:607`) chains them, so the player test and `orphan_sweep`
    cover them.
  - They are not added to `REL_SHAPES` yet. Task 2 decides how SROs reach them.

- [ ] **Step 1: Probe TypeDB 3.15.**
  - Against a throwaway database (never `gecko`), with `mem_types.tql:80-81` as
    precedent:
    - define `plays rel:role @card(0..1)`;
    - insert two such relations for one player;
    - confirm the commit is refused.
  - Record a ruling in the ledger:
    - **accepted:** the `subtechnique-of:subtechnique` and `revoked-by:revoked` plays
      carry `@card(0..1)`;
    - **refused:** the adapter alone enforces the limit (Task 2).
- [ ] **Step 2: Write the failing tests.**
  - Add the three relations to `REQUIRED_ROLES`.
  - Extend `required_attributes_of_the_live_cyber_schema`, or add a sibling test, to
    assert that `tactic` requires `tactic-shortname`.
- [ ] **Step 3: Run the tests and confirm they fail.**
  - Run: `cargo nextest run -p gecko-bin --no-default-features --features cyber --test cyber_stix_sync_test -j 1`
  - Expected: the new assertions fail.
- [ ] **Step 4: Implement** the schema additions and the shape consts.
- [ ] **Step 5: Run the tests and confirm they pass.**
  - Run: the Step 3 command, then `--test schema_init_test --test cyber_sensors_sync_test --test cyber_detect_test`, then `cargo nextest run -p mem-gecko --test schema_migration_test -j 1`.
  - Expected: all pass, and the schema applies to a fresh database and over an
    existing one.
- [ ] **Step 6: Commit** with the message
  `feat(cyber): schema types for tactics, sub-techniques and revocations`.

### Task 2: Import writes the three relations and replaces them on re-import

**Files:**
- Modify: `extensions/cyber-gecko/src/stix/mod.rs`:
  - `sdo_type_hint` (`:60`)
  - the retired-id block (`:285-299`)
  - the kill-chain block (`:309-313`)
  - the SRO loop (`:389-410`)
  - `StixRelations` (`:116`)
  - `STIX_MAPPING_VERSION` (`:129`)
  - `cyber_post_sync` (`:651`)
  - `replacement_writes` (`:741`)
- Create: `gecko-bin/tests/common/structure.rs`, a structure fixture.
  `attack::fixture_json()` is not changed.
- Test: unit tests in `stix/mod.rs`; integration tests in `cyber_stix_sync_test.rs`

**Interfaces:**
- **Consumes:** Task 1's shape consts.
- **Produces:**
  - **`StixRelations` gains two sorted id lists:**
    - `techniques: Vec<String>`, every in-bundle attack-pattern concept id;
    - `revocable: Vec<String>`, every in-bundle concept id of type `attack-pattern`,
      `intrusion-set`, `malware` or `tool`.
  - **`typed` now carries** `subtechnique-of`, `revoked-by` and `technique-tactic`
    rows, with `rel_type` set to the consts.
  - **`structure::fixture_json()`** contains:
    - two enterprise tactics, TA0002 `execution` and TA0003 `persistence`;
    - a live parent technique, P;
    - two live sub-techniques, S1 and S2, each with a `subtechnique-of` to P;
    - a retired sub-technique, SR, with a `subtechnique-of` to P;
    - a chain: revoked technique R1 → revoked R2 → live S1;
    - a deprecated technique, D;
    - a revoked malware, M, with `revoked-by` pointing to a live tool, T;
    - phases on P, S1 and S2;
    - the minimum detection objects that `claim` and `blinded` need in Tasks 3–4.
    - Every object has a distinct `mitre-attack` id, exported as a `pub const`.

    The shared helpers it needs are moved to `common/` without changing their
    output.

- [ ] **Step 1: Write the failing unit tests** in `stix/mod.rs`. Each builds a bundle
  inline.
  - **`a_tactic_is_typed_with_its_shortname`.**
  - **`a_tactic_without_a_shortname_or_with_an_unmapped_domain_is_refused`:** both
    cases make `to_okf` return `Err`, and the message names the tactic.
  - **`a_technique_links_to_the_tactic_its_phase_names`:** this gives one
    `technique-tactic` row.
  - **`a_phase_naming_no_tactic_is_refused`:** the error contains the technique's STIX
    id and the phase name.
  - **`a_mobile_phase_does_not_link_an_enterprise_tactic`:** this errors.
  - **`foreign_kill_chains_and_tactic_free_bundles_are_kept_as_attributes`.**
  - **`subtechnique_of_and_revoked_by_are_typed`:** a revoked malware → tool gives
    `revoked-by`.
  - **`an_unadmitted_revocation_pair_stays_a_concept_link`:** for example, a
    course-of-action → course-of-action.
  - **`a_second_parent_or_replacement_is_refused`:** the error names the source.
  - **`a_retired_object_keeps_its_attack_id_as_retired`.**
  - **`techniques_and_revocable_list_their_ids`.**
- [ ] **Step 2: Write the failing integration tests** in `cyber_stix_sync_test.rs`,
  over `structure::fixture_json()`:
  - **`stix_sync_writes_subtechnique_revocation_and_tactic_relations`:** each relation
    links the expected pairs, counted by concept id.
  - **`a_reimport_replaces_a_changed_parent_and_a_lifted_revocation`:**
    - Edit the bundle (as `edited_fixture`, `:768`, does): re-point S2's parent to S1,
      remove M's `revoked-by` and clear M's `revoked`, and remove a phase from P.
    - Re-sync.
    - S2 has exactly one parent, S1; M plays no `revoked-by`; and P has one fewer
      `technique-tactic`.
  - **`upgrading_from_mapping_2_leaves_no_untyped_subtechnique_or_revocation_links`:**
    - Follow `a_database_synced_by_the_previous_adapter_is_retyped` (`:992-1018`):
      - take `to_okf` output;
      - set the tactics' `type_hint` to `cyber-object`, and drop their
        `tactic-shortname`;
      - push `OkfLink`s with `link_text` `subtechnique-of`/`revoked-by` for each SRO;
      - set a stale `file_hash` on every concept;
      - `sync_bundle` with no post-sync writes (`&[]`).
    - Then sync with the current adapter.
    - No `concept-link` has either `link-text`, the typed relations exist, and the
      tactics are `tactic`.
- [ ] **Step 3: Run the tests and confirm they fail.**
  - Run: `cargo test -p cyber-gecko --lib stix`, and `cargo nextest run -p gecko-bin --no-default-features --features cyber --test cyber_stix_sync_test -j 1`.
  - Expected: the new tests fail.
- [ ] **Step 4: Implement the adapter, per the spec's Adapter section.**
  - **Tactic index:** a pre-pass builds
    `BTreeMap<(kill_chain_name, shortname), concept_id>` from every tactic, live or
    retired.
    - A tactic with no `x_mitre_shortname`, or with a domain outside the map, errors.
    - Two tactics with the same key error, and the error names both.
  - **Resolving phases:** a technique phase whose `kill_chain_name` is a map value
    resolves through the index, and a miss errors. When the index is empty, no phase
    resolves and none errors. The `kill-chain-phase` attribute is kept as it is.
  - **SROs:** `subtechnique-of` and `revoked-by` map to their shapes in the SRO loop
    (`:401`), either by joining `REL_SHAPES` or by a dedicated lookup. Either way:
    - `technique-tactic` is never reachable from an SRO;
    - the insert-only path in `cyber_post_sync` (`:658`) skips all three.

    Uniqueness is checked while collecting.
  - **Replace writes:** `replacement_writes` deletes, before inserting:
    - every `subtechnique-of` and `technique-tactic` whose `subtechnique`/`technique`
      is in `techniques`;
    - every `revoked-by` whose `revoked` is in `revocable`.

    It then inserts the current set, one multi-row write per (relation, source
    subtype, target subtype).
  - **Bump `STIX_MAPPING_VERSION` to `"3"`.**
- [ ] **Step 5: Run the tests and confirm they pass.**
  - Run: the Step 3 commands, then `--test cyber_detect_test --test schema_init_test --test cyber_sensors_sync_test`.
  - Expected: all pass. Any existing assertion changed by the version bump alone is
    named in the report.
- [ ] **Step 6: Commit** with the message
  `feat(cyber): import tactics, sub-techniques and revocations as typed relations`.

### Task 3: `coverage_gaps` skips retired techniques and counts sub-techniques

**Files:**
- Modify: `extensions/cyber-gecko/schema/cyber_functions.tql` (`undetected-techniques`,
  `:6`)
- Modify: `extensions/cyber-gecko/src/detect/mod.rs` (`coverage_gaps`, `:228`)
- Test: `gecko-bin/tests/cyber_detect_test.rs`

**Interfaces:**
- **Consumes:** the `subtechnique-of` relation and `structure::fixture_json()` from
  Task 2.
- **Produces:** each `coverage_gaps` row is `{ concept_id, attack_id }`, plus
  `sub_techniques: { "claimed": n, "total": m }` when the technique has live
  sub-techniques.

- [ ] **Step 1: Write the failing integration tests** over the structure fixture:
  - **`coverage_gaps_skips_retired_techniques`:** R1, R2, SR and D are absent.
  - **`coverage_gaps_counts_a_parents_claimed_sub_techniques`:**
    - With S1 claimed, P's row has `sub_techniques: {claimed: 1, total: 2}`. SR is not
      counted.
    - S2's row has no `sub_techniques` key.
  - **`a_claimed_parent_does_not_cover_its_sub_techniques`:** P is claimed and not
    listed, and S1 and S2 are both listed.
  - **The existing gap assertions** at `cyber_detect_test.rs:127,146` still pass on
    the unchanged shared fixture. If the liveness filter changes them, the change is
    named in the report and justified.
- [ ] **Step 2: Run the tests and confirm they fail.**
  - Run: `cargo nextest run -p gecko-bin --no-default-features --features cyber --test cyber_detect_test -j 1`
  - Expected: the new tests fail.
- [ ] **Step 3: Implement.**
  - **`undetected-techniques()`:** add the liveness pattern, the same as `live()`
    (`detect/mod.rs:243`). Its `@doc` text stays the same.
  - **`coverage_gaps`:** one extra read covers every live sub-technique's parent and
    whether it has an asserted claim. The counts are built host-side.
- [ ] **Step 4: Run the tests and confirm they pass.**
  - Run: the Step 2 command, `--test schema_init_test`, and `cargo nextest run -p mem-gecko --test schema_migration_test -j 1`.
  - Expected: PASS, and the function redefine applies over an existing database.
- [ ] **Step 5: Commit** with the message
  `feat(cyber): coverage gaps skip retired techniques and count claimed sub-techniques`.

### Task 4: Name a retired technique's replacement

**Files:**
- Modify: `extensions/cyber-gecko/src/detect/mod.rs`:
  - `ensure_logic_and_technique` (`:1060`)
  - `blinded` (`:346`)
  - `claim_statuses` (`:428`)
- Test: unit tests in `detect/mod.rs`; integration tests in `cyber_detect_test.rs`

**Interfaces:**
- **Consumes:** `revoked-by` and `retired-attack-id`, and the structure fixture.
- **Produces:**
  - **`fn replacement_of(edges: &BTreeMap<String, String>, start: &str) -> Option<String>`**
    follows revoked → replacement concept ids with a visited set. It returns the first
    id with no outgoing edge, or `None` on a cycle. The caller checks that the end is
    live.
  - **A `blinded` row** gains `retired_technique` and `replaced_by` only when its
    technique has a `retired-attack-id`. `replaced_by` is null when there is no live
    replacement. Rows for live techniques are unchanged, because the existing tests
    compare them as exact JSON (`cyber_detect_test.rs:401-409`, `840-851`).

- [ ] **Step 1: Write the failing unit tests** for `replacement_of`:
  - a chain `a→b→c` returns `c`;
  - a cycle `a→b→a` returns `None`;
  - an id with no edge returns itself.
- [ ] **Step 2: Write the failing integration tests:**
  - **`claiming_a_revoked_technique_names_its_replacement`:** claiming R2 errors. The
    message contains R2's id, `revoked`, and S1's attack-id, and no belief is
    committed.
  - **`claiming_through_a_revocation_chain_names_the_live_end`:** claiming R1 names
    S1.
  - **`claiming_a_deprecated_technique_says_it_has_no_replacement`:** D.
  - **`blinded_names_the_replacement_of_a_revoked_claimed_technique`:**
    - Claim S2, then re-sync with S2 revoked by S1. That's an edit: set S2's
      `revoked`, and add a `revoked-by`.
    - The row has `retired_technique` equal to S2's id and `replaced_by` equal to S1's
      attack-id.
  - **`blinded_on_a_deprecated_claimed_technique_has_no_replacement`:**
    `replaced_by: null`.
  - **Update `claim_on_deprecated_technique_is_unassessed` (`:890-901`)** and
    `claim_on_retired_technique_is_unassessed` (`:865`) for the two new keys, only if
    their technique now carries a `retired-attack-id`.
- [ ] **Step 3: Run the tests and confirm they fail.**
  - Run: `cargo test -p cyber-gecko --lib detect`, and `cargo nextest run -p gecko-bin --no-default-features --features cyber --test cyber_detect_test -j 1`.
  - Expected: the new tests fail.
- [ ] **Step 4: Implement.**
  - **Reading the edges:** `claim` and `blinded` each read every `revoked-by` edge in
    one query (157 on v20.1), into the map `replacement_of` takes.
  - **In `claim`,** when the live lookup is empty, read the attack-patterns with that
    `retired-attack-id`. Several may share it, so take every row.
    - A revoked match with a live, attack-id-bearing end gives "technique '<id>' is
      revoked; ATT&CK replaces it with '<end>'; claim that technique instead".
    - Otherwise: "… is deprecated (or revoked) with no replacement".
    - With no match at all: the current error.
  - **In `blinded`,** `claim_statuses` also fetches `retired-attack-id`.
- [ ] **Step 5: Run the tests and confirm they pass.**
  - Run: the Step 3 commands.
  - Expected: PASS.
- [ ] **Step 6: Commit** with the message
  `feat(cyber): claim and blinded name a retired technique's replacement`.

### Task 5: Real-bundle check and ledger

**Files:**
- Modify: `gecko-bin/tests/cyber_stix_sync_test.rs` (`graph_counts`, `:1094`;
  `stix_full_bundle_resync_is_stable`, `:1116`)
- Modify: `docs/refactor/OWED-WORK-LEDGER.md` (the S3 row, D-M, and the dependency
  line `:106`)

- [ ] **Step 1: Extend the counts.** `graph_counts` already counts every
  `typed_relation_players()` relation (`:1101`), so add only the `tactic` entity
  count.
- [ ] **Step 2: Run against v20.1.**
  - Run: `GECKO_ATTACK_BUNDLE=/private/tmp/claude-501/-Users-eddie-Documents-repos-gecko/b58caccd-4178-4fc7-a581-9b53edddbe9b/scratchpad/attack/ea.json cargo nextest run -p gecko-bin --no-default-features --features cyber --test cyber_stix_sync_test stix_full_bundle_resync_is_stable -j 1 --no-capture`
  - Expected:
    - it passes, and the counts are stable across the re-sync;
    - `subtechnique-of` 481, `revoked-by` 157, `technique-tactic` 1,099 and `tactic`
      15;
    - the existing counts are unchanged;
    - 22 blind claims, and 21 steps adding 24 sources.
  - Record the timings. If the bundle is gone, record the skip.
- [ ] **Step 3: Update the ledger.**
  - Mark S3 ✅ with its tests and the counts.
  - Record D-M as settled (option A), with a pointer to the spec.
  - Add the follow-ons: rolling `blinded` up to the parent, and grouping by tactic.
- [ ] **Step 4: Run the full verification.**
  - Run: `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1`, fmt, both clippy runs, `cargo audit` and `cargo deny check licenses bans sources`.
  - Expected: all clean.
- [ ] **Step 5: Commit** with the message
  `docs(ledger): S3 lands: typed sub-techniques, revocations and tactics`.

## Finish

- Final whole-branch review on Fable, then one fix wave.
- A PR against `main`, with CI watched.
