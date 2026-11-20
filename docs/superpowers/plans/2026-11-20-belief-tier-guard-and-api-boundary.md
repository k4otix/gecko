# Belief-Tier Guard and Extension API Boundary (A4, A10) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the belief tier writable only through mem's writer, and prove it at runtime:
- extensions compile against `gecko-extension-api` alone, and no extension is offered mem's writer (A10);
- every `sync_bundle` refuses to delete a concept a belief edge references, and refuses any sync that changed the belief tier;
- a live rig reconciles every production path against an in-process `MemWriter` reference (A4), and a hostile test shows raw writes are refused by the guard and reported by the rig.

**Architecture:**
- **gecko-extension-api:** gains the OKF types and `ExtensionCallback`; loses `SandboxCtx` and the `inject_epistemic_host_fns` hook.
- **gecko-engine:**
  - re-exports the moved types;
  - gains `db::surface` (introspected surface and fingerprint) and `db::label` (`validate_schema_label`);
  - `sync_bundle` takes `belief_tier_root`, runs a pre-GC reference check, and brackets its whole transaction with two fingerprints.
- **cyber-gecko:** drops `gecko-engine`. A CI script fails if any crate under `extensions/` depends on `gecko-engine` or `typedb-driver`.
- **mem-gecko:** exports `BELIEF_TIER_ROOT`.
- **gecko-bin:** `build_host_bridge` binds the writer itself. A binary-driven rig and a reference database reconcile per-label deltas.

**Tech Stack:** Rust 2024 workspace, TypeDB 3.15 (`typedb-driver ~3.15`), tokio (incl. `tokio::process`), nextest, bash + `jq` in CI.

**Spec:** `docs/superpowers/specs/2026-11-20-belief-tier-guard-and-api-boundary-design.md`. It includes the 2026-11-20 pre-GC amendment, O6 and D-O. The owning spec is `docs/refactor/SPEC-W5.md` §1.2, §2.2, §3.5, §5 rows 2, 3, 8. Read also `docs/superpowers/specs/2026-11-20-multi-bundle-databases-design.md`: this plan runs on top of it.

## Global Constraints

- **Stacking.**
  - Branch `feat/belief-tier-guard`, created from `feat/multi-bundle-databases` (PR 1), or from `main` once PR 1 has merged.
  - PR 1 provides:
    - syncs scoped to their own bundle;
    - GC that keeps a concept other bundles also contain;
    - `DbError::ConceptIdCollision`;
    - `sync-stix --bundle-name`;
    - same-bundle link resolution;
    - a core-schema migration list that fixes the `bundle` `@doc`;
    - `cyber_detect_test` without its raw logic-concept insert;
    - a shared-database binary test.
  - PR 1 does not change `sync_bundle`'s parameters. This plan does not touch the core schema or its migrations.
  - Line numbers below are from `main` at `0802463`. Expect drift in `bundle.rs`, `router.rs`, `main.rs`, `CLAUDE.md`, `README.md` and the ledger, and locate by function name or text.
  - After any rebase onto a changed PR 1, diff PR 1's `core/gecko-engine/src/okf/types.rs` doc comments against `core/gecko-extension-api/src/okf.rs`, and carry the changes across.
- **Parameterized TQL.** Values go through `given`/`GraphValue` params, in tests too. Only schema labels are interpolated, and only after `validate_schema_label`. The surface and fingerprint read labels from the live schema.
- **Greenfield comments.** No plan or epic tags (`A4`, `A10`, `PR 1`) and no changelog phrasing in code comments (CLAUDE.md "Comments are greenfield").
- **Never fabricate success.**
  - A count query that errors or returns other than one row is an error, never 0.
  - A stream's `Some(Err(e))` is an error.
  - A guard that inspects nothing is an error.
- **Integration tests** use throwaway databases via `common::TestDb`, never the database `gecko`, and run with `--test-threads=1` against the local TypeDB on `localhost:1729`.
- **Lint and format before every commit (as `ci.yml` runs them):**
  - `cargo clippy --locked --workspace --exclude js-sandbox --no-default-features --all-targets -- -D warnings`
  - `cargo clippy --locked --workspace --exclude js-sandbox --no-default-features --features cyber --all-targets -- -D warnings`
  - `cargo fmt --all -- --check`
  - Code reachable only under `cyber` is `#[cfg(feature = "cyber")]`-gated. Never `#[allow(dead_code)]`. Every helper, field and constant is used by the commit that adds it.
- **`Cargo.lock`** is committed when dependencies change. CI uses `--locked`.
- **Commits:** end every message with the co-author attribution line your session specifies. Commit on the feature branch only, and never push.
- **Unchanged behaviour:**
  - The A3 lint (`no_raw_belief_tier_tql_outside_mem_tql`) and the P-W5-1 probe (`schema_probe_w2w3.rs`) keep their pass/ignore sets.
  - `cyber_detect_test` and `cyber_stix_sync_test` keep every assertion. Only call sites change.
  - Engine and `gecko-bin` code that names `gecko_engine::okf::types::*` or `gecko_engine::sandbox::engine::ExtensionCallback` compiles unchanged.

## Review Focus

1. **A re-sync whose GC would delete a record concept that a belief-tier relation references.** Examples: a playbook removed or renamed after `claim`/`disposition`, or a technique dropped from a newer ATT&CK release.
   - The sync must refuse before any delete, with an error naming each concept ID and relation and saying how to recover.
   - It must not delete the concept and leave a mutilated belief edge, and it must not report only a count change.
   - Task 4 pins this with `gc_of_a_concept_a_belief_edge_references_is_refused_before_any_delete` (mem-only). Task 6 pins it with the cyber case `gc_of_a_concept_a_belief_edge_references_is_refused`.
2. **A sync into a database without mem's schema** (core only) must fail with `DbError::Schema` naming `memory-item` and write nothing. It must not fail with a confusing driver error. Task 4 pins it with `sync_without_the_belief_tier_root_is_refused`.
3. **A `type_hint` naming a type the schema lacks** (`no-such-type`) must give `DbError::Schema` and write nothing, on insert and on update, not a raw `INF2` query error. Task 4 pins both with `unknown_type_hint_is_refused_as_a_schema_error`.
4. **An extension write that only adds an attribute to an existing belief** (no new instance) must still be refused. The changes are `Attributes` on `memory-item` and `belief`. Task 6 pins it as the third guard case.
5. **The fingerprint on a fresh schema** must hold a zero for every key, including relations that own no attribute. Task 3 pins it with `fingerprint_of_an_empty_belief_tier_is_all_zero`, with a pinned fallback if TypeDB errors on that shape.

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `core/gecko-extension-api/src/okf.rs` | create | the OKF types moved from the engine, verbatim |
| `core/gecko-extension-api/src/lib.rs` | modify | `pub mod okf`; `ExtensionCallback`; remove the hook, `SandboxCtx`, the `Arc` import |
| `core/gecko-extension-api/src/sandbox_ctx.rs` | delete | — |
| `core/gecko-engine/src/okf/types.rs` | modify | becomes `pub use gecko_extension_api::okf::*;` plus its module doc |
| `core/gecko-engine/src/sandbox/engine.rs` | modify | `pub use gecko_extension_api::ExtensionCallback;` |
| `extensions/cyber-gecko/Cargo.toml`, `src/stix/mod.rs`, `src/detect/mod.rs`, `Cargo.lock` | modify | API-crate imports only; drop `gecko-engine` |
| `scripts/check-extension-deps.sh` | create | the A10 dependency guard |
| `.github/workflows/ci.yml` | modify | a step running the guard |
| `core/mem-gecko/src/lib.rs` | modify | remove the hook override; add `BELIEF_TIER_ROOT` |
| `gecko-bin/src/main.rs` | modify | `build_host_bridge` without `SandboxCtx`; `sync_manifest` passes the root |
| `gecko-bin/tests/epistemic_bridge_test.rs` | modify | keep the run-stamp test; delete the hook test and its double |
| `core/gecko-engine/src/db/label.rs` | create | `validate_schema_label` (moved) + unit tests |
| `core/gecko-engine/src/db/surface.rs` | create | `BeliefTierSurface`, fingerprint, changes |
| `core/gecko-engine/src/db/mod.rs`, `db/router.rs` | modify | modules; `DbError::BeliefTierWrite`, `DbError::BeliefEdgeReferencesRemovedConcept` |
| `core/gecko-engine/src/syncer/bundle.rs` | modify | pre-GC check, fingerprint guard, `type_hint` check, docs |
| `gecko-bin/tests/common/surface.rs` | modify | `live_belief_tier_surface` delegating to the engine |
| `gecko-bin/tests/belief_tier_monopoly_test.rs`, `schema_probe_w2w3.rs` | modify | use the delegate; `read_col` back in the probe |
| `gecko-bin/tests/belief_tier_surface_test.rs` | create | live surface and fingerprint tests (mem only, not cyber-gated) |
| `gecko-bin/tests/bundle_sync_test.rs`, `no_drag_test.rs`, `sandbox_test.rs`, `cyber_detect_test.rs`, `cyber_stix_sync_test.rs`, and every other `sync_bundle(` caller on the PR 1 branch (incl. `multi_bundle_test.rs`) | modify | new parameter; `sandbox_test` applies mem's schema |
| `gecko-bin/tests/belief_tier_reconciliation_test.rs` | create | the rig (Task 5); the hostile and cyber GC-refusal tests (Task 6); `#![cfg(feature = "cyber")]` |
| `extensions/README.md`, `CLAUDE.md`, `README.md`, `docs/refactor/KNOWN-LIMITATIONS.md` | modify | boundary, hook removal, sync guard, pre-GC refusal |
| `docs/refactor/OWED-WORK-LEDGER.md`, `docs/refactor/SPEC-W5.md`, `docs/superpowers/specs/2026-11-20-multi-bundle-databases-design.md` | modify | A4/A10 done, O6, D-O, S5 wording; §5 note; PR 1 goal-4 wording |

