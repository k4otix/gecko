# TypeDB review fixes: query correctness and schema tightening — short design

2026-11-22. This comes from a review of gecko's schemas and queries against the TypeDB
modeling and TypeQL guides (typedb/typedb-skills). The user approved two passes, chose
"only a currently asserted belief can be superseded", and approved `@card(1)` on
`divergence-precedence`'s prevailing and overridden roles, with an erratum to SPEC-W2-W3.

## Intent

The review found:

- **An access-control hole.** As-of recall ignores visibility.
- **Writes that report success having done nothing, or having forked history.**
- **Reads that over-count.**
- **A schema that leaves Rust to enforce what TypeDB could.** Roles may be left empty,
  ownership counts are unchecked, fields that recall needs are optional, and ranges are
  unchecked.

The first pass fixes behaviour. The second makes the database refuse malformed data itself.

**Goals.** Every finding below is fixed and pinned by a test that failed first. Existing
databases upgrade through `schema-init` / `sync` with no manual step.

**Non-goals.**

- Renaming relations or functions.
- Rewriting the deprecated `(roles) isa rel` syntax.
- Batched multi-row reads (needs a `GraphReader` API change).
- The duplicated function bodies in `blast-radius`, `canonical-entity` and
  `claim-platforms`.

These are ledgered as follow-ons.

**Anti-goals.**

- Never loosen the belief-edge registry.
- Never edit a ratified spec's text; stale text goes to errata D-G.
- Never fail an upgrade on data that today's writers produce.

## Approaches

- **Two passes (chosen).** Query and Rust fixes first, then schema constraints after a
  probe. The schema pass changes what existing databases accept, so it is probed and tested
  on its own.
- *One combined pass.* Rejected. A constraint TypeDB refuses would block the urgent
  visibility fix.

## Pass 1 — query correctness (branch `fix/query-correctness`)

1. **As-of recall is visibility-gated.**
   - Extract `fun visible-to($n: belief, $agent: agent) -> boolean` from `gate`'s
     ownership disjunction. `gate` calls it, so its behaviour is unchanged.
   - `recall_as_of` takes the `RunContext`. Its read matches the actor's `agent` and
     requires `true == visible-to($b, $ag)`.
   - Rows are sorted newest `valid-from` first, then by concept id, before `max_chunks`
     truncates them, so the result is deterministic.
   - Supersession is not applied as of T: `believed-at`'s window semantics stand.
2. **Retrieval tombstoning.** `retrieval_events_safe_to_tombstone_read` returns an event only
   when every belief it informed is superseded, and only once per event (`select $rid;
   distinct;`).
3. **`resolve_prediction`.**
   - The link op requires exactly one row and that no outcome is recorded yet.
   - A mismatch is `Storage`, naming the belief and saying nothing was committed. This
     follows `record_prediction`'s `name_missing_ids` mapping.
4. **`detect.precision`** counts only dispositions whose belief is `asserted`. A superseded
   verdict no longer counts alongside its correction.
5. **No duplicate rows within one write.** A `not {}` guard does not see inserts from the
   same query, so rows are deduplicated host-side before the write:
   - `create_links` on (source, target);
   - `create_citations` on (source, url);
   - cyber's generic SRO writes on (source, target, shape).
6. **Supersede requires the old belief to be `asserted`.** The existing `expecting(1)` then
   refuses a second supersede or a supersede of a retracted belief. The error names the
   belief and says it is not a current belief, and nothing is committed.
7. **`informs_synthesis_op` and `set_was_used_op` require exactly one row.** A miss maps to
   `Storage`, naming both ids, and nothing is committed.
8. **Ownership and provenance counts in the schema.**
   - `memory-item plays ownership:owned @card(1), plays source-link:memory @card(1..)`.
   - This is additive and checked against existing data at commit. `gate` and `visible-to`
     can then rely on one ownership.

## Pass 2 — schema tightening (branch `feat/schema-tightening`, after pass 1 merges)

1. **Probe first.** On TypeDB 3.15, confirm each form before any other change:
   - an additive `@card(1)` on a bare `relates` passes on conforming data and fails on
     violating data;
   - `belief owns salience @card(1)` specializing an inherited `owns` is accepted;
   - the mem migration that turns `belief owns valid-from @card(0..1)` into `@card(1)`
     works;
   - `@range(0.0..1.0)` at `owns` level on a `double` attribute is accepted;
   - each mem migration passes the registry.
2. **Required roles get `@card(1)`** (or `@card(1..)` where several players are meant).
   - **mem:** `derivation`, `evidence`, `justification`, `support`, `attack`,
     `contradiction`, `supersession`, `source-link`, `ownership`, `rests-on`, `execution`,
     `consolidation-step`, `scoped`, `population-member`, `resolves`, `surfaced`,
     `informs-synthesis`, and `divergence-precedence` (prevailing and overridden).
   - **core:** `containment`, `hierarchy`, `okf-link`'s source and target, and `citation`'s
     `as` roles.
   - **cyber:** `technique-detection`, `alert-disposition`, `strategy-detects`,
     `strategy-analytic`, `analytic-requirement`, and the STIX SRO relations.
   - **Exceptions:**
     - `prediction-resolution:resolving-episode` stays `0..1`, because it is linked
       later.
     - Optional roles that exist today, such as `exception-scope`, stay as they are.
   - **STIX sweep first.** A garbage-collected cyber concept leaves one-player SRO
     relations, so the cyber post-sync orphan sweep extends to every relation shape before
     those roles get `@card(1)`.
   - **SPEC-W2-W3 §2.1.** It gains a D-G erratum entry for the `divergence-precedence`
     change.
3. **Required attributes.**
   - `belief owns valid-from @card(1)`, through a mem migration.
   - `belief owns salience @card(1)`, `base-activation @card(1)` and `last-access
     @card(1)`.
4. **Ranges at `owns` level.**
   - `@range(0.0..1.0)` on belief and derivation `confidence`, and on `predicted-probability`.
   - `@range(-1.0..1.0)` on `similarity-score`.
   - `@range(0..)` on `access-count`, `corroboration-count` and `candidate-count`.
5. **Upgrade check.** A database populated by every current writer path (the reconciliation
   rig's drives) re-applies the tightened schema and commits.

## Left to the builder

- Error wording beyond the pinned parts.
- Which pass-2 relations need `@card(1..)` rather than `@card(1)`, decided from their
  writers.
- Migration ids.
- Test fixture shapes.
