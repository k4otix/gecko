---
type: subsystem
title: OKF Bundles and the Parser
description: The OKF bundle layout GECKO ingests — Markdown files with YAML frontmatter under a directory with an optional bundle.json — and how the parser derives concept ids, GECKO frontmatter (engine, scopes, timeout-ms), one program per concept, links, citations and directory hierarchy.
tags: [okf, parser, bundles, frontmatter, playbooks]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-28T02:42:19.865Z
sources:
  - id: openwiki-source-5ef877dd43feaca6e2d26d7e
    resource: repo://core/gecko-engine/src/okf/linker.rs
  - id: openwiki-source-9478224a11f0f3ba15b4145c
    resource: repo://core/gecko-engine/src/okf/parser.rs
  - id: openwiki-source-3362e981e7b74e13c0d406bb
    resource: repo://core/gecko-extension-api/src/okf.rs
generated: { by: "claude-code", at: "2026-11-28T02:42:19.865Z" }
---

# OKF Bundles and the Parser

An OKF bundle is a directory tree of Markdown documents with YAML frontmatter. The parser (`core/gecko-engine/src/okf/parser.rs` and `linker.rs`) turns it into an `OkfBundle`, whose types live in `core/gecko-extension-api/src/okf.rs` so importing extensions (such as cyber's STIX adapter) can build bundles directly.

## Bundle layout

```
sample-bundle/
├── bundle.json            # optional: {"name": "...", "description": "..."}
├── concepts/gecko_engine.md
└── playbooks/hello_js.md
```

- **Name.** `bundle.json`'s non-empty `name`, else the directory name. A missing or invalid manifest silently falls back. The name identifies the `bundle` entity and, by default, the target database; it does **not** namespace concept ids.
- **Files.** Every `*.md` file found recursively, sorted for deterministic output. `index.md` and `log.md` (case-insensitive) are reserved and skipped.
- **Concept id.** The bundle-relative path without `.md`, POSIX separators, no prefix (`playbooks/hello_js`). Ids must not collide across bundles sharing a database; the syncer refuses a collision.

## Frontmatter

`parse_concept` splits a leading `---` … `---` block and parses it as YAML.

| Key | Field | Rule |
| --- | --- | --- |
| `type` | `concept_type` | **Required**, non-empty after trim (`MissingType` / `EmptyType`). |
| `title`, `description`, `resource`, `tags`, `timestamp` | standard OKF fields | Optional. |
| `engine` | `engine: Option<ScriptEngine>` | `js` / `javascript` / `quickjs` → `QuickJs`. Any other value yields `None`, which makes the concept non-executable documentation. |
| `scopes` | `scopes: Vec<String>` | S3 capability grants, e.g. `[mem:recall, cyber-gecko:claim]`. |
| `timeout-ms` | `timeout_ms` | Integer in `1..=MAX_TIMEOUT_MS`, else `InvalidTimeout`. |
| anything else | `extra_metadata` | Kept as strings (non-strings JSON-encoded); synced as `metadata-json`. |

`file_hash` is the SHA-256 of the raw file and drives the syncer's change detection.

## One program per concept

`extract_program(body, engine)` collects the fenced code blocks whose language tag resolves to the declared engine and joins them in document order with blank lines. Fences in other languages (`python`, `sql`, `mermaid`, untagged) stay documentation, so prose, illustrative snippets and the real program can be interleaved. The join happens at parse time because TypeDB attribute sets are unordered. A concept with no engine or no matching fence has `program: None` and `PlaybookRun::from_concept` returns `None` for it.

```markdown
---
type: playbook
engine: quickjs
scopes: [mem:recall, mem:derive]
---
```

<!-- openwiki: broken internal link [/openwiki/sandbox/js-guest.md] link "/openwiki/sandbox/js-guest.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
The final evaluated expression of the program becomes the run's structured output; see [JavaScript Guest and Playbook API](/openwiki/sandbox/js-guest.md).

## Links, citations and hierarchy

`extract_links` scans `[text](target)` (skipping image links `![…]`):

- `http://` / `https://` targets → `OkfCitation` (synced as `citation` → `external-resource`).
- Targets ending `.md` or containing `.md#` / `.md?` → `OkfLink` to a concept id resolved by `resolve_relative_path`: anchors and queries are stripped, `.md` removed, a leading `/` is bundle-root-relative, otherwise relative to the source's directory (`../tables/orders.md#schema` from `datasets/users` → `tables/orders`).
- Other targets are ignored.

`parse_bundle` emits `HierarchyEdge`s from each file's directory up through every ancestor directory, with `""` as the root parent, then de-duplicates them. Directory ids equal their path, matching concept ids.

## Extension-built concepts

<!-- openwiki: broken internal link [/openwiki/extensions/cyber-gecko.md] link "/openwiki/extensions/cyber-gecko.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Importers fill fields the Markdown parser leaves empty: `type_hint` (insert as a `concept` subtype instead of plain `concept`) and `typed_attributes` (real typed `owns`, written through `given` rows after the label passes `validate_schema_label`). See [cyber-gecko](/openwiki/extensions/cyber-gecko.md).

## Tests

Unit tests in `parser.rs` and `linker.rs` cover frontmatter errors, timeout bounds, multi-fence programs, link resolution and hierarchy. `core/gecko-engine/tests/pipeline_tests.rs` parses and runs concepts end to end. `sample-bundle/` is the canonical fixture.

<!-- openwiki: broken internal link [/openwiki/okf/graph-sync.md] link "/openwiki/okf/graph-sync.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Next: [Bundle Sync into TypeDB](/openwiki/okf/graph-sync.md).
