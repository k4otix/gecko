# Second cut pass — design

Closes owed-work ledger item **X1**, under decisions **U1**, **U2** and **U3** (ledger, "Decided
2026-11-24"). It **supersedes** parts of ratified INVARIANTS, SPEC-W2-W3, SPEC-W5 and CONTRACT-W8,
and the locked CONFLICT-REPORT, all listed below. Those documents are not edited.

## Intent

GECKO's schema `@doc`s exist for agents that introspect the graph. Today the shipped schema
advertises a divergence mechanism, a consolidation daemon, intentions, learned constraints,
playbooks, working sets and argumentation relations that nothing writes. The code behind them is
stubs, a host-side ranker with no caller, and a dedup tick that no CLI or scheduler runs.

- **Producers:** no production path writes any label in groups (a)–(c) below. The only writer of
  `consolidation-step` and `consolidation-state` is `dedup_episodes_ops` (`tql.rs:713-715`), and
  only tests call it.
- **Readers:**
  - `mem_gecko::disposition` and `DerivationMethod::rank()` are called only by each other and their
    tests;
  - `ConsolidationDaemon` is implemented by a no-op and by a dedup tick, and called only by tests;
  - the `consolidation-state "tombstoned"` guards in `tql.rs` are vacuous on every database a test
    did not dedup.
- **Invariants:** no invariant (1–9, OP-1…OP-6) names a cut label. The labels INVARIANTS 5 and 9
  name stay: `pivot`, `prediction-resolution`, `rests-on`, `resolution` and `resolves`.

GECKO is alpha, and the user ruled that a clean design outweighs keeping existing databases
upgradable (U3: inert leftovers, as with populations). So the mechanism goes entirely: schema,
Rust, migrations, tests and probes. Git keeps all of it if a real consumer appears.

## Approaches

- **Cut writers and code only, keep the schema.** Rejected by U3: the schema `@doc`s would still
  advertise what GECKO never holds, which misleads the introspecting reader they exist for.
- **Cut, with type-removal migrations for existing databases** (the form cyber used for
  `evidence-of`, `requires-evidence` and `sensor-coverage`). Rejected: U3 chose inert leftovers, and
  the populations cut ruled out migration shims. The one migration added is the `@doc` fix in goal 6,
  because without it every existing database fails `schema-init`.
- **Chosen: the full cut,** with leftovers on existing databases.

## Goals

1. **(a) Divergence (U1).**
   - **Schema:** the whole block at `mem_types.tql:259-276`. That is `divergence-exception`,
     `divergence-precedence` and its roles `exception-belief`, `prevailing`, `overridden` and
     `exception-scope`, and the plays of `divergence-exception`, `belief` and `concept`.
   - **Migration:** `mem/divergence-precedence-required-roles`.
   - **Rust:** `core/mem-gecko/src/disposition.rs` and its re-exports (`lib.rs:15`, `:24`), and
     `DerivationMethod::rank()` with its test (`epistemic.rs:92-110`, `:325-351`).
     `DerivationMethod::provable()` stays: invariant 7 names it.
   - **Probes:**
     - in `gecko-bin/tests/schema_probe_w2w3.rs`, the W2 tests (P-PREC-1…5, P-DISP-1…3) and their
       helpers are deleted;
     - P-COMP is trimmed to the define-clean composition with cyber, with no divergence inserts;
     - P-W5-1 keeps its core and cyber relation checks under a name without "divergence";
     - the W3 `canonical-entity-at` test and P-REDEF stay;
     - the file is renamed `schema_probe_w3.rs`, with its module doc rewritten;
     - `probes/w2_disposition.tql` and the orphan `probes/w2_precedence.tql` are deleted.
2. **(b) Consolidation (U2).**
   - **`core/mem-gecko/src/consolidation.rs`, deleted whole:** the trait with its tick and eleven
     job methods, `NoopConsolidationDaemon`, `MemConsolidationDaemon`, `ConsolidationReport` and
     `DedupSummary`, with their re-exports.
   - **`core/mem-gecko/src/writer/consolidation.rs`, deleted whole:** `dedup_episodes`,
     `duplicate_episode_candidates`, `retrieval_events_safe_to_tombstone` and `DuplicateCandidate`.
     The last method is not named in the ledger, but its only intended caller is a stub, and its
     read carries a `tombstoned` guard.
   - **TQL builders:** `duplicate_episode_candidates_read`, `dedup_episodes_ops` and
     `retrieval_events_safe_to_tombstone_read`.
   - **Schema:** `consolidation-step` and its three plays, and the attributes `consolidation-state`
     (with `memory-item owns consolidation-state`) and `consolidation-action`. `created-at` stays,
     because `supersession` owns it.
   - **Migration:** `mem/consolidation-step-required-roles`.
   - **The guard** `not { $e has consolidation-state "tombstoned"; }` in `enumerate_episodes_read`
     (`tql.rs:560`) is deleted **in the same commit** as the attribute. Once the attribute leaves
     the schema it names an unknown type, and index reconciliation on a fresh database would fail.
3. **(c) Schema diet (U3).**
   - **Deleted, 12 mem labels:** `intention`, `goal-hierarchy`, `working-set`, mem's `playbook`,
     `execution`, `tool-invocation`, `learned-constraint`, `contextualizes`, `evidence`,
     `justification`, `support` and `attack`.
   - **Deleted, 2 cyber labels:** `sighting` and `sighting-of`.
   - **Also deleted:** every `plays` line naming them, on `memory-item`, `episode`, `doc-run`,
     `agent`, `concept`, `intention`, `learned-constraint` and `observable`.
   - **Attributes owned only by cut types are deleted:** `execution-status`, `deadline`,
     `stability-tier`, `corroboration-count`, `justification-status` and `tool-name`.
   - **Kept:**
     - `condition`, because `anomaly` owns it;
     - `execution-episode`, the supertype of `retrieval-event` (its `@doc` still reads true);
     - `ioc-value`, because `observable` owns it.
   - **Migrations:** the cleanups `mem/evidence-required-roles`, `mem/justification-required-roles`,
     `mem/support-required-roles`, `mem/attack-required-roles` and `mem/execution-required-roles`
     (`tql.rs:28-47`, `:73-77`) are deleted. `check_migrations` requires every role a cleanup names
     to be required on a shipped type, so they cannot stay. Mem's migration count drops from 20 to
     13; the test re-derives it.
4. **(d) What the cut leaves behind.**
   - `EpistemicError::NotYetImplemented` (`gecko-extension-api/src/error.rs:15-18`) is deleted: the
     stubs were its only producers, and nothing matches on it.
   - `IndexOutcome` is deleted. `best_effort_remove` returns `()`, since its one production caller
     discards the result. `supersede_with_failing_remove_reports_failed` (`writer/tests.rs:331-341`)
     goes with it: `supersede_survives_a_failing_remove` already pins the behaviour that matters.
   - The `mem-gecko` crate doc (`lib.rs:3-7`) is rewritten. It claims mem links episodes "via
     `contextualizes`".
   - Plan-tag comments in `mem_types.tql` and `mem_functions.tql` go:
     - the `A2.x` and `A3` tags;
     - the out-of-order `# N —` ordinals;
     - "sketch said `long`";
     - "four-layer".

     `invariant N` and `design §X.Y` references stay. Comments are not stored, so this changes no
     database.
   - **Stale comments are fixed:**
     - `core/gecko-engine/src/pipeline.rs:143-146` ("contextualizing relation");
     - `extensions/cyber-gecko/src/detect/mod.rs:195` ("Alert/sighting");
     - `core/mem-gecko/src/writer/index.rs:44` ("non-tombstoned");
     - `core/mem-gecko/src/writer/mod.rs:20,24,34`;
     - the `NotYetImplemented` docs at `gecko-extension-api/src/error.rs:6` and `epistemic.rs:7`.
   - `execution-episode`'s `@doc` ("a step of an execution run") stays. It still reads true, and
     changing it would need a second doc migration.
5. **The belief-tier rig is re-derived, not trimmed** (`belief_tier_reconciliation_test.rs`).
   - `DISPOSITIONS` loses the 17 rows for labels that leave the surface.
   - `pivot`, `resolution` and `resolves` point at ledger rows that exist. `pivot`'s owner `M5` was
     cut, so `pivot` folds into **M1**: the same invariant-5 primitive, with the same reopen
     trigger. M1 becomes "`rests-on` and `pivot` reachable from a playbook". `resolution` and
     `resolves` move to **M3-r**.
   - `KNOWN_OWNERS` becomes `["M1", "M2", "M3-r"]`.
   - The dedup and daemon drives, their deltas and the imports go.
6. **`memory-item`'s `@doc` tells the truth** (the user's pick over keeping the stale doc).
   - Today it says "four-layer" and lists "evidence, support/attack". It becomes one accurate
     sentence about what memory items are and the edges they carry: derivation, supersession,
     assumption (`rests-on`) and provenance.
   - TypeDB refuses a changed `@doc` on an existing type, so a migration `mem/memory-item-doc` is
     added, after the precedent `mem/belief-required-window-doc`.
   - **The checker change.** `check_migrations`' "reinstalled" check
     (`gecko-extension-api/src/migration.rs:61-75`) needs a shipped statement that, minus its kind
     keyword, starts with `<target> @doc(`. The shipped line is
     `entity memory-item @abstract, sub okf-concept @doc(`, so no target matches today.
     - The check learns to skip leading annotations (`@abstract,`) between the label and the `sub`
       clause.
     - Unit tests cover the abstract form and keep refusing a target with no shipped `@doc`.
     - The shipped line is not reordered: `core/mem-gecko/src/lib.rs:276,285` assert its text.
   - **Probe first.** On live TypeDB 3.15, find the `undefine @doc from …;` form that removes
     `memory-item`'s `@doc` from a database holding the shipped declaration. The first candidate is
     `undefine @doc from memory-item sub okf-concept;`, mirroring belief's precedent. Confirm that the
     shipped define then installs the new `@doc`. A test pins the observed form.
   - **Fallback, if TypeDB refuses every form:** keep `memory-item`'s `@doc` byte-identical. There is
     then no migration and no checker change, and `KNOWN-LIMITATIONS.md` says the doc still names
     the cut layers. Recreating databases is not the fallback, because the upgrade tests must keep
     passing.
