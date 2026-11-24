# Populations cut — design

Closes owed-work ledger item **A7** by deleting what it would have fed.
**Supersedes** parts of ratified SPEC-W2-W3, SPEC-W5 and INVARIANTS (listed below).
Those documents are not edited.

## Intent

A7 asked for a `scoped` writer, so that `blast-radius` could reach the beliefs scoped to
a retracted population member. An independent review found that nothing at any layer
would use it.

- **Readers:** `blast_radius` has no production caller. Its intended consumers,
  `precompute_blast_radius` and `invalidate_blast_radius`, return `NotYetImplemented`
  (`core/mem-gecko/src/consolidation.rs:170-182`).
- **Producers:** nothing defines or materializes a population in production. Only
  tests call `define_population`, `materialize_population` and
  `close_population_membership`. No spec evaluator exists (ledger P1).
- **Retraction:** nothing retracts a belief or a concept. In a real database, only the
  derivation branch of `blast-radius` gets input, from `mem.derive`.
- **The cohort branch is incoherent.**
  - It follows open memberships only (`mem_functions.tql:82-84`), so it finds a cohort's
    beliefs only *before* the member leaves, which is the event it is meant to analyse.
  - One member of N sweeps every belief scoped to the whole cohort.
- **Executing a stored spec** (`spec-text`) that a guest supplied would break "all TQL is
  parameterized".

GECKO is alpha. The user ruled that a clean design outweighs keeping existing databases
upgradable. So populations, `scoped` and the cohort machinery go entirely, and
`blast-radius` returns to a plain belief-dependency closure. Git keeps all of it if a
real consumer appears; the likely one is cyber asking "which beliefs about this asset
group go stale".

## Approaches

- **Build A7 as a Rust-only `MemWriter::scope_belief`.** Rejected: it would feed one
  branch of an uncalled function, joined against a relation nothing materializes.
- **Build A7 with `mem.*` sandbox calls** to define and materialize populations and
  scope beliefs. Rejected: it needs a spec evaluator over guest text, which is an
  injection surface, for no caller.
- **Fallback: simplify `blast-radius` only**, and park the population schema and writer.
  Rejected: it keeps about 1,000 lines of dead, speculative substrate.
