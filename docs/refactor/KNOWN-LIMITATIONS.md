# Known limitations (gecko-core-substrate-refactor)

Short, load-bearing caveats so nothing in this branch is described as shipped
end-to-end when it is not.

## `mem.*` sandbox namespace — RESOLVED (recall proven end-to-end)

**Status: shipped end-to-end.** The guest binding landed, so the full `mem.*`
surface (`recall`/`remember`/`derive`/`supersede`/`contest`/`retract`/`restsOn`) is now callable from
exec-doc JS in a live `gecko run`, and A5.7 retrieval-provenance correlation is
proven through the real sandbox against live TypeDB.

- `EpistemicReader::recall` (present-state, index-seeded, and as-of-T paths) and the
  A5.7 retrieval-provenance write path (`retrieval-event` stamping +
  `informs-synthesis` correlation on a later `assert_belief`) are implemented in
  `core/mem-gecko` and covered by that crate's tests (`writer_reader_integration`,
  `semantic_index_integration`).
- The host reader bridge (`core/gecko-engine/src/sandbox/mem_host.rs`):
  `dispatch_mem_recall` binds the host `RunContext`, parses the payload into a
  `RecallQuery`/`ContextBudget`, calls the injected `EpistemicReader`, and serializes
  the chunks; `epistemic_extension_callback` routes `MemHostFn::Recall` to it through
  the same async→sync bridge and the SAME per-run scratch as the writer. `MemWriter`
  is handed over as the reader via `EpistemicWriter::as_epistemic_reader`.
- **The guest binding now exists** (`core/js-sandbox/src/main.rs`, compiled into the
  committed `core/gecko-engine/src/sandbox/quickjs.wasm`): a `mem` global exposes the
  functions, each marshalling the exact payload the host parses and (for
  `recall`) unwrapping the `{ chunks }` envelope to hand JS the array directly. Every
  call is S3 default-deny gated on the concept's granted `mem:<fn>` scope.
- **Proven live:** `gecko-bin/tests/mem_recall_e2e_test.rs` drives a real guest
  program (`mem.recall(...)` → `mem.derive(..., chunks.map(c => c.id))`) through
  `WasmExecutor` against live TypeDB and asserts the synthesized belief carries
  `retrieval-provenance = "semantic"`, an `informs-synthesis` edge, and a `surfaced`
  link with `was-used = true` — the first end-to-end demonstration of A5.7. A
  negative test proves an un-scoped `mem.recall` is S3-denied before any host
  dispatch. A worked example ships at `sample-bundle/playbooks/mem_recall_demo.md`.

**Residual caveats (not blocking):**
- A host call runs its extension callback once and the guest reads the reply into a
  buffer sized to fit, up to 16 MiB. A reply over 16 MiB is refused with "host response
  exceeded 16 MiB" after the callback has run, so an effectful call (a write, or a
  `mem.recall` that mints a `retrieval-event`) has taken effect even though the guest sees
  an error.
- The guest is a prebuilt binary kept out of CI; after editing
  `core/js-sandbox/src/main.rs` you must `just build-guest` and re-commit the wasm
  (see `docs/refactor/guest-wasm.md`).

## Sync guard

- **Same-count residual.** `sync_bundle` fingerprints the belief tier (instance,
  attribute-ownership and role-player counts per belief-tier label) at the start
  and end of its transaction and refuses any difference. A change that keeps
  every count is not seen: a belief-tier attribute value replaced in place, or one
  instance deleted and an identical-shape one inserted.
- **Retired concepts.** A sync that drops a concept a belief-tier relation
  references retires it instead of deleting it: the concept keeps its ID and the
  attributes its type requires, loses its other attributes and every relation
  except containment and belief-tier relations, and gains `retired-at`.
  `gecko run` refuses a retired concept. Restoring its file revives it, and it is
  deleted once nothing references it. A reader learns of a retirement only from
  the concept itself; recall does not flag beliefs about it. A retired ID stays
  contained by its bundle, so another bundle syncing that ID is refused as a
  collision while the belief exists. A later sync keeps the first `retired-at`,
  and the CLI reports `Concepts retired: <n>` for the concepts that sync newly
  retired. The CLI's `sync` applies the schemas itself; a library caller of
  `sync_bundle` must apply the current schema first, since a database without
  `retired-at` cannot retire a concept. `gecko run` works on a database without
  `retired-at`, where no concept can be retired.
