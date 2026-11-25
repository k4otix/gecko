# TypeDB implementation notes

Observed on the live engine (TypeDB 3.12, re-checked on 3.15.0); recorded so they are not
re-derived. Unless a note says otherwise, it holds on 3.15.0. Moved verbatim from the
owed-work ledger on 2026-11-24.

- **`[DEX15]` — a changed type annotation is not re-`define`-able.** Re-`define` is a no-op
  only for an identical definition; a different `@doc` on an existing type is refused.
  `redefine relation <name> @doc("…");` replaces it. (REVIEW D1.)
- **`[REP3]`/`[REX38]` — functions are validated one at a time on `redefine`,** against the
  stored versions of their callees, even inside a single `redefine` block. Callee-first order
  is required for a coordinated signature change. (REVIEW D2.)
- **`[REX27]` — `redefine` of an identical `owns … @card(…)` is an error,** so a type statement
  carried into a function's `redefine` text fails on re-apply. (REVIEW D3.)
- **`[FUN5]` — a persisted function is immutable under `define`,** for a changed and for a
  byte-identical body alike. (SPEC-W2-W3 §1.4, P-REDEF.)
- **`[REP1]` value-into-attribute.** `insert $x has attr $v` where `$v` is a function- or
  `reduce`-returned value fails at compile ("cannot be declared as both a 'Attribute' and as
  a 'Value'"). Writers that persist a computed score must bind an attribute instance; a
  `given` parameter with `has attr == $p` is fine (the populations writer uses that form).
- **`[REP35]` parameterized `update`.** `update $o has valid-to == $t;` with `$t` a `given`
  datetime works; `update $o has valid-to $t;` is refused ("declared as both a 'Value' and as a
  'Attribute'"). On an owns `@card(0..1)` attribute, `update` inserts the value when the
  instance has none and replaces it when it has one, with no cardinality error. Guarded by
  `not { $o has valid-to $vt; $vt <= $t; }`, it never extends an earlier value. Observed on
  3.15.0, 2026-11-25.
- **A role-only relation with `has` inside `not` persists in a function.** `not { supersession
  (superseded: $b), has created-at $ca; $ca <= $t; };` defines in a function body, and the
  exported text re-plans with no rewrite, so the anonymous form needs no `$s isa … links`.
  Observed on 3.15.0, 2026-11-25.
- **`let $x in <single-valued fn>` yields one row.** A `return first` scalar is coerced to a
  one-row stream, so `select-for-context`'s `let $score in retrieval-score($b, $now)` is
  well-formed. Settled.
- **`database.schema()` exports each function's source text verbatim** (unindented, comments
  dropped), so the schema planner can compare a fragment's function with the stored one after
  normalizing whitespace and comments.
- **Nested negation works in a write's `match`** — `not { $o isa population-member, links (…);
  not { $o has valid-to $vt; }; };` guards an `insert` against a second open row.
- **Write answers are countable.** Under `query_with_rows`, `match … insert` and
  `match … update` answers are row streams with one row per match (counts of 0, 1 and 2 were
  all observed). `GraphWrite.expect_rows` relies on this.
- **The live check dropped its `@key`/`@unique` condition.** Matching on annotations returns
  `[REP254]` (not implemented; still so on 3.15.0). An ambiguous key is still refused at commit by the expect-1
  row count.
- **Unknown labels error; unmet conditions return empty.** On 3.15.0 a type query naming a
  label the schema lacks raises `INF2`/`INF3`, and a known type that fails the condition
  (`$t label belief; $t plays x:y;` with no such `plays`) returns no rows. 3.12 also raised
  `FCP1` for some unsatisfiable patterns. The live check passes only on a returned row and
  reads any error as a failed condition (`InvalidInput`), so it refuses in both cases; a DB
  outage during the check is therefore labelled `InvalidInput`, with the storage cause kept
  in the message.
- **The memory-item backstop uses a negation.** It is written as
  `$m label memory-item; not { $s sub $m; $s sub P; };` and passes when it returns one
  document. On 3.12 the pass-on-empty form raised `FCP1`; on 3.15.0 it returns no rows.
- **`reduce … count` over an unsatisfiable pattern panicked the 3.12 server**
  (`compiler/executable/reduce.rs:132`). Fixed in 3.15.0 (typedb#8021): the same query returns
  a count of 0.
- **Alias definitions are unimplemented** (`[DEX1]`, still so on 3.15.0). The registry refuses any `alias` in
  extension fragments regardless.
- **Types and roles can be renamed** (since 3.12.2): `redefine <type> label <new>;` and
  `redefine <relation>:<role> label <new>;` both commit on 3.15.0. The registry refuses
  either in an extension fragment.
- **Optional variables are dereferenced explicitly** (3.15.0). Fetching a `try`-bound
  variable needs `$v?`; the bare form still works but logs a deprecation warning that says
  it will fail in the next version. `try` in a write stage is deprecated in favor of
  `if { isset $v; } then { … }`.
