# G1 + G2 — Validated TypeQL & committed id-scheme decision

**Task 1 (pre-coding gates).** Every SKETCH TQL block from the refactor plan
(A2 schema, A2.9 population/resolution, A2.10 retrieval, A3 functions) was
executed against a **live TypeDB 3.12.0 server** (throwaway db, dropped after).
Validation = *server-accepted and, for functions, correct results on a fixture* —
not eyeballed. `typeql-check` (Homebrew) rejected even the known-good core schema
(incompatible parser vintage), so the running server was the sole source of truth.

Harness: a throwaway Rust example (`cargo run -p gecko-engine --example g1_validate`,
now deleted) using `typedb-driver` 3.12 — applied core + mem schema + functions,
inserted a fixture, and asserted the result set of **all 11 functions** plus the
`@values` rejection and stored-spec-text execution. **16/16 checks passed.**

The validated schema lives in `core/mem-gecko/schema/mem_types.tql` and the
validated functions in `core/mem-gecko/schema/mem_functions.tql` (both committed as
A2/A3 drafts). The full text is reproduced below for review.

---

## G1 — Validated schema (A2.1–A2.10)

Applies additively on `core/gecko-engine/schema/core_schema.tql` (apply core
first, then this). Server-accepted verbatim.

```tql
define

  # ══════════════════════════════════════════════════════════════════
  # A2 — mem-gecko substrate ontology (additive on core_schema.tql)
  # ══════════════════════════════════════════════════════════════════

  # ── A2.2 State enums ──────────────────────────────────────────────
  attribute belief-state value string @values("asserted", "retracted", "superseded", "contested");
  attribute consolidation-state value string @values("raw", "candidate", "consolidated", "archived", "tombstoned");
  attribute derivation-method value string @values("type-join", "cardinality", "functional-dependency", "llm-synthesis");
  attribute entrenchment value string @values("axiom", "user-stated", "tool-derived", "inferred", "llm");
  attribute stability-tier value string @values("volatile", "provisional", "stable", "canonical");
  attribute visibility value string @values("private", "team", "shared");
  attribute justification-status value string @values("in", "out");

  # ── A2.4 Bitemporal + decay attrs ─────────────────────────────────
  attribute event-time value datetime;
  attribute ingest-time value datetime;
  attribute valid-from value datetime;
  attribute valid-to value datetime;
  attribute salience value double;
  attribute base-activation value double;
  attribute access-count value integer;
  attribute last-access value datetime;
  attribute half-life-hours value double;

  # ── misc scalar attrs ─────────────────────────────────────────────
  attribute confidence value double;
  attribute corroboration-count value integer;
  attribute supersession-reason value string;
  attribute created-at value datetime;
  attribute computed-at value datetime;
  attribute condition value string;
  attribute execution-status value string @values("pending", "active", "suspended", "completed", "failed");
  attribute deadline value datetime;
  attribute agent-id value string;
  attribute origin-kind value string;          # OPEN, adapter-extensible set (invariant 9) — NOT @values
  attribute run-id value string;
  attribute tool-name value string;
  attribute pivot-method value string;
  attribute calibration-score value double;
  attribute predicted-probability value double;
  attribute outcome value string;

  # ── A2.9 population attrs ──────────────────────────────────────────
  attribute spec-dialect value string;         # (dialect, text) modelled as 2 attrs; structs unimpl in 3.x
  attribute spec-text value string;            # a stored TypeDB match clause — the DEFINING predicate (SoR)
  attribute spec-hash value string;

  # ── A2.10 retrieval-provenance attrs ──────────────────────────────
  attribute retrieval-method value string @values("semantic", "typed", "hybrid");
  attribute retrieval-provenance value string @values("semantic", "typed", "hybrid", "none");
  attribute similarity-score value double;
  attribute was-used value boolean;
  attribute candidate-count value integer;     # sketch said `long`; 3.x has no long -> integer (64-bit)

  # ══════════════════════════════════════════════════════════════════
  # A2.1 Memory-item hierarchy (four-layer substrate)
  #   id scheme: concept-id @key inherited from okf-concept (mem/ep|bel/{ulid})
  # ══════════════════════════════════════════════════════════════════
  entity memory-item @abstract, sub okf-concept,
      owns salience @card(0..1),
      owns base-activation @card(0..1),
      owns access-count @card(0..1),
      owns last-access @card(0..1),
      owns consolidation-state @card(0..1),
      plays derivation:derived,
      plays derivation:source,
      plays evidence:evidenced,
      plays evidence:evidence-item,
      plays justification:consequent,
      plays justification:antecedent,
      plays support:supporter,
      plays support:supported,
      plays attack:attacker,
      plays attack:attacked,
      plays supersession:superseded,
      plays supersession:superseding,
      plays source-link:memory,
      plays ownership:owned,
      plays rests-on:resting,
      plays rests-on:assumption,
      plays surfaced:item;               # anything surfaced by a retrieval (typically a belief)

  entity episode sub memory-item,
      owns event-time @card(1),
      owns ingest-time @card(1),
      owns half-life-hours @card(0..1),
      plays prediction-resolution:resolving-episode,
      plays execution:episode-step,
      plays tool-invocation:invocation-episode;

  entity belief sub memory-item,
      owns belief-state @card(1),
      owns confidence @card(0..1),
      owns entrenchment @card(0..1),
      owns valid-from @card(0..1),
      owns valid-to @card(0..1),
      owns retrieval-provenance @card(0..1),   # write-once summary flag (A2.10)
      plays contradiction:claim,
      plays scoped:scoped-belief,
      plays prediction-resolution:predicted-belief,
      plays informs-synthesis:synthesized;

  entity playbook sub memory-item,
      owns stability-tier @card(0..1),
      owns corroboration-count @card(0..1);

  entity working-set sub memory-item;

  # episode domain subtype used by retrieval provenance
  entity execution-episode sub episode,
      plays informs-synthesis:retrieval;

  # ══════════════════════════════════════════════════════════════════
  # A2.3 Provenance relations (reified)
  # ══════════════════════════════════════════════════════════════════
  relation derivation,
      relates derived,
      relates source @card(1..),
      owns derivation-method @card(1),
      owns confidence @card(0..1);

  relation evidence,                       # n-ary
      relates evidenced,
      relates evidence-item @card(1..);

  relation justification,                  # JTMS IN/OUT
      relates consequent,
      relates antecedent @card(0..),
      owns justification-status @card(0..1);

  relation support,
      relates supporter,
      relates supported;

  relation attack,
      relates attacker,
      relates attacked;

  # A2.3 contradiction -> anomaly hub (claims 2..)
  entity anomaly sub memory-item,
      owns condition @card(0..1),
      plays contradiction:hub;

  relation contradiction,
      relates hub,
      relates claim @card(2..);

  # deprecation / supersession
  relation supersession,
      relates superseded,
      relates superseding,
      owns supersession-reason @card(0..1),
      owns created-at @card(1);

  # source-link: memory -> origin (1.. in concept|document|code-block|doc-run)
  relation source-link,
      relates memory,
      relates origin @card(1..),
      owns origin-kind @card(0..1);

  # ══════════════════════════════════════════════════════════════════
  # A2.5 Prospective: intention + goal-hierarchy
  # ══════════════════════════════════════════════════════════════════
  entity intention sub memory-item,
      owns execution-status @card(1),
      owns deadline @card(0..1),
      plays goal-hierarchy:parent-goal,
      plays goal-hierarchy:child-goal;

  relation goal-hierarchy,                 # self-referential on intention
      relates parent-goal,
      relates child-goal;

  # ══════════════════════════════════════════════════════════════════
  # A2.6 Anti-context: learned-constraint + contextualizes
  # ══════════════════════════════════════════════════════════════════
  entity learned-constraint sub memory-item,
      owns condition @card(0..1),
      plays contextualizes:constraint;

  relation contextualizes,                 # constraint -> any concept
      relates constraint,
      relates context-target;

  # ══════════════════════════════════════════════════════════════════
  # A2.7 Multi-agent: agent + ownership
  # ══════════════════════════════════════════════════════════════════
  entity agent,
      owns agent-id @key,
      plays ownership:owner;

  relation ownership,
      relates owned,
      relates owner @card(1..),
      owns visibility @card(0..1);

  # ══════════════════════════════════════════════════════════════════
  # A2.8 Cross-cutting primitives (substrate, invariant 5)
  # ══════════════════════════════════════════════════════════════════
  relation rests-on,
      relates resting,
      relates assumption @card(1..);

  relation pivot,                          # reified n-ary investigative move + calibration
      relates pivoting-agent,
      relates from-state,
      relates to-state,
      owns pivot-method @card(0..1),
      owns calibration-score @card(0..1);

  relation prediction-resolution,
      relates predicted-belief,
      relates resolving-episode,
      owns predicted-probability @card(0..1),
      owns outcome @card(0..1);

  # doc-run provenance
  entity doc-run,
      owns run-id @key,
      owns event-time @card(0..1),
      plays execution:run,
      plays source-link:origin;

  relation execution,
      relates run,
      relates episode-step;

  relation tool-invocation,
      relates invocation-episode,
      owns tool-name @card(0..1);

  # ══════════════════════════════════════════════════════════════════
  # A2.9 Populations & cross-source resolution
  # ══════════════════════════════════════════════════════════════════
  entity population,
      owns spec-dialect @card(0..1),
      owns spec-text @card(0..1),
      owns spec-hash @key,                 # dedup cohorts: two beliefs same cohort -> same node
      plays scoped:population,
      plays population-member:owning-population;

  relation scoped,                         # belief -> population
      relates scoped-belief,
      relates population;

  # MATERIALIZED membership: derived cache of the spec's evaluation
  relation population-member,
      relates owning-population,
      relates member,
      owns computed-at @card(0..1);        # staleness marker

  # cross-source identity (invariant 9): canonical entity IS a population;
  # resolution is a reified identity BELIEF (provenance, method, state, supersession)
  entity resolution sub belief,
      plays resolves:resolution-belief;

  relation resolves,                       # two source-records -> same entity
      relates resolution-belief,
      relates record-a,
      relates record-b;

  # ══════════════════════════════════════════════════════════════════
  # A2.10 Retrieval provenance (episodic tier, firewalled from derivation)
  # ══════════════════════════════════════════════════════════════════
  relation surfaced,                       # reified: what was surfaced + the SEARCH signal
      relates surfacer,
      relates item,
      owns similarity-score @card(0..1),   # cosine; meaningful only for semantic method
      owns was-used @card(0..1);           # consumed by synthesis vs merely returned

  relation informs-synthesis,              # ties a synthesized belief to the retrieval
      relates retrieval,
      relates synthesized;

  # retrieval-event: episodic record that a retrieval happened
  entity retrieval-event sub execution-episode,
      owns retrieval-method @card(0..1),
      owns candidate-count @card(0..1),
      plays surfaced:surfacer;

  # ══════════════════════════════════════════════════════════════════
  # Additive role wiring onto core authored/reference tier (records)
  #   concept & external-resource are the source-record / origin players
  # ══════════════════════════════════════════════════════════════════
  # concept is the authored/reference record tier: it is the population member and the
  # cross-source resolution record. (population-member:member / resolves:record-* are scoped
  # to `concept` — not external-resource — so typed functions can return a single `{ concept }`
  # type; external-resource is not a `okf-concept` subtype and would break return-type inference.)
  entity concept,
      plays source-link:origin,
      plays contextualizes:context-target,
      plays population-member:member,
      plays resolves:record-a,
      plays resolves:record-b;

  # external-resource participates only as a source-link origin (citation target), never as a
  # typed-function return player.
  entity external-resource,
      plays source-link:origin;
```