- **Cost.** Every sync reads the surface and takes two fingerprints: nine reads
  in all, since each fingerprint is one grouped count per measure. With mem and
  cyber composed, in a debug build on near-empty databases, the guard took a
  median of about 4.3 ms per sync (23 syncs). The cost grows with the belief
  tier, because each read scans its instances.

## Schema constraints and upgrades

- **The schema enforces required roles, ownership and recall fields.** Every relation
  role the design lists is required (the bare roles of `pivot` and
  `prediction-resolution` stay optional), a memory item needs one ownership and a
  source-link, a belief needs `valid-from`, `salience`, `base-activation` and
  `last-access`, and confidence, predicted probability and similarity scores are ranged.
  `schema-init` (and `sync` / `sync-sensors`, which apply the schema first) deletes each
  one-player row of a relation that gained a required role and clamps stored similarity
  scores into `[-1, 1]`, in the schema transaction, before the constraint lands
  (`tightened_schema_applies_to_a_populated_database`). A database last synced before
  role `@doc`s shipped carries none of the cleanup triggers, so its cleanups are
  skipped and a one-player row there fails `schema-init` naming the relation.
- **Other violating rows fail the upgrade.** An existing database holding a row no
  cleanup covers (a belief missing a required field, an out-of-range score, a one-player
  belief edge) fails `schema-init` with TypeDB's constraint error naming the type; it
  needs a manual cleanup. Core, mem and cyber each apply in their own schema
  transaction, in that order: the refusing fragment commits nothing, and earlier
  fragments' cleanups stay committed. The state is consistent, and re-running after the
  fix completes the upgrade. Today's writers produce no such row. This covers a
  `rests-on` written before the edge was stamped: it has no declarer, run or
  `created-at`, no migration invents them, and the shipped mem schema refuses it
  (`an_unstamped_rests_on_fails_the_upgrade`).
- **A database that predates the populations cut takes the shipped schema and keeps
  inert leftovers.** The shipped schema applies over each earlier vintage and the
  population types and functions it declared stay stored and unused. A database from
  the previous release (the `95ebac1` fixture, which has `population-members` but no
  `-at` functions and no `blast-step`) gains `blast-step` and `canonical-entity-at` and
  keeps `population-members` and `population-member`
  (`previous_release_upgrades_to_the_shipped_schema`). A database initialized before
  the tightening (which has `population-members`, `population-members-at` and the
  other `-at` functions) applies the shipped schema over its stored data
  (`tightened_schema_applies_to_a_populated_database`). One initialized after the step
  functions landed applies too: its `blast-radius` answers over `rests-on` and
  derivation, and its `blast-step-at` and `blast-radius-at` stay stored and unused.
  That claim was observed by a one-off probe at commit `1276444` and is not pinned by
  a test; `narrowing_a_callee_under_a_stored_caller_applies` pins only TypeDB's
  behavior when a callee narrows under a stored caller. `gecko drop <db>`,
  `schema-init` and a re-sync recreate a database without the leftovers. See
  `docs/superpowers/specs/2026-11-24-populations-cut-design.md`.
- **A database that predates the second cut keeps its cut types as inert leftovers
  too.** These are:
  - divergence (`divergence-exception`, `divergence-precedence`);
  - consolidation (`consolidation-step`, `consolidation-state`, `consolidation-action`);
  - the belief-tier types nothing wrote (`intention`, `goal-hierarchy`, `working-set`,
    mem's `playbook`, `execution`, `tool-invocation`, `learned-constraint`, `contextualizes`,
    `evidence`, `justification`, `support`, `attack`, and cyber's `sighting` and
    `sighting-of`);
  - their attributes and `plays` lines.

  Their one-player rows are no longer cleaned up. `memory-item`'s `@doc` is replaced by the
  `mem/memory-item-doc` migration (`a_database_holding_the_old_memory_item_doc_upgrades`).
  A database whose episodes a test dedup marked `tombstoned` re-indexes those episodes,
  because reconciliation no longer reads that state. Drop and recreate a database to shed
  the leftovers. See `docs/superpowers/specs/2026-11-24-second-cut-design.md`.
- **Garbage collection deletes every relation a concept plays.** A sync that deletes a
  concept first deletes each relation the concept plays in, of any extension, so a
  relation linking it to a concept another bundle owns is deleted with it rather than
  left without a required player
  (`garbage_collecting_a_concept_deletes_every_non_surface_relation_it_plays`). A
  concept a belief edge references is retired instead.

## Playbook frontmatter

