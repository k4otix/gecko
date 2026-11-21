# Several bundles in one database

**Status:** draft for the user's approval, 2026-11-20.
**Ledger item:** O1 (new Track O, OKF bundles and OKF v0.2).
**Supersedes:** the 2026-07-07 decision "single-bundle-scoped, one bundle per TypeDB database"
(commit `9b153e1`), in the part that forbids sharing a database. Its other parts stand.

## Intent

A database can hold only one bundle today: every sync deletes each `concept` that is not in the
bundle being synced (`core/gecko-engine/src/syncer/bundle.rs:143-156`), and new concepts are
linked to every `bundle` in the database (`create_containment`, `:603-612`). So an OKF playbook
bundle and an imported STIX bundle cannot share a database, and cyber's detection flow (a
playbook's `claim` needs both its logic concept and the ATT&CK technique) cannot run end to end.
`cyber_detect_test` works around it with a raw insert.

The fix lets several bundles share a database while keeping what the July decision protected:

- **Concept IDs stay plain OKF paths** (`playbooks/hello_js`), as OKF v0.2 still defines them
  ("the path of the concept's file within the bundle"). No bundle prefix, no linker namespacing.
- **No silent overwrite.** A sync whose concept IDs already belong to another bundle is refused,
  with nothing written.
- **One bundle per database stays the default.** `sync` still picks the database from
  `bundle.json`'s name; sharing a database is opt-in with `--database`.

## Goals

1. **A sync touches only its own bundle.** Garbage collection, change detection, in-place
   updates, containment, hierarchy and links all act on the concepts the bundle being synced
   contains, identified through `containment` to the `bundle` with that `bundle-name`.
2. **Collisions are refused.** If an incoming concept ID already exists and is not contained in
   this bundle — whether another bundle contains it or no bundle does — the sync returns a new
   `DbError::ConceptIdCollision { bundle, ids: Vec<(String, Option<String>)> }`, listing each ID
   with the bundle that holds it (`None` for an uncontained concept). The variant carries every
   colliding ID; its message lists at most 20 of them plus a count of the rest. Nothing commits.
3. **Links resolve within the bundle.** A link creates a `concept-link` only when its target is
   contained in the same bundle. A link whose target is missing from the bundle stays unresolved,
   as OKF v0.2 §6.1 allows, and never attaches to another bundle's concept with the same path.
   Hierarchy edges are scoped the same way. Citations are not scoped: `external-resource` is
   keyed by URL, which is cross-bundle identity by design (OKF v0.2's `resource`).
4. **STIX imports keep their bundle name, with an override.** `sync-stix` keeps the bundle name
   `stix-sync` by default, so existing cyber databases and the default database are unchanged, and
   gains `--bundle-name <name>` to override it. Re-importing a newer ATT&CK release replaces the
   previous one, and a concept the new release drops that a belief edge references is retired, not
   deleted (see the retire-not-delete design). The bundle's `bundle-path` becomes the real file path for new imports (it is
   set-once, so existing databases keep `stix-sync`). Two ATT&CK domains (Enterprise and Mobile)
   cannot share a database: they ship the same MITRE `identity` and `marking-definition` objects,
   and some groups and software, under the same STIX ids, so the second import is refused as a
   collision. Sharing objects across imports is recorded as Track S work (S5), not built here.
5. **Docs say the new rule.** `CLAUDE.md`, `README.md`, and the "single-bundle-scoped" comments in
   `okf/parser.rs`, `okf/types.rs`, `okf/linker.rs` and `syncer/bundle.rs` describe it: one
   bundle per database by default, bundles may share a database, and concept IDs must not
   collide. The core schema's `bundle` `@doc` says the same; it changes through a core-schema
   migration (TypeDB refuses a changed annotation under `define`, `[DEX15]`), applied the way
   mem's `MEM_MIGRATIONS` are.

## Non-goals and anti-goals

- **Not** bundle-qualified IDs or `gecko run --bundle`. IDs stay unique per database.
- **Not** cross-bundle links. A future OKF version may define them; this change keeps links
  bundle-local.
- **Not** OKF v0.2 adoption (`sources`, `generated`, trust and lifecycle fields, Attested
  Computation). That is the rest of Track O.
- **Not** bundle renames or legacy repair. Changing `bundle.json`'s `name` on an existing
  database now gives a collision refusal; the error message says to sync under the old name or
  into a fresh database. A database that went through such a rename under the old code (two
  `bundle` entities, concepts contained by the wrong one or by both) gets the same refusal and the
  same guidance; no adoption path is built.
- **Anti-goal:** any behaviour change for a database holding one bundle. Every existing sync test
  passes unchanged.

## Approaches considered

- **Global IDs, collision refused (chosen).** Smallest change; keeps OKF IDs and `gecko run`.
- **Bundle-scoped IDs** (`(bundle, path)` identity). Rejected: brings back the non-standard
  namespacing removed in July and touches the schema key, links, containment, `gecko run`, cyber's
  lookups and every test using an ID.