---

### Task 1: Move the OKF types and `ExtensionCallback` into the API crate, drop cyber's engine dependency, add the CI guard

**Precondition:** this task runs after PR 1 (`feat/multi-bundle-databases`) is merged, or on top of its branch. Create `feat/belief-tier-guard` from whichever holds PR 1, and confirm `git log` shows PR 1's commits.

**Files:**
- Create: `core/gecko-extension-api/src/okf.rs`
- Modify: `core/gecko-extension-api/src/lib.rs` (module list at about `:32-39`, re-exports, beside `HostImportDef`)
- Modify: `core/gecko-engine/src/okf/types.rs` (whole file), `core/gecko-engine/src/sandbox/engine.rs:21-24`
- Modify: `extensions/cyber-gecko/Cargo.toml`, `extensions/cyber-gecko/src/stix/mod.rs:2,264,293`, `extensions/cyber-gecko/src/detect/mod.rs:25`, `Cargo.lock`
- Create: `scripts/check-extension-deps.sh`
- Modify: `.github/workflows/ci.yml` (a new step right after "Guard — no candle in the candle-free build", about `:146-153`)
- Modify: `extensions/README.md` (the section "Why the bridge is wired in the binary, not the trait", about `:169-175`; checklist step 1, about `:179-180`), `README.md` (the `core/gecko-extension-api/` bullet, about `:41`)

**Interfaces:**
- Produces:
  - `gecko_extension_api::okf::{OkfBundle, OkfConcept, OkfLink, OkfCitation, HierarchyEdge, TypedAttribute, TypedAttrValue, ScriptEngine}`, with `ScriptEngine`'s inherent `as_str`/`from_lang`. Derives, `serde` attributes and doc comments are copied unchanged from the PR 1 version of the file.
  - `pub type ExtensionCallback = std::sync::Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>;` at the crate root of `gecko_extension_api`.
  - `gecko_engine::okf::types` re-exports `gecko_extension_api::okf::*`, and `gecko_engine::sandbox::engine::ExtensionCallback` re-exports the API type. `ExecutionResult` and `HostImports` stay in the engine.
  - `scripts/check-extension-deps.sh`: exit 0 and print `OK: …` when clean. Exit 1 with a `::error::` line on any failure, including a failing `cargo` command.

**Script shape (pinned; it must fail closed):**
- `#!/usr/bin/env bash`, `set -euo pipefail`. Portable to macOS bash 3.2, so no `mapfile`.
- `meta=$(cargo metadata --no-deps --format-version 1 --locked)`, as its own statement.
- `root=$(printf '%s' "$meta" | jq -r .workspace_root)`.
- `crates=$(printf '%s' "$meta" | jq -r --arg dir "$root/extensions/" '.packages[] | select(.manifest_path | startswith($dir)) | .name')`.
- If `crates` is empty: print `::error::no workspace crate found under extensions/` and exit 1.
- For each crate `c`:
  - `tree=$(cargo tree -p "$c" --all-features -e normal,build,dev --prefix none --locked)`, as its own statement, so a cargo failure aborts the script.
  - The first line of `tree` must start with `"$c v"`; otherwise print `::error::cargo tree for $c printed no tree` and exit 1.
  - Matching runs on the captured text. `if bad=$(printf '%s\n' "$tree" | grep -E '^(gecko-engine|typedb-driver) ' | sort -u); then` (a match) print `::error::$c depends on <each line>` and mark the crate failed. Else (no match) the crate is clean. No `|| true` anywhere.
- At the end, exit 1 if any crate failed, else print `OK: <crates> depend on neither gecko-engine nor typedb-driver.`

- [ ] **Step 1: Write the guard script** as pinned above, and `chmod +x` it.
- [ ] **Step 2: Run it and confirm it fails.** Run `scripts/check-extension-deps.sh`. Expected: exit 1, with `::error::cyber-gecko depends on gecko-engine v0.1.0 (…)` and a `typedb-driver v3.15.0` line (transitive through the engine).
- [ ] **Step 3: Wire it into CI.** Add a step `Guard — extensions depend only on gecko-extension-api` with `run: scripts/check-extension-deps.sh`, after the candle guard. Include a two-line comment saying what it enforces. `jq` is preinstalled on `ubuntu-latest`.
- [ ] **Step 4: Move the types.**
  - Create `okf.rs` from `core/gecko-engine/src/okf/types.rs` verbatim, with the module doc adjusted to say these are the OKF types extensions and the engine share.
  - Add `pub mod okf;` and `ExtensionCallback` (with a one-sentence doc) to `lib.rs`. The crate already has `chrono` with `serde`, `serde` and `serde_json`. Add no dependency.
  - Replace the engine's `okf/types.rs` body with a module doc and `pub use gecko_extension_api::okf::*;`. Replace the `ExtensionCallback` definition in `sandbox/engine.rs` with a re-export, beside the existing `HostImportDef` re-export.
- [ ] **Step 5: Cut cyber over.**
  - `stix/mod.rs` imports `gecko_extension_api::okf::{…}`, including the two `OkfLink` paths at `:264` and `:293`. `detect/mod.rs:25` imports `gecko_extension_api::ExtensionCallback`.
  - Remove `gecko-engine = …` from `extensions/cyber-gecko/Cargo.toml`. `git grep -n gecko_engine -- extensions/` must be empty.
  - Run `cargo build --no-default-features --features cyber -p gecko-bin` to refresh `Cargo.lock`.
- [ ] **Step 6: Update the docs.**
  - **`extensions/README.md`, "Why the bridge is wired in the binary, not the trait":** rewrite it to say that `ExtensionCallback` lives in `gecko-extension-api`, and that the bridge is still composed in `build_host_bridge` because the host owns what a bridge captures (the `GraphReader`, the `BeliefEdgeWriter` bound to the extension, the `RunContext`). Adding a bridged extension means adding one fold there. Write it greenfield, with no "now".
  - **Checklist step 1:** the crate depends only on `gecko-extension-api`, enforced in CI by `scripts/check-extension-deps.sh`.
  - **`README.md`:** the API crate also holds the OKF import types and `ExtensionCallback`, and extensions depend on it alone.
- [ ] **Step 7: Run the checks and confirm they pass.**
  - `scripts/check-extension-deps.sh`. Expected: `OK: cyber-gecko …`, exit 0.
  - **One-off negative check:**
    - Temporarily add `typedb-driver.workspace = true` to cyber's `[dependencies]`.
    - Refresh the lockfile with `cargo update -p cyber-gecko`. Without this, `--locked` fails on a stale lock and the script exits 1 for the wrong reason.
    - Run the script. Expected: exit 1, and the output contains `::error::cyber-gecko depends on typedb-driver v3.15.0`.
    - Revert with `git checkout extensions/cyber-gecko/Cargo.toml Cargo.lock`, re-run the script and confirm OK.
    - Record all three outputs in your report.
  - `cargo test -p cyber-gecko --no-default-features`, `cargo test -p gecko-engine --no-default-features --lib`, `cargo test -p gecko-extension-api --no-default-features`. Expected: PASS.
  - `cargo test -p gecko-bin --no-default-features --features cyber --no-run`, to compile every test crate unchanged. Expected: PASS.
  - Both clippy commands and the fmt check. Expected: clean.
