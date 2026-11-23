# Log-Source Recommendation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A new cyber detect host import, `recommend_sources`. It returns an ordered, greedy
list of ATT&CK log sources to add, and the asserted claims each step unblinds, plus the
claims that no log source can fix.

**Architecture:**

- **The pure greedy planner.** `extensions/cyber-gecko/src/detect/recommend.rs` holds it,
  pinned by unit tests and proptest.
- **The host import.** It lives in `src/detect/mod.rs` and reuses `blinded`'s preconditions
  and `claim-platforms()` reads.
  - Those parts are factored out of `blinded` without changing its behaviour.
  - It reads each blind analytic's missing log-source keys with their names and channels,
    then calls the planner.

**Tech Stack:** Rust, TypeDB 3.15 (TypeQL functions already in
`extensions/cyber-gecko/schema/cyber_functions.tql`), proptest 1.x (a dev-dependency of
cyber-gecko only).

**Spec:** `docs/superpowers/specs/2026-11-22-log-source-recommendation-design.md`. The S2
spec, `docs/superpowers/specs/2026-11-21-attack-detection-model-design.md`, defines
`blinded`.

## Global Constraints

- **All TQL is parameterized.** Values go through `given` parameters, never string
  interpolation. The existing `live("x")` helper interpolates only variable names, which is
  allowed.
- **No schema change.** No new TypeQL function and no migration. The import uses the existing
  `claim-platforms()` and `analytic-gaps()`.
- **Never claim to fix what a step cannot fix.** A zero-requirement analytic, or a declared
  platform ATT&CK gives no analytic for, is listed as unfixable with its reason. It never
  appears as a step.
- **Never return an empty list for a graph the check cannot judge.** The errors for no
  sensor file and no detection strategies are exactly `blinded`'s.
- **Never recommend a platform the user did not declare.** Every pair comes from
  `claim-platforms()`, which reads only declared platforms.
- **The steps are deterministic.**
  - Ratio = pairs fixed ÷ new log sources, compared by integer cross-multiplication.
  - Ties are broken by more pairs fixed, then platform ascending, then the sorted key list
    ascending.
- **Recommendations name log sources only**, by their log-source key. They never name a
  fallback data component.
- **`blinded`'s output and errors are unchanged.** Its existing tests in
  `gecko-bin/tests/cyber_detect_test.rs` must pass untouched.
- **Comments are greenfield**: no plan or task tags.
- **Never use `#[allow(dead_code)]`.**
- Integration tests run with `--test-threads=1`. Never touch the `gecko` database.
- **Before each commit, run:**
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`
  - the same clippy run with `--features cyber`
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Several claims on one technique.** Two claims share one analytic's gap set. A step must
   count and list both, and `cumulative_unblinded` counts (claim, platform) pairs. Pinned by
   `two_claims_share_a_step` (Task 1) and `no_sensor_recommends_each_platform_in_order`
   (Task 2).
2. **A sync lands between reads.** A blind pair whose analytic now reads as covered, or as
   having no analytic, must error, as `blinded` does. It must not plan a step with an empty
   gap set. Pinned by `plan_rejects_an_empty_gap_set` (Task 1). The I/O layer maps a
   covered analytic to the `blinded` "graph changed" error.
3. **Overlapping gaps across platforms.** `nsm:flow` is missing on both Windows and Linux.
   Adding it on one platform must not count as adding it on the other: keys are per
   platform. Pinned by `same_key_on_two_platforms_is_two_sources` (Task 1).
4. **A blind pair where one analytic has no requirements and another has gaps.** The pair is
   fixable through the second analytic, and must not be listed as unfixable. Pinned by
   `zero_requirement_analytic_beside_a_fixable_one` (Task 1, at the planner-input level), and
   by the I/O layer passing only analytics that have requirements.
5. **Everything covered.** `steps: []` and `unfixable: []` is a valid answer only after both
   preconditions pass. Pinned by `fully_covered_recommends_nothing` (Task 2).

---

### Task 1: The pure greedy planner

**Files:**
- Create: `extensions/cyber-gecko/src/detect/recommend.rs`.
- Modify: `extensions/cyber-gecko/src/detect/mod.rs`. Add `#[cfg(test)] mod recommend;` only.
- Modify: `extensions/cyber-gecko/Cargo.toml`. Add
  `[dev-dependencies] proptest = { version = "1", default-features = false, features = ["std"] }`.
