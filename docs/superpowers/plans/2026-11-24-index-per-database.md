# One Semantic Index per Database Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** after startup, each database's semantic index holds exactly that database's
current-state embeddables, in a file of its own.

**Architecture:**
- **The index trait** gains `ids()` and a compacting `remove_many`.
- **`MemWriter` reconciles at startup.** It diffs the index's ids against the graph's
  current-state set, removes stale ids, then embeds missing ones. This replaces the
  in-memory dirty set.
- **The CLI** resolves `<dir>/<percent-encoded database>.hnsw` from a new
  `[semantic_index] dir`, and refuses the old `path` key.

**Tech Stack:** Rust 2024 workspace, TypeDB 3.15, `hnsw_rs`, `tokio`, `tracing`, `serde`/`toml`.

**Spec:** `docs/superpowers/specs/2026-11-24-index-per-database-design.md`

## Global Constraints

- All TQL is parameterized (`Params`), with no string interpolation of values.
- Comments are greenfield: no plan tags (`N3`) and no "used to" / "no longer". Each
  type and function carries one accurate doc sentence.
- No `#[allow(dead_code)]`. Code made dead by the cut is deleted.
- Bulk index paths call `upsert_many` and `remove_many` once each, never per-item persists.
- Index work is best-effort and post-commit. A failure warns (`tracing::warn!`) and never
  fails a graph write or startup.
- Lock poisoning on best-effort paths uses `.unwrap_or_else(|e| e.into_inner())`.
- Integration tests run with `--test-threads=1` / `-j 1`. Never touch the database named
  `gecko`.
- Before each commit: `cargo fmt --all -- --check`, plus both
  `cargo clippy --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`
  and `cargo clippy --workspace --exclude js-sandbox --no-default-features --features cyber --all-targets -- -D warnings`.
- Ratified docs (CONTRACT-W8, SPEC-W5, SPEC-W2-W3, INVARIANTS.md) are never edited. Frozen
  fixtures under `*/tests/fixtures/` keep their text.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **An empty graph opening a non-empty foreign file** must leave an empty file on disk.
   The next open shows zero ids. Task 2 pins it with
   `reconcile_with_an_empty_graph_empties_the_file`.
2. **A `gecko.toml` still carrying `path = "gecko.hnsw"`** fails `gecko run` with a
   message naming `dir`, while `gecko drop`, `databases` and `doctor` still work. Task 3.
3. **A database name with `/`, `..` or non-ASCII** never escapes `<dir>`, and two names
   never share a file. Task 3 pins it with
   `encoded_names_stay_in_dir_and_are_injective`.
4. **Queries after a large stale removal** still return every retained id, because the
   compaction leaves no tombstones. Task 1. Searching-mode reset can't be observed with
   `hnsw_rs` 0.3.4 and is checked by review.
5. **A warm start whose id sets match** embeds nothing, even after a
   `resolve_prediction`. Task 2's counting-embedder test pins it.

---

### Task 1: The index trait gains `ids` and a compacting `remove_many`

**Files:**
- Modify: `core/gecko-extension-api/src/semantic.rs:138-182` (the `SemanticIndex` trait)
- Modify: `core/gecko-semantic-index/src/index.rs`:
  - `persist`, `:166-198`;
  - the `SemanticIndex for HnswIndex` impl, `:207-333`;
  - the unit tests.

**Interfaces:**
- Produces:
  - `fn ids(&self) -> Option<std::collections::HashSet<ConceptId>>` on `SemanticIndex`,
    defaulting to `None`. `None` means the backend is self-managed, so the caller skips
    the diff.
  - `fn remove_many(&self, ids: Vec<ConceptId>) -> Result<()>` on `SemanticIndex`,
    defaulting to a loop over `remove`.
  - `HnswIndex` overrides both. `NoopIndex` keeps both defaults.

