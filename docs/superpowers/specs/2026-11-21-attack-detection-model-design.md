# ATT&CK's current detection model and platform-aware blindness (ledger S2, D-L, S4) — design

2026-11-21. The user chose these in conversation:

- **What the check is for, in priority order:** first which platforms you are blind on for a technique, then what to add, and last that a claimed detection is unreliable.
- **How sensor coverage is described:** by log-source name, with data-component names as the fallback.
- **How the sensor inventory gets in:** a sensor file the user keeps and syncs.
- **How the check is built:** modelled in the graph and computed by TypeDB functions (approach A).
- **What a claim is checked against:** only the technique it names, with no roll-up from sub-techniques.

A builder check and a Fable review against the code and the v20.1 bundle shaped the rest. Each
such decision is marked *(author's call)*.

## Intent

cyber's `blinded` host import is meant to flag detection logic that claims a technique when the
sensors in place cannot see what that detection needs. It reports nothing on current ATT&CK
(Enterprise v20.1, ATT&CK spec 3.3.0). There are three reasons:

- **Detection now has three levels instead of a direct link.** The old "data component `detects` technique" link is replaced by:
  - **Detection strategies** (`x-mitre-detection-strategy`): 708 in all, 706 of them live, one per live technique, linked to it by a `detects` SRO.
  - **Analytics** (`x-mitre-analytic`, 1,773): a strategy references its analytics through `x_mitre_analytic_refs`, and no analytic is shared between strategies.
    - Each analytic has exactly one `x_mitre_platforms` entry.
    - Each has 0–10 `x_mitre_log_source_references` of the form `{x_mitre_data_component_ref, name, channel}`.
    - **No live strategy has two analytics for the same platform** (1,758 pairs, every one distinct). "Blind on platform P" therefore means that strategy's one analytic for P cannot run.
- **Data sources are deprecated.** All 38 are deprecated, and no data component carries `x_mitre_data_source_ref`.
- **Nothing writes sensors.** Nothing in the codebase or tests writes a `sensor` or `sensor-coverage`.

**Goals.**

1. **The model.** The STIX adapter models strategies, analytics and analytic requirements as typed records, and they stay correct across a re-import of a changed ATT&CK release.
2. **The sensor file.** It declares the platforms the user runs, and for each sensor its platforms, log sources and fallback data components. It is synced idempotently into the ATT&CK database.
3. **The check.** For each asserted detection claim, `blinded` reports the technique and two lists:
   - **Blind platforms:** each declared platform whose analytic for this technique cannot run, with what it lacks.
   - **Unassessed platforms:** each declared platform ATT&CK gives no analytic for.

   An analytic **can run** when it has at least one requirement and every requirement is covered. A graph `blinded` cannot judge is reported as an error, never as an empty list.
4. **S4.** An opt-in real-bundle check that records the validated ATT&CK release.

**Non-goals.**

- **Pre-current bundles get no blinded check.** Bundles from before ATT&CK v18 still import, but their detection data is not modelled. On such a graph, `blinded` errors and says so.
- **No log-source alias table.** Names are compared after trimming whitespace and ignoring case (§2), and are otherwise exact.
- **No live sensor ingestion from a SIEM.**
- **No sub-technique roll-up** (the user's call).
- **No technique-wide "blind spots" view.**
- **No population-scoped coverage.** The `coverage-scope` role stays unused.
- **Not S3.** S3 still needs to model tactics and sub-technique links.

**Anti-goals.**

- **Never report a platform the user did not declare.**
- **Never count a zero-requirement analytic as one that can run.** Fifty analytics list no log source.
- **Never let a fallback component name that matches nothing silently grant or deny coverage.**
- **Never return an empty result for a graph the check cannot judge.** An empty result must mean "every claim can see its evidence".
- **Never leave a stale requirement or coverage link after a re-sync.**
- **Never fabricate a sensor field or an empty channel.**
- **No string-interpolated values in TQL.**

## Constraints

- **Repo rules apply:** parameterized TQL, `@doc` on every type and role (a changed `@doc` on an existing type needs a `redefine` migration, `[DEX15]`), greenfield comments, `--test-threads=1`, and never touch the `gecko` database.
- **CI must stay candle-free.** `serde_yaml` (workspace 0.9) is already a dependency.
- **Deletion versus retirement.** `sync_bundle` deletes a removed concept unless a belief edge references it, in which case it is retired. Every new function ignores concepts that have `retired-at`, `revoked true` or `deprecated true`.
- **An in-place concept update keeps the entity, its type and extension relations** (`core/gecko-engine/src/syncer/bundle.rs` §6, `clear_concept_content`), and clears only attributes, outgoing concept-links and citations.
- **Post-sync `GraphWrite`s are insert-only today**, guarded by `not { … }`.
- **`GraphWrite::expect_rows` checks a whole op's total.**
- **A `SchemaMigration` is one query string.** Several ordered steps need several entries.

## Approaches

- **A. Model it in the graph and compute it in TypeDB functions (chosen).** Typed strategy, analytic and requirement records; functions answer "blind where" and "what's missing". This matches `undetected-techniques()`, and an agent reading the graph can find the answer itself.
- *B. Flatten to technique → component at sync time.* Rejected: it cannot answer "which platform".
- *C. Compute it in Rust in the detect bridge.* Rejected: the logic would leave the graph.

## Design

### 1. Schema (`extensions/cyber-gecko/schema/cyber_schema.tql`)

**New types:**

- **`entity detection-strategy sub cyber-object`** plays `strategy-detects:strategy` and `strategy-analytic:strategy`.
- **`entity analytic sub cyber-object`** owns `platform @card(0..)` and plays `strategy-analytic:analytic` and `analytic-requirement:analytic`.
- **`relation strategy-detects`** relates `strategy` and `technique` (played by `attack-pattern`).
- **`relation strategy-analytic`** relates `strategy` and `analytic`.
- **`relation analytic-requirement`** relates `analytic` and `component` (played by `data-component`), and owns:
  - `log-source-name @card(1)`, the name as ATT&CK spells it, trimmed;
  - `log-source-key @card(1)`, the trimmed, lowercased name used for matching;
  - `log-source-channel @card(0..)`, every distinct channel ATT&CK gives for that (analytic, component, name), for display only.

  There is one relation per (analytic, component, trimmed name). A channel of `"None"` (127 of them) or an absent channel writes no channel attribute. *(Author's call.)*
- **`attribute deprecated value boolean`**, owned by `cyber-object`. The adapter writes it on every object carrying `x_mitre_deprecated`. Today that flag exists only inside `metadata-json`.
- **`entity monitored-environment sub cyber-object`** owns `platform @card(1..)`. There is one per sensor file.

**Changes to `sensor`.** It gains `owns platform @card(1..)`, `owns log-source-name @card(0..)`, `owns log-source-key @card(0..)` and `owns covered-component-name @card(0..)`, where `covered-component-name` is a new string attribute. *(Author's call, from the review.)*

- **Why the fallback is an attribute.** Fallback components are stored on the sensor and matched by name at query time, not as a `sensor-coverage` relation.
  - **No stale coverage.** An in-place update clears attributes, so a component removed from a kept sensor stops covering. A relation would have survived the update.
  - **No silent loss on an ATT&CK re-import.** A component dropped or re-id'd upstream cannot silently drop a link, because there is none.
- **`sensor-coverage` is removed**, including `covered-source`, `covered-component` and `coverage-scope`, and the `plays` that reference it. Nothing has ever written it.

**Also removed:**

- `evidence-of`, `requires-evidence` and their roles;
- `data-source` and `data-component`'s `plays` of them;
- `fun blinded-detections`.

**Kept and documented.** `data-source` stays, because deprecated data sources still import. The `@doc`s of `data-source`, `data-component` and `sensor` are rewritten to drop the data-source wording, each with its own `redefine` migration.

**Every new type and role carries a greenfield `@doc`.**

**Migrations** are ordered `SchemaMigration` entries, one query each. In order:

1. delete instances of `evidence-of`, `requires-evidence` and `sensor-coverage` (cyber-owned types only);
2. `undefine` the function;
3. `undefine` the `plays`;
4. `undefine` the roles and relations;
5. one `redefine` per changed `@doc`.

The registry scanner (`core/mem-gecko/src/belief_edge/scan.rs`) does not classify `match … delete`. The builder **probes first** that a data-delete entry runs in the schema transaction and passes the registry without naming a core type. If it cannot, stop and report: do not loosen the registry.

### 2. A concept whose type changes (`core/gecko-engine/src/syncer/bundle.rs`)

Today an existing v20.1 database holds every strategy and analytic as a bare `cyber-object`. An in-place update would keep that type, so every new typed write would match nothing.

*Author's call, from the review:* `sync_bundle` handles a changed `type_hint` inside its guarded transaction.

- **Detection.** The plan compares each existing concept's stored entity type with the manifest's `type_hint`.
- **Replacement.** A concept whose type changed is deleted and reinserted under the new type in the same transaction. It gets fresh containment, attributes and outgoing links, and loses incoming links and extension relations; post-sync writes and other concepts' links recreate those.
- **Belief-referenced concepts are refused.** If a belief edge references such a concept, the sync refuses and names it, because a type change cannot carry the edge. This is checked before anything is written, like the existing `check_type_hints`.
- **Other bundles benefit.** This is generic: S3 will need it again for tactics, which are bare `cyber-object` today. The belief-tier fingerprint guard runs as for any sync.
- **The content hash also changes.** A concept's `file_hash` covers the raw STIX JSON plus an adapter mapping-version constant, which this change bumps, so the first re-import after the upgrade refreshes every concept's attributes (it lands `deprecated`).

### 3. STIX adapter (`extensions/cyber-gecko/src/stix/mod.rs`)

**Mappings.**
- `sdo_type_hint` maps `x-mitre-detection-strategy` to `detection-strategy` and `x-mitre-analytic` to `analytic`.
- `owns_platform` includes `analytic`.
- In `REL_SHAPES`:
  - `"detects"` becomes `strategy-detects`, from `detection-strategy` to `attack-pattern`;
  - the `requires-evidence` shape and the synthesized `evidence-of` block are deleted;
  - a pre-current data-component `detects` now falls to a concept-link.

**Synthesized records.**
- `strategy-analytic` is synthesized from `x_mitre_analytic_refs`.
- `analytic-requirement` rows come from `x_mitre_log_source_references`, grouped by (analytic, component, trimmed name), with the channel set collected.
- A reference to a component not in the bundle is dropped, which matches how external endpoints are handled now.

**Replace, not append.** For `strategy-analytic` and `analytic-requirement`, the post-sync writes first delete every such relation whose strategy or analytic is in this manifest, then insert the current set. A re-sync then reflects a changed release exactly. *(Author's call.)* Counts after a re-sync are unchanged; S4 records the timing beside S1's 1.2 s baseline. `strategy-detects` keeps the insert-only shape: a technique's strategy link does not change content in place.

### 4. Sensor file and `gecko sync-sensors` (new `extensions/cyber-gecko/src/sensors.rs`; a `gecko-bin` CLI arm)

**Format** (YAML):

```yaml
platforms: [Windows, Linux, macOS]        # the platforms you run
sensors:
  - name: windows-edr                     # required, unique; [A-Za-z0-9._-]+
    platforms: [Windows]                  # required, non-empty
    log_sources: [WinEventLog:Security, WinEventLog:Sysmon]
    components: [Process Creation]        # optional fallback, ATT&CK data-component names
```

**Conversion.** `sensors::to_okf(yaml) -> Result<OkfBundle, String>` produces:
- one `monitored-environment` concept, `sensors/environment`;
- one `sensor` concept per entry, `sensors/sensor/<name>`;
- every field as typed attributes. There are no post-sync writes.

**Refused input.** The sync fails and commits nothing, naming the problem, when the file has:
- a missing or empty `platforms`;
- a sensor with no `name`, a name outside `[A-Za-z0-9._-]+`, or a duplicate name;
- a sensor with no `platforms`, or with neither `log_sources` nor `components`;
- a non-string entry.

**The CLI arm.** `gecko sync-sensors <file> --database <db> [--bundle-name <n>]` mirrors `sync-stix`. The bundle name defaults to `sensors`. *(Author's call:)* `--database` is required, because the default would be a fresh database with no techniques.

**Pre-sync check.** Before syncing, the command reads the database:
- **The database must hold ATT&CK.** If it has no `strategy-detects`, the command refuses: "sync the ATT&CK bundle into this database first".
- **Fallback components must match.** Every fallback component name must match exactly one live data component, compared trimmed and case-insensitively. Otherwise the command refuses, naming **every** bad entry, and nothing is synced.

**Post-sync notice.** After syncing, the command prints:
- log-source keys that no live analytic uses;
- sensor platforms not in the top-level `platforms`;
- declared platforms that no live analytic uses.

**Re-syncs.** The same file synced twice changes nothing. A fallback component that later disappears from ATT&CK is reported by `blinded` (§6), not lost silently.

### 5. Functions (`extensions/cyber-gecko/schema/cyber_functions.tql`)

Every function skips retired, revoked and deprecated objects.

- **`requirement-covered($r: analytic-requirement) -> boolean`**, or an inlined equivalent. A requirement is covered when a sensor that shares the analytic's platform owns the requirement's `log-source-key`, or owns a `covered-component-name` equal to the component's name (trimmed, case-insensitive). The builder decides how to normalize the component name for matching, for example by storing a key on `data-component` too.
- **`analytic-gaps($a: analytic) -> { analytic-requirement }`** returns the analytic's uncovered requirements.
- **`claim-platforms() -> { concept, attack-pattern, string, string }`** returns one row per asserted claim and declared platform: the logic, the technique, the platform, and a status.
  - `blind`: the technique's strategy has an analytic for the platform that cannot run.
  - `covered`: it has one that can run.
  - `unassessed`: it has none.
- **`blinded-detections()` is removed.**

### 6. The `blinded` import (`extensions/cyber-gecko/src/detect/mod.rs`)

**Refusals.** The import errors, with a message naming the cause, when:
- the graph has no `monitored-environment` ("no sensor file synced");
- the graph has no `strategy-detects` ("no current ATT&CK detection data").

**Output.** Otherwise it returns every claim that has at least one blind or unassessed platform:

```json
{ "blinded_detections": [ { "concept_id": "...", "technique": "T1059",
    "blind_platforms": [ { "platform": "Linux", "analytic": "AN0110",
        "no_log_sources": false,
        "missing": [ { "log_source": "auditd:SYSCALL",
                       "data_components": ["Network Connection Creation", "Process Creation"] } ] } ],
    "unassessed_platforms": ["macOS"] } ],
  "stale_sensor_components": [ { "sensor": "linux-audit", "component": "Process Creation" } ] }
```

- **`missing` is grouped by log source,** because one sensor entry fixes them all, and lists the components each one would serve, sorted.
- **`no_log_sources` is always present.** When it is `true`, `missing` is `[]`, because no sensor can close that gap. *(Author's call.)*
- **Claims on a retired technique** are reported with every declared platform under `unassessed_platforms`.
- **`stale_sensor_components`** lists fallback names that no longer match a live component.
- **Order is deterministic:** concept id, then platform, then log source.

This changes the output shape for playbooks, and the PR states it.

### 7. Tests

Live TypeDB tests run with `--test-threads=1`.

**Fixture updates.** These fixtures move to the current shape:
- `gecko-bin/tests/cyber_detect_test.rs`;
- `cyber_stix_sync_test.rs` (its `requires-evidence` and `evidence-of` assertions);
- the STIX fixture in `belief_tier_reconciliation_test.rs`.

The rig's disposition table needs no new entries, because none of the new types is on the belief tier.

**`blinded` cases** (each is a test):
- blind with no sensor, and `missing` lists every requirement grouped by log source;
- covered by log source, including a trailing-space or case-variant ATT&CK name matched by a clean sensor entry;
- covered by the component fallback;
- partial coverage is still blind, and lists only the gaps;
- an undeclared platform is never reported;
- a declared platform without an analytic is unassessed;
- a zero-requirement analytic gives `no_log_sources: true` and `missing: []`;
- a sensor on the wrong platform does not cover;
- a sensor removed from the file stops covering;
- a component removed from a kept sensor stops covering;
- a deprecated analytic is ignored;
- a claim on a retired technique is unassessed;
- a fallback component gone from ATT&CK appears in `stale_sensor_components`;
- `blinded` errors with no sensor file, and errors on a pre-current graph.

**Re-sync and migration:**
- a re-import whose analytic lost a log-source reference leaves no stale requirement;
- a re-import with a type change turns a bare `cyber-object` strategy into `detection-strategy` (a database seeded with the pre-change adapter);
- a belief-referenced concept whose type changes is refused and commits nothing;
- migrating a database that holds `evidence-of` / `requires-evidence` instances removes the types.

**`sync-sensors`:**
- each refusal in §4, including every bad component named at once;
- refusal on a database without ATT&CK;
- twice in a row changes nothing.

**Adapter unit tests:**
- type hints;
- `strategy-analytic` synthesis;
- requirement grouping with a channel set, and `"None"` written as no channel;
- a pre-current data-component `detects` becomes a concept-link;
- `typed_relation_players_match_the_schema`.

**S4.** `stix_full_bundle_resync_is_stable` is opt-in: it reads `GECKO_ATTACK_BUNDLE` and skips without it. It imports twice and asserts every relation count, the new ones included, is unchanged. It records the release (Enterprise 20.1) and both sync timings in the ledger.

### 8. Docs

- **Ledger:**
  - D-L decided by this design;
  - S2 ✅ and S4 ✅;
  - the sequence lines;
  - S3's row notes that the type-change handling now exists.
- **KNOWN-LIMITATIONS:**
  - `blinded` needs current ATT&CK and a synced sensor file;
  - name matching is trimmed and case-insensitive, otherwise exact, with no aliases;
  - no population-scoped coverage;
  - a type-changed concept loses its incoming extension relations until the post-sync writes recreate them.
- **`extensions/README.md`:** a line on `type_hint` changes.
- **`detect/mod.rs` module doc:** describes the check.

## Left to the builder

- Role names beyond those given.
- Function bodies, and whether `requirement-covered` is a function or inlined.
- How component names are normalized for matching.
- The YAML struct shape.
- The migration query text.
- How the type-change check reads stored types.
- Fixture ids.