---

## G1 — Validated functions (A3 + A2.9/A2.10)

Applies on top of the schema above (same schema transaction or a later one).
Server-accepted verbatim; each function's result set was asserted on a fixture.

```tql
define

  # ══════════════════════════════════════════════════════════════════
  # A3 — substrate functions (persisted; query-time reasoning)
  # ══════════════════════════════════════════════════════════════════

  # 1 — is-superseded($b) -> bool
  fun is-superseded($b: memory-item) -> boolean:
      match
          {
              (superseded: $b, superseding: $newer) isa supersession;
          } or {
              $b has belief-state "superseded";
          } or {
              $b has belief-state "retracted";
          };
      return check;

  # 2 — believed-at($t) -> {belief}  (temporal; index NOT involved)
  fun believed-at($t: datetime) -> { belief }:
      match
          $b isa belief, has belief-state "asserted", has valid-from $vf;
          $vf <= $t;
          not { $b has valid-to $vt; $vt <= $t; };
      return { $b };

  # 3 — derivation-chain($b) -> {memory-item}  (recursive transitive closure)
  fun derivation-chain($b: memory-item) -> { memory-item }:
      match
          {
              (derived: $b, source: $m) isa derivation;
          } or {
              (derived: $b, source: $mid) isa derivation;
              let $m in derivation-chain($mid);
          };
      return { $m };

  # 4 — retrieval-score($m,$now) -> double
  #   THREE validated 3.12 constraints shape this body:
  #   (a) CONTINUOUS half-life decay is NOT expressible in pure TQL — datetime-datetime yields a
  #       duration, but duration/duration, duration->scalar, scalar->duration and ln/log are all
  #       REJECTED. So the smooth 0.5^(age_hours/half_life) multiplier is applied HOST-SIDE (Rust Reader).
  #   (b) A DURATION value cannot be used as a filtering comparison predicate: `$age <op> <dur-literal>`
  #       parses but matches ZERO rows (verified for >,>=,< against a computed OR literal duration).
  #       Temporal filtering must be done on DATETIME values (datetime comparisons filter correctly).
  #   (c) A variable assigned ONLY inside `or`-branches is NOT bound at `return`. Keep $score top-level.
  #   So: consume $now via a datetime comparison (last-access must not be in the future) and return
  #   the static activation component; recency decay is layered on host-side.
  fun retrieval-score($m: memory-item, $now: datetime) -> double:
      match
          $m has base-activation $ba, has salience $s, has last-access $la;
          $la <= $now;                                 # datetime comparison (filters correctly); consumes $now
          let $score = $ba + $s;                       # top-level binding, visible to `return`
      return first $score;

  # 6 — contradicts($b1,$b2) -> bool
  fun contradicts($b1: belief, $b2: belief) -> boolean:
      match
          $c isa contradiction, links (claim: $b1, claim: $b2);
          not { $b1 is $b2; };
      return check;

  # 7 — blast-radius($retracted) -> {memory-item}  (recursive over rests-on U derivation)
  fun blast-radius($r: memory-item) -> { memory-item }:
      match
          {
              (assumption: $r, resting: $m) isa rests-on;
          } or {
              (source: $r, derived: $m) isa derivation;
          } or {
              {
                  (assumption: $r, resting: $mid) isa rests-on;
              } or {
                  (source: $r, derived: $mid) isa derivation;
              };
              let $m in blast-radius($mid);
          };
      return { $m };

  # 5 — gate($n,$agent,$now) -> bool  (composes is-superseded + validity + visibility)
  fun gate($n: belief, $agent: agent, $now: datetime) -> boolean:
      match
          $n isa belief, has belief-state "asserted", has valid-from $vf;
          $vf <= $now;
          not { true == is-superseded($n); };
          not { $n has valid-to $vt; $vt <= $now; };
          (owned: $n, owner: $agent) isa ownership;
      return check;

  # 8 — select-for-context($agent,$now) -> {belief, double}  (tuple stream)
  fun select-for-context($agent: agent, $now: datetime) -> { belief, double }:
      match
          $b isa belief;
          true == gate($b, $agent, $now);
          let $score in retrieval-score($b, $now);
      return { $b, $score };

  # ── Retrieval-provenance function (A2.10) ─────────────────────────
  # walks informs-synthesis; MUST NOT be reachable from derivation-chain
  fun retrieval-provenance-of($b: belief) -> { retrieval-event }:
      match
          $r isa retrieval-event;                      # narrow: `retrieval` role is played by execution-episode
          (retrieval: $r, synthesized: $b) isa informs-synthesis;
      return { $r };

  # ── Population & resolution functions (A2.9) ──────────────────────
  # reads the materialized population-member view.
  #   NOTE: sketch signature was ($pop,$now) but 3.12 rejects unused params ([FRP1]); the
  #   materialized read does not consume time. The STRICT variant (re-evaluate the stored
  #   spec-text for a named entity-set) is executed host-side (the spec-text is a runnable
  #   match clause — validated below), so it does not need a persisted $now-taking function.
  fun population-members($pop: population) -> { concept }:
      match
          (owning-population: $pop, member: $m) isa population-member;
      return { $m };

  # transitive closure over PROVABLE resolution beliefs; MUST NOT collapse probabilistic
  #   PROVABLE-ness lives on the resolution belief's `derivation` relation (derivation-method),
  #   NOT on the belief itself — join through derivation to read the method.
  fun canonical-entity($rec: concept) -> { concept }:
      match
          {
              $res isa resolution, has belief-state "asserted";
              (derived: $res, source: $src) isa derivation, has derivation-method $dm;
              { $dm == "type-join"; } or { $dm == "cardinality"; } or { $dm == "functional-dependency"; };
              { (resolution-belief: $res, record-a: $rec, record-b: $other) isa resolves; }
              or
              { (resolution-belief: $res, record-a: $other, record-b: $rec) isa resolves; };
          } or {
              $res2 isa resolution, has belief-state "asserted";
              (derived: $res2, source: $src2) isa derivation, has derivation-method $dm2;
              { $dm2 == "type-join"; } or { $dm2 == "cardinality"; } or { $dm2 == "functional-dependency"; };
              { (resolution-belief: $res2, record-a: $rec, record-b: $mid) isa resolves; }
              or
              { (resolution-belief: $res2, record-a: $mid, record-b: $rec) isa resolves; };
              let $other in canonical-entity($mid);
          };
      return { $other };
```