- [ ] **Step 1: Write the failing unit tests** in `index.rs`'s test module. Reuse
  `tmp_path` and `meta`, and the `v_a`/`v_b` helpers or near variants.
  - `ids_lists_the_live_ids`: upsert `a`, `b`, `c`, remove `b`. Assert
    `ids() == Some({a, c})`.
  - `remove_many_compacts_and_persists_once`:
    - upsert 50 ids and run one `query` (which toggles searching mode), then
      `remove_many` 45 of them;
    - assert `len() == 5`, and that `query(v, 5, …)` returns exactly the 5 retained ids;
    - reopen with `HnswIndex::open` and assert `ids()` is the same 5.
  - `remove_many_to_empty_persists_the_empty_state`: upsert 3, `remove_many` all 3, then
    reopen. Assert `ids() == Some({})` and `is_empty()`.
  - `remove_many_of_unknown_ids_is_a_no_op`: upsert 1, record the file's mtime and bytes,
    `remove_many([unknown])`. Assert the bytes are unchanged and `len() == 1`.
  - `two_handles_on_one_path_persist_valid_json`: open two `Arc<HnswIndex>` on one path.
    Two threads each upsert 50 distinct ids in a loop.
    - Assert **every** upsert returned `Ok`. Today the shared `x.hnsw.tmp` makes a rename
      fail.
    - `serde_json::from_slice::<serde_json::Value>` of the file succeeds.
    - No `*.tmp` sibling remains.
  - `noop_index_ids_is_none`: `NoopIndex.ids()` is `None`.

- [ ] **Step 2: Run them and verify they fail to compile** (no `ids` / `remove_many`).
  Run: `cargo test -p gecko-semantic-index --no-default-features`
  Expected: compile errors naming `ids` and `remove_many`.

- [ ] **Step 3: Implement.**
  - **Trait:** the two defaults, each with one doc sentence. `ids`'s doc states the `None`
    meaning.
  - **`HnswIndex::ids`:** take a read lock with `.unwrap_or_else(|e| e.into_inner())`, and
    return `Some(active.keys().cloned().collect())`.
  - **`HnswIndex::remove_many`:**
    - Take the write lock. If no requested id is in `active`, return `Ok(())`.
    - Otherwise, collect the retained `LiveEntry`s sorted by concept id, build a fresh
      `Inner::empty(dim)`, and replay them with `insert_entry`.
    - Store `searching_mode_set = false` (`Ordering::Release`), then `persist` once.
  - **`persist`:**
    - The temp name is `format!("{}.{}.{}.tmp", file_name, std::process::id(), n)`, a
      sibling of the target, where `n` comes from a `static TMP_COUNTER: AtomicU64`.
    - On a failed rename it removes the temp file, best-effort.

- [ ] **Step 4: Run them and verify they pass.**
  Run: `cargo test -p gecko-semantic-index --no-default-features`
  Expected: all pass.

- [ ] **Step 5: Run fmt and both clippy runs, then commit** with the message
  `feat(index): SemanticIndex lists its ids and removes in one compacting batch`.

---

### Task 2: `MemWriter` reconciles at startup, and the dirty set is cut

**Files:**
- Modify: `core/mem-gecko/Cargo.toml`: move `tokio` into `[dependencies]` with
  `features = ["rt"]`, and add `tracing = { workspace = true }`. Keep the dev-dependency
  feature set for tests.
- Modify: `core/mem-gecko/src/writer/index.rs` (the whole dirty-set and rebuild section).
- Modify: `core/mem-gecko/src/writer/mod.rs`: drop the `dirty` field and its init, and fix
  the module docs at `:8` and `:57`.
- Modify: `core/mem-gecko/src/writer/beliefs.rs`:
  - `resolve_prediction`, `:350`, indexes its episode;
  - the comments at `:215`.
- Modify: `core/mem-gecko/src/writer/edges.rs:112` and `writer/consolidation.rs:81`
  (comments).
- Modify: `core/mem-gecko/src/consolidation.rs:58-59` and `:274`: `vectors_marked_dirty` →
  `vectors_failed`.
- Modify: `core/mem-gecko/src/tql.rs`: delete `fetch_belief_embeddable` (`:566-580`).
- Modify: `core/mem-gecko/src/writer/tests.rs:239-360`: the dirty-set tests are rewritten
  as below.
