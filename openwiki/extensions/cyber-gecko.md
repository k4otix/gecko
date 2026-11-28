---
type: extension
title: cyber-gecko Detection-Engineering Extension
description: The optional cyber-gecko domain extension — its STIX/ATT&CK schema, the sync-stix and sync-sensors import paths, the claim/disposition belief writes, and the coverage_gaps, blinded, recommend_sources and precision reports served through its host bridge.
tags: [cyber, extension, stix, attack, detection, sensors]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-28T02:42:19.865Z
sources:
  - id: openwiki-source-95fa3adec5dda1f54bd35e1b
    resource: repo://extensions/cyber-gecko/src/detect/mod.rs
  - id: openwiki-source-ede84d411a82d3d81bc1f4ac
    resource: repo://extensions/cyber-gecko/src/lib.rs
  - id: openwiki-source-8ea282978e07804c45b27fdc
    resource: repo://extensions/cyber-gecko/src/sensors.rs
  - id: openwiki-source-d1a98b3ef213d1769930dd97
    resource: repo://extensions/cyber-gecko/src/stix/mod.rs
  - id: openwiki-source-c42ae122f8faf30c8246ac70
    resource: repo://gecko-bin/src/main.rs
generated: { by: "claude-code", at: "2026-11-28T02:42:19.865Z" }
---

# cyber-gecko Detection-Engineering Extension

<!-- openwiki: broken internal link [/openwiki/architecture/assembler-and-extensions.md] link "/openwiki/architecture/assembler-and-extensions.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
`extensions/cyber-gecko` is the reference domain extension. It compiles in under the `cyber` feature of `gecko-bin` and is activated with `enabled = ["cyber"]` in `gecko.toml`. It follows the contract in [Assembler Pattern and Extension Contract](/openwiki/architecture/assembler-and-extensions.md) and depends only on `gecko-extension-api`. The user-facing reference, with a full worked example against ATT&CK Enterprise, is `extensions/cyber-gecko/README.md`.

## Schema

`CyberGecko::schema()` concatenates `schema/cyber_schema.tql` and `schema/cyber_functions.tql`; `schema_migrations()` returns `migrations::CYBER_MIGRATIONS`.

- **Record types** subtype the core `concept` through `cyber-object` (keyed by the STIX id): `attack-pattern`, `tactic`, `intrusion-set`, `threat-actor`, `campaign`, `malware`, `tool`, `indicator`, `observable`, `course-of-action`, `data-source`, `data-component`, `detection-strategy`, `analytic`, `host-asset`, `monitored-environment`, `sensor`.
- **Record relations**: `subtechnique-of`, `revoked-by`, `technique-tactic`, `uses`, `indicates`, `attributed-to`, `targets`, `mitigates`, `observed-as`, `strategy-detects`, `strategy-analytic`, `analytic-requirement` (a log source an analytic reads for a data component).
- **Belief edges** (declared in `CYBER_BELIEF_EDGES`, `src/lib.rs`):
  - `technique-detection` — belief role `detection-claim`; players `detecting-logic` (a `concept` by `concept-id`) and `detected-technique` (an `attack-pattern` by `attack-id`).
  - `alert-disposition` — belief role `disposition-belief`; player `dispositioned-logic`; required owned attribute `detection-verdict`.
- **TypeDB functions** back the reports: `undetected-techniques`, `requirement-covered`, `analytic-gaps`, `live-analytic`, `has-live-requirement`, `claim-platforms`.

<!-- openwiki: broken internal link [/openwiki/memory/belief-edges.md] link "/openwiki/memory/belief-edges.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Belief edges are written through mem's `BeliefEdgeWriter` so the belief and its edge commit together; see [Belief Edges and the Belief-Tier Boundary](/openwiki/memory/belief-edges.md).

## Import paths

### `gecko sync-stix <file>`

`cyber_gecko::stix::to_okf` parses a STIX 2.1 bundle into an `OkfBundle` of typed concepts (with `typed_attributes`, not `metadata-json`) plus `StixRelations`. `cyber_post_sync` turns those relations into multi-row parameterized `GraphWrite`s grouped by query shape, which the engine runs **inside the sync transaction**. Concept ids and log-source values travel as parameters; only fixed relation/role labels and resolved role-player subtypes are interpolated. A re-import under the same `--bundle-name` (default `stix-sync`) replaces the previous one, including strategy/analytic/requirement links. Each ATT&CK domain belongs in its own database; overlapping objects in a second domain are refused as a concept-id collision (`stix_collision_guidance` in `gecko-bin/src/main.rs` adds guidance to that error).