- **Chosen: the full cut** (the user's pick).

## Goals

1. **`blast-radius($r: memory-item) -> { memory-item }`** is the recursive closure over
   `rests-on` and `derivation` only, through one step function,
   `blast-step($r: memory-item)`. Its `@doc`s say exactly that.
   - The narrowing from `okf-concept` to `memory-item` is deliberate. Only the cohort
     branch needed the wider type, and `EpistemicReader::blast_radius` already takes a
     `MemId`.
   - No caller passes a plain concept: the only entry for one was the inherent
     `blast_radius_at`, which is deleted.
2. **Deleted from the mem schema:**
   - the entity `population`;
   - the relations `scoped` and `population-member`;
   - the attributes `spec-dialect`, `spec-text`, `spec-hash` and `computed-at`. Only
     `population-member` owns `computed-at`. `valid-from` and `valid-to` stay, since
     `belief` owns them.
   - the plays `belief plays scoped:scoped-belief` and `concept plays population-member:member`.
3. **Deleted from the mem functions:** `blast-radius-at`, `blast-step-at`,
   `population-members` and `population-members-at`.
   - `canonical-entity`, `canonical-entity-at` and the rest stay.
   - The false comment "canonical entity IS a population" (`mem_types.tql:275`) is
     corrected. The comments over `concept`'s plays drop their population wording.
4. **Deleted Rust:**
   - `core/mem-gecko/src/populations.rs` and `core/mem-gecko/src/writer/populations.rs`;
   - `MemWriter.population_locks`, the `MaterializeReport` re-export, `MemWriter::blast_radius_at`,
     `MemWriter::population_members` (`writer/reader.rs:88`), `tql::blast_radius_at_read`,
     `tql::population_members_read`, and every population TQL builder in `tql.rs`.
     Removing these public methods is intended, since nothing outside tests calls them.
   - the schema-text asserts on widened or `-at` signatures in `core/mem-gecko/src/lib.rs`.
   - `EpistemicReader::blast_radius` keeps its `MemId` signature. Its doc names only the
     two edges.
5. **Deleted migrations** in `core/mem-gecko/src/tql.rs`: `mem/population-member-window-doc`,
   `mem/scoped-required-roles` and `mem/population-member-required-roles`.
6. **Tests and probes:**
   - Every test, probe and probe file that exists for populations, `scoped`, the cohort
     branch or the `-at` blast functions is deleted.
   - A test that only partly uses them is trimmed to its remaining subject.
   - Expectations that count or list the belief-edge surface lose `scoped`.
   - The known sites:
     - `gecko-bin/tests/schema_probe_w2w3.rs` and the `gecko-bin/tests/probes/w3_*` files;
     - `core/mem-gecko/tests/writer_reader_integration.rs`;
     - `core/mem-gecko/tests/schema_acceptance_test.rs`;
     - `core/mem-gecko/tests/closure_functions_test.rs`;
     - `core/mem-gecko/tests/schema_migration_test.rs`;
     - `gecko-bin/tests/schema_init_test.rs`, `belief_tier_reconciliation_test.rs`,
       `belief_tier_surface_test.rs` and `bundle_sync_test.rs`;
     - `core/mem-gecko/src/belief_edge/scan.rs`.
7. **A guard** keeps the cut from drifting back. A test asserts that the shipped mem
   schema and functions declare and reference none of the deleted labels. It matches
   them as whole TypeQL labels, not substrings, so that `exception-scope` or prose like
   "S3-scoped" can't trip it. A label character is `[A-Za-z0-9_-]`, and a boundary is
   checked only on a side where the needle ends in one:
   - the declarations `entity population`, `relation scoped` and
     `relation population-member`;
   - the instance and role references `isa population`, `isa scoped`, `scoped:`,
     `population-member:` and `scoped-belief`;
   - the attribute labels `spec-dialect`, `spec-text`, `spec-hash` and `computed-at`;
   - the function names `blast-radius-at`, `blast-step-at`, `population-members` and
     `population-members-at`.
8. **Docs:**
   - Ledger: A7, B4 and P1–P4 are closed as cut, pointing to this design. A5 and A6
     note that their population-versus-instance case lost its subject, and their
     instance-versus-instance and member-concept `exception-scope` halves stand.
   - `KNOWN-LIMITATIONS.md` gains the existing-database note (below).
   - Any README or extension doc naming populations is updated.

## Non-goals

- **The divergence types** (`divergence-exception`, `divergence-precedence` and its
  `exception-scope` role) stay. Their scope is a member concept, not a population.
- **`resolves` and `canonical-entity*`** stay.
- **`believed-at` and the other as-of functions** stay, other than the blast and
  population ones.
- **A retraction writer.** Nothing retracts yet. `blast-radius` stays the read a future
  retraction would use.
- **The consolidation stubs** `precompute_blast_radius` and `invalidate_blast_radius`
  stay as stubs.

## Anti-goals

- Leaving any reference to a deleted type in shipped schema, functions, Rust or docs.
  The guard test (goal 7) and a final repo-wide search enforce this.
- Changing any other function's results. `blast-radius` over `rests-on` and `derivation`
  returns what it returns today when no population data exists.

## Constraints

- **Existing databases are not upgraded.**
  - Schema apply is additive, so a database initialized before this change keeps the old
    population types and functions as inert leftovers.
  - The signature narrows only for a database that stores the `okf-concept` form. The
    frozen `mem_schema_95ebac1.tql` already takes `memory-item`, so
    `previous_release_upgrades_to_the_shipped_schema` keeps passing with its assertions
    updated.
  - A database initialized by `main` just before the cut stores
    `blast-step($r: okf-concept)` under a `blast-radius` that calls it, and no frozen
    fixture has that shape. The implementation probes it on live TypeDB from a new frozen
    fixture of that schema, and a test pins what it observes.
  - If that apply is refused, the supported path is to recreate the database: drop it,
    then `schema-init` and re-sync. The test then pins that the refusal names the
    function.
  - `KNOWN-LIMITATIONS.md` states the observed behaviour, not a possibility.
  - The same rule covers every test that applies a frozen older schema
    (`gecko-bin/tests/fixtures/schemas_pre_tightening/` included): keep it if the
    upgrade still applies, delete it otherwise.
  - No migration work is in scope: no undefine/redefine shims to make old databases
    upgrade.
- **The belief-tier surface is introspected.** It loses `scoped` and nothing else
  (`population-member` has no memory-item role and was never on it). Exact counts in
  tests are re-derived from the shipped schema, not hand-edited to pass.
- **Frozen fixtures keep their text.**
  - `closure_functions_ref.tql` holds four oracle bodies, two of them blast ones that
    reference deleted types. Its two `canonical-entity` oracle functions move verbatim
    into a new fixture, `closure_canonical_ref.tql`, and the old file is deleted. The
    closure test's blast half then checks against a Rust reachability oracle instead of
    the reference body.
  - `mem_schema_95ebac1.tql`, `cyber_schema_1cd621d.tql` and
    `gecko-bin/tests/fixtures/schemas_pre_tightening/` are untouched.
  - No new frozen schema is added. A database initialized by today's `main` is checked
    by a one-off probe whose outcome `KNOWN-LIMITATIONS.md` states. A small synthetic
    test pins TypeDB's callee-narrowing behaviour, and adds the function name to the
    error if TypeDB's own text lacks it.
- **Ratified docs are not edited.** This design supersedes, for populations and the
  cohort branch:
  - SPEC-W2-W3:
    - §3.4 b1, the widened `blast-radius` and its cohort branch;
    - §3.1, the as-of variants of `blast-radius` and `population-members`;
    - §3.2, the membership window;
    - §3.5, the populations-writer clauses;
    - §2.2 "member-scoped exception", only insofar as it presumes a population-scoped
      belief.
  - SPEC-W5:
    - the §1.1/§1.2 rows for `scoped` and `population-member`;
    - the "mem-owned belief edges" list, which loses `scoped`.
  - CONTRACT-W8's draft references to the `scoped`, `population` and `population-member`
    walk (`CONTRACT-W8.md:305-323`) are superseded the same way.
  - INVARIANTS (`RATIFIED`; it takes no in-place edit, so it carries no pointer to this
    design, and the ledger carries one instead):
    - invariant 9's verification note "canonical entity IS a population" (`:104`) and
      the population half of its verdict (`:105`, `:210`). `canonical-entity` is a
      closure over provable `resolves` beliefs;
    - invariant 6's caveat and decision on `population-members` (`:83`, `:209`);
    - §8.2, insofar as it frames the divergence exception as instance versus population
      (`:244-258`). The instance-versus-instance reading and the precedence relation
      stand;
    - §8.4 work item W3's as-of population variant and cohort `blast-radius` (`:272`).
- **Project rules:**
  - `@doc` on every remaining type and function.
  - Greenfield comments.
  - No `#[allow(dead_code)]`.
  - fmt and both clippy runs clean.
  - Integration tests run with `-j 1`, and never touch the `gecko` database.

## Verification

- The full candle-free suite passes.
- A repo-wide search for `population`, `scoped`, `spec-hash`, `computed-at`,
  `blast-radius-at` and `blast_radius_at` outside `tests/fixtures/`, the ratified docs,
  `docs/superpowers/` and the ledger's closed rows returns only intended hits. The cyber
  "population of hosts" limitation is one.
- `blast-radius` tests cover:
  - a `rests-on` chain;
  - a `derivation` chain;
  - a mixed chain;
  - a cycle that terminates.

  They exist or are added in `closure_functions_test.rs` or
  `writer_reader_integration.rs`.
- `closure_functions_test.rs` keeps checking `blast-radius` on random graphs, against a
  Rust reachability oracle over the generated `rests-on` and `derivation` edges.
- `garbage_collecting_a_concept_deletes_every_non_surface_relation_it_plays` keeps a
  non-surface relation in a build without `cyber`, through a relation the test defines.

## Left to the builder

- **Which partly-population tests are trimmed and which deleted,** within goal 6. The
  divergence and `canonical-entity` probe tests keep their non-population assertions.
- **The exact guard-test location:** `core/mem-gecko/src/lib.rs` tests or
  `schema_acceptance_test.rs`.