- Modify: `core/mem-gecko/tests/semantic_index_integration.rs`, which holds the RED test
  at `:305`.
  - `forced_upsert_failure_is_repaired_by_drain_dirty_set` (`:448`) is replaced.
  - Every `rebuild_index_from_graph` call is renamed.
  - The `is_dirty` assertion at `:135` is deleted.
  - The stale "dirty" wording at `:6` and `:410` is fixed.
- Modify: `core/mem-gecko/tests/consolidation_daemon_test.rs:455,469,487`:
  `vectors_marked_dirty` → `vectors_failed`.
- Modify: `gecko-bin/src/main.rs:1034`: rename the call to `reconcile_index()`, and leave
  its result unused until Task 3.
- Fix the stale "dirty" doc wording:
  - `core/gecko-extension-api/src/semantic.rs:143`;
  - `core/gecko-semantic-index/src/index.rs:164-165`;
  - `writer/index.rs:1`;
  - `writer/mod.rs:18`;
  - `writer/tests.rs:59`.
  The test names `…_is_not_dirty` and `…_never_dirties` are renamed to say what they
  assert.
- Modify: `gecko-bin/tests/belief_tier_reconciliation_test.rs:703`, and any other
  `rebuild_index_from_graph` caller found by `grep -rn rebuild_index_from_graph`: rename it
  to `reconcile_index`.

**Interfaces:**
- Consumes: Task 1's `ids()` and `remove_many()`.
- Produces:
  - `pub async fn reconcile_index(&self) -> Result<IndexReconciliation>` on `MemWriter`.
    It replaces `rebuild_index_from_graph`.
  - `pub struct IndexReconciliation { pub added: usize, pub removed: usize, pub full_rebuild: bool }`,
    re-exported from `mem_gecko`.
  - `IndexOutcome::Failed` replaces `MarkedDirty`.

- [ ] **Step 1: Write the failing tests.**

  **In `semantic_index_integration.rs`** (live TypeDB), using the existing `Fixture`,
  `tmp_index_path`, `ctx_at`, `belief` and `budget` helpers:
  - **The existing RED test**
    `an_index_built_for_another_database_does_not_hide_this_ones_beliefs`, renamed to call
    `reconcile_index`. It also asserts that reopening the file shows `ids() == {held}`.
  - **`reconcile_with_an_empty_graph_empties_the_file`:**
    - fill `path` from database A (one belief, `reconcile_index`);
    - open an index on the same path over a fresh, empty database B and call
      `reconcile_index`;
    - assert it returns `removed == 1` and `added == 0`;
    - reopen `HnswIndex::open(&path…)` and assert `ids() == Some({})`.
  - (Drop-and-recreate under one name is pinned at the CLI level in Task 3. The writer
    never sees the database name, so at this level it would duplicate the RED test.)
  - **`a_failed_live_upsert_is_repaired_by_the_next_reconcile`** replaces the `:448`
    test:
    - a `ToggleFailIndex` with failing upserts, then `assert_belief` returns `Ok`;
    - a fresh writer with a real `HnswIndex` on a fresh path calls `reconcile_index`;
    - it returns `added == 1`, and recall returns the belief.
  - **`items_written_while_the_index_was_off_are_recalled`:**
    - through `MemWriter::without_index`, assert a belief and `observe` an episode;
    - then a writer with an index on a fresh path calls `reconcile_index` and gets
      `added == 2`;
    - recall returns the belief, and `ids()` contains the episode's id.
  - **`a_warm_start_with_matching_ids_embeds_nothing`:**
    - with a `CountingEmbedder` (wrapping `StubEmbedder`, counting `embed_document` calls
      in an `AtomicUsize`), assert a belief, `observe` an episode and `resolve_prediction`
      a prediction;
    - drop the writer, reopen on the same path with a fresh counter, and call
      `reconcile_index`;
    - assert the counter is `0`, and the result is `added == 0`, `removed == 0`.
    - Write the prediction with `EpistemicWriter::record_prediction`
      (`gecko-extension-api/src/epistemic.rs:234`), then `resolve_prediction`.
  - **`a_failed_persist_during_reconcile_keeps_episode_vectors`:**
    - `observe` an episode through `without_index`;
    - open a real `HnswIndex` at `<tmpfile>/idx.hnsw`, where `<tmpfile>` is a **regular
      file**, so `persist` fails at mkdir and memory keeps the vectors;
    - `reconcile_index` returns `Ok`, and `index.ids()` contains the episode id.
    - Before the fix, the drain deletes it, so the test fails.

  **In `writer/tests.rs`** (serviceless), rewriting the dirty-set tests:
  - `assert_belief_survives_failing_index` and `assert_belief_survives_failing_embed`:
    the write returns `Ok`.
  - `supersede_with_failing_remove_reports_failed`: `best_effort_remove` returns
    `IndexOutcome::Failed`.
  - Every `is_dirty` / `dirty_len` assertion is deleted. The rest of each test stays.