---

## G1 — Deviations from the sketch (block -> what changed -> why)

| Block | Sketch | Change | Why (server behavior) |
|---|---|---|---|
| A2.1 `memory-item` | `sub okf-concept @abstract` | `@abstract, sub okf-concept` | `[ANN9]` `@abstract` is not accepted after `sub`; it must precede `sub` with a comma. |
| A2.0 identity | id scheme TBD | `memory-item sub okf-concept` inherits `concept-id @key`; values `mem/ep/{ulid}`, `mem/bel/{ulid}` | Flat, no actor prefix — see G2 decision. |
| A2.9 `population-spec` | one `(dialect, text)` value | two attrs `spec-dialect` + `spec-text` | Structs are unimplemented in TypeDB 3.x; a 2-tuple attribute value type is not available, so the pair is modelled as two owned attributes. |
| A2.9 `scoped` | `relates belief-side:belief?` | `relates scoped-belief` (played by `belief`) | `relates role:type?` is not valid TypeQL; a role is declared bare and the player is constrained by `plays`. |
| A2.9 `population-member` | `relates of` | `relates owning-population` | `of` is a reserved keyword; renamed. |
| A2.9 `resolves` | `relates link, a, b` + `resolution ... plays resolves:link` | `relates resolution-belief, record-a, record-b` | Clarity; `link` avoided (near-reserved `links`); records scoped to `concept`. |
| A2.9 membership/resolution record players | `entity` | scoped to `concept` only (not `external-resource`) | `external-resource` is **not** a subtype of `okf-concept`; a role played by both yields a `{concept, external-resource}` union with no common covering supertype, which breaks typed-function return inference (`[FIN4]`). `external-resource` stays a `source-link:origin` player only. |
| A2.10 `retrieval-event` | `sub execution-episode` | added `entity execution-episode sub episode` first | The sketch referenced `execution-episode` without defining it in the new hierarchy. |
| A2.10 `surfaced:item` | player = episode | player = `memory-item` | The surfaced thing is normally a belief; scoping `item` to `episode` rejected the fixture insert (`[QUA1]` type-inference error) because a belief cannot play it. |
| A2.10 `candidate-count` | `long` | `integer` | TypeDB 3.x has no `long`; `integer` is 64-bit signed. |
| A2.10 `origin-kind` | (enum-ish) | plain `string`, no `@values` | Invariant 9: origin-kind is an open, adapter-extensible set. |
| A3 fn return types | `-> bool` | `-> boolean` | 3.x spells it `boolean`. |
| A3 `retrieval-provenance-of` | `(retrieval:$r ...)` | added `$r isa retrieval-event;` | The `retrieval` role is played by `execution-episode` (broader); without narrowing, inferred return type is wider than the declared `retrieval-event` (`[FIN0]` type error). |
| A3 `population-members` | `($pop, $now)` | `($pop)` | `[FRP1]` 3.12 rejects **unused function parameters**; the materialized-view read does not consume time. The strict re-evaluation variant (run the stored `spec-text`) is executed host-side. |
| A3 `canonical-entity` | `$res has derivation-method` | join `(derived:$res ...) isa derivation, has derivation-method $dm` | `derivation-method` is owned by the `derivation` **relation**, not by the belief; reading it off the belief is a type error. Provable = method in {type-join, cardinality, functional-dependency}. |
| A3 `retrieval-score` | `0.5^(age_hours/half_life)` continuous decay | static `base-activation + salience`, `$now` consumed via `$la <= $now` (datetime compare); smooth decay applied host-side | Three verified 3.12 limits, below. |
| A3 all `-> {entity}` returns | root `entity` | concrete/covering types (`{concept}`, `{okf-concept}`, `{memory-item}`, `{belief}`, `{retrieval-event}`) | Return types must be a single covering type of the inferred players. |

