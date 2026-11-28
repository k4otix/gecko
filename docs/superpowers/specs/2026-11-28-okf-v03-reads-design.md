# OKF v0.3 reads (O7) — design

This closes owed-work ledger item **O7**. The spec is OKF v0.3,
<https://github.com/open-knowledge-format/open-knowledge-format/blob/main/SPEC.md> (released
2026-11-18; read at commit `527e81a`). Section numbers below are v0.3's. It extends the O2 + O4
design, `docs/superpowers/specs/2026-11-28-okf-v02-provenance-and-trust-design.md`, which is
landed and not edited: where the two disagree, this one wins.

## Intent

GECKO's O2 + O4 code reads OKF frontmatter by v0.2's text. v0.3 makes that text precise and
changes how a consumer reads some existing values (§13). The aim is that every fact GECKO puts
in the graph means what v0.3 says it means, for any reader of the graph: an agent writing
`$c has stale-after $s; $s <= $now;` or `$c has trust-tier "human-reviewed";` gets the right
answer without knowing anything GECKO-specific.

Done when, for any bundle:
- the trust tier counts only current verification events (§5.3);
- a `.md` file that is not a concept by §3.2 is skipped, not a failed sync, and dot-paths and
  symlinks are never read;
- an unreadable `stale_after` reads as stale;
- date-only timestamps, empty values, scalar `tags` and unknown `status` values read as
  §5, §5.4 and §11.2 say;
- a database synced by the v0.2 reader picks all of this up on its next sync, without the user
  editing any file.

## Goals, non-goals, anti-goals

- **Goal:** v0.3's consumer rules (§3.2, §5, §5.3–§5.5, §11.2) for every key GECKO already reads.
- **Goal:** the change reaches already-synced databases (see "Re-reading unchanged files").
- **Non-goal: new fields.** `superseded_by` (§5.4) and the bundle-root `index.md`'s `title`,
  `description` and `license` (§8.1), weighed against `bundle.json`, are split into ledger item
  **O7b** (decided with the user). Each needs its own graph shape.
- **Non-goal:** `sources`, keyed footnotes and `sources` path resolution. That is O3, designed
  against v0.3 after this lands.