- Modify: `Cargo.lock`. CI runs `--locked`. After adding, run `cargo audit` and
  `cargo deny check licenses bans sources`. Both must pass; duplicate-version warnings are
  acceptable.
- Test: an in-file `#[cfg(test)] mod tests` in `recommend.rs`.

**Interfaces:**
- **Produces** (crate-private, used by Task 2):

```rust
/// One asserted claim's blind declared platform.
pub(crate) struct BlindPair {
    pub claim: usize,            // index into the caller's claim list
    pub platform: String,
    /// One entry per live analytic on `platform` that has a requirement:
    /// that analytic's missing log-source keys there. Each set is non-empty.
    pub analytics: Vec<BTreeSet<String>>,
}

pub(crate) struct Step {
    pub platform: String,
    pub add: Vec<String>,        // keys new in this step, sorted
    pub unblinds: Vec<usize>,    // indices into the `pairs` slice, ascending
    pub cumulative_unblinded: usize,
}

/// The greedy plan over `pairs`. Errs, naming the pair's index and platform, on a pair
/// with no analytics or an analytic with an empty gap set.
pub(crate) fn plan(pairs: &[BlindPair]) -> Result<Vec<Step>, String>;
```

**Algorithm.** This is the part the signature does not determine.

- **State.**
  - `added: BTreeSet<(platform, key)>`.
  - `fixed: Vec<bool>`, one per pair.
- **Candidates.** One per (pair not yet fixed, analytic): `(platform, remaining)`, where
  `remaining` is the analytic's keys not yet in `added` for that platform. Identical
  candidates are deduplicated.
- **Pairs a candidate fixes.** Every unfixed pair on the candidate's platform that has some
  analytic whose keys are all in `added ∪ candidate`.
- **Each round.**
  - Score every candidate. Choose the best by ratio (`fixed_a * new_b > fixed_b * new_a`),
    then by more pairs fixed, then platform ascending, then `remaining` as a sorted `Vec`
    ascending.
  - Apply it: extend `added`, mark its pairs fixed, and push a `Step`. `unblinds` holds the
    newly fixed pair indices.
- **Stop** when every pair is fixed. Every valid pair has a candidate that fixes it, namely
  its own analytic, so the loop always terminates with every pair fixed.

- [ ] **Step 1: Write the failing unit tests.** Use a helper
  `pair(claim, platform, &[&[&str]])`.
  - `plan_of_nothing_is_empty`: `plan(&[])` is `Ok(vec![])`.
  - `plan_rejects_an_empty_gap_set`: a pair with `analytics: [{}]` returns `Err` naming
    index 0 and its platform. A pair with `analytics: []` also returns `Err`.
  - `two_sources_useless_alone_are_one_step`: a single pair on `Windows` with one analytic
    `{a, b}` gives exactly one step: `add == [a, b]`, `unblinds == [0]`,
    `cumulative_unblinded == 1`.
  - `two_claims_share_a_step`: pairs 0 and 1 (claims 0 and 1), both `Windows {a, b, c}`,
    give one step with `unblinds == [0, 1]` and `cumulative_unblinded == 2`.
  - `cheaper_ratio_goes_first`:
    - pair 0 is `Windows {a}`;
    - pair 1 is `Windows {b, c, d}`;
    - pair 2 is `Windows {b, c, d}`, a second claim.
    
    Step 1 adds `[a]` (ratio 1/1). Step 2 adds `[b, c, d]` (2/3), and `cumulative_unblinded`
    runs 1 then 3.
  - `shared_source_shrinks_later_steps`:
    - pair 0 is `Windows {a}`;
    - pair 1 is `Windows {a, b, c}`.
    
    Step 1 adds `[a]` (1/1 beats 2/3) and fixes only pair 0. Step 2 adds only `[b, c]`.
  - `ratio_tie_prefers_more_pairs`:
    - pair 0 is `Windows {a}`;
    - pair 1 is `Windows {a, b}`.
    
    `{a}` scores 1/1 and `{a, b}` scores 2/2, so the ratios tie, and more pairs fixed wins.
    The result is one step: `add == [a, b]`, `unblinds == [0, 1]`.
  - `ties_break_by_platform_then_keys`:
    - pair 0 is `Windows {x}`;
    - pair 1 is `Linux {y}`;
    - pair 2 is `Linux {a}`.
    
    The steps run Linux `[a]`, then Linux `[y]`, then Windows `[x]`.
  - `same_key_on_two_platforms_is_two_sources`:
    - pair 0 is `Windows {nsm:flow}`;
    - pair 1 is `Linux {nsm:flow}`.
    
    The result is two steps, each adding `nsm:flow` and fixing one pair.
  - `either_analytic_fixes_the_pair`: a pair on `Windows` with analytics `{a, b, c}` and
    `{d}` gives one step that adds `[d]`.
  - `zero_requirement_analytic_beside_a_fixable_one`: Task 2 passes only analytics that have
    requirements, so this is a pair with one analytic `{a}`. It plans normally. This test
    documents the contract in its doc comment.