- **Keep one bundle per database; treat STIX imports as non-bundles.** Rejected: an import still
  needs idempotency, change detection and garbage collection for its own objects, which rebuilds
  the bundle sync scoped to the import, the same change by another name.

## Design

All in `core/gecko-engine/src/syncer/bundle.rs`, inside the existing single write transaction.

- **Bundle membership read.** `fetch_existing_hashes` is replaced by a read returning, for every
  concept in the database, its `concept-id`, `file-hash`, and owning `bundle-name` (or none):
  `match $c isa concept, has concept-id $id; try { $ct isa containment, links (container: $b, member: $c); $b has bundle-name $bn; };`
  with a fetch of `$id`, `$c.file-hash`, `$bn?`. The stream loop handles `Some(Err(e))` as an
  error (CLAUDE.md "Observability tells the truth").
- **Plan.** `plan_sync` stays pure. It receives only the concepts this bundle owns, so its
  insert/update/skip/delete logic is unchanged. A new pure `find_collisions(manifest, all) ->
  Vec<(String, Option<String>)>` returns manifest IDs held by another bundle or by none.
  Collisions are checked before any write.
- **Containment** matches `$b isa bundle, has bundle-name == $name`.
- **Hierarchy and links** add `$ct1 isa containment, links (container: $b, member: $s);` and the
  same for `$t` to their match, with `$b` bound by name, so both ends must be in this bundle.
- **GC** acts only on concepts the bundle owns that are absent from the manifest. A concept owned
  by this bundle alone is deleted, as today. A concept also contained by another bundle loses only
  this bundle's containment and stays.
- **Concepts with more than one containment** exist in databases where two bundles were synced
  under the earlier code, whose `create_containment` contained each written concept in every
  `bundle` in the database (and through raw writes). Each containing bundle owns such a concept for
  change detection and updates; GC by one of them releases only its own containment, as above.
  For any other bundle the concept is a collision.
- **Memory-item IDs:** the membership read covers `concept` only. A manifest ID equal to a memory
  item's ID (mem's `mem/…` namespace) is not reported as a collision; it fails at insert as TypeDB's
  key violation, and nothing commits.
- **STIX naming:** `to_okf` keeps `bundle_name = "stix-sync"`; the `SyncStix` arm applies
  `--bundle-name` and sets `bundle_path` to the input file's path. The default database still
  derives from the bundle name, so it stays `stix-sync`.
- **Behaviour change, deliberate:** a raw-inserted `concept` subtype that no bundle contains
  (cyber's `host-asset`, `sensor`) is no longer garbage-collected by a sync. It does block a bundle
  that ships the same ID (goal 2). For the same reason, a link whose target is such an uncontained
  concept stays unresolved (goal 3), where before it resolved.

## Tests

- `two_bundles_share_a_database`: sync bundle A, then B with disjoint IDs; both sets of concepts
  are present, each contained by its own bundle only; re-syncing A leaves B's concepts, links and
  containment untouched.
- `resync_garbage_collects_only_its_own_bundle`: remove a concept from A and re-sync; only that
  concept is deleted.
- `colliding_concept_id_is_refused`: B reuses one of A's IDs; `Err(ConceptIdCollision)` naming
  the ID and `A`; a snapshot of concepts, containment and links is unchanged.
- `uncontained_concept_counts_as_a_collision`: a raw-inserted concept with no containment blocks
  a bundle that ships that ID.
- `link_does_not_cross_bundles`: B links to `/tables/x`, which only A has; no `concept-link` is
  created.
- `sync_stix_bundle_name_override`: through the binary, two `sync-stix` runs with different
  `--bundle-name` values and disjoint STIX fixtures share a database; a run with the default name
  re-imports in place.
- `second_attack_domain_is_refused`: two STIX fixtures sharing an `identity` object, under
  different `--bundle-name` values, give `ConceptIdCollision` for the second.
- `playbooks_and_attack_share_a_database` (cyber, Tier 2, through the `gecko` binary, passing
  `--database` on every call since `gecko run` defaults to `gecko`): sync an OKF
  playbook bundle and `sync-stix` the STIX test bundle into one `--database`, then `gecko run` a
  playbook that calls cyber `claim` on `T1059`; the claim succeeds and both bundles' concepts
  remain.
- `cyber_detect_test` drops its raw logic-concept insert and syncs a playbook bundle instead.
- Every existing `bundle_sync_test`, `no_drag_test`, `sandbox_test` and `cyber_stix_sync_test`
  assertion passes unchanged.

## Constraints

Parameterized TQL only; greenfield comments; never fabricate success; throwaway test databases,
never `gecko`, `--test-threads=1`; both clippy runs and fmt clean; `Cargo.lock` committed if
dependencies change.

## Left to the builder

- The exact fetch shape for the membership read, within the rule above.
- Error message wording, within goal 2 and the rename guidance.
- Whether `find_collisions` and the ownership filter share one pass.