- [ ] **Step 2: Run them and verify the failures.**
  Run: `cargo test -p mem-gecko --no-default-features -- --test-threads=1 2>&1 | tail -40`
  Expected: compile errors for `reconcile_index`, `IndexReconciliation` and
  `IndexOutcome::Failed`. Before the rename, the original RED test failed with
  `left: []`.

- [ ] **Step 3: Implement `reconcile_index`** in `writer/index.rs`:
  - **No embedder or no index:** return `Ok(IndexReconciliation::default())`.
  - **Enumerate:** call `enumerate_current_state_embeddables`, which now sorts **and**
    `dedup_by` id.
  - **Full rebuild**, when `idx.model_id() != emb.model_id()`, or when `idx.ids()` is
    `None` and `idx.is_empty()`:
    - call `idx.reset()`;
    - embed everything, then one `upsert_many`;
    - set `full_rebuild = true`.
  - **Self-managed index** (`ids() == None` and the index is not empty): return with zeros.
  - **Diff:**
    - `stale` is the index ids minus the graph ids. Call `remove_many(stale)` first; on an
      error, `warn!` and continue.
    - `missing` is the graph items whose id is not in the index. Embed them, then one
      `upsert_many`; on an error, `warn!`.
  - **Embedding:**
    - one `tokio::task::spawn_blocking` per batch, moving `Arc<dyn Embedder>` and the
      `(id, text, meta)` items, and returning the successfully embedded items;
    - an embed failure for one item warns and skips it;
    - a `JoinError` warns and embeds nothing.
  - **The result counts** the ids actually submitted to each successful call.
  - **Best-effort hooks:**
    - `best_effort_index` warns on failure;
    - `best_effort_remove` warns and returns `IndexOutcome::Failed`.
  - **`resolve_prediction`:** after the committed write, call
    `best_effort_index(&ep_id, &format!("prediction resolved: {oc}"), meta)`, with the
    same `FilterMeta` that `observe` builds.
    - Move the title string into one helper, used by both `resolve_prediction` and
      `tql::resolve_prediction_ops`, so the live text and the stored title can't drift.
  - **Deletions:** the dirty-set methods (`mark_dirty`, `dirty_snapshot`, `is_dirty`,
    `dirty_len`, `clear_dirty`, `drain_dirty_set`), the `dirty` field and
    `tql::fetch_belief_embeddable`.

- [ ] **Step 4: Run the tests and verify they pass.**
  Run: `cargo test -p mem-gecko --no-default-features -- --test-threads=1`
  Expected: all pass, the new integration tests included.

- [ ] **Step 5: Run the whole candle-free suite** (other crates call the renamed API).
  Run: `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1 2>&1 | tail -15`
  Expected: all pass, with the `main.rs:1034` call renamed in this task.

- [ ] **Step 6: Run fmt and both clippy runs, then commit** with the message
  `fix(mem): reconcile the semantic index with the graph at startup`.

---

### Task 3: One index file per database in the CLI