- [ ] **Step 8: Commit**, including `Cargo.lock`. `feat: OKF types and ExtensionCallback in gecko-extension-api; extensions depend on it alone (CI-guarded)`

### Task 2: Remove `inject_epistemic_host_fns` and `SandboxCtx`

**Files:**
- Modify: `core/gecko-extension-api/src/lib.rs`:
  - `:141-153` (the hook and its doc);
  - `:23` (`use std::sync::Arc`);
  - `:38` and `:52` (the `sandbox_ctx` module and its re-export).
- Delete: `core/gecko-extension-api/src/sandbox_ctx.rs` (including its unit tests)
- Modify: `core/mem-gecko/src/lib.rs:11-15` (imports), `:105-113` (override)
- Modify: `gecko-bin/src/main.rs:30-33` (imports), `:949-1034` (`build_host_bridge` and its doc)
- Modify: `gecko-bin/tests/epistemic_bridge_test.rs`
- Modify: `extensions/README.md:31` (trait-table row), `CLAUDE.md:71` (the parenthetical naming the hook)

**Interfaces:**
- Produces: `async fn build_host_bridge(concept_id: &str, cfg: &GeckoConfig, writer_config: DbConfig, extensions: Vec<Box<dyn GeckoExtension>>) -> Result<ExtensionCallback>` with the same signature. The body:
  - builds `mem_writer` as today;
  - takes `let writer: Arc<dyn EpistemicWriter> = mem_writer.clone();` and `let reader = writer.clone().as_epistemic_reader();`;
  - mints one `RunContext` (`ActorId::new("system")`, `ExecutableDoc { concept_id }`);
  - wraps the base dispatcher with `epistemic_extension_callback`, then folds cyber's bridge on top when cyber is active.
  - There is no `match` and no `None` arm. The writer reaches only mem's bridge.

- [ ] **Step 1: Record the red state.** Save the output of `git grep -n 'inject_epistemic_host_fns\|SandboxCtx' -- '*.rs'` for your report. This task has no failing test: the removal itself is the change, and the grep is its evidence before and after.
- [ ] **Step 2: Change the test.** In `epistemic_bridge_test.rs`:
  - Delete `hook_delivers_working_writer_to_domain_extension` (`:233-285`), `DomainDouble` (`:143-163`) and the `belief_run_id` helper (`:100-…`), which only that test used.
  - In `mem_host_fn_through_bridge_is_run_stamped_and_ignores_forged_run_id`, remove the `SandboxCtx`/`inject_epistemic_host_fns` activation and its `has_epistemic_writer` assert, and pass `fx.writer()` straight to `epistemic_extension_callback`. Every other assertion stays.
  - Rewrite the module doc to the one remaining property. Drop the unused imports.
- [ ] **Step 3: Remove the hook.**
  - Delete the trait method and its doc, `sandbox_ctx.rs`, the module line and the re-export.
  - Delete mem's override and the imports it alone used.
  - Rewrite `build_host_bridge` as in Interfaces. Its doc comment says it builds mem's writer and hands it only to mem's bridge, stamped with one host-minted `RunContext`, and folds cyber's bridge when active. No extension receives the writer.
  - Update the inline comments at `main.rs:970-975` and `:993-996`.
- [ ] **Step 4: Update the docs.**
  - **`extensions/README.md`:** delete the `inject_epistemic_host_fns` trait-table row.
  - **`CLAUDE.md:71`:** replace "(`inject_epistemic_host_fns`)" with "(`epistemic_extension_callback`, which `build_host_bridge` binds to mem's writer)".
  - **The API crate doc:** state that the host hands no extension mem's writer.
  - Re-run the Step 1 grep, plus `git grep -n 'inject_epistemic_host_fns\|SandboxCtx' -- ':!docs/superpowers' ':!docs/refactor'`. Both must be empty. Put the before and after outputs in your report.
- [ ] **Step 5: Run the tests and confirm they pass.**
  - `cargo test -p gecko-extension-api --no-default-features` and `cargo test -p mem-gecko --no-default-features --lib`.
  - `cargo test -p gecko-bin --bins --no-default-features --features cyber`. The `main.rs` unit tests, including `cyber_bridge_requires_cyber_belief_edges`, stay green.
  - `cargo test -p gecko-bin --test epistemic_bridge_test --test mem_recall_e2e_test --no-default-features -- --test-threads=1`.
  - Both clippy commands. Expected: PASS and clean.
- [ ] **Step 6: Commit.** `feat: remove inject_epistemic_host_fns; the host binds mem's writer to mem's bridge only`

### Task 3: The belief-tier surface and fingerprint as engine code

**Files:**
- Create: `core/gecko-engine/src/db/label.rs`, `core/gecko-engine/src/db/surface.rs`
- Modify: `core/gecko-engine/src/db/mod.rs` (`pub mod label; pub mod surface;`, `pub use label::validate_schema_label;`)
- Modify: `core/gecko-engine/src/syncer/bundle.rs:62-82` (remove the local fn; import `crate::db::validate_schema_label`)
- Modify: `core/mem-gecko/src/lib.rs` (add the const)
- Modify: `gecko-bin/tests/common/surface.rs` (whole file), `gecko-bin/tests/belief_tier_monopoly_test.rs:29-35,427-442`, `gecko-bin/tests/schema_probe_w2w3.rs:62-65,1945`
- Create: `gecko-bin/tests/belief_tier_surface_test.rs`

**Interfaces:**
- Produces, in `gecko_engine::db::label`:
  - `pub fn validate_schema_label(label: &str) -> Result<(), DbError>`, moved verbatim with its doc.
- Produces, in `gecko_engine::db::surface`:
  - `pub struct BeliefTierSurface { pub entities: BTreeSet<String>, pub relations: BTreeSet<String> }`, deriving `Debug, Clone, PartialEq, Eq`.
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)] pub enum BeliefTierMeasure { Instances, Attributes, Links }`, with a `Display` of `instances`/`attributes`/`links`.
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub struct BeliefTierChange { pub label: String, pub measure: BeliefTierMeasure, pub before: u64, pub after: u64 }`, with `pub fn delta(&self) -> i64` and a `Display` of `{label} {measure} {before}→{after}`.
  - `#[derive(Debug, Clone, Default, PartialEq, Eq)] pub struct BeliefTierFingerprint { pub counts: BTreeMap<(String, BeliefTierMeasure), u64> }`.
  - `impl BeliefTierFingerprint { pub fn changes_to(&self, after: &Self) -> Vec<BeliefTierChange> }`: the keys whose values differ, in key order. A key on only one side is a change whose missing side is 0. The guard and the rig always compare fingerprints of one surface; the unit test pins the rule.
  - `pub async fn belief_tier_surface(tx: &typedb_driver::Transaction, root: &str) -> Result<BeliefTierSurface, DbError>`:
    - `entities` is the reflexive `sub` closure of `root`.
    - `relations` is every relation owning a role that is played by ≥1 type and only by `entities`. This is the recipe in `common/surface.rs` today: one `$t plays $r; $rel relates $r;` read, grouped by role label host-side.
    - `root` passes `validate_schema_label` first.
    - **Error mapping:**
      - The driver's unknown-label error for `root` (`INF2`/`INF3`, raised by `query()` or by the first stream item) maps to `DbError::Schema`, and the message names `root`.
      - Every other driver or stream error is `DbError::Query`, so a connection failure is not reported as a missing label.
      - If the closure query succeeds but `entities` lacks `root`, the result is `DbError::Schema`.
      - If the closure plays some role but `relations` is empty, the result is `DbError::Schema`.
  - `pub async fn belief_tier_fingerprint(tx: &typedb_driver::Transaction, surface: &BeliefTierSurface) -> Result<BeliefTierFingerprint, DbError>`:
    - For each label in `entities ∪ relations`, it records `Instances` (`match $x isa <label>; reduce $n = count;`) and `Attributes` (`match $x isa <label>, has $a; reduce $n = count;`). For each relation, it also records `Links` (`match $x isa <label>, links ($p); reduce $n = count;`).
    - Each label passes `validate_schema_label`.
    - Each query must yield exactly one row with an integer `n`. Anything else is `DbError::Query` naming the label and measure.
    - `isa` (not `isa!`), so supertypes count their subtypes' instances.