7. **A guard** keeps the cut from drifting back.
   - A test asserts that the shipped mem and cyber schemas and functions declare and reference none
     of the deleted labels and attributes, matched as whole TypeQL labels: a label character is
     `[A-Za-z0-9_-]`, with the boundary checked on any side where the needle ends in one.
   - The populations guard is a mem unit test over `MEM_SCHEMA` (`core/mem-gecko/src/lib.rs:105-170`),
     and mem cannot see cyber. Its matcher `deleted_labels_absent` moves to `gecko-extension-api`
     beside `check_migrations` (`#[doc(hidden)]`). The mem test then gains the X1 needles, and a cyber
     unit test checks `sighting` and `sighting-of`.
   - The needles are:
     - every type deleted in goals 1–3, as a declaration (`entity X`, `relation X`, `attribute X`);
     - every deleted role as a scoped reference (`execution:run`, `consolidation-step:run`, …), not
       a bare `R:`;
     - each attribute name.

     `sighting` and `evidence` are matched only as declarations and role references, because
     `belief_edge` tests use `sighting` as a synthetic label and prose uses "evidence".
8. **Docs.**
   - Ledger:
     - X1 is closed, pointing here;
     - M1's row names `pivot`;
     - "Superseded and stale passages" points here.
   - `KNOWN-LIMITATIONS.md`:
     - the optional-roles list (`:82-86`) loses the cut roles;
     - the consolidation daemon section (`:166-186`) and its actor paragraph go;
     - the existing-database leftover note (`:101-117`) extends to the X1 types, attributes and
       plays, and states the goal-6 outcome;
     - one line says a database that holds test-made `tombstoned` episodes re-indexes them once the
       guard goes.