**Files:**
- Modify: `gecko-bin/src/config.rs`:
  - `SemanticIndexConfig` (`:47-80`): add `dir: Option<String>`, and keep
    `path: Option<toml::Value>` as a refused key;
  - `default_gecko_toml` (`:248-262`);
  - the tests at `:438` and `:454`.
- Modify: `gecko-bin/src/config/paths.rs`:
  - the index resolvers (`:100-125`);
  - the non-creating cache root (`:62-75`);
  - the tests (`:213-247`);
  - `runtime_assets_never_resolve_inside_repo_or_target` (`:252`), which stays pure
    through `derive_index_dir`.
- Modify: `gecko-bin/src/main.rs`:
  - `build_epistemic_writer` (`:962`) and its call (`:1189`), passing
    `writer_config.database`;
  - the startup log (`:1036-1041`);
  - `Commands::Drop` (`:468`) and `cmd_drop` (`:658`).
- Modify: `gecko-bin/src/doctor.rs`: `check_index` (`:381-403`) and its test (`:581-594`).
- Modify: `gecko.toml:18` (delete the `path` line), and delete line `:33` of `.gitignore`.
- Modify: `gecko-bin/tests/belief_tier_reconciliation_test.rs:252-258`, which writes
  `dir = <tempdir>` instead of `path`.

**Interfaces:**
- Consumes: Task 2's `MemWriter::reconcile_index() -> Result<IndexReconciliation>`.
- Produces (re-exported from `config`, replacing `index_path`):
  - `pub fn index_dir(cfg: &GeckoConfig) -> Result<PathBuf>`. It resolves `dir`, or else
    `<cache root>/index`. It creates nothing. It errors when `path` is set, with the text
    in Step 3, or when the result exists and is not a directory.
  - `pub fn index_file(cfg: &GeckoConfig, database: &str) -> Result<PathBuf>`. It returns
    `index_dir(cfg)?.join(format!("{}.hnsw", encode_database_name(database)?))`.
  - `fn encode_database_name(name: &str) -> Result<String>`, private to `paths.rs`. It
    keeps `[A-Za-z0-9_-]` and writes every other UTF-8 byte as `%XX` (uppercase hex). An
    empty name is an error.
  - `fn cache_root(cfg: &GeckoConfig) -> Result<PathBuf>`, which resolves without creating.
    `cache_dir` becomes `cache_root` plus `create_dir_all`.
  - `fn derive_index_dir(cache_root: &Path, sic: &SemanticIndexConfig) -> PathBuf`, pure,
    which `index_dir` wraps.
  - `build_epistemic_writer` takes the database name, cloned from `writer_config.database`
    before `writer_config` is moved (`main.rs:1183-1185`).

- [ ] **Step 1: Write the failing unit tests.**
  - **In `paths.rs`:**
    - `index_file_defaults_to_cache_index_dir`: with `GECKO_CACHE_DIR` pinned under
      `ENV_LOCK` to a tempdir, `index_file(&default_enabled_cfg, "alpha")` is
      `<tmp>/index/alpha.hnsw`, and `<tmp>/index` does not exist afterwards.
    - `explicit_dir_is_used`: `dir = "/x/idx"` gives `/x/idx/alpha.hnsw`.
    - `encoded_names_stay_in_dir_and_are_injective` (with an explicit `dir`, so it reads
      no env):
      - for `["a/b", "..", "données", "a%2Fb", "A-b_9"]`, every `index_file(…)` has
        parent `== index_dir(…)`;
      - the five filenames are pairwise distinct;
      - `"A-b_9"` encodes to itself, and `"a/b"` encodes to `a%2Fb`, distinct from
        `"a%2Fb"`'s `a%252Fb`.
    - `empty_database_name_is_refused`.
    - `a_stale_path_key_is_refused_naming_dir`:
      `path = Some(toml::Value::String("gecko.hnsw".into()))`. The `index_dir` error
      contains both `[semantic_index] path` and `[semantic_index] dir`.
    - `a_dir_that_is_a_file_is_refused`.
    - The old `index_path_defaults_derive_but_explicit_overrides` test is deleted. The
      "no runtime asset under the repo" test is kept, adjusted to `index_dir`.
  - **In `config.rs`:**
    - the `:438` and `:454` tests assert on `dir`;
    - `a_config_with_path_still_parses`: config load accepts `path`, and only the
      resolver refuses it.
    - `default_gecko_toml_dir_line_is_accepted_when_uncommented`: replace the
      `# dir = ` line with its uncommented form, parse it, and `index_dir` succeeds.
  - **In `doctor.rs`:** `check_index_reports_dir_state_without_side_effects`:
    - disabled → `Ok`;
    - enabled with an absent `dir` → `Warn`, and the dir is still absent afterwards;
    - an existing dir → `Ok`;
    - `path` set → `Warn`, with a message containing `dir`;
    - `dir` is a regular file → `Warn`.
  - `documented_config_snippets_are_accepted` (existing) must keep passing.