### Verified TypeDB 3.12 idiom findings (the non-obvious ones)

1. **Duration arithmetic is largely unusable for scoring.** `datetime - datetime`
   yields a `duration` and `datetime + duration` yields a `datetime` (both OK).
   But `duration / duration`, `duration * scalar`, any `duration -> scalar`, and
   `ln`/`log` are **rejected** (`[QUA3]`). Worse, **a `duration` value cannot be
   used as a filtering comparison**: `$age > P1D`, `$age >= P0D`, `$age < P30D`
   all *parse* but match **zero rows** (verified for computed *and* literal
   durations). => Do all temporal filtering on `datetime` values
   (`$la <= $now` filters correctly); compute continuous decay host-side.
2. **A variable assigned only inside `or`-branches is not bound at `return`.**
   Such a function type-checks and commits but returns zero rows at call time.
   Bind returned/aggregated variables at the top conjunction level.
3. **Unused function parameters are a hard error** (`[FRP1]`).
4. **`return check`** is the boolean check-function idiom; a boolean function is
   consumed in a pattern as `let $r in f(...); $r == true;` (or `not { true == f(...); }`).
   Single-scalar functions use `return first $x`; streams use `return { $x }`;
   tuples `return { $x, $y }` with `-> { A, B }` consumed via `let $x, $y in f()`.