- [ ] **Step 2: Write the proptest properties.** The strategy generates up to 8 pairs over
  the platforms {`Linux`, `Windows`} and keys {`k0` … `k5`}, with 1–3 analytics of 1–4 keys
  each. The properties are:
  - `every_listed_pair_is_fixed_by_the_sources_so_far`: after replaying steps 0..=i, every
    pair in step i's `unblinds` has an analytic whose keys are all added on its platform.
    A pair is not fixed before step i.
  - `no_step_re_adds_a_source`: no step's `add` is empty, and no step's `add` overlaps the
    sources already added on its platform.
  - `every_pair_listed_exactly_once`: the `unblinds` across all steps are a permutation of
    `0..pairs.len()`. The last `cumulative_unblinded` equals `pairs.len()`.
  - `steps_stay_on_their_pairs_platform`: for every index in a step's `unblinds`,
    `pairs[i].platform == step.platform`.
  - `analytic_order_does_not_matter`: shuffling each pair's `analytics` leaves the sequence
    of `(platform, add)` unchanged. Use proptest's `Just(..).prop_shuffle()` or a generated
    permutation. `Step` derives `PartialEq` and `Debug`.
  - `first_step_has_the_best_ratio`: step 0's ratio (`unblinds.len()` ÷ `add.len()`) is at
    least the initial ratio of every candidate, computed by brute force in the test.
  - Keep proptest's default of 256 cases. Do not commit a `proptest-regressions/` directory.
    If one appears, add the pinned case as a unit test instead.

- [ ] **Step 3: Run them to watch them fail.**
  - Run: `cargo test -p cyber-gecko --no-default-features recommend`
  - Expected: FAIL, because `plan` is not defined.

- [ ] **Step 4: Implement `plan` in `recommend.rs`,** following the algorithm above. Add a
  module doc of one or two greedy-method sentences.

- [ ] **Step 5: Run the tests to watch them pass.**
  - Run: `cargo test -p cyber-gecko --no-default-features recommend`
  - Expected: every unit test and proptest passes.
  - Then run the full lint set from Global Constraints. The module is declared
    `#[cfg(test)] mod recommend;` in this task, so its items are not dead code in the
    non-test build. Task 2 drops the `#[cfg(test)]`.

- [ ] **Step 6: Commit.**

```bash
git add extensions/cyber-gecko/src/detect/recommend.rs extensions/cyber-gecko/src/detect/mod.rs extensions/cyber-gecko/Cargo.toml Cargo.lock
git commit -m "feat(cyber): greedy planner for the log sources that unblind the most claims"
```

---

### Task 2: The `recommend_sources` host import