- **A playbook synced before `timeout-ms` was stored keeps the 30 s default** until its
  content changes. Change detection hashes the file's content, so a re-sync skips an
  unchanged file and never writes its `timeout-ms`. Editing the file's content fixes it;
  touching it doesn't. `consumes` and `produces` behave the same way: an unchanged,
  already-synced file doesn't gain them in `metadata-json` until it is edited.
- **A bad `timeout-ms` fails the whole sync.** The parser refuses any value that isn't an
  integer from 1 to 600000 (`MAX_TIMEOUT_MS`), including `0`, `"5s"`, `null` and a blank
  value, and names the file. A bundle that synced while the old parser ignored such a value
  no longer syncs until it is fixed.
- **A bad `engine`, or an `engine` with no program, fails the whole sync.** A present
  `engine` must be `js`, `javascript` or `quickjs` (any case); anything else, including a
  number, `null` and a blank value, is refused naming the file and the value. A concept
  that declares a supported engine must have non-blank code in a fence in one of those
  languages. A bundle that synced while such a concept was silently treated as
  documentation no longer syncs until the file is fixed. A concept with no `engine` key is
  documentation, as before.

## OKF v0.2 frontmatter

GECKO reads OKF with v0.2 semantics. OKF v0.3 (<https://github.com/open-knowledge-format/open-knowledge-format/blob/main/SPEC.md>)
changes some of the rules below, and ledger item O7 adopts them. Until it lands:
- the trust tier counts every `verified` event, where v0.3 counts only those at or after
  `generated.at`;
- an unparseable `stale_after` stores nothing, so a reader may take the concept as fresh;
- date-only and empty values warn;
- a scalar `tags` is dropped;
- a `.md` file without frontmatter fails the sync.

- **A malformed v0.2 provenance value is warned about, not refused.** `generated`,
  `timestamp`, `verified`, `status` and `stale_after` follow OKF §11's soft guidance: a value
  the parser can't read (a date without a time, `stale_after: next tuesday`, `verified: 5`)
  gets no typed attribute, stays in `metadata-json` as written, and `gecko sync` prints a
  `warning:` line naming the file, key and value. A malformed `verified` writes no
  `trust-tier`. `engine`, `timeout-ms` and `scopes` still fail the sync.
- **A file synced before v0.2 support keeps its old attributes** until its content changes.
  Change detection hashes the file's content, so a re-sync skips an unchanged file and never
  writes its `generated-by`, `trust-tier`, `okf-status`, `stale-after` or `verification`
  relations. Editing the file fixes it. A bundle's `okf-version` is rewritten on every sync.
- **Only RFC 3339 times are typed.** OKF §5 allows any ISO 8601 datetime with an explicit
  offset; GECKO types those with seconds and a full offset (`2026-06-30T14:00:00Z`,
  `…+09:00`). Other forms, such as `2026-06-30T14:00Z` or a `+09` offset, are warned about
  and kept in `metadata-json` as written.
- **Quote `okf_version`.** YAML reads an unquoted `okf_version: 0.10` as the number `0.1`,
  and that is what is stored.
- **Absence is not stored.** A concept with no `verified` has no `trust-tier` (OKF: unverified),
  and one with no `status` has no `okf-status` (OKF: stable). Staleness is
  `stale-after <= now`, computed by the reader; nothing acts on it, nor on `status`.

## Semantic index files

The index is one file per database, `<dir>/<database>.hnsw`, where `dir` is
`[semantic_index] dir` or `cache_dir/index/`. The file is keyed by the database name
only, so names that differ only in case share a file on a case-insensitive filesystem,
as do same-named databases on two TypeDB servers. Results stay correct, because every
startup reconciles the index with the graph, but each alternation re-embeds the
difference. A `gecko.toml` that still sets `[semantic_index] path` is refused by
`gecko run` with a pointer to `dir`.

## As-of recall, supersede and retract

As-of recall returns only beliefs visible to the caller (an actor with no agent node gets an
empty result).

- **Supersessions.** As-of recall at T returns the beliefs that held at T: those whose
  half-open validity window `[valid-from, valid-to)` contains T and that no supersession at
  or before T replaced. A supersede closes the old belief's `valid-to` at the instant the
  successor's `valid-from` opens, so a read before it sees the old belief and a read from it
  on sees only the successor (`believed_at_reads_across_a_supersede`).
- **Older databases.** A database written before supersede closed `valid-to` answers the
  same way, through its supersession edges
  (`a_legacy_superseded_belief_without_valid_to_ends_at_its_supersession`). The new
  `believed-at` reaches a database when `gecko schema-init` or `sync` next applies the
  schema.
- **Empty and inverted windows.** A belief asserted and superseded at one `occurred_at`
  (within one run) gets the empty window `[t, t)`, and a supersede dated before the old
  belief's `valid-from` gives it an inverted one. Either way no as-of read ever returns it;
  its successor holds from the supersede on
  (`a_same_instant_supersede_leaves_the_old_belief_an_empty_window`,
  `a_backdated_supersede_is_returned_by_no_as_of_read`).
- **Retractions.** A retract closes the belief's `valid-to` at the run's `occurred_at`, by
  the same rule as supersede: it never extends an earlier end and shortens a later one. As-of
  recall returns the belief before the retraction and not from it on
  (`as_of_recall_ends_a_retracted_belief_at_its_retraction`). A belief asserted and retracted
  in one run gets the empty window `[t, t)` (`a_same_instant_retract_leaves_an_empty_window`).
  `occurred_at` is fixed when a run starts, so a long run that retracts a belief asserted
  by a run that started after it records a `valid-to` before that belief's `valid-from`. The
  inverted window matches no as-of read, as for a backdated supersede.
- **One retraction per belief.** `belief plays retraction:retracted @card(0..1)`, so a
  second retraction of one belief, from two processes racing past the state check, is
  refused at commit (`a_belief_with_two_retractions_is_refused_at_commit`). Supersede has no
  such guard: two processes racing a supersede, or a supersede and a retract, can still both
  commit.
- **`belief-state` is not read.** `supersede` and `retract` both close `valid-to`, so only a
  belief raw-written as `retracted` or `superseded` with no `valid-to` (and, for a
  supersede, no supersession edge) stays in as-of reads.

A supersede or a retract requires a currently asserted belief; superseding or retracting a
retracted or already superseded belief is refused (`superseding_a_retracted_belief_is_refused`,
`retracting_a_superseded_belief_is_refused`, `retracting_twice_is_refused`).

## Writes act only on what the caller can see

A caller may only act on beliefs visible to it (`visible-to`: team or shared visibility, or
owned by the caller). The rule binds every `MemWriter` caller, not only the guest: the caller
is the `RunContext` actor, which the host stamps and the sandbox cannot forge, and it covers
cyber's belief-edge seam (`assert_belief_with_edges`), whose evidence ids come from the guest.
Each write enforces it inside its own write, so a refused write commits nothing:

- **`supersede`** requires the old belief to be asserted and visible to the caller; another
  agent's private belief is refused like a missing one, and the entrenchment guard reads only
  a visible belief, so the refusal does not reveal its tier
  (`superseding_another_agents_private_belief_is_refused_and_commits_nothing`,
  `superseding_a_team_belief_of_another_agent_is_allowed`).
- **`retract`** requires the belief to be asserted and visible to the caller; another
  agent's private belief is refused with the same message as a missing one
  (`retracting_another_agents_private_belief_is_refused_and_commits_nothing`,
  `retracting_a_team_belief_of_another_agent_is_allowed`). It has no entrenchment guard:
  there is no replacement to compare tiers against, so visibility and the `mem:retract`
  scope are the gate. Its reply lists the blast radius only as the beliefs that rested on the
  retracted one that the caller can see and that are still current, read after the
  retraction commits, so a refused retract reveals nothing
  (`retract_reports_the_visible_current_beliefs_that_rested_on_it`). Dependents are
  reported, not retracted.
- **`contest`** requires every claim to be visible to the caller
  (`contesting_another_agents_private_belief_is_refused`).
- **`rests_on`** requires the resting item and every assumption to be visible to the
  caller, by the same rule as `derive`'s evidence; a refused item gets the same message as
  a missing one, in either position. The edge records the declaring agent, the run and the
  time. A repeated assumption, a self-dependency and an empty list are refused
  (`rests_on_another_agents_private_belief_is_refused_and_commits_nothing`,
  `rests_on_gates_the_resting_item_and_every_assumption_in_one_call`,
  `rests_on_stamps_the_declarer_the_run_and_the_time`).
- **`derive`** (`assert_belief` with evidence) requires every evidence belief to be visible to
  the caller. Evidence that is not a belief (an episode, an anomaly) is held to the same rule
  through its `ownership`; those items are written without a visibility, which the substrate
  treats as private, so only their owners may cite them
  (`deriving_from_another_agents_private_belief_is_refused`,
  `deriving_from_another_agents_episode_is_refused`).

`record_prediction` and `resolve_prediction` are not reachable from the guest and do not
check visibility.

Another agent may supersede or retract a team or shared belief; a supersede may use a
private replacement. Either removes the belief from its original owner's recall.

## cyber-gecko `blinded`: ATT&CK's current detection model

- **Needs current ATT&CK and a synced sensor file.** `blinded` reads detection
  strategies, analytics and their log-source requirements from the ATT&CK 18+ detection
  model, and the platforms and sensors from a sensor file synced with
  `gecko sync-sensors`. A database without detection strategies, or without a synced
  sensor file, makes `blinded` return an error that says which is missing.
- **Name matching is trimmed and case-insensitive, otherwise exact.** A sensor's log
  source or data component matches a requirement on its trimmed, lowercased name; there
  are no aliases, so `WinEventLog:Sysmon` and `Microsoft-Windows-Sysmon/Operational` are
  different sources.
- **Platforms match exactly.** A declared platform is compared with ATT&CK's analytic
  platforms as spelled; `gecko sync-sensors` refuses one that matches only ignoring case
  (`windows` for `Windows`), and reports one that no live analytic runs on in any case as
  a notice.
- **No population-scoped coverage.** A platform is blind or covered for the whole
  database; coverage is not narrowed to a population of hosts.
- **A platform with several analytics counts as covered if any can run.** ATT&CK ships
  one analytic per platform per strategy, so this does not occur on v20.1, but a future
  bundle with several could hide a blind analytic behind a covered one.
- **`recommend_sources` is greedy, not optimal.** Each step takes the analytic gap set
  with the most (claim, platform) pairs unblinded per new log source. Where each
  technique has one analytic per platform, as on ATT&CK v20.1, the full list's total is
  the fewest sources that fix every pair, and only the order is heuristic: the best k
  sources for a budget of k may differ. Where a platform has several analytics, the total
  can also exceed the fewest. Every log source costs the
  same, and the list names log sources only, never a fallback data component. Its
  `no-analytic` reason also covers a claim on a retired, revoked or deprecated technique,
  which `blinded` reports as unassessed on every platform.
- **A pre-existing `sensor` without `platform` fails the schema upgrade.** The current
  schema requires a sensor's `platform`; a database holding a `sensor` written under the
  earlier cyber schema, which had none, cannot apply it. Nothing in GECKO wrote sensors
  before `gecko sync-sensors`, so only a hand-written one is affected.
- **A one-player detection claim or disposition fails the schema upgrade.** Today's code
  cannot produce a `technique-detection` or `alert-disposition` missing a player; an old
  database holding one fails schema-init with TypeDB's error naming the relation, and
  because `sync`, `sync-stix` and `sync-sensors` apply the schema first, none of them
  runs until it is cleaned. `gecko query` is read-only, so run these in a write
  transaction with an external TypeDB client (for example TypeDB console), then re-run
  `schema-init`:

  ```
  match $r isa technique-detection; { not { $r links (detecting-logic: $p0); }; } or { not { $r links (detected-technique: $p1); }; } or { not { $r links (detection-claim: $p2); }; }; delete $r;
  match $r isa alert-disposition; { not { $r links (dispositioned-logic: $p0); }; } or { not { $r links (disposition-belief: $p1); }; }; delete $r;
  ```
- **A type-changed concept loses its incoming extension relations until the post-sync
  writes recreate them.** A sync that retypes a concept (a changed `type_hint`) deletes
  and re-inserts it, which drops the relations other concepts held to it. The STIX
  post-sync writes recreate the ones they own within the same transaction; a relation
  another extension wrote by hand is not recreated. A retype of a concept referenced by a
  belief edge, or contained by another bundle, is refused.
- **`cyber-object`'s `@doc` does not mention `deprecated`.** Redefining it would modify
  the core `concept` type, which the schema registry refuses.
- **One ATT&CK domain per database.** Enterprise, Mobile and ICS ATT&CK ship some of
  the same STIX objects (the MITRE identity and marking, and shared groups, software,
  campaigns and data components), so a second domain synced into a database that holds
  the first is refused, and `sync-stix` says to import it into its own database with
  `--database`. Under the first domain's `--bundle-name` the second would replace it
  instead. The detection reports never cross domains, so nothing is lost for them; a
  query or belief spanning domains needs two databases.
- **Validated against Enterprise ATT&CK 20.1** (`stix_full_bundle_resync_is_stable`,
  opt-in); other releases are untested.