## Decided with the user (2026-11-24)

- The `memory-item` `@doc` is fixed with a migration and a checker change, falling back to leaving
  it unchanged (goal 6).
- `pivot` folds into M1, with no new row (goal 5).
- CONTRACT-W8 is superseded whole, and SPEC-W2-W3 §2 whole. This is wider than U2's §2, §2a and §3
  and U1's §2.1–§2.4, because the rest of each describes deleted code.
- `EpistemicError::NotYetImplemented` and `MemWriter::retrieval_events_safe_to_tombstone` are
  deleted, though the ledger does not name them (goals 2 and 4).

## Non-goals

- **Kept:** `pivot`, `prediction-resolution`, `rests-on`, `resolution`/`resolves` and
  `canonical-entity*` (INVARIANTS 5 and 9).
- `contest()` and the `contested` belief state stay. They are the live tie output.
- No retraction or pruning writer. Nothing deletes beliefs, so the never-delete posture of
  INVARIANTS D-4 holds; only its citation of `consolidation-state` is superseded.
- No removal migrations for existing databases (goal 6's `@doc` fix aside).
- The HNSW "tombstone" wording in `gecko-semantic-index` is internal and unrelated; it stays.

## Anti-goals

- Leaving any reference to a deleted type in shipped schema, functions, Rust or editable docs. The
  guard test (goal 7) and a final repo-wide search enforce this.
- Changing any stored function body. None references a cut label, so no `redefine` is planned.
- Hand-editing exact counts to pass. Surface counts, fingerprint sizes, migration counts and
  `CUT_LEFTOVERS` are re-derived from live runs. The inventory estimates the surface with cyber at
  37 → 20 labels and 98 → 53 fingerprint keys.

## Constraints

- **Existing databases.**
  - Define is additive, so a database initialized before this change keeps the cut types, plays and
    attributes as inert leftovers. One-player `evidence`, `justification`, `support`, `attack` and
    `execution` rows in a pre-tightening database are no longer cleaned and stay as leftover data.
    No apply fails on them.
  - `tightened_mem_migrations` (`gecko-bin/tests/schema_init_test.rs:484-489`) filters
    `MEM_MIGRATIONS` by id; a new `@doc` migration id is added to that filter.
  - `previous_release_upgrades_to_the_shipped_schema` (95ebac1) and
    `tightened_schema_applies_to_a_populated_database` (pre-tightening) must keep passing, with
    goal 6's migration and re-derived expectations: `added`, `CUT_LEFTOVERS`, and the removed
    dedup and daemon block.
  - `old_cyber_detection_types_migrate_away` is unaffected: `sighting` stays as a leftover.
- **Frozen fixtures keep their text:**
  - `mem_schema_95ebac1.tql`;
  - `cyber_schema_1cd621d.tql`;
  - `gecko-bin/tests/fixtures/schemas_pre_tightening/`.
- **Ratified and locked docs are not edited.** This design supersedes:
  - **INVARIANTS:**
    - §8.2 whole (`:242-260`);
    - the invariant-9 verdict clause on divergence-exceptions (`:105`), and §7(a) and §7(b) on
      divergence (`:210`, `:220`);
    - §8.4 W2 (`:271`) and W8 (`:278`);
    - D-4's citation of `consolidation-state` (`:189`);
    - §4's divergence and consolidation mechanisms (`:156`).

    §5's stale `contextualizes` anchor (`:175`) is resolved by the deletion.
  - **SPEC-W2-W3:**
    - §2 whole (`:168-417`);
    - the W2 rows of §0, §1.1, §1.2, §3.6, §4, §5 and §6.
  - **SPEC-W5:**
    - the §1.1 delta paragraph (`:140-153`) and the ratchet example on `divergence-precedence`;
    - §1.2's `sighting-of` row;
    - §2.3's P-W5-1 ratchet;
    - §3.2's co-mint discipline;
    - §3.3's third cyber edge;
    - §3.4's daemon, sighting writer and W2 additions;
    - §3.5's mem-owned edges and daemon writes;
    - the cut labels in §4's surface list;
    - §5 rows 4 and 6.
  - **CONTRACT-W8, whole.** With no daemon it has no subject. CF-1…CF-3 remain useful guidance if a
    daemon is ever proposed, but they bind nothing.
  - **CONFLICT-REPORT** (locked): T4 (`:100-113`) and T8 (`:159-173`).
- **Project rules:**
  - `@doc` on every remaining type and function;
  - greenfield comments;
  - no `#[allow(dead_code)]`;
  - fmt and both clippy runs clean;
  - integration tests with `-j 1`, never touching the `gecko` database.

## Test sites that change

Beyond the files goals 1–5 name:

- `core/mem-gecko/src/lib.rs:289-290` (the `playbook` and `working-set` asserts) and `:329-344` (the
  `consolidation-state` test);
- `core/mem-gecko/tests/schema_acceptance_test.rs`: `:119-185` (trimmed to `rests-on` and
  supersession), `:281-325`, `:464-481`, the required-role cases at `:521-610`, the macro rows at
  `:681-705`, and `:1014-1023`;
- `gecko-bin/tests/no_drag_test.rs:128-131` and `:190-201`;
- `core/mem-gecko/tests/consolidation_daemon_test.rs`, deleted whole;
- `core/mem-gecko/tests/writer_reader_integration.rs:26` and the dedup, tombstone and `IndexOutcome`
  tests at about `:2232-2996`. The helpers `recalling_writer` and `recall_once` stay, because a kept
  test uses them;
- `gecko-bin/tests/schema_init_test.rs:995`, `:1195-1213` and the `("mem", 20)` count at `:1296`.

## Verification

- The full candle-free suite passes.
- The goal-6 probe outcome is pinned by a test.
- The goal-7 guard passes.
- A repo-wide search outside these paths returns only intended hits (the synthetic `belief_edge`
  labels, HNSW tombstones, the frozen fixtures):
  - `tests/fixtures/`;
  - the ratified and locked docs;
  - `docs/superpowers/`;
  - the historical docs, which are not edited: `LEDGER-HISTORY.md`, `REVIEW-2026-11.md`,
    `g1-g2-validated.md` and `probes/W2-W3-PROBE-LOG.md`.

  The search terms are every deleted label, `consolidation-state`, `tombstoned`, `disposition::`,
  `ConsolidationDaemon`, `NotYetImplemented`, `IndexOutcome`, `dedup_episodes` and `rank()`.
- The rig passes, with every surface label accounted for or dispositioned.
- `belief_tier_surface_test.rs` passes unchanged.
- Index reconciliation on a fresh database still enumerates episodes (an existing integration test
  covers this).

## Left to the builder

- Commit boundaries. Goal 2's attribute and guard deletions share one commit.
- The exact rewritten `@doc` and crate-doc sentences.
- Whether the trimmed P-COMP and P-W5-1 keep their test names.