**Files:**
- Modify: `extensions/cyber-gecko/src/detect/mod.rs`.
  - Add `"recommend_sources"` to `DETECT_FNS` (`[&str; 6]`) and to the dispatch.
  - Change `#[cfg(test)] mod recommend;` to `mod recommend;`.
  - Extract from `blinded`, keeping its read order (environment, then strategies, then
    `claim-platforms`) and its error strings verbatim:
    - `async fn require_judgeable(graph) -> Result<(), String>`, the two precondition
      reads with their exact errors;
    - `async fn claim_statuses(graph) -> Result<BTreeMap<ClaimKey, ClaimStatuses>, String>`,
      the `claim-platforms()` read and grouping;
    - `async fn live_analytics(graph, technique_concept_id, platform)
      -> Result<Vec<(Value, String)>, String>`, the analytic read from `blind_analytics`,
      giving (attack-id or null, concept id) sorted as it is today;
    - `async fn has_live_requirement(graph, analytic_concept_id) -> Result<bool, String>`,
      the `limit 1` requirement read;
    - `fn graph_changed(platform: &str) -> String`, the message from
      `blind_platform_entries`'s `changed` closure.

    `blinded` and `recommend_sources` both call these, so no TQL or error text is
    duplicated.
  - Add `async fn recommend_sources(graph) -> Result<Value, String>`.
  - Add a gap read returning key, display name and channels.
  - Add a module-doc bullet.
- Modify: `extensions/cyber-gecko/src/lib.rs`. Add a `HostImportDef { name:
  "recommend_sources", description: "Recommend the log sources to add, in greedy order, to
  unblind the most detection claims" }`.
- Modify: `extensions/cyber-gecko/README.md`. Add a `## \`recommend_sources\`` section after
  `blinded`'s, covering the scope, the call, an output example and the method.
- Modify: `docs/refactor/KNOWN-LIMITATIONS.md`. Add a bullet under the `blinded` section:
  - the result is greedy, not optimal;
  - there is no cost per source;
  - log sources only, never fallback components;
  - `no-analytic` also covers a claim on a retired, revoked or deprecated technique.
- Test: `gecko-bin/tests/cyber_detect_test.rs`. Add a `// ── recommend_sources ──` section
  that reuses `Rig`, `attack::` and `sync_sensor_file`.

**Interfaces:**
- **Consumes:** Task 1's `recommend::{BlindPair, Step, plan}`.
- **Produces** this host-import output:

```json
{
  "steps": [
    { "platform": "Linux",
      "add": [ { "log_source": "<smallest trimmed spelling>", "key": "<log-source-key>",
                 "channels": ["<sorted, distinct>"] } ],
      "unblinds": [ { "concept_id": "…", "technique": "T1059" | null,
                      "technique_concept_id": "…" } ],
      "cumulative_unblinded": 2 }
  ],
  "unfixable": [
    { "concept_id": "…", "technique": "T1059" | null, "technique_concept_id": "…",
      "platform": "…", "reason": "no-requirements" | "no-analytic" }
  ]
}
```

- **Ordering.**
  - `add` follows the step's sorted keys.
  - `unblinds` follows the planner's pair order.
  - Pairs are built by iterating `claim_statuses` (a `BTreeMap`, in `ClaimKey` order) and
    then each claim's platforms in `BTreeMap` order.
  - `unfixable` follows that same order.

- **Classification**, per (claim, platform) from `claim_statuses`:
  - `covered` among its statuses → skip, as in `blinded`.
  - Contains `blind`:
    - Read `live_analytics` on (technique, platform). For each analytic where
      `has_live_requirement` holds, read its gaps, which gives that analytic's key set.
    - Cache these reads by `(technique_concept_id, platform)` within one call, so two
      claims on one technique share one read.
    - An analytic that has a requirement but no gaps is `Err(graph_changed(platform))`.
      So is finding no analytic. This is raised before `plan` is called, so `plan`'s own
      `Err` is only an internal-invariant message.
    - No analytic with requirements → `unfixable`, `"no-requirements"`.
    - Otherwise → a `BlindPair`.
  - Otherwise (unassessed) → `unfixable`, `"no-analytic"`.
    - **Ruling:** this includes a claim whose technique is retired, revoked or deprecated,
      which `claim-platforms()` reports as unassessed on every platform. The README and
      KNOWN-LIMITATIONS say so. No third reason is added, because the spec defines two.
- **The source table.** Across every gap read in the call, build one
  `BTreeMap<(platform, key), (smallest trimmed name, BTreeSet<channel>)>`. Each step's `add`
  entries look up their name and channels there. The channels are the union over every
  requirement with that key on that platform.

- **The gap read** is parameterized, like `missing_log_sources`:

  ```
  given $aid: string, $plat: string;
  match $a isa analytic, has concept-id $ac; $ac == $aid;
        let $r in analytic-gaps($a, $plat);
  fetch { "name": $r.log-source-name, "key": $r.log-source-key,
          "channels": [ $r.log-source-channel ] };
  ```

