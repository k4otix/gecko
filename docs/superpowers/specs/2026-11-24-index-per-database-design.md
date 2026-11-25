# One semantic index per database — design

Closes owed-work ledger item **N3**.

## Intent

The CLI keeps one HNSW snapshot (`cache_dir/index/gecko.hnsw`, or one explicit
`[semantic_index] path`) for every database. At startup the writer rebuilds the index
only when it is empty or was built with another model, so database B's run reuses A's
vectors and indexed recall misses B's beliefs. The failing test
`an_index_built_for_another_database_does_not_hide_this_ones_beliefs`
(`core/mem-gecko/tests/semantic_index_integration.rs`) shows it: B recalls nothing.

The same skip hides three more cases where the file looks valid but its contents are
wrong:

- a database dropped and recreated under the same name, which is the documented recovery
  path after the populations cut;
- beliefs written while the index was turned off;
- failed index writes. They are queued for repair only in memory, and the only drain runs
  at startup, so in practice nothing is repaired.

## Approaches

- **A. One file per database, with the database name in the snapshot header.** Rejected:
  it fixes only the shared-file case, and none of the three above.
- **B. Reconcile at startup, plus one file per database.** Chosen (the user's pick). After
  startup the index holds exactly the graph's current-state embeddables, whatever happened
  before. One file per database keeps alternating runs incremental.

## Goals

1. **Startup reconciliation** replaces `MemWriter::rebuild_index_from_graph`, with a name
   that says it reconciles.
   - A model-id mismatch still resets and re-embeds everything.
   - Otherwise it diffs the index's ids against `enumerate_current_state_embeddables`,
     which covers **both beliefs and episodes**. Enumeration gains a de-duplication by
     id after its sort; today it only sorts.
     - Ids in the index but not the graph are removed first, in one `remove_many`. A
       later embed or upsert failure then never leaves foreign vectors behind, and the
       adds go into a compacted graph.
     - Ids in the graph but not the index are then embedded and added in one
       `upsert_many`.
     - Matching id sets cost no embedding.
   - The reconciliation's batch embedding runs through `tokio::task::spawn_blocking`
     (moving an `Arc` of the embedder and the texts). The single-text embeds on the
     live paths (`recall`, assert, observe) are unchanged here.
   - **New mem-gecko dependencies:** `tokio` (`rt`) and `tracing` move into
     `[dependencies]`. mem-gecko is in `core/`, so the extension dependency check is
     unaffected.
   - Why an id-set diff is exact, checked against the write paths:
     - title, owner and visibility are written once at insert;
     - the only transitions (asserted → superseded, episode → tombstoned) each have a
       live removal;
     - every live upsert uses `Asserted`;
     - the prefilter reads only owner and visibility.
2. **The live set matches the enumerated set.** `resolve_prediction` inserts an owned
   episode that the live path never indexes, but enumeration returns it. It is indexed
   live, through the same best-effort upsert as `observe`, so a warm start after it embeds
   nothing.
3. **The in-memory dirty set is cut.**
   - Its only drain runs at startup, when the set holds nothing but the rebuild's own marks.
     That drain looks up beliefs only, so after a failed persist it would delete valid
     episode vectors.
   - Startup reconciliation is the repair. A failed best-effort index write logs a warning
     and leaves the graph write committed (invariant 8 is unchanged), and the next startup
     reconciles it.
   - Cut: `mark_dirty`, `dirty_snapshot`, `is_dirty`, `dirty_len`, `drain_dirty_set`, the
     `dirty` field, and `tql::fetch_belief_embeddable`, whose only caller is the drain.
   - `IndexOutcome::MarkedDirty` becomes `Failed`. The consolidation report field
     `vectors_marked_dirty` becomes `vectors_failed`.
   - The CLAUDE.md rule "a failure marks the id dirty for background repair" is updated to
     say a failure is reconciled at the next startup.
4. **`SemanticIndex` additions,** in `core/gecko-extension-api/src/semantic.rs`:
   - `fn ids(&self) -> Option<HashSet<ConceptId>>`, defaulting to `None`. `None` means the
     backend manages itself (`NoopIndex`, a future native backend). The writer then skips
     the diff and keeps the empty-or-model-mismatch rule.
   - `fn remove_many(&self, ids: Vec<ConceptId>) -> Result<()>`, defaulting to a loop over
     `remove`.
   - `HnswIndex` overrides both.
     - `ids` reads under the lock with `.unwrap_or_else(|e| e.into_inner())`. A poisoned
       lock must not return `None`, which would silently skip reconciliation.
     - `remove_many` **compacts**. It rebuilds `Inner` from the retained entries' stored
       vectors, with no re-embedding, replayed in sorted concept-id order so the ANN
       graph is deterministic. No tombstone outlives the call.
     - It resets `searching_mode_set`, as `reset()` does, because the fresh `Hnsw` starts
       with searching mode off.
     - It persists once, including when the result is empty, so stale vectors can't come
       back on the next open.
     - When none of the requested ids is present, it is a no-op: no compaction and no
       persist.
   - `persist` writes a temp file named `<file>.<pid>.<counter>.tmp`, with a process-wide
     counter. Two processes, or two handles in one process, can't collide on one temp
     file and interleave into corrupt JSON.
5. **One file per database.** The resolver returns `<dir>/<encoded database>.hnsw`.
   - `[semantic_index] path` is replaced by `dir`, a directory that defaults to
     `cache_dir/index/`. The shipped `gecko.toml` drops `path`, so the index lives under
     the cache root, outside the repo tree. `.gitignore`'s `gecko.hnsw` entry goes.
   - **A config that still sets `path` is refused** with an error that names `dir`.
     - `SemanticIndexConfig` keeps a `path` field for this, so serde doesn't silently
       ignore it.
     - The refusal happens in the index resolver, not at config load, so `up`, `drop`,
       `databases` and `doctor` keep working on a stale config (assembler wiring).
     - `doctor` has no database argument, so its index check works on the directory,
       through the non-creating resolver, and has no side effects:
       - an existing directory is Ok;
       - an absent directory is a WARN that it is created on the next run;
       - a stale `path`, or a `dir` that is a regular file, is a WARN that the next run
         will fail. It gives the fix, not "rebuilt on next run".
   - A configured `dir` that is an existing regular file is refused with an actionable
     error.
6. **The database name is encoded, not refused.**
   - The engine does no name validation: a name is `--database` or the bundle name, which
     is free text that only TypeDB constrains.
   - So the filename keeps `[A-Za-z0-9_-]` as is and percent-encodes every other UTF-8
     byte (`%` included). The encoding is injective, so no two names share a file, and no
     name `sync` accepts is refused here.
   - An empty name is refused.
   - Names that differ only in case share a file on a case-insensitive filesystem, as do
     databases of the same name on two TypeDB servers. Reconciliation keeps them correct,
     at the cost of re-embedding on each alternation. This is stated in
     `KNOWN-LIMITATIONS.md`.
7. **Wiring.**
   - `build_epistemic_writer` takes the database name.
   - `gecko drop` removes `<dir>/<encoded database>.hnsw`, best-effort, when the index is
     enabled.
     - It runs only **after** the TypeDB drop succeeds, so a refused drop (the reserved
       `gecko` database included) never touches its index.
     - `cmd_drop` receives the `GeckoConfig`.
     - It resolves the path without creating directories: `cache_dir()` creates them, so
       the resolver gets a non-creating variant.
     - Both a resolution failure (a stale `path`, a `dir` that is a file) and a removal
       failure only warn, and the drop still succeeds. Reconciliation keeps results
       correct without the removal; it only avoids orphan files.
   - The `gecko init` template (`default_gecko_toml`, `config.rs:254-257`) shows a
     commented `dir`, not `path`, and names `cache_dir/index/<database>.hnsw`.
   - The startup log says "reconciled with the graph" and reports how many ids were added
     and removed.

## Non-goals

- Persisting a repair queue. Startup reconciliation supersedes it.
- Recording the database or graph identity in the snapshot header. Reconciliation makes
  it unnecessary.
- Migrating an existing `gecko.hnsw`. It is a rebuildable cache (invariant 8): the first
  run per database re-embeds.
- Locking between concurrent processes on one file. A lost update is repaired at the next
  startup.

## Constraints

- Bulk paths batch: one `upsert_many` and one `remove_many` per reconciliation, never
  per-item persists.
- Index work stays best-effort. When the graph read succeeds, an embed or persist failure
  warns and never fails startup.
- Ratified docs are not edited.
- Comments are greenfield, with no `#[allow(dead_code)]`. fmt and both clippy runs are
  clean.
- Integration tests run with `-j 1` and never touch the `gecko` database.

## Verification

- **mem-gecko, live TypeDB:**
  - the existing RED test passes;
  - **drop and recreate under one name:** a file holding the dropped database's vectors is
    reconciled, and reopening the file shows no stale ids;
  - **written while the index was off:** a belief and an episode asserted through
    `without_index` are recalled after a reconciling startup;
  - **matching sets do no embedding,** checked with a counting embedder, including after a
    `resolve_prediction`;
  - **a failed persist during reconciliation keeps episode vectors** in memory, and they
    are recalled in that process.
- **gecko-semantic-index unit tests:**
  - `ids` lists the live ids;
  - `remove_many` persists once, compacts (a query after removing most ids still returns
    every retained id, including after an earlier query toggled searching mode), and
    persists the empty state;
  - `remove_many` of unknown ids leaves the file untouched;
  - two handles on one path persisting in turn leave valid JSON.
- **gecko-bin unit tests:**
  - the resolver yields `<dir>/<db>.hnsw` for the default and an explicit `dir`;
  - the encoding is injective over sample names (`a/b`, `..`, `données`, `a%2Fb`);
  - an empty name is refused;
  - a config with `path` is refused by the resolver, names `dir`, and leaves `drop` and
    `databases` working;
  - a `dir` that is a regular file is refused;
  - `doctor` warns, with the fix, on a stale `path` and on a `dir` that is a file, and
    creates nothing;
  - `gecko drop` of the reserved `gecko` database leaves its index file in place;
  - the `gecko init` template parses and its commented `dir` line, once uncommented, is
    accepted.
- **Callers updated:**
  - `gecko-bin/tests/belief_tier_reconciliation_test.rs:252-258` and its
    `rebuild_index_from_graph` call at `:703` (Tier 2);
  - `doctor.rs:581-594`;
  - `config.rs:438` and `:454`;
  - `config/paths.rs:213-247`;
  - the dirty-set tests in `writer/tests.rs` and `semantic_index_integration.rs`, which
    are rewritten as reconciliation tests;
  - `documented_config_snippets_are_accepted` still passes.
- The full candle-free suite, fmt and both clippy runs pass.

## Left to the builder

- The reconciliation method's exact name.
- Whether the encoder lives in `config/paths.rs` or a small module of its own.