5. **Recursion / transitive closure works** and **terminates on cyclic data via
   fixpoint dedup** — `canonical-entity` over a symmetric `resolves` edge returned
   the finite equivalence class without infinite recursion.
6. **A stored `spec-text` (a real `match` clause) executes verbatim.** The fixture
   stored `match $c isa concept, has tag "internet-facing";`; fetched back and run,
   it returned exactly the tagged concepts — and returned *more* than the
   materialized `population-member` view held, demonstrating the A2.9 lazy/epoch
   bounded-staleness divergence live (strict re-eval > stale materialized cache).
7. `@values` rejects illegal enum values at write (verified: `belief-state "bogus"` refused).
8. Abstract subtype declaration is `entity X @abstract, sub Y, ...` (annotation before `sub`).

### G1 acceptance checks (all passed on the live fixture)

is-superseded (true/false) - believed-at (as-of two instants) - derivation-chain
(recursive) - blast-radius (recursive over rests-on U derivation) - gate
(excludes superseded) - select-for-context (gated + scored tuple stream) -
retrieval-provenance-of - population-members - canonical-entity (both seams of a
provable resolution collapse to one cluster while staying queryable) - stored
spec-text executes - `@values` rejection. **16/16.**

---

## G2 — Committed id-scheme decision

**Decision: SHIP FLAT `mem/ep/{ulid}` and `mem/bel/{ulid}` — no actor prefix.**
Heterogeneous key shapes across id populations (variable-length source-ids vs
fixed ULIDs vs content hashes) are a **non-issue**. This is the *expected* branch
of the plan's decision rule, and the source evidence confirms it.

