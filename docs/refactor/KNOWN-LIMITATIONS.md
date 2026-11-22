# Known limitations (gecko-core-substrate-refactor)

Short, load-bearing caveats so nothing in this branch is described as shipped
end-to-end when it is not.

## `mem.*` sandbox namespace — RESOLVED (recall proven end-to-end)

**Status: shipped end-to-end.** The guest binding landed, so the full `mem.*`
surface (`recall`/`remember`/`derive`/`supersede`/`contest`) is now callable from
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
  five functions, each marshalling the exact payload the host parses and (for
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
- The guest reads a host reply into a grow-and-retry buffer capped at 16 MiB; a
  single `mem.recall` returning more than that throws a clear "response exceeded
  16 MiB" error rather than truncating. **Caveat on the retry:** the host runs the
  extension callback to completion *before* it discovers the reply overflowed the
  buffer, so a retry re-runs the callback. For `mem.recall` that is effectful (it
  mints a `retrieval-event`), so a reply exceeding the 1 MiB first tier would record
  a *duplicate* retrieval-event on the retry — muddying A5.7 for that one recall.
  Low reachability today (recall replies are small; the write ops return tiny `{id}`
  payloads that never overflow), but the proper fix is a **host-reported
  required-size ABI**: on overflow the host returns the needed size (without
  re-running the callback) so the guest allocates exactly once. Tracked follow-up.
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

## As-of recall and supersede

As-of recall returns only beliefs visible to the caller (an actor with no agent node gets an
empty result), but it applies no supersession as of T: a belief later superseded is still
returned for a time before the supersession. A supersede requires a currently asserted
belief; superseding a retracted or already superseded belief is refused. The host trait method
`EpistemicReader::believed_at` is still unscoped by agent.

## Consolidation ("dreaming") daemon: episode dedup ships, the other jobs are stubs

`core/mem-gecko/src/consolidation.rs` ships the `ConsolidationDaemon` trait, a no-op
daemon, and `MemConsolidationDaemon`, whose `tick` mints one run and dedups duplicate
episodes under it through `MemWriter::dedup_episodes`. Nothing calls `tick` outside tests:
there is no scheduler and no CLI command. The other ten jobs are build-later stubs that
return `EpistemicError::NotYetImplemented` (no panic). The dedup tick mints no belief or
derivation, so the provable-merge checks of CONTRACT-W8 §3 stay owed. A daemon tick is not
exempt from the belief-tier accounting: the reconciliation rig drives the real daemon's
tick and requires every belief-tier label its writes change to be accounted for, as for
any other caller.

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
- **A pre-existing `sensor` without `platform` fails the schema upgrade.** The current
  schema requires a sensor's `platform`; a database holding a `sensor` written under the
  earlier cyber schema, which had none, cannot apply it. Nothing in GECKO wrote sensors
  before `gecko sync-sensors`, so only a hand-written one is affected.
- **A type-changed concept loses its incoming extension relations until the post-sync
  writes recreate them.** A sync that retypes a concept (a changed `type_hint`) deletes
  and re-inserts it, which drops the relations other concepts held to it. The STIX
  post-sync writes recreate the ones they own within the same transaction; a relation
  another extension wrote by hand is not recreated. A retype of a concept referenced by a
  belief edge, or contained by another bundle, is refused.
- **`cyber-object`'s `@doc` does not mention `deprecated`.** Redefining it would modify
  the core `concept` type, which the schema registry refuses.
- **Validated against Enterprise ATT&CK 20.1** (`stix_full_bundle_resync_is_stable`,
  opt-in); other releases are untested.