### `gecko sync-sensors <file> --database <db>`

`sensors.rs` parses a YAML file of `platforms` and `sensors` (each with `name`, `platforms`, `log_sources` and/or fallback `components`), refusing unknown fields. `cmd_sync_sensors` (`gecko-bin/src/main.rs`) requires `--database`, checks that it exists and holds ATT&CK detection strategies, that the bundle name (default `sensors`) holds only sensors, that platforms match ATT&CK spelling exactly, and that each fallback component matches exactly one live data component. `to_okf` emits one `monitored-environment` and one `sensor` per entry. After syncing it prints notices for unused log sources and uncovered platforms. Several sensor files may share a database under different bundle names; `blinded` reads their union.

## Host imports and the detect bridge

`host_imports()` declares `claim`, `disposition`, `coverage_gaps`, `blinded`, `recommend_sources`, `precision`. None is served by `call_import`, which always errors that the detect host bridge is not wired. They are served by `detect::cyber_extension_callback(graph, edges, ctx, base)`, which intercepts `cyber-gecko` calls whose function is in `DETECT_FNS`, runs `dispatch` via `block_in_place` + `block_on`, and passes everything else to `base` (mem's bridge). `build_host_bridge` folds it in only when cyber-gecko is registered.

Each call needs its S3 scope in the playbook frontmatter, e.g. `scopes: [cyber-gecko:claim, mem:retract]`.

| Function | Effect |
| --- | --- |
| `claim` | Validates `logic_id`, `technique_id`, `method` (a `DerivationMethod`), `confidence ∈ [0,1]`, optional `evidence`; refuses a missing logic or technique (and a retired technique, naming its replacement); writes a private belief with a `technique-detection` edge; replies `{belief_id, logic_id, technique_id}`. |
| `disposition` | Records `true-positive` / `false-positive` / `benign-true` on an existing logic as a belief with an `alert-disposition` edge. |
| `coverage_gaps` | Live techniques with no asserted claim, plus a per-tactic `live`/`undetected` grouping; retired, revoked and deprecated objects are excluded. |
| `blinded` | For each asserted claim × declared platform: covered, blind (analytics exist but none can run with current sensors) or unassessed. Errors when no sensor file is synced or no current ATT&CK detection data exists. |
| `recommend_sources` | Greedy planner (`detect/recommend.rs`): each step adds the analytic log sources that unblind the most (claim, platform) pairs per new source; lists `unfixable` pairs. |
| `precision` | TP / (TP + FP) over a trailing `window` (1 s – 100 years); `benign-true` counts as neither; `null` when both are 0. Does not validate that `logic_id` exists. |

<!-- openwiki: broken internal link [/openwiki/memory/sandbox-mem-bridge.md] link "/openwiki/memory/sandbox-mem-bridge.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Withdrawing a claim is a mem operation: `mem.retract(belief_id, reason)` from a playbook granted `mem:retract`. It returns the blast radius, i.e. beliefs that rest on the claim, for example a verdict recorded with `assumes` (a `rests-on` edge via `mem.restsOn`). See [Sandbox → mem Bridge](/openwiki/memory/sandbox-mem-bridge.md).

## Sample assets

- `extensions/cyber-gecko/sample-bundle/` — two detections and five playbooks (`claim_detection`, `coverage_report`, `record_verdict`, `detection_precision`, `withdraw_claim`) that take inputs from `gecko run --args`.
- `extensions/cyber-gecko/sample-sensors.yaml`.

## Tests

- `gecko-bin/tests/cyber_stix_sync_test.rs` — STIX import, re-import replacement, collisions.
- `gecko-bin/tests/cyber_sensors_sync_test.rs` — sensor-file refusals and notices.
- `gecko-bin/tests/cyber_detect_test.rs` — the detect functions end to end.
- `gecko-bin/tests/cyber_sample_test.rs` — the README walkthrough on the sample bundle.
- Unit tests in `src/lib.rs` check that the shipped schema has no cut labels and uses current relation syntax.

Caveats about ATT&CK's detection model and `blinded` are in `docs/refactor/KNOWN-LIMITATIONS.md`.