### Evidence (TypeDB 3.x source, `/Users/eddie/Documents/repos/typedb`)

1. **Entity/relation physical key = internal iid, not the `@key` value.** An
   `ObjectVertex` key is a fixed 11 bytes: `[1 prefix][2 type-id][8 object-id]`
   (`encoding/graph/thing/vertex_object.rs:24-32,90-97`). The 8-byte object-id is
   drawn from an **in-memory per-type monotonic `AtomicU64` counter**
   (`encoding/graph/thing/vertex_generator.rs:47-50,163-171`:
   `entity_ids[type_id].fetch_add(1, Relaxed)`). No attribute value is embedded.
2. **`@key` is a secondary attribute index, enforced by validation + lock — it
   does not anchor storage.** The key attribute is a separate attribute vertex
   joined to its owner by a `has` edge; uniqueness is enforced by an
   operation-time has-reverse index scan
   (`concept/thing/thing_manager/validation/operation_time_validation.rs:411-479`)
   plus a commit-time exclusive lock keyed on `(unique-infix, attr-type-vertex,
   deterministic value bytes, owner-type-vertex)`
   (`concept/thing/thing_manager.rs:1847-1874`). The owner's storage key never
   contains the `@key` value.
3. **Attribute-vertex key = `[prefix][type-id][value-or-hash]`.** Fixed-size values
   inline; strings > 16 bytes become `[8 value-prefix][8 seahash][1 disambiguator]`
   (`encoding/graph/thing/vertex_attribute.rs:152-159,582-624`). So a `concept-id`
   fetch is a **value-index seek** on the attribute keyspace, not a scan — measured
   at ~0.37 ms/point-lookup below.