- Produces, in `mem_gecko`: `pub const BELIEF_TIER_ROOT: &str = "memory-item";`, with a doc saying it is the root type of the belief tier mem's writer owns.
- Produces, in `gecko-bin/tests/common/surface.rs`:
  - `pub async fn live_belief_tier_surface(db: &TestDb) -> BeliefTierSurface`: opens a read transaction and returns `belief_tier_surface(&tx, mem_gecko::BELIEF_TIER_ROOT)`, panicking with the error on `Err`.
  - It is the file's only item, so both including crates use everything in it.
- Consumes: `DbError` (`core/gecko-engine/src/db/router.rs:22`).

**Fallback for `has $a` on a type that owns no attribute (pinned):** the ledger records `reduce … count` over an unsatisfiable pattern returning 0 on 3.15, but not this exact shape.
- If `fingerprint_of_an_empty_belief_tier_is_all_zero` shows the attribute query erroring for such a label, change `belief_tier_fingerprint` to read each label's owned attribute types once: `match $t label <label>; $t owns $at; let $l = label($at); select $l;`, which includes inherited ownership.
- For a label that owns none, the fallback records `Attributes = 0` without running the count. The key stays, so the all-keys assertion holds.
- Note in the report which path shipped.

- [ ] **Step 1: Write the failing tests.**
  - **Unit tests in `label.rs`:** `validate_schema_label_accepts_identifiers_and_refuses_the_rest`. `memory-item`, `attack_pattern` and `t1` are `Ok`. `""`, `1abc`, `belief;`, `a b` and `x"` are each `Err(DbError::Schema(_))`.
  - **Unit test in `surface.rs`:** `changes_to_reports_each_differing_key`.
    - `before = {(belief, Instances): 1, (belief, Attributes): 2}`.
    - `after = {(belief, Instances): 2, (belief, Attributes): 2, (rests-on, Links): 2}`.
    - The changes are exactly `[belief Instances 1→2, rests-on Links 0→2]`, in that order, and the first has `delta() == 1`.
    - `before.changes_to(&before)` is empty.
  - **Live tests in `gecko-bin/tests/belief_tier_surface_test.rs`** (not cyber-gated). Each test applies `gecko_engine::db::CORE_SCHEMA` and then, except where noted, `MemGecko::new().schema()`, like `no_drag_test.rs:46-53`.
    - `surface_of_the_mem_schema_has_the_root_closure_and_its_belief_edges`:
      - `entities ⊇ {memory-item, belief, episode, anomaly, resolution, retrieval-event}`.
      - `relations ⊇ {supersession, derivation, ownership, source-link, rests-on, surfaced, informs-synthesis, scoped, resolves}`.
      - `relations` excludes `population-member`, `containment` and `concept-link`.
      - `entities` excludes `concept`.
    - `surface_without_the_root_is_an_error`: core only. `belief_tier_surface(&tx, "memory-item")` is `Err(DbError::Schema(m))` with `m.contains("memory-item")`.
    - `fingerprint_of_an_empty_belief_tier_is_all_zero` (Review Focus 5):
      - Every entity label has an `Instances` and an `Attributes` key.
      - Every relation label also has a `Links` key.
      - The map has exactly `2·|entities| + 3·|relations|` keys, and every value is 0.
    - `fingerprint_counts_instances_attributes_and_links`:
      - Take `f0`, then one write transaction with parameterized inserts: beliefs `mem/bel/a` and `mem/bel/b`, each with `concept-id` and `belief-state "asserted"`, and then `(resting: $a, assumption: $b) isa rests-on`. Commit, then take `f1`.
      - `f0.changes_to(&f1)`, as a set, is exactly:
        - `belief` and `memory-item`: Instances 0→2 and Attributes 0→4;
        - `rests-on`: Instances 0→1 and Links 0→2.
- [ ] **Step 2: Run and confirm the failure.** Run `cargo test -p gecko-engine --lib --no-default-features label surface` and `cargo test -p gecko-bin --test belief_tier_surface_test --no-default-features --no-run`. Expected: compile errors on the missing modules and functions.
- [ ] **Step 3: Implement** `label.rs`, `surface.rs`, the `db/mod.rs` lines and `BELIEF_TIER_ROOT`. Point `bundle.rs` at the moved `validate_schema_label`. No query text in `surface.rs` contains a write verb, so the A3 lint stays clean.
- [ ] **Step 4: Delegate the test helpers.**
  - Replace `common/surface.rs` with `live_belief_tier_surface`, and update its module doc.
  - Move `read_col` back into `schema_probe_w2w3.rs`, verbatim. It is the probe's helper, used at `:130,399,582`.
  - At `:1945`, the probe uses `live_belief_tier_surface(&db).await.relations`. Fix its `use surface::…` line.
  - In `belief_tier_monopoly_test.rs`, the local fn becomes `belief_tier_labels(db) -> BTreeSet<String>`, returning `entities ∪ relations` of `live_belief_tier_surface`, and its caller at `:533` uses it. Drop the `read_col`/`belief_edge_surface` import and keep the cyber gating as it is.
- [ ] **Step 5: Run the tests and confirm they pass.**
  - The Step 2 commands, then `cargo test -p gecko-bin --test belief_tier_surface_test --no-default-features -- --test-threads=1`. If the all-zero test errors on a relation's attribute count, apply the pinned fallback and re-run.
  - `cargo test -p gecko-bin --test belief_tier_monopoly_test --test schema_probe_w2w3 --no-default-features --features cyber -- --test-threads=1`. Expected: the same pass/ignore set as before; compare against a run on the Task 2 commit.
  - Both clippy commands. Expected: PASS and clean.
- [ ] **Step 6: Commit.** `feat(engine): introspected belief-tier surface and fingerprint; mem exports its root label`

### Task 4: The pre-GC reference check, the fingerprint guard, and the `type_hint` check

