# OKF v0.2 provenance, trust and lifecycle (O2 + O4) — design

This closes owed-work ledger items **O2** and **O4**, the first of two PRs that adopt OKF
v0.2. The second, **O3** (`sources`, keyed footnotes and the citation fallback), has its own
design. The spec is OKF v0.2,
<https://raw.githubusercontent.com/GoogleCloudPlatform/knowledge-catalog/refs/heads/main/okf/SPEC.md>.
The original rows are `docs/refactor/LEDGER-HISTORY.md:213-215`.

## Intent

The ledger's trigger for O2–O4, "someone needs to sync a v0.2-authored bundle", has fired.
`openwiki/` is an OKF 0.2 bundle that OpenWiki generated from this repo, and the aim is to
dogfood it: GECKO's own wiki synced into GECKO with v0.2's meaning kept.

Today the bundle parses, but every v0.2 field falls into the opaque `metadata-json` blob,
and `timestamp` stays empty because no page has one.

Done when:
- `generated`, `verified`, `status` and `stale_after` are typed facts on the concept;
- the bundle-root `okf_version` is a typed fact on the bundle;
- a malformed value in any of them is warned about by name, kept in `metadata-json` as
  written, and never refused or invented;
- every v0.1 bundle syncs exactly as before.

## Goals, non-goals, anti-goals

- **Goal:** what the spec's §5.2–§5.5 and §8/§12 say, readable by a TypeQL query.
- **Non-goal: anything acting on these fields** (decided with the user and the reviewer).
  - `gecko run` neither refuses nor warns on a `deprecated`, `draft` or stale playbook,
    because the spec calls trust and lifecycle "advisory signals, not access control".
  - No persisted `stale-concepts` function ships. A caller writes
    `$c has stale-after $s; $s <= $now;`.
- **Non-goal:** `sources`, footnotes and the citation fallback. That is O3, the next PR.
- **Non-goal:** Attested Computation (O5).
- **Anti-goal: these fields reaching mem's belief tier** (decided with the user and the
  reviewer). Human review is not entrenchment, and `stale_after` is not a validity window.
  - The fields are concept-tier, so a future playbook can weigh by them without any change
    to mem.
  - The belief-tier surface fingerprint can't see them. It includes only relations whose
    roles are played exclusively by `memory-item` subtypes
    (`core/gecko-engine/src/db/surface.rs:203-207`).
- **Anti-goal:** rejecting a bundle that a conformant OKF consumer accepts (§11).

## Approaches

- **Chosen: typed concept facts, and malformed values warned and kept raw.**
- **Rejected: mapping into mem** (see the anti-goals).
- **Rejected: refusing a malformed value at parse,** the N5/N10 precedent.
  - That precedent covers GECKO's own execution fields (`engine`, `timeout-ms`, `scopes`),
    where a wrong value changes what runs, so those keep refusing.
  - OKF's provenance fields are a third-party format, and its §11 says to treat their
    constraints "as soft guidance".
- **Rejected: dropping a malformed value silently,** which is today's `timestamp`
  behaviour. It loses the author's mistake without a word.

## Design

### The malformed-value rule

This covers `generated`, `timestamp`, `verified`, `status`, `stale_after` and `okf_version`.

- **A key that parses wholly and holds only known subkeys** is typed and removed from
  `metadata-json`.
- **A key with an unknown subkey** (`generated: { by, at, model: x }`, or an extra field
  in a `verified` event) has its known parts typed. The key also stays whole in
  `metadata-json`, as written, with no warning: §4.1 says unknown keys are legitimate and
  "SHOULD" be preserved. My call: this loses nothing and doesn't make a producer's
  extension look like a mistake.
- **A key with any malformed part stays whole in `metadata-json`, as written,** so nothing
  is lost. The parse adds a warning naming the file, the key and the value as written,
  using the N10 `yaml_as_written` helper.
- **The well-formed parts of that key are still typed:** a valid `generated.at` beside a
  missing `by`, and the valid events of a partly bad `verified` list.
- **No typed value is invented for a malformed part.**
- **Today's `extract_timestamp` silent drop** (`parser.rs:470-484`) moves to this rule.

**Timestamps.** Every timestamp-valued key is an RFC 3339 datetime with an explicit UTC
offset, as §5 says. A date-only value (`2026-06-01`), or a datetime without an offset, is
malformed. My call: the spec gives full datetimes everywhere, and normalising a date would
invent a time.

**Warnings.** Warnings are collected on the parse result:
- `OkfBundle.warnings: Vec<String>`, which defaults to empty for the extension
  converters;
- `gecko sync` prints each one as `  warning: …` after the sync result, the way
  `sync-sensors` prints notices.

They live on the parse result rather than on `SyncResult`, because the parser finds them.
That is my call; the reviewer suggested `SyncResult`.

### Fields