4. **iids surface to the driver client.** `Entity/Relation/Attribute` each carry an
   `iid` with an `iid()` accessor; `Concept::try_get_iid()` returns it
   (`typedb-driver/rust/src/concept/instance.rs:30-74`, `concept/mod.rs:98-104`).
   IID-match `$x iid 0x...` exists as a TypeQL constraint (`analyze/conjunction.rs:97-99`),
   not a dedicated RPC.
5. **Storage-layer append pattern is inherent, independent of our id scheme.** All
   object vertices share one column family (`DefaultOptimisedPrefix11`, keyspace
   `0x0`, `encoding/encoding.rs:40,75`) ordered by `(prefix, type-id, big-endian
   object-id)`. Because the object-id counter is monotonic, new rows always append
   at the right edge of each type's range — **regardless** of what ULID we store in
   `concept-id`. Our id-value choice therefore cannot create or avoid that pattern;
   it only affects the `concept-id` *attribute* index's value-prefix ordering.

Consequence for identity (invariant 9): source-ids, ULIDs and hashes each live as
attribute-vertex values under their own attribute type, joined by `has`; they do
**not** share a physical entity keyspace, so mixing key shapes costs nothing.

### Residual measurement (live, throwaway db)

Synthetic high-rate episodic insert, 50,000 monotonic-ULID `episode` inserts
(1,000/txn), plus an entropy-first (random-prefix) comparison and a fetch-by-@key
probe:

| Run | Throughput | first-10-batch avg | last-10-batch avg | drift |
|---|---|---|---|---|
| monotonic-prefix (timestamp-first ULID analog) | ~18,500 eps/s | ~54 ms | ~50 ms | **x0.93 (no slowdown)** |
| entropy-first (random-prefix ULID analog) | ~14,900 eps/s | ~100 ms | ~63 ms | x0.63 |
| fetch-by-`@key` (`concept-id` value-index seek) | ~0.37 ms / lookup | — | — | — |

**No append-hotspot / write-amplification at 50k:** monotonic batches did *not*
slow down (drift < 1.0). Entropy-first was **slower**, not faster, so there is
**no evidence to switch ULID -> entropy-first now**; the timestamp-first ULID
stays. Fetch-by-`@key` is a sub-millisecond value-index seek, confirming point (3).

*Caveat (logged, not skipped):* 50k is well below the 10^6-10^7 scale where RocksDB
compaction / index write-amplification would manifest; a single fresh-db run cannot
surface long-horizon compaction effects. The switch to an entropy-first ULID (random
bits ahead of the timestamp) remains a **value-only change with zero schema impact**
if a future large-scale measurement shows a hotspot on the `concept-id` attribute
index. Know this before 10^7 episodes exist.