- [ ] **Step 1: Write the failing integration tests.** The `Rig` claims `LOGIC_ID` on
  T1059. For two claims, reuse the existing `playbooks/detect-a` and `playbooks/detect-b`
  ids from the `blinded` tests, claiming `detect-b` first so the ordering is pinned.
  - `no_sensor_recommends_each_platform_in_order`:
    - **Setup.** Two claims. The sensor file is
      `platforms: [Windows, Linux, macOS, Containers]\nsensors: []\n`.
    - **Expected steps.** Exactly 2, with platforms in the order `["Linux", "Windows"]`.
      - Each step's `add` keys equal the platform's missing keys from
        `linux_missing()` / `windows_missing()`, lowercased.
      - Each step's `unblinds` lists both claims.
      - `cumulative_unblinded` runs `[2, 4]`.
    - **Expected unfixable,** exactly in this order (platforms sort by byte order,
      `Containers < Linux < Windows < macOS`):
      1. `detect-a` × `Containers` `"no-analytic"`;
      2. `detect-a` × `macOS` `"no-requirements"`;
      3. `detect-b` × `Containers` `"no-analytic"`;
      4. `detect-b` × `macOS` `"no-requirements"`.
    - **Windows `add`, asserted exactly:**
      - `{log_source: "NSM:Flow", key: "nsm:flow", channels: []}`. The fixture's `"None"`
        channel is dropped at import.
      - `{log_source: "WinEventLog:Security", key: "wineventlog:security", channels:
        ["EventCode=4103", "EventCode=4688"]}`. This is the union across two requirements.
      - `{log_source: "WinEventLog:Sysmon", key: "wineventlog:sysmon", channels:
        ["EventCode=1", "EventCode=10"]}`.
      
      Confirm these values against `gecko-bin/tests/common/attack.rs` while writing the
      test. If the fixture disagrees, the fixture wins; ledger it.
    - Linux's `linux:syslog` entry has `channels: []`, the no-channel case.
  - `partial_sensor_recommends_only_the_remainder`:
    - **Setup.** One claim. The sensor file has `platforms: [Windows, Linux]` and a Windows
      sensor with `log_sources: [WinEventLog:Sysmon, WinEventLog:Security]`.
    - **Expected.** Step 1 is Windows, adding only `nsm:flow` (ratio 1/1). Step 2 is Linux,
      adding three keys.
  - `fully_covered_recommends_nothing`: with `WINDOWS_FULL`, the result is exactly
    `{"steps": [], "unfixable": []}`.
  - `recommend_sources_errors_without_a_sensor_file` and
    `recommend_sources_errors_without_detection_strategies`: each returns `Err` equal to
    `blinded`'s error on the same graph. Assert by calling both and comparing.
  - `blinded_is_unchanged`: no new test. Every existing `blinded` test must pass untouched.

- [ ] **Step 2: Run them to watch them fail.**
  - Run: `cargo test -p gecko-bin --features cyber --no-default-features --test cyber_detect_test recommend -- --test-threads=1`
  - Expected: FAIL with `unknown detect function`, or the call falls through to `base`.

- [ ] **Step 3: Implement it.**
  - The extractions, `recommend_sources`, the gap read, the registration in `lib.rs`, the
    README section and the KNOWN-LIMITATIONS bullet.
  - `blinded` calls `require_judgeable` and `claim_statuses`, so its behaviour is unchanged.

- [ ] **Step 4: Run all the detect tests to watch them pass.**
  - Run: `cargo test -p gecko-bin --features cyber --no-default-features --test cyber_detect_test -- --test-threads=1`
  - Expected: all pass, the new `recommend` tests and every existing `blinded` test.
  - Then run `cargo test -p cyber-gecko --no-default-features` and the full lint set.

- [ ] **Step 5: Commit.**

```bash
git add extensions/cyber-gecko/src/detect/mod.rs extensions/cyber-gecko/src/lib.rs extensions/cyber-gecko/README.md docs/refactor/KNOWN-LIMITATIONS.md gecko-bin/tests/cyber_detect_test.rs
git commit -m "feat(cyber): recommend_sources lists the log sources to add, in greedy order"
```