- **Non-goal: acting on these fields.** As in O2 + O4, `gecko run` neither refuses nor warns on
  a stale, draft, deprecated or unverified playbook (§5.3: "advisory signals, not access
  control").
- **Non-goal:** GECKO's own keys. `engine`, `timeout-ms` and `scopes` keep their current rules;
  §11.2 governs keys the OKF spec defines.
- **Anti-goal:** a stored fact a naive reader misreads: an unreadable `stale_after` that reads
  as fresh, or a `human-reviewed` tier on text no human read after its last change.
- **Anti-goal:** refusing a bundle a conformant v0.3 consumer accepts (§11).
- **Anti-goal:** depending on the `okf` crate (D-OKF-crate). Its README's "Upgrading a bundle
  from OKF v0.2" table is a cross-check only.

## Constraints

- The v0.3 rules apply whatever `okf_version` a bundle declares, 0.2, none or an unknown one
  (decided in the ledger). 0.2 bundles stay readable (§12.1), and the tier change only lowers a
  tier, "the safe direction" (§13.1).
- `openwiki/` (declares `okf_version: "0.2"`) still parses, and its pages still read as
  `machine-confirmed`: each page's `verified.at` equals its `generated.at`, so its event is
  current.
- The O2 + O4 malformed-value rule stands: a value GECKO can't read is warned about by file,
  key and value, kept in `metadata-json` as written, and not invented. The one deliberate
  exception is the `stale-after` sentinel below, which the spec forces.
- Schema `@doc` changes go through `CORE_MIGRATIONS` as `redefine … @doc(…)` entries, the
  pattern of `core/bundle-shared-database-doc` (`core/gecko-engine/src/db/mod.rs:17-22`).

## Approaches

### An unreadable `stale_after` (§5.5: "MUST NOT treat … as fresh")

- **Chosen (A): a sentinel instant.** Store `stale-after` as `1970-01-01T00:00:00Z`
  (`DateTime::UNIX_EPOCH`). The concept is then stale at every `now`, which is exactly what
  "never fresh" means, and the plain `stale-after <= now` query gives the right answer. The raw
  value stays in `metadata-json` and `gecko sync` warns. The epoch, not `DateTime::MIN_UTC`:
  year −262143 does not survive TypeQL's datetime literals and value range, and the epoch is
  safe in TypeDB, TypeQL, chrono and JS. `stale-after`'s `@doc` names the sentinel.
- **Rejected (B): a separate marker attribute** (`stale-after-malformed`), leaving
  `stale-after` absent. Every reader must know to check two attributes, so the obvious query
  stays wrong, which is the anti-goal.
- **Rejected: a boolean stale flag written at sync.** O2 + O4 rejected it: it goes stale with
  nothing changing.

### Scope

- **Chosen: O7 reads existing values by v0.3; O7b adds `superseded_by` and bundle metadata**
  (decided with the user).
- **Rejected: one PR for all eight ledger sub-items.** The new fields need graph-shape
  decisions (a relation or a string for `superseded_by`; `index.md` versus `bundle.json` for
  bundle title and description) unrelated to reading values correctly.

## Design

Code: `core/gecko-engine/src/okf/provenance.rs` (`take_provenance`, `parse_time`),
`core/gecko-engine/src/okf/parser.rs` (`parse_concept`, `parse_bundle`, `walk_md_files`,
`read_okf_version`, `extract_string`, `extract_string_list`),
`core/gecko-engine/schema/core_schema.tql`, `core/gecko-engine/src/db/mod.rs`.

### 1. The trust tier counts only current events (§5.3)

- **Currency compares against the content time GECKO stores, the concept's `timestamp`
  attribute** (decided with the user). An event is **current** when the concept has no
  `timestamp`, or when its `at` ≥ `timestamp`. Equal counts. So any reader can repeat the check
  from the graph (`verified-at >= timestamp`, or no `timestamp`) and always agree with the
  stored `trust-tier`; no rule lives only in the parser.
- The content time is unchanged from O2 + O4: `generated.at` when readable, otherwise the legacy
  `timestamp` (Appendix B). Its consequences:
  - `generated` with a readable `at` compares against `generated.at`, as §5.3 says. That includes
    `generated: { at: T }` with no `by` (the missing `by` still warns).
  - No `generated` and a legacy `timestamp`: compares against `timestamp`. Appendix B lets a
    consumer use it as the content time, and it can only lower a tier.
  - No readable content time at all (no `generated`, `generated.at` absent or malformed, and no
    readable `timestamp`): every event is current. A malformed `generated.at` already warns.
    This is the one case the strict reading would count no event, and the user accepted it for
    the graph's self-consistency.
- Tier: a current event from a `human:<id>` actor gives `human-reviewed`; current events from
  other actors only give `machine-confirmed`.
- **No current event writes no `trust-tier`.** That covers absent `verified`, `verified: []`
  (the same as absent, §11.2), a malformed `verified`, entries all ignored for a missing
  `by`/`at`, and valid events that all predate the concept's `timestamp`. Graph readers get one rule: no
  `trust-tier` means unverified (decided with the user). `"unverified"` stays in the attribute's
  `@values`, so no `@values` migration is needed. The Rust `TrustTier::Unverified` variant goes
  if nothing constructs it.
- **Every valid event is still stored** as a `verification` relation, current or not, so a
  reader can show "verified before the last change" (§5.3).
- `trust-tier`'s `@doc` becomes: derived from the current verification events, those at or
  after the concept's `timestamp` (all of them when it has none); absence means unverified.

### 2. Which files are concepts (§3.1, §3.2)

- **Frontmatter delimiters follow §4** (*my call*, from the builder check). The frontmatter
  opens only when the file's first line is `---`, and closes at the next line that is `---`.
  - Trailing spaces, tabs and `\r` are ignored on both lines, so CRLF files read alike. A closer
    written `--- ` then never makes an already-synced concept vanish. This tolerance goes beyond
    §4's "exactly"; it is *my call*, from the plan review.
  - One leading UTF-8 BOM is ignored.
  - A file whose first line is blank and whose first non-blank line is `---` is not a concept,
    and it warns (it is never quietly skipped). Today
  `split_frontmatter` accepts any first line starting with `---` and any later line starting
  with `---`. So a frontmatter-less file whose first line is a `----` rule, with another one
  later, fails the whole sync on prose parsed as YAML.
- **A `.md` file is a concept only when it has frontmatter with a non-empty `type`.**
  - No frontmatter at all (the first line isn't exactly `---`): skipped quietly. §3.2 names
    `README.md` and `AGENTS.md` as the expected case, and a warning on every sync would be noise.
    *My call.*
  - An opening `---` with no closing `---`: skipped with a warning. The author meant
    frontmatter.
  - Frontmatter (including empty frontmatter) with no `type`, or `type` null, empty or blank:
    skipped with a warning such as `Markdown file '<path>' has no 'type'; it is not a concept and
    is not synced`.
  - `type` a number or a boolean: the concept's type, as written (the `extract_string` rule).
    `type` a list or a mapping: skipped with a warning.
  - `ParseError::MissingType` and `EmptyType` go if nothing returns them.
- **A previously synced concept whose file stops being a concept** is dropped from the bundle,
  so the sync garbage-collects it, or retires it when a belief edge references it
  (`core/gecko-engine/src/syncer/bundle.rs`, `retire_concepts`). The warning is what makes this
  visible.
- **Frontmatter that is not valid YAML, or not a mapping, still fails the sync** (unchanged).
  The bundle is non-conformant (§11.1), and §11 doesn't list it among what a consumer must accept.
- **Dot-paths (§3.2 rule 4).** A file or directory below the bundle root whose name starts with
  `.` is not part of the bundle. The walk skips it quietly, at any depth. The root's own name
  doesn't matter.
- **Symlinks (§3.2 rule 5).** The walk reads entry types without following links
  (`DirEntry::file_type`) and **never follows a symlink**, file or directory. Each skipped
  symlink gets a warning naming the reason: its target is outside the bundle root, it is broken,
  or it is inside the bundle (its target is read at its own path). *My call, refining the user's
  detail 3:* an in-bundle target is already walked at its real path, so following the link
  would only add a duplicate concept under a second id, and the forbidden out-of-root case is
  then never reached. No symlink is followed, so no cycle is possible and no visited set is
  needed. A link in a body that names the symlink's path resolves to no concept (a broken link,
  which §11.2 tolerates). The walk sorts each directory's entries so its output and warnings are
  deterministic.
- **The root `index.md`** (`read_okf_version`, `pick_root_index`) is read only when it is a
  regular file. A symlinked root index is not read, and a warning says so. It is never treated
  as a concept (unchanged).

### 3. An unreadable `stale_after` is stale (§5.5)

- A present `stale_after` that `parse_time` can't read (including a blank string) stores
  `stale-after = 1970-01-01T00:00:00Z`, stays in `metadata-json` as written, and warns. The
  warning says the value is stored as stale, not "not typed".
- `stale_after: null` or `""` is absent (§11.2): no attribute, no warning, nothing in
  `metadata-json`.
- `stale-after`'s `@doc` becomes: the instant from which a concept's content is stale (OKF
  `stale_after`); `1970-01-01T00:00:00Z` marks a value that could not be read, kept as written in
  `metadata-json`.
- The sentinel is a real instant, so a bundle that writes `stale_after: "1970-01-01"` reads the
  same way. Both mean "stale", so the overlap is harmless.

### 4. Timestamps and empty values (§5, §11.2 rule 1)

- `parse_time` accepts the two v0.3 forms: an RFC 3339 `date-time` (seconds and an offset
  required) and a `full-date` (`2026-06-30`), which means `2026-06-30T00:00:00Z`. A date-time
  without an offset, or any other ISO 8601 form, is invalid and warns. RFC 3339's own variants
  are accepted, as chrono reads them: lowercase `t`/`z`, fractional seconds, and a space between
  date and time (RFC 3339 §5.6 allows it). *My call*, from the plan review.
- serde_yaml reads an unquoted `2026-06-30` as a string, so quoted and native values read the
  same. A test pins this.
- The same parser serves `generated.at`, `verified[].at`, `stale_after` and the legacy
  `timestamp`. *My call:* `timestamp` isn't in §5's list, but one rule for all is simpler and
  loses nothing.
- **Null, `""` and `[]` are the same as an absent key** for the OKF keys GECKO reads: `type`
  (see §2), `title`, `description`, `resource`, `tags`, `generated`, `timestamp`, `verified`,
  `status`, `stale_after` and the root `okf_version`. They are not typed, not kept in
  `metadata-json`, and not warned about. Inside a mapping, `generated.at: null` is an absent
  `at`, and a `verified` entry with a null `by` or `at` is ignored with a warning (§11.2 rule 3,
  unchanged).
- A whitespace-only string (`status: "  "`) is not empty. It stays a malformed value, warned
  about and kept raw, as today. *My call.* An empty mapping (`generated: {}`) isn't in rule 1's
  list and keeps today's "missing `by`" warning.

### 5. One item where a list is expected (§11.2 rule 2)

- `tags`: a scalar string is one tag and is never split at commas (`tags: "a, b"` is the one
  tag `a, b`). A scalar number or boolean is one tag, as written.
- In a `tags` list, strings are kept; numbers and booleans are kept as written (today they are
  dropped silently); null items are dropped; a list or mapping item is dropped with a warning,
  and the raw `tags` stays in `metadata-json` (the O2 + O4 rule for `verified`). A `tags` that is
  itself a mapping warns, writes no tags, and stays in `metadata-json`.
- `verified` already treats a bare mapping as one event (unchanged).
- `scopes` is GECKO's key and keeps its current reading (a non-goal), though it shares
  `extract_string_list` today. The tags rule must not leak into it.

### 6. An unknown `status` is not `stable` (§5.4)

- Storage is unchanged: `okf-status` as written. Absence still means `stable`.
- `okf-status`'s `@doc` becomes: a concept's OKF lifecycle status as written; absence means
  stable, and a value other than draft, stable or deprecated is not stable.

### 7. Extension keys (§4.1)

Nothing to build. `x-…` keys and unknown keys at any level already land in `metadata-json`, and
a mapping with an unknown subkey is kept whole. A regression test pins it.

### 8. Wording

Code comments, the `core_schema.tql` section comment and the KNOWN-LIMITATIONS section stop
saying "OKF v0.2". They become version-neutral, or say 0.3 where a rule is 0.3's.

### 9. Re-reading unchanged files

Change detection skips a concept whose stored `file-hash` equals the file's SHA-256
(`plan_sync`, `syncer/bundle.rs:161-172`). Without a change here, a database the v0.2 reader
synced would keep its old `trust-tier`s, including the `human-reviewed` tiers item 1 exists to
lower, until each file happened to be edited. O2 + O4 shipped that limitation for its own fields.

- *My call:* the OKF concept hash becomes SHA-256 over an `OKF_READ_VERSION` constant, then the
  file's bytes, following cyber's `concept_hash(STIX_MAPPING_VERSION, obj)`
  (`extensions/cyber-gecko/src/stix/mod.rs:137-150`). Every stored hash then differs once, so the
  first sync after upgrading rewrites every concept, at the cost of one full sync per bundle.
  Bumping the constant is how a future reader change reaches old databases.
- `compute_file_hash` has no caller outside its own unit test (checked 2026-11-28), so nothing
  expects it to equal `OkfConcept.file_hash`. Remove it, or salt it the same way.
- Only Markdown OKF bundles re-sync. Cyber's STIX import and sensors compute their own hashes
  outside `parser.rs`, so an ATT&CK import is not rewritten.
- This also retires the KNOWN-LIMITATIONS bullet "A file synced before v0.2 support keeps its
  old attributes".

### Schema and migrations

`core_schema.tql` gets the three new `@doc` strings (`trust-tier`, `okf-status`, `stale-after`)
and its section comment loses "v0.2". `CORE_MIGRATIONS` gets one `redefine attribute <name>
@doc("…");` entry per attribute, each triggered by the old `@doc` text, for example
`core/trust-tier-current-events-doc`, `core/okf-status-unknown-doc` and
`core/stale-after-sentinel-doc`. `core_migrations_lead_to_the_shipped_schema` must stay green.
Before relying on it, confirm on the live TypeDB 3.15 that `redefine attribute … @doc` commits:
the shipped precedent redefines an entity's `@doc`. No type, value type, `@values` or card
changes.

### Docs

- `docs/refactor/KNOWN-LIMITATIONS.md`, "OKF v0.2 frontmatter" becomes "OKF frontmatter":
  - drop the "until O7" list;
  - replace the "synced before v0.2 support keeps its old attributes" bullet with what
    `OKF_READ_VERSION` does: the first sync after a reader change rewrites every concept of a
    Markdown bundle once;
  - reword "Only RFC 3339 times are typed" as v0.3's own rule, with date-only accepted;
  - add the sentinel, symlinks never followed, quietly skipped files without frontmatter, and
    that a concept with no readable content time counts every verification event as current;
  - keep "Quote `okf_version`" and "Absence is not stored", the latter updated for unknown
    `status` and for the one trust-tier rule.
- `docs/refactor/OWED-WORK-LEDGER.md`: add the **O7b** row to Next up (after O7, before O3:
  `superseded_by`; `index.md` `title`/`description`/`license` versus `bundle.json`). On close,
  move O7 to "Landed since the review" with this design and its pinning tests, and update the
  header's "until O7 lands" line.

## Tests

Parser (`okf::parser` tests). Each is watched failing first unless marked as a regression test:
- `only_current_events_give_the_tier`: a human event before `generated.at` and an agent event
  after it give `machine-confirmed`, and both events are stored.
- `an_event_at_generated_at_is_current`.
- `a_legacy_timestamp_is_the_currency_time`: no `generated`, and a human event before
  `timestamp`, writes no tier.
- `generated_at_without_by_is_the_currency_time`.
- `with_no_content_time_every_event_is_current`: no `generated` and no `timestamp`; `generated`
  without `at`; a malformed `generated.at` (still warned).
- `the_tier_agrees_with_the_stored_times`: over a table of the cases above, the tier equals
  what `verified-at >= timestamp` (or no `timestamp`) gives on the parsed concept.
- `delimiters_are_exact_lines`: a first line of `----` or `--- x` is no frontmatter; a `----`
  line doesn't close the block; CRLF delimiters work.
- `no_current_event_writes_no_tier`: `[]`, all predating, all ignored entries.
- `a_file_without_type_is_not_a_concept`: no `type`, null, `""`, blank, empty frontmatter; each
  warns, and `parse_bundle` goes on.
- `a_file_without_frontmatter_is_skipped_quietly`.
- `an_unclosed_frontmatter_is_skipped_with_a_warning`.
- `a_numeric_type_is_read_as_written`.
- `dot_paths_are_not_part_of_the_bundle`: a `.hidden/x.md` and a `.draft.md`.
- `a_symlink_is_never_followed` (`#[cfg(unix)]`): out of root, broken, in-bundle file, and
  in-bundle directory cycle; each warns, none is read, and the walk ends.
- `a_symlinked_root_index_is_not_read` (`#[cfg(unix)]`).
- `an_unreadable_stale_after_is_stale`: the sentinel, the raw value in `metadata-json`, and the
  warning.
- `a_date_only_value_is_midnight_utc`: quoted and unquoted, for `generated.at`, `verified[].at`
  and `stale_after`.
- `a_date_time_without_an_offset_is_invalid`.
- `empty_and_null_values_are_absent`: each key in §4's list, with no warning and no
  `metadata-json` entry.
- `a_scalar_tags_is_one_tag`: `"a, b"` stays one tag, and `5` is the tag `5`.
- `tag_items_are_kept_as_written`.
- `a_scalar_scopes_is_unchanged`: a regression test that the tags rule didn't leak.
- `extension_keys_land_in_metadata_json`: a regression test.
- `the_read_version_salts_the_file_hash`.
- Edited: `trust_tiers_follow_the_verifiers` (`[]` → `None`), `test_parse_concept_missing_type`,
  `test_parse_concept_empty_type`, the `okf_version` warning tests (null and `""` no longer
  warn), and any stale-after warning assertion.

Sync (`gecko-bin/tests/bundle_sync_test.rs` and siblings, real TypeDB):
- `a_malformed_stale_after_reads_as_stale`: `stale-after <= now` returns the concept.
- `a_bundle_with_a_readme_syncs`: a frontmatter-less `README.md` neither fails the sync nor
  becomes a concept.
- `a_concept_that_loses_its_type_is_dropped`: re-sync after deleting `type` garbage-collects it,
  and retires it when a belief edge references it.
- `a_concept_synced_by_the_previous_reader_is_rewritten`: a stored unsalted hash makes the next
  sync rewrite the concept, and a human tier made stale by `generated.at` is lowered.
- `a_database_holding_the_old_okf_docs_upgrades`: modelled on
  `a_database_holding_the_old_memory_item_doc_upgrades`.
- `okf_v02_test.rs`: `the_repos_own_openwiki_bundle_parses` also asserts that every page that
  has `verified` reads as `machine-confirmed` (a regression test).

## Left to the builder

- How `parse_concept` reports "not a concept" (an `Option`, an enum or a skip reason) and the
  exact warning texts, within the rules above.
- Whether `TrustTier::Unverified`, `MissingType`, `EmptyType` and `compute_file_hash` are
  removed, depending on remaining callers.
- The `OKF_READ_VERSION` value and where the constant lives.
- Reading the `okf` crate README's v0.2 → v0.3 table and confirming each row is covered here or
  is O3 or O7b. Report any gap; don't silently widen scope.
- Confirming, against `plan_sync` and the retire path, that rewriting an unchanged concept (§9)
  keeps belief edges that reference it intact.