**Files:**
- Modify: `core/gecko-engine/src/db/router.rs:21-34` (two new variants, after PR 1's `ConceptIdCollision`)
- Modify: `core/gecko-engine/src/syncer/bundle.rs`:
  - the module doc (`:1-25`);
  - `sync_bundle` and its doc (`:129-223`);
  - `insert_concept_entities` (`:416-440`) and `attach_typed_attributes` (`:515-554`);
  - a new `async fn belief_edge_references(tx: &Transaction, ids: &[&str], surface: &BeliefTierSurface) -> Result<Vec<(String, String)>, DbError>`.
- Modify: `gecko-bin/src/main.rs` (`sync_manifest`, about `:647-664`)
- Modify: every `sync_bundle(` caller. On `main` these are `bundle_sync_test.rs:70,80,110,176`, `cyber_detect_test.rs:92`, `cyber_stix_sync_test.rs:146,238,318`, `no_drag_test.rs:60` and `sandbox_test.rs:80`. Re-list them on the branch with `git grep -n 'sync_bundle(' -- '*.rs'`, since PR 1 adds more, including `multi_bundle_test.rs`.
- Modify: `gecko-bin/tests/sandbox_test.rs:75-78` (apply mem's schema after core), and any other caller whose database lacks mem's schema. This is a setup change only.
- Modify: `extensions/README.md` ("Import-time writes" bullet, about `:160-162`), `CLAUDE.md` ("Epistemic memory" paragraph), `README.md` ("Knowledge Graph Syncer" bullet, about `:31`), `docs/refactor/KNOWN-LIMITATIONS.md` (new section)
- Test: `gecko-bin/tests/bundle_sync_test.rs`

**Interfaces:**
- Produces:
  - `DbError::BeliefTierWrite { changes: Vec<BeliefTierChange> }`. Its `#[error(…)]` text says the sync was refused because it changed the belief tier, and lists every change using `BeliefTierChange`'s `Display` (a small helper fn is fine).
  - `DbError::BeliefEdgeReferencesRemovedConcept { references: Vec<(String, String)> }`, holding `(concept_id, relation_label)` pairs, sorted and deduplicated. Pairs rather than two parallel lists, so each concept is named with the relation that holds it.
    - Its message lists every pair.
    - It says the recovery available today: restore the removed concept's file, or sync into a fresh database.
    - It does not suggest superseding the belief: the edge stays in the graph after a supersede, so the sync would still be refused.
  - `pub async fn sync_bundle(db: &mut TypeDbRouter, manifest: &OkfBundle, extension_writes: &[GraphWrite], belief_tier_root: &str) -> Result<SyncResult, DbError>`. It is required, with no default. The doc says:
    - the schema must contain the type `belief_tier_root`;
    - a concept the sync would garbage-collect that plays a role in a belief-tier relation stops the sync before any delete (with the recovery above);
    - otherwise the sync is refused with `BeliefTierWrite` when any belief-tier label's instance, attribute-ownership or role-player count differs between the start of the transaction and just before commit;
    - the residual is a same-count swap: a value replaced in place, or one instance deleted and an identical-shape one inserted.
- Consumes: Task 3's `belief_tier_surface`, `belief_tier_fingerprint`, `changes_to`, `validate_schema_label`, and `mem_gecko::BELIEF_TIER_ROOT`.

**Order inside `sync_bundle`:**
1. Right after `db.begin_write()`, before any read or write (including PR 1's membership read): `surface = belief_tier_surface(&tx, belief_tier_root)?` and `before = belief_tier_fingerprint(&tx, &surface)?`.
2. PR 1's reads, collision check and plan, unchanged.
3. **Pre-GC check.** Over exactly the IDs PR 1's GC will delete (`plan.to_delete` after PR 1's "keep concepts other bundles contain" rule), `belief_edge_references`:
   - runs `given $id: string; match $c isa concept, has concept-id $cid; $cid == $id; $r isa! $rt, links ($c); let $rl = label($rt); select $cid, $rl;` with one row per ID;
   - keeps the rows whose `$rl` is in `surface.relations`;
   - handles stream errors.
   - A non-empty result returns `DbError::BeliefEdgeReferencesRemovedConcept` before `delete_concepts_gc` runs.
4. GC, writes, extension writes, as today.
5. Before `tx.commit()`: `after = belief_tier_fingerprint(&tx, &surface)?`. If `before.changes_to(&after)` is non-empty, return `Err(DbError::BeliefTierWrite { changes })`. `tx` drops uncommitted.

**`type_hint` checks:**
- Read the closure once per sync, only when some written concept carries a `type_hint` other than `"concept"`: `match $t sub concept; let $l = label($t); select $l;`. Handle stream errors.
- In both `insert_concept_entities` and `attach_typed_attributes`, each `type_name` passes `validate_schema_label` and must be in the closure. If it is not, return `DbError::Schema(format!("type_hint {type_name:?} is not a subtype of `concept`"))` before that function's first query.
- The closure is computed once and passed to both (for example as `Option<&HashSet<String>>`), so it is not read twice.

- [ ] **Step 1: Write the failing tests** in `bundle_sync_test.rs`. Each uses a fresh `TestDb` and the existing `create_test_bundle` fixture.
  - `extension_write_into_the_belief_tier_is_refused`:
    - Core and mem schema.
    - `sync_bundle(&mut db, &bundle, &[raw], BELIEF_TIER_ROOT)`, where `raw = GraphWrite::single("given $id: string, $st: string; insert $b isa belief, has concept-id == $id, has belief-state == $st;", vec!["id".into(), "st".into()], vec!["mem/bel/raw".into(), "asserted".into()]).expecting(1)`.
    - The result matches `Err(DbError::BeliefTierWrite { changes })`, and the set of `changes[*].label` is exactly `{"memory-item", "belief"}`.
    - Afterwards `match $c isa concept;` counts 0, so the whole sync rolled back.
  - `sync_without_the_belief_tier_root_is_refused` (Review Focus 2): core schema only. The result is `Err(DbError::Schema(m))` with `m.contains("memory-item")`, and the database holds 0 `concept` and 0 `bundle`.
  - `unknown_type_hint_is_refused_as_a_schema_error` (Review Focus 3), on core and mem schema:
    - **Insert case.** Set `bundle.concepts[0].type_hint = Some("no-such-type".into())`. The result is `Err(DbError::Schema(m))` with `m.contains("no-such-type")`, and 0 concepts were written.
    - **Update case.**
      - Sync the unmodified bundle (Ok).
      - Then re-sync with `concepts[0]` given a new `file_hash`, `type_hint = Some("no-such-type".into())` and one `TypedAttribute { name: "title".into(), value: TypedAttrValue::String("t".into()) }`. This routes the concept through the update path into `attach_typed_attributes`.
      - The result is `Err(DbError::Schema(m))` with `m.contains("no-such-type")`, and the concept's stored `file-hash` is unchanged.
  - `gc_of_a_concept_a_belief_edge_references_is_refused_before_any_delete` (Review Focus 1, mem-only), on core and mem schema:
    - Sync the bundle.
    - Write a raw setup transaction in the test (parameterized): a belief `mem/bel/ref` with `concept-id` and `belief-state`, and a `source-link` with `memory: $b` and `origin: $c`, where `$c` is one of the bundle's concepts. Give it the attributes `source-link` requires (`core/mem-gecko/schema/mem_types.tql:160-166`). `source-link:origin` is played by `concept`, so this edge is on the surface.
    - Re-sync the bundle with that concept removed from `concepts` and from `links`/`hierarchy`.
    - The result matches `Err(DbError::BeliefEdgeReferencesRemovedConcept { references })` with `references == vec![(<that id>, "source-link".into())]`.
    - Afterwards the concept still exists, and the database's concept count is unchanged.
  - The existing tests (`test_bundle_sync`, `failing_extension_write_rolls_back_the_whole_sync`, and PR 1's) only gain the argument. Every assertion stays.
- [ ] **Step 2: Run and confirm the failure.** Run `cargo test -p gecko-bin --test bundle_sync_test --no-default-features --no-run`. Expected: compile errors on the new arity and the missing variants.
- [ ] **Step 3: Implement** the variants, `belief_edge_references`, the guard, the `type_hint` checks and the doc comments:
  - the `sync_bundle` doc, as in Interfaces;
  - the module doc's "Record-path guardrail" gains two sentences: every sync fingerprints the belief tier and refuses a change; and a concept a belief edge references is not garbage-collected.
  - The production section must still not name the four needles in `syncer_stays_on_the_record_path` (`bundle.rs:752-773`).
- [ ] **Step 4: Update the callers.**
  - `sync_manifest` passes `mem_gecko::BELIEF_TIER_ROOT`.
  - Every test caller passes `mem_gecko::BELIEF_TIER_ROOT`.
  - `sandbox_test` applies `MemGecko::new().schema()` after core.
  - Re-run `git grep -n 'sync_bundle(' -- '*.rs'` and confirm every hit has four arguments.
- [ ] **Step 5: Measure the guard's cost.**
  - Run `test_bundle_sync`'s unchanged second sync, which skips every concept, with timing around `sync_bundle` (a temporary `eprintln!` of `Instant::elapsed`), once with the guard and once with both fingerprints stubbed out locally.
  - Report both numbers and remove the instrumentation. The spec states 196 count queries per sync with mem and cyber composed; Task 7 records the measured overhead.
- [ ] **Step 6: Update the docs.**
  - **`extensions/README.md`, "Import-time writes":**
    - The sync is guarded. A sync whose writes (or whose importer's concepts) change any belief-tier type's instance, attribute or link count is refused with `DbError::BeliefTierWrite` and commits nothing.
    - A concept's `type_hint` must name a `concept` subtype.
    - Garbage collection refuses to delete a concept a belief edge references (`DbError::BeliefEdgeReferencesRemovedConcept`), so a re-import that drops a referenced record is refused. Recovery: keep or restore the record, or import into a fresh database. "Retire, not delete" is ledger item O6.
  - **`CLAUDE.md`, "Epistemic memory":** add one sentence: `sync_bundle(…, belief_tier_root)` refuses to GC a concept a belief edge references, fingerprints the introspected belief tier (`gecko_engine::db::surface`) at the start and end of its transaction, and refuses any change; the runtime reconciliation is `gecko-bin/tests/belief_tier_reconciliation_test.rs`.
  - **`README.md`, "Knowledge Graph Syncer":** a sync that would change the belief tier, or delete a record a belief references, is refused and rolled back.
  - **`README.md`, the `sync-stix` help text (`main.rs` `SyncStix` doc) and any PR 1 wording saying a re-import "replaces" the previous one:** add "unless a belief edge references a record the new import drops, in which case the import is refused".
  - **`docs/refactor/KNOWN-LIMITATIONS.md`:** a section "Sync guard" with two bullets:
    - the fingerprint's same-count residual;
    - the pre-GC refusal, its cases (a renamed or deleted playbook with a claim, an ATT&CK re-import dropping a claimed technique) and the recovery, pointing to O6.
- [ ] **Step 7: Run the tests and confirm they pass.**
  - `cargo test -p gecko-engine --lib --no-default-features`, for `syncer_stays_on_the_record_path` and `plan_sync`.
  - `cargo test -p gecko-bin --no-default-features --features cyber --test bundle_sync_test --test no_drag_test --test sandbox_test --test cyber_detect_test --test cyber_stix_sync_test -- --test-threads=1`, plus PR 1's test files that call `sync_bundle` or the binary's `sync`/`sync-stix`.
  - Both clippy commands. Expected: PASS and clean.
- [ ] **Step 8: Commit.** `feat(sync): refuse to GC a concept a belief edge references; refuse a sync that changes the belief tier; type_hint must name a concept subtype`

### Task 5: The reconciliation rig `memwriter_is_the_sole_belief_tier_writer`

**Files:**
- Create: `gecko-bin/tests/belief_tier_reconciliation_test.rs` (`#![cfg(feature = "cyber")]`, `mod common;`)

**Interfaces:**
- Consumes:
  - from Task 3: `belief_tier_surface`, `belief_tier_fingerprint`, `BeliefTierFingerprint::changes_to`, `BeliefTierMeasure`;
  - `mem_gecko::{MemWriter, BeliefEdgeRegistry, BELIEF_TIER_ROOT}` and `MemWriter::edge_writer_for`;
  - `RouterGraphStore`;
  - `gecko_semantic_index::{HnswIndex, StubEmbedder}`;
  - `env!("CARGO_BIN_EXE_gecko")`.
- Produces (file-private; Task 6 reuses them in the same file):
  - `struct Rig { _tmp: tempfile::TempDir, config: PathBuf, cache: PathBuf, okf_dir: PathBuf, stix: PathBuf }` and `fn Rig::new() -> Rig`. It writes:
    - the `gecko.toml`, with `[typedb] endpoint = "localhost:1729"`, `mode = "external"`; `[extensions] enabled = ["cyber"]`; `[semantic_index] enabled = true`, `path = "<tmp>/prod.hnsw"`, `embedder = "stub"`, `backend = "hnsw"`, `record_retrieval_provenance = true`;
    - the OKF bundle (`bundle.json` named `rig-playbooks`, with `playbooks/rig_mem.md` and `playbooks/rig_cyber.md`);
    - the STIX bundle as `stix.json`, which is a verbatim copy of `BUNDLE` from `cyber_stix_sync_test.rs:23`.
  - `const MEM_PLAYBOOK: &str = "playbooks/rig_mem";`, `const CYBER_PLAYBOOK: &str = "playbooks/rig_cyber";`.
  - `async fn gecko(rig: &Rig, db: &TestDb, args: &[&str]) -> String`:
    - runs the binary with `tokio::process::Command`, passing `--config <rig.config> --database <db.name> <args…>` and `GECKO_CACHE_DIR=<rig.cache>`;
    - asserts exit success, printing stdout and stderr on failure;
    - returns stdout.
  - `async fn run_playbook(rig: &Rig, db: &TestDb, concept_id: &str) -> serde_json::Value`: calls `gecko(…, &["run", concept_id])`, asserts stdout contains `Execution successful!`, and parses the JSON after `Output: ` on its line.
  - `async fn setup(rig: &Rig, db: &TestDb)`: `schema-init`, `sync <okf_dir>`, `sync-stix <stix>`, through `gecko`. It never passes `--bundle-name`, so the STIX bundle stays `stix-sync` and Task 6's in-process `to_okf` manifest owns the same concepts.
  - `async fn fingerprint(db: &TestDb) -> BeliefTierFingerprint`: a read transaction, `belief_tier_surface(&tx, BELIEF_TIER_ROOT)`, then `belief_tier_fingerprint`. Both `.expect`.
  - `type Delta = BTreeMap<(String, BeliefTierMeasure), i64>;`, `fn delta(before: &BeliefTierFingerprint, after: &BeliefTierFingerprint) -> Delta` (non-zero entries only, from `changes_to`), and `fn labels(d: &Delta) -> BTreeSet<String>`.
  - `fn unaccounted(expected: &Delta, observed: &Delta) -> BTreeSet<String>`: the labels whose observed delta differs from the expected one, under any measure. The rig's per-step assertion is `unaccounted(&R(n), &P(n)).is_empty()`, with both deltas printed on failure. Task 6 calls the same helper.

**Playbooks** (fenced `javascript`, `engine: quickjs`; the final expression is the output):
- Fixture text is built with `format!` from shared Rust `const`s: `MEM_PLAYBOOK`, `CYBER_PLAYBOOK`, `T_EP`, `T_Q`, `T_B1`, `T_B2` and `T_S`. Each value is emitted as a JS string literal (e.g. `serde_json::to_string(T_EP)`), so both sides make identical calls.
- **`rig_mem`** has scopes `[mem:remember, mem:recall, mem:derive, mem:supersede, mem:contest]`. It runs these calls:
  - `e = mem.remember(T_EP)`;
  - `chunks = mem.recall(T_Q, { maxChunks: 5 })`;
  - `b1 = mem.derive(T_B1, chunks.map(c => c.id), { confidence: 0.6 })`;
  - `b2 = mem.derive(T_B2, [])`;
  - `s = mem.supersede(b1.id, T_S, "revised")`;
  - `a = mem.contest([s.id, b2.id])`.
  - Output: `{ episode: e.id, recalled: chunks.length, b1: b1.id, b2: b2.id, s: s.id, anomaly: a.anomaly }`.
- **`rig_cyber`** has scopes `[cyber-gecko:claim, cyber-gecko:disposition]`. It runs these calls:
  - `c = gecko.callExtension("cyber-gecko", "claim", { logic_id: <CYBER_PLAYBOOK>, technique_id: "T1059", method: "external-tool", confidence: 0.9 })`;
  - `d = gecko.callExtension("cyber-gecko", "disposition", { logic_id: <CYBER_PLAYBOOK>, verdict: "true-positive" })`.
  - Output: `{ claim: c.belief_id, disposition: d.belief_id }`.

**Reference database** (in-process writer calls only, after the same `setup` through the binary):
- `ref_writer: Arc<MemWriter>` is built as follows:
  - store: `RouterGraphStore::new(Arc::new(Mutex::new(ref_db.router())))`;
  - writer: `MemWriter::new(store, Some(Arc::new(StubEmbedder::new(384))), Some(Arc::new(HnswIndex::open(<tmp>/ref.hnsw, model_id, 384)?)))`;
  - then `.with_retrieval_provenance(true).with_belief_edges(BeliefEdgeRegistry::from_extensions(CORE_SCHEMA, &[mem_gecko::extension(), cyber_gecko::extension()])?)`;
  - then `rebuild_index_from_graph().await`.
  - This mirrors `build_epistemic_writer` (`main.rs:773-854`) with `embedder = "stub"`.
- **Step 5 reference** uses one `ctx = RunContext::new(ActorId::new("system"), ExecutableDoc { concept_id: MEM_PLAYBOOK }, now)`. The same `ctx` goes to every call, so recall's scratch reaches the derive. It makes these calls:
  - `observe(&ctx, EpisodeDraft { text: T_EP, event_time: ctx.occurred_at, ingest_time: ctx.occurred_at })`;
  - `reader.recall(&ctx, RecallQuery { text: T_Q, as_of: None }, ContextBudget { max_chunks: 5, max_tokens: None })`, with `reader = ref_writer.clone().as_epistemic_reader().unwrap()`;
  - `assert_belief(&ctx, …)` twice: T_B1 with the recalled ids and confidence 0.6, then T_B2 with no evidence. Each uses `owner: ctx.actor`, `Visibility::Private`, `entrenchment: None` and `DerivationMethod::LlmSynthesis`, as `belief_draft_from_payload` builds them (`mem_host.rs:179-188`);
  - `supersede(&ctx, b1, T_S draft, "revised")`;
  - `contest(&ctx, &[s, b2])`.
- **Step 6 reference** uses a new `RunContext` with `ExecutableDoc { concept_id: CYBER_PLAYBOOK }`. It calls `ref_writer.edge_writer_for("cyber-gecko")?.assert_belief_with_edges(&ctx, …)` twice, with the draft, evidence, method and edge that `claim` and `disposition` build (`extensions/cyber-gecko/src/detect/mod.rs:95-200`). Copy their text formats and attributes exactly.

**Steps the rig asserts** (`P(n)` is the production delta, `R(n)` the reference delta; the empty delta is the expectation for steps 2-4 and 7):
1. After `schema-init`, every value of `fingerprint(prod)` and of `fingerprint(ref)` is 0.
2. `sync <okf_dir>`: `unaccounted(&Delta::new(), &P(2))` is empty.
3. `sync-stix <stix>`: the same check for `P(3)`.
4. Re-`sync` and re-`sync-stix`: the same check for `P(4)`. Afterwards both playbook concepts and the `attack-pattern` with `attack-id "T1059"` exist (parameterized count reads), so the multi-bundle setup holds.
5. `run rig_mem`: the output has `recalled >= 1` (so `derivation`/`informs-synthesis` are exercised; if 0, stop and report), and `unaccounted(&R(5), &P(5))` is empty.
6. `run rig_cyber`: `unaccounted(&R(6), &P(6))` is empty.
7. `status`: the same empty check as steps 2-4, for `P(7)`.

`accounted` is the union of `labels(P(n))` over steps 5 and 6.

**Writer-only drives**, on the reference database only, after step 6, each measured as its own delta:
- `rests_on(&ctx, b2, &[s])`;
- `record_prediction(&ctx, b2, 0.7)`, then `resolve_prediction(&ctx, b2, Outcome::Confirmed)`;
- `define_population("rig-pop", "typeql-3", "match $c isa concept;")` (the form `core/mem-gecko/tests/writer_reader_integration.rs:1471` uses), `materialize_population` with the two playbook ids at `now`, and `close_population_membership` for one of them. These are owned by P1/P2.
- `observe` twice with an identical text, then `dedup_episodes(first, second)`. Owned by C2.
- mem has no disposition writer today (A6 owes it), so there is nothing to drive for it.

**Disposition table:**
- `const DISPOSITIONS: &[(&str, LabelDisposition)]`.
- `enum LabelDisposition` (not `Disposition`, which clashes with `mem_gecko::Disposition`):
  - `WriterOnly { methods: &'static [&'static str], owner: &'static str }`;
  - `Owed { owner: &'static str }`;
  - `Allowlisted { owner: &'static str }`.
- *Accounted* is computed, never listed.
- `const KNOWN_OWNERS: &[&str] = &["A5", "A7", "A8", "D-O"];`. Every owner must be one of these; extend it only together with a ledger item.
- `const D_O_LABELS: &[&str]` is the pinned list of labels no ledger item owns. D-O is the ledger's open decision "owners for schema-only belief-tier types" (Task 7). It holds `rests-on`, `prediction-resolution`, `resolution`, `resolves`, `playbook`, `working-set`, `intention`, `learned-constraint`, `evidence`, `justification`, `support`, `attack`, `goal-hierarchy`, `contextualizes`, `pivot`, `execution` and `tool-invocation`.
- Expected entries, derived from `core/mem-gecko/src/tql.rs` (the only place the A3 lint allows belief-tier writes) and the composed schema:
  - **WriterOnly**, owner `D-O`:
    - `rests-on` (`rests_on`);
    - `prediction-resolution` (`record_prediction`, `resolve_prediction`).
  - **Owed:**
    - `sighting`, `sighting-of` → `A8`;
    - `divergence-exception`, `divergence-precedence` → `A5`;
    - `scoped` → `A7`;
    - every other label in `D_O_LABELS` → `D-O`.
  - **Allowlisted:** none.
- Expected `accounted` is `memory-item`, `episode`, `execution-episode`, `retrieval-event`, `belief`, `anomaly`, `ownership`, `source-link`, `derivation`, `surfaced`, `informs-synthesis`, `supersession`, `contradiction`, `technique-detection` and `alert-disposition`. It is not asserted as a list. If the live run differs, check the label against `tql.rs` before changing the table, and report every change.

**Ratchet assertions:**
- Let `surface = entities ∪ relations` of the production surface.
- Every surface label is in exactly one of `accounted` or the table, and a failure names the label. In particular, a table label that appears in `accounted` fails.
- Every table label is on `surface`, so a stale entry fails.
- Every owner is in `KNOWN_OWNERS`.
- The set of table labels with owner `D-O` equals `D_O_LABELS`.
- Every `WriterOnly` label is changed by the drive of one of its `methods`.
- Every writer-only drive changes only labels that are in `accounted` or are `WriterOnly` entries naming that drive's method.
- The populations and dedup drives change no label outside `accounted`.
- No `Owed` label is changed by any delta.

- [ ] **Step 1: Write the test** `memwriter_is_the_sole_belief_tier_writer` (`#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`) with the helpers, fixtures, reference, steps, drives, table and assertions above. It uses two `TestDb`s, `rig_prod` and `rig_ref`.
- [ ] **Step 2: Run it.** Run `cargo test -p gecko-bin --test belief_tier_reconciliation_test --no-default-features --features cyber -- --test-threads=1 --nocapture`. Expected: PASS.
  - If a step's `unaccounted` set is non-empty, print both deltas. Do not edit an expectation to match an observed number: the reference is the expectation. Find the unaccounted write or the reference call that differs from the production call, and report it.
  - If a table entry is wrong, the fix must cite the `tql.rs` op that writes, or the absence of one.
- [ ] **Step 3: Run the non-vacuity spot check.**
  - Temporarily remove the `supersede` call from the reference's step 5 only, run the test and confirm it fails on step 5, naming `supersession`.
  - Revert, and record the failure output in your report.
- [ ] **Step 4: Run** both clippy commands. Expected: clean. The file is empty under the cyber-less build, through the crate-level `cfg`, and every helper and field is used in the cyber build.
- [ ] **Step 5: Commit.** `test: W5a runtime rig reconciles every production path against MemWriter (memwriter_is_the_sole_belief_tier_writer)`

### Task 6: `hostile_extension_raw_belief_write_is_flagged` and the cyber GC refusal

**Files:**
- Modify: `gecko-bin/tests/belief_tier_reconciliation_test.rs`

**Interfaces:**
- Consumes:
  - from Task 5: `Rig`, `setup`, `run_playbook`, `fingerprint`, `delta`, `labels`, `unaccounted`, `CYBER_PLAYBOOK`;
  - from Task 4: `sync_bundle(…, BELIEF_TIER_ROOT)` and `DbError::{BeliefTierWrite, BeliefEdgeReferencesRemovedConcept, Schema}`;
  - `gecko_engine::db::run_graph_writes`;
  - `cyber_gecko::stix::{to_okf, cyber_post_sync}`;
  - `gecko_engine::okf::parser::parse_bundle`.
- Produces:
  - `Rig` gains `okf_dir_without_cyber: PathBuf`, a copy of the OKF bundle without `rig_cyber.md`. `Rig::new` writes it. Added here, where the GC test reads it.
  - `fn hostile_writes(belief_id: &str) -> Vec<GraphWrite>`, which returns two parameterized writes, each `.expecting(1)`:
    - a raw belief: `insert $b isa belief, has concept-id == $id, has belief-state == $st;` with `id = "mem/bel/hostile"` and `st = "asserted"`;
    - a raw `technique-detection`: match the logic concept by `concept-id == CYBER_PLAYBOOK`, the `attack-pattern` by `attack-id == "T1059"` and the existing belief by `concept-id == belief_id`, then `insert technique-detection (detecting-logic: $lc, detected-technique: $t, detection-claim: $b);`.

- [ ] **Step 1: Write the test** `hostile_extension_raw_belief_write_is_flagged`.
  - **Setup:** a fresh `TestDb` (`rig_hostile`), `setup`, then `out = run_playbook(…, CYBER_PLAYBOOK)`. Take `belief_id = out["claim"]` and `f0 = fingerprint(&db)`. Build `(manifest, typed_rels) = to_okf(<the stix.json text>)`, the same manifest as the setup sync, so garbage collection removes nothing.
  - **Through the guard:**
    - `sync_bundle(&mut db.router(), &manifest, &[cyber_post_sync(&typed_rels), hostile_writes(&belief_id)].concat(), BELIEF_TIER_ROOT)` matches `Err(DbError::BeliefTierWrite { changes })`.
    - The set of `changes[*].label` is exactly `{"memory-item", "belief", "technique-detection"}`.
    - `fingerprint(&db) == f0`.
  - **Attribute-only write** (Review Focus 4):
    - The extension write is `match $b isa belief, has concept-id $bc; $bc == $bid; not { $b has confidence $x; }; insert $b has confidence == $c;` with `bid = out["disposition"]` (its belief has no confidence) and `c = GraphValue::Double(0.1)` (`GraphValue` has no `From<f64>`), `.expecting(1)`.
    - It gives `BeliefTierWrite` whose changes are exactly `memory-item` and `belief` with measure `Attributes`, and the fingerprint is unchanged.
  - **Through an importer:**
    - Use `manifest` plus one extra concept, cloned from `manifest.concepts[0]` with `concept_id = "hostile/working-set"`, `type_hint = Some("working-set".into())`, no typed attributes and a fresh `file_hash`.
    - The sync gives `Err(DbError::Schema(m))` with `m.contains("working-set")`, and the fingerprint is unchanged.
  - **Around the guard** (non-vacuity, through the rig's own reconciliation):
    - Run `hostile_writes(&belief_id)` with `run_graph_writes` in a raw `begin_write` transaction and commit.
    - `unaccounted(&Delta::new(), &delta(&f0, &fingerprint(&db).await))` equals the guard's label set from the first case. The rig, given no expected writes, reports exactly the labels the guard refused.
- [ ] **Step 2: Write the test** `gc_of_a_concept_a_belief_edge_references_is_refused` (Review Focus 1, cyber case).
  - Use a fresh `TestDb`, `setup`, and `run_playbook(CYBER_PLAYBOOK)`, then take `f0`.
  - `sync_bundle(&mut db.router(), &parse_bundle(&rig.okf_dir_without_cyber).expect("parse"), &[], BELIEF_TIER_ROOT)` matches `Err(DbError::BeliefEdgeReferencesRemovedConcept { references })`.
  - `references` contains `(CYBER_PLAYBOOK, "technique-detection")` and `(CYBER_PLAYBOOK, "alert-disposition")`.
  - `fingerprint == f0`, and the `CYBER_PLAYBOOK` concept still exists.
  - The same refusal through the binary: `gecko sync <okf_dir_without_cyber>` exits non-zero, and its stderr names `playbooks/rig_cyber`. The `gecko` helper asserts success, so add a sibling `gecko_expect_failure(...) -> String` used only here, returning stderr.
- [ ] **Step 3: Run them.** Run `cargo test -p gecko-bin --test belief_tier_reconciliation_test --no-default-features --features cyber -- --test-threads=1`. Expected: all three tests PASS.
  - If the raw relation insert fails its row count instead of being refused, the match did not bind. Fix the fixture, never the assertion.
- [ ] **Step 4: Run** both clippy commands. Expected: clean.
- [ ] **Step 5: Commit.** `test: raw belief-tier writes during a sync are refused by the guard and reported by the rig`

### Task 7: Ledger, specs, SPEC-W5 implementation note, full verification

**Files:**
- Modify: `docs/refactor/OWED-WORK-LEDGER.md`. Locate everything by text: PR 1 adds a track, a "Where things stand" bullet, sequence lines, an S5 row and Track O, so `main`'s line numbers are stale.
- Modify: `docs/refactor/SPEC-W5.md` §5 (after the row-1 implementation note)
- Modify: `docs/superpowers/specs/2026-11-20-multi-bundle-databases-design.md` (goal 4)

- [ ] **Step 1: Update the ledger.**
  - **Status and sequence:**
    - A4 and A10 become ✅ "(this PR)".
    - "Where things stand" gains a "Landed in this PR" bullet: A10 (types moved, cyber depends on the API crate alone, CI-guarded, no extension is offered the writer) and A4 (the pre-GC check and the sync guard in production, the rig, the hostile test).
    - In the sequence lines, drop "partial" from A10 and mark A4 and A10 done on both `S1 ✅ ─► A4 + A10` and PR 1's `O1 ✅ ─► A4 + A10`.
    - Show C8 as unblocked. "Recommended next" no longer names A4 + A10.
  - **Track O** gains **O6**, "retire, not delete":
    - When a concept that a belief edge references leaves its bundle, the sync keeps the entity and its key, clears its content (including `code-block` and `scope`, so a removed playbook cannot run), marks it retired, and reports it in `SyncResult`.
    - This mirrors STIX's revoked objects (`extensions/cyber-gecko/src/stix/mod.rs:155-175`).
    - It replaces the pre-GC refusal for that case. Owning section: the belief-tier guard spec's pre-GC amendment.
    - If PR 1 already used O6, take the next free O-number and use it everywhere this plan says O6, including the spec, the README, KNOWN-LIMITATIONS and the error message.
  - **Open decisions** gains **D-O**, "owners for schema-only belief-tier types". It lists `D_O_LABELS` from Task 5 and blocks nothing; it is referenced by A4's ratchet.
  - **S5 (from PR 1):** reword its "a second domain replaces the first" and any "re-import replaces" text, adding "unless a belief edge references a record the new import drops, which is refused (pre-GC check; O6)".
- [ ] **Step 2: Finalize PR 1's spec goal 4.** In `docs/superpowers/specs/2026-11-20-multi-bundle-databases-design.md`, change "Re-importing a newer ATT&CK release replaces the previous one" to "replaces the previous one, unless the pre-GC check refuses because a belief edge references a concept the new release drops (see the belief-tier guard spec)".
- [ ] **Step 3: Add the SPEC-W5 §5 implementation note** for rows 2, 3 and 8, as "Implementation note (2026-11-20, not ratified text)" in the style of the existing notes. It covers:
  - **Row 2** ships in `gecko-bin/tests/belief_tier_reconciliation_test.rs` (`cyber`, live TypeDB).
    - The production database is driven only through the `gecko` binary. The expectation is a reference database driven by in-process `MemWriter`/`edge_writer_for("cyber-gecko")` calls. This keeps §2.2 row 1 literally: the harness's own MemWriter calls are the accounting.
    - The measure is the production fingerprint: per label, instance, attribute-ownership and role-player counts.
    - The §1.2 disposition table adds a *writer-only* class. Labels with no ledger owner are owned by D-O.
  - **Row 3 deviation:** since D-D step 1, a bridge holds only `Arc<dyn GraphReader>`, so a stub composed through `build_host_bridge` cannot write. The remaining raw route is sync-time output (`extension_writes`, an importer's `type_hint`), which is what the test drives, through the guard and around it. This applies D-D and needs no separate ratification.
  - **Row 8** is discharged: the reconciliation is green with cyber on the seam.
  - **Detection became prevention:** `sync_bundle` refuses a belief-tier change in production (`DbError::BeliefTierWrite`). Record its residual (same-count swap) and the measured cost from Task 4 Step 5.
  - **Amendment for the user to ratify:** the pre-GC check. A sync refuses to delete a concept a belief edge references (`DbError::BeliefEdgeReferencesRemovedConcept`). The recovery today is to restore the record or use a fresh database; the follow-on is O6.
  - Tracked as ledger items A4 and A10.
- [ ] **Step 4: Run the full verification.**
  - `cargo fmt --all -- --check`.
  - Both clippy commands.
  - `scripts/check-extension-deps.sh`.
  - `cargo tree --workspace --exclude js-sandbox --no-default-features | grep -qi candle` must find nothing (the existing CI guard).
  - `cargo nextest run --workspace --exclude js-sandbox --no-default-features --features cyber --locked --test-threads=1`.
  - Expected: all green, with the probe's ignored set unchanged.
- [ ] **Step 5: Commit.** `docs: A4 and A10 done; O6 and D-O; SPEC-W5 §5 implementation note for rows 2, 3 and 8`