- [ ] **Step 2: Run them and verify they fail.**
  Run: `cargo test -p gecko-bin --no-default-features --lib 2>&1 | tail -30`
  Expected: compile errors for `index_dir`, `index_file` and `dir`.

- [ ] **Step 3: Implement.**
  - **The `path` refusal text,** exact:
    `` `[semantic_index] path` is no longer read: the index is one file per database. Set `[semantic_index] dir` to a directory instead, or remove `path` to use the cache directory. ``
    - Greenfield note: this is user-facing error text, so "no longer" is acceptable here.
      The rule binds code comments, not user messages.
  - **The `dir`-is-a-file refusal** names the path and says to point `dir` at a directory.
  - **`build_epistemic_writer(store, cfg, edges, database: &str)`:**
    - It uses `config::index_file(cfg, database)?` and calls `reconcile_index()`.
    - It logs `info!` with the path, `added`, `removed` and `full_rebuild`, and the
      message `"Semantic index enabled (hnsw); reconciled with the graph"`.
  - **`Commands::Drop`** passes `&cfg` to `cmd_drop(config, &cfg)`.
    - **After** a successful `delete_database`, and only if `cfg.semantic_index.enabled`:
      - resolve `index_file(cfg, &name)`;
      - on an error, `eprintln!` a warning and still return `Ok`;
      - otherwise call `std::fs::remove_file`, ignoring `NotFound` and warning on any
        other error.
    - The `gecko` refusal comes from `delete_database`, so its index file is never reached.
  - **`check_index`** uses `index_dir`:
    - an existing dir → Ok, `"index directory at …"`;
    - an absent dir → Warn, `"index directory absent at … — created and filled from the graph on the next run"`;
    - an `Err` → Warn, `"{e:#} — the next run will fail until this is fixed"`.
  - **`default_gecko_toml`:** replace the `path` comment block with
    `# The index directory; one <database>.hnsw file per database. Omitted (the default) uses cache_dir/index/.`
    followed by `# dir      = "/explicit/index/dir"`.
  - **`gecko.toml`:** delete the `path` line. **`.gitignore`:** delete `gecko.hnsw`.

- [ ] **Step 4: Run the tests and verify they pass.**
  Run: `cargo test -p gecko-bin --no-default-features --lib`
  Expected: all pass.