| Frontmatter | Graph | Notes |
|---|---|---|
| `generated.at` | the existing `timestamp` attribute | Wins over a legacy `timestamp`. The legacy key is read only when there is no valid `generated.at` (§13.1). A `timestamp` that a valid `generated.at` overrides stays in `metadata-json` as written, with no warning, because it isn't read. My call: dropping it would be a silent loss |
| `generated.by` | `generated-by` (string) | Stored as written. `"claude-code"` breaks §7's `<producer>/<version>` and is still accepted. `generated` without `by`, which §5.2 calls REQUIRED, is malformed |
| `verified` | a `verification` relation per event, plus `trust-tier` on the concept | See below |
| `status` | `okf-status` (string), stored as written | Any non-empty string, including unknown ones such as `published`; §5.4 lists three values but does not restrict them. A non-string or blank value is malformed. Absent means `stable` by §5.4; nothing is stored (my call: store what the author wrote; the attribute's doc states the default) |
| `stale_after` | `stale-after` (datetime) | Never a stale flag: a flag written at sync goes stale without anything changing |
| root `index.md` `okf_version` | `okf-version` (string) on `bundle` | See below |

**`verified`**
- **The `verification` relation.** It is a unary relation: one role,
  `verification:verified-concept` played by `concept`, owning `verified-by` and
  `verified-at`, each `@card(1)`. Pairing two multi-valued attributes on the concept would
  lose which actor goes with which time.
  - GC (`delete_concepts_gc`) and retirement (`retire_concepts`) already delete every
    non-belief relation a concept plays, so these relations need no new cleanup there.
  - `clear_concept_content` (`bundle.rs:879`) deletes only attributes, `concept-link` and
    `citation`, so it gains a delete of the concept's `verification` relations.
  - **It is guarded by a schema check** (added at approval, from the review).
    - `gecko sync` applies the schema first (`gecko-bin/src/main.rs:803`), but
      `sync_bundle` is a library entry point. `tightened_schema_applies_to_a_populated_database`
      (`gecko-bin/tests/schema_init_test.rs:1075`) syncs against the frozen `PRE_CORE`
      schema, and a query that names an undefined type fails at parse.
    - `sync_bundle` already reads the schema once (`bundle.rs:352`). From that read it
      decides whether the schema defines `verification` and `okf-version`.
    - It issues the `verification` delete and the `okf-version` clear only when they are
      defined. The writes need no guard, because they happen only when the parsed bundle
      carries a value.
    - The check reuses `db::required::defines_attribute`. A sibling check for relations,
      or the `verified-by` attribute standing in for it, is the builder's choice.
  - Exact duplicate events are written once.
- **A bare `{ by, at }` mapping is a one-element list** (§5.2, MUST).
- **An event is valid** when `by` is a non-empty string and `at` is a valid timestamp.
- **`trust-tier`** is derived by the parser and is sound there, because it depends only on
  the file. It is one of `unverified`, `machine-confirmed` or `human-reviewed`, held to
  those three by `@values`, since GECKO computes it. It is written **only when `verified`
  is present**.
  - **Key absent:** nothing is stored. §5.3 makes absence mean `unverified`, and the
    attribute's `@doc` says so, as with `okf-status`.
    - This is my call; the builder check raised it. If absence were written as
      `unverified`, every v0.1 concept would gain an attribute on its next sync.
    - The STIX and sensor converters, which never pass through the parser, would also
      need to set a tier.
    - So `OkfConcept.trust_tier` is an `Option` that they leave `None`.
  - **`verified: []`:** `unverified` is stored, because the author explicitly wrote an
    empty list. An empty list classifies as `unverified`; beware `[].iter().all(..)`
    returning true.
  - **Any valid event whose `by` starts with `human:` and has a non-empty id:** the tier
    is `human-reviewed`.
  - **Valid events, none of them human:** the tier is `machine-confirmed`.
  - **Malformed as a whole** (`verified: 5`), **or a list with no valid entry:** no
    `trust-tier` is written, never a fabricated `unverified`. Bad entries in a mixed list
    are skipped with a warning each.
    - Broken and absent then look the same on `trust-tier`.
    - A query tells them apart by the raw `verified` left in `metadata-json`, and the sync
      warned about it.
  - **The `human:` match is case-sensitive.** `HUMAN:x` and a bare `human:` are not human.
    My call: §7 says producers "MUST use" `human:`, and it is silent on case and on an
    empty id.

**`okf_version`**
- **Only the bundle-root `index.md` is read,** for its frontmatter's `okf_version` alone,
  and it stays reserved: it is not a concept (§3.1).
- **The value is stored as written** through `yaml_as_written`, so an unquoted
  `okf_version: 0.2` is stored as `0.2`.
- **An unknown version is consumed best-effort** (§12).
- **No warning for:**
  - a missing root `index.md`;
  - an `index.md` without frontmatter;
  - an `index.md` without the key.
- **A warning, and no stored value, for:**
  - unparseable frontmatter (the file is not a concept, so it doesn't fail the sync);
  - any value other than a non-empty string or a YAML number: `null`, a blank
    `okf_version:`, `""`, a boolean, a list or a map.
- **The value is written in `upsert_bundle`** (`bundle.rs:467`) and cleared when the root
  `index.md` drops it.
- **`OkfBundle` gains `okf_version: Option<String>`.** Its five construction sites (the
  parser, `sensors.rs`, `stix/mod.rs`) set `None` outside the parser.

**Spec note (§12 vs §13).** §13 calls v0.2 a minor bump, yet lists two breaking changes.
GECKO reads both the v0.1 fallbacks and the v0.2 fields whatever `okf_version` says, so the
inconsistency changes nothing here.

### Schema

The new types go in `core_schema.tql`. They are additive `define`s, so no migration is
needed. Each attribute, relation and role carries a greenfield `@doc`:
- the attributes `generated-by`, `trust-tier`, `okf-status`, `stale-after`, `verified-by`,
  `verified-at` and `okf-version`;
- the `verification` relation and its role;
- `concept` owns `generated-by`, `trust-tier`, `okf-status` and `stale-after`, each
  `@card(0..1)`, and plays `verification:verified-concept`;
- `bundle` owns `okf-version @card(0..1)`.

Names are prefixed `okf-` where a bare name could clash later, as with `status`. No
current schema defines any of these names.

### Docs

- **`docs/refactor/KNOWN-LIMITATIONS.md`:**
  - **Already-synced files.** An unchanged file synced before this change keeps its old
    content-hash plan, so it gains no v0.2 attributes until it is edited. This matches the
    `timeout-ms` line and was decided with the reviewer: folding a parser version into the
    hash would re-sync ATT&CK-sized bundles on every parser change.
  - **Malformed values.** A malformed v0.2 provenance value is warned about and kept raw,
    not refused.
- **The ledger:**
  - O2 and O4 move to Landed;
  - O3 goes to Next up, carrying the reviewer's notes for its design:
    - an explicit `sources: []` means no citations, with no fallback to body links;
    - the footnote scan skips code fences and `[^id]:` definition lines;
    - an unmatched label is warned about;
    - a resource is stored as given, with `external-resource`'s `@doc` widened by a
      `CORE_MIGRATIONS` redefine;
    - the dedup key for two entries naming one resource is decided.
- **The OKF section of the parser docs** in `extensions/README.md`, if it lists frontmatter
  keys.

## Tests

- **Parser unit tests** (`parser.rs`):
  - one per field, valid and malformed, each malformed case checking the warning text and
    that the raw value stays in `extra_metadata`;
  - `generated.at` over `timestamp`, and `timestamp` used when `generated` has no valid
    `at`;
  - a date-only and an offset-less timestamp are malformed;
  - the trust tiers: absent, `[]`, a bare mapping, machine only, mixed human and machine,
    `HUMAN:x`, bare `human:`, `verified: 5`, and a list with one bad entry beside a good one;
  - `status: published` is stored, and `status: 3` is malformed;
  - `okf_version`: quoted, unquoted, a non-scalar, frontmatter that won't parse, and a
    subdirectory `index.md` whose key is ignored.
- **Sync integration tests** (TypeDB):
  - each attribute and the `verification` relation round-trip;
  - an edited concept replaces its verification events and doesn't add to them;
  - GC and retirement leave no `verification` behind;
  - `okf-version` is set and then cleared on the bundle;
  - `gecko sync` prints the warnings (a CLI test);
  - `sync_bundle` of a **v0.1** bundle, followed by an edited update, still syncs against
    a schema without `verification` or `okf-version`
    (`tightened_schema_applies_to_a_populated_database` stays green). A bundle that
    carries v0.2 values fails loudly at TypeDB parse on such a schema, as `timeout-ms`
    does. This was amended at plan review: the writes are deliberately unguarded, and
    skipping them silently would fabricate success.
- **A hand-written v0.2 fixture bundle** holding every case above that needs a file: a
  `human:` verifier, `status`, `stale_after`, a bare-mapping `verified`, and a root
  `index.md` with `okf_version: "9.9"`.
- **An `openwiki/` smoke test** (serviceless, in CI, with loose assertions so a
  regeneration doesn't break an unrelated PR):
  - the repo's own bundle parses with no warnings;
  - every page has a `timestamp` from `generated.at` and a `generated_by`;
  - every page with `verified` is `machine-confirmed`, and the rest have no tier;
  - `okf_version` is `Some("0.2")`.
- **Regression:** `sample-bundle/`, the cyber sample bundle and the STIX import sync
  unchanged; the walkthrough tests already cover them.

## Before this PR

The **Q11 link fix** lands as its own small PR first. `resolve_relative_path`
(`linker.rs:113-147`) returns `None` for a link whose `..` climbs above the bundle root, so
the link is dropped as a tolerated broken link instead of resolving to a wrong concept. It
must land before the OpenWiki link-fix run, which may produce links out of the wiki to
repository files.

## Left to the builder

- Exact `@doc` wording, field names on `OkfConcept`, and the warning wording within the
  shape above.
- Where the parsing helpers live, in `parser.rs` or a new `okf/provenance.rs`.
- The fixture bundle's path and layout.