- [ ] **Step 5: Run the CLI checks on live TypeDB.**
  Add `gecko-bin/tests/index_per_database_cli_test.rs`. It carries no `cfg(feature)` gate,
  and its config uses `[extensions] enabled = []`. It has its own small rig: a tempdir
  `gecko.toml` with `dir = <tempdir>/idx`, a throwaway database name, and a bundle written
  to the tempdir that holds one JS playbook granting `mem:observe` (or the narrowest
  mem-writing scope) and calling it. It runs the binary through `env!("CARGO_BIN_EXE_gecko")`,
  and asserts on **combined** stdout and stderr.
  - `drop_removes_only_that_databases_index_file`:
    - schema-init, sync and `gecko run` the playbook;
    - `<dir>/<db>.hnsw` exists;
    - create a sibling `other.hnsw`;
    - `gecko drop <db>` exits 0, `<db>.hnsw` is gone, and `other.hnsw` remains.
  - `a_recreated_database_does_not_recall_the_dropped_ones_items`:
    - run once (one observed episode), then drop;
    - assert `gecko databases` no longer lists the name;
    - schema-init and sync the same name, then run a second playbook that recalls the
      first episode's text and logs the result count;
    - the log shows 0 hits from the dropped database's item. Create the stale file by
      copying the first run's `<db>.hnsw` aside before the drop and back after, so the
      test exercises reconciliation, not the drop's cleanup.
  - `drop_of_the_reserved_database_leaves_its_index_file`:
    - its config sets `[typedb] endpoint = "localhost:1"`, so no server is reachable even
      if the guard regressed;
    - create `<dir>/gecko.hnsw`;
    - `gecko drop gecko` exits non-zero, and the file still exists.
  - `a_stale_path_key_fails_run_but_not_databases_or_drop`: a config with
    `path = "x.hnsw"`, after a real schema-init and sync of a throwaway database.
    - `gecko databases` exits 0;
    - `gecko run <concept>` exits non-zero, and the output contains `[semantic_index] dir`;
    - `gecko drop <db>` exits 0, and the output warns.
  Run: `cargo test -p gecko-bin --test index_per_database_cli_test --no-default-features -- --test-threads=1`
  Expected: all pass, with 4 tests run (not 0).

- [ ] **Step 6: Run the whole candle-free suite.**
  Run: `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber -j 1 2>&1 | tail -15`
  Expected: all pass, `belief_tier_reconciliation_test` included.

- [ ] **Step 7: Run fmt and both clippy runs, then commit** with the message
  `fix(cli): one semantic index file per database under [semantic_index] dir`.

---

### Task 4: Docs

**Files:**
- Modify: `CLAUDE.md:75`. The rule's last sentence becomes:
  `Index mutations are best-effort and post-commit; a failure logs a warning, never fails the authoritative graph write, and is repaired when the next startup reconciles the index with the graph.`
- Modify: `docs/refactor/KNOWN-LIMITATIONS.md`. Add a semantic-index note:
  - the index is one file per database under `[semantic_index] dir`, keyed by the database
    name;
  - names that differ only in case share a file on a case-insensitive filesystem, as do
    same-named databases on two TypeDB servers;
  - results stay correct, and each alternation re-embeds the difference;
  - a `[semantic_index] path` key is refused.
- Modify: `docs/refactor/OWED-WORK-LEDGER.md`. Mark N3 done (✅) with its tests and this
  PR, and remove it from "Next up".
- Modify: `README.md` / `docs/INSTALL.md` wherever they describe the index location or
  `gecko.hnsw` (check with `grep -n "hnsw\|semantic_index" README.md docs/INSTALL.md`).

- [ ] **Step 1: Edit the files listed above.**
- [ ] **Step 2: Run the guard tests.**
  Run: `cargo test -p gecko-bin --no-default-features --lib documented_config_snippets_are_accepted`
  Expected: pass.
  Run: `grep -rnwE "rebuild_index_from_graph|drain_dirty_set|mark_dirty|is_dirty|dirty_len|dirty_snapshot|MarkedDirty|vectors_marked_dirty|fetch_belief_embeddable|index_path" --include='*.rs' --include='*.md' --include='*.toml' . | grep -v -e target/ -e docs/superpowers/ -e LEDGER-HISTORY`
  Expected: no hits other than ratified docs (listed and left unedited).
  Run: `grep -rn "gecko\.hnsw" --include='*.md' --include='*.toml' . | grep -v -e target/ -e docs/superpowers/ -e LEDGER-HISTORY`
  Expected: no hits other than ratified docs.
- [ ] **Step 3: Commit** with the message `docs: the semantic index is one file per database, reconciled at startup`.

---

## Finish

- Run the full candle-free suite with `-j 1`, fmt, both clippy runs, `cargo audit` and
  `cargo deny check licenses bans sources`.
- Final whole-branch review on Fable, with one fix pass.
- Push `fix/index-per-database` and open a PR against `main`, then watch CI.
