# Writing a GECKO extension

An extension adds a domain to GECKO: a TypeQL schema fragment, host functions that
sandboxed playbooks can call, and (optionally) TypeDB functions. Extensions are
composed into a binary at **compile time** by the Assembler and **activated** at
run time by `gecko.toml`'s `[extensions] enabled`. The `mem` substrate is
forced-on and not listed; everything else is opt-in.

This document is the contract for authoring one. It is deliberately vendor-neutral
— the same rules apply whoever (or whatever) writes the code. Code-quality
conventions that apply repo-wide (parameterized TQL, greenfield comments, never
fabricate success on bad input) live in the root `CLAUDE.md`; this file covers the
extension **seams**.

`mem-gecko` (`core/mem-gecko`) and `cyber-gecko` (`extensions/cyber-gecko`) are the
two reference implementations. When in doubt, copy their shape.

## The `GeckoExtension` trait

Defined in `core/gecko-extension-api/src/lib.rs`. Implement it and expose a
constructor the Assembler can register:

| Method | Purpose |
| --- | --- |
| `name()` | Stable identifier, e.g. `"cyber-gecko"`. Used for host-call routing and the S3 scope prefix. |
| `schema()` | The extension's TypeQL, applied after core + mem in registration order. Return `""` for none. |
| `schema_migrations()` | Migrations that bring an earlier version of the schema forward on an existing database, applied before `schema()`. |
| `belief_edges()` | Declares the relations this extension writes alongside a belief, through `BeliefEdgeWriter`. |
| `host_imports()` | Declares the callable host functions (`HostImportDef { name, description }`). |
| `call_import(name, args)` | **Synchronous** dispatch for host imports that need nothing but their arguments. |
| `inject_epistemic_host_fns(ctx, writer)` | Mem's hook that binds the belief-write path into the sandbox; other extensions do not use it. |

Register it in the Assembler (`build_extensions` in `gecko-bin/src/main.rs`), and
build the extension only inside the CLI arms that use it — a typo in
`[extensions] enabled` must not break unrelated commands like `up`/`drop`.

## The schema fragment

- Subtype the core **`concept`** for records (observed, authoritative facts). It is
  concrete, so an unrecognized input can be ingested as a bare `concept` subtype.
  Do **not** subtype the abstract `okf-concept` unless your type is itself abstract
  (TypeDB requires an abstract type to have an abstract supertype).
- **Cross-fragment `plays`/`owns` additions** onto core or mem types (e.g.
  `belief plays technique-detection:detection-claim;`) are applied additively from
  your fragment. Validate them against live TypeDB — `plays` additions are
  reliable; `owns` additions onto a foreign type are riskier.
- Records that carry queryable fields should persist them as **typed `owns`**, not
  as a `metadata-json` blob. Populate `OkfConcept.typed_attributes` (a
  `Vec<TypedAttribute>`); the syncer writes each as a real typed attribute on the
  entity's subtype. See cyber-gecko's STIX adapter.
- **Document every type, role, and function with `@doc`.** These strings live in the
  graph and are readable at runtime (`get_doc`, `get_relates_doc`), so a client — or
  an agent introspecting the live schema — can discover what your ontology means. It
  is **never inherited**, so a subtype and each `as`-renamed role needs its own. The
  string goes right after the label/return-type: `entity foo @doc("…"), owns …`,
  `relates bar @card(1..) @doc("…")`, `fun f() -> { foo } @doc("…"):`. On an
  **attribute** it must precede `value`, separated by a comma:
  `attribute foo @doc("…"), value string @values("a","b");`. Keep it to one accurate,
  greenfield sentence (no plan/epic tags). Attribute-level `@doc` is currently left to
  the source `#` comments; type/role/function coverage is the convention.
- **`alias` is a reserved word.** The belief-edge registry refuses an extension fragment
  (or migration) that contains the token `alias` anywhere, so don't use it as a label.
- **No type or role renames.** The registry also refuses a fragment or migration that
  renames a type or role (`redefine <type> label <new>;`,
  `redefine <relation>:<role> label <new>;`), including the extension's own types: it
  resolves every type by the label the fragments declare.
- **No modifying core or mem.** The registry refuses an extension fragment or migration
  that `undefine`s or `redefine`s anything core or mem introduces, or that defines a
  function with a core or mem function name. It runs before any extension schema is
  applied (`schema-init`, `sync`, `sync-stix`) and again when `run` builds the bridge.
- **Changing your own functions.** Edit them in your `define` fragment; the engine
  redefines changed functions itself. An explicit `redefine fun` whose body names a core
  or mem type is refused.
- **Retiring a belief edge** cannot be done from an extension fragment. It is a reviewed
  change to the fragments, applied to a fresh database or by a migration that mem reviews.
- **`@meta("key","value")` (client-readable schema metadata) is deliberately unused.**
  It only pays off when an *unknown-at-compile-time* client reads the schema; GECKO
  composes its schema at compile time and its current clients never introspect it. Two
  concrete candidates are parked for when a runtime schema-introspection consumer (an
  LLM agent reasoning over the live graph is the expected first one) lands: tagging
  cyber types with their STIX/ATT&CK origin, and tagging types with their invariant
  number / source-of-truth-vs-rebuildable classification.

## Host-import seams: which one to use

A sandboxed playbook calls `gecko.callExtension(ext, func, args)`. Every such call
is **S3 default-deny gated** in `wasm_executor.rs`: it runs only if the concept
granted the `"{ext}:{func}"` scope in its frontmatter (e.g.
`scopes: [cyber-gecko:claim]`). The gate is checked **before** any dispatch, so a
host function never has to re-check scopes.

There are two ways to service a call. Pick by what the function needs:

### 1. Plain synchronous imports — `call_import`

For a function that needs only its arguments (pure computation, in-memory work),
implement `call_import`. It is synchronous and receives no graph handle, no writer,
and no run identity. This is the simplest seam and the default.

If an import receives a missing or invalid field, **return an error** — never
default it (`""`, `0`) and report success. If your extension has no plain imports,
`host_imports()` returns `vec![]` and `call_import` errors.

### 2. Host-bridge imports — for graph / belief / run-identity access

An import that must read the graph, write beliefs with the extension's declared
edges, or know the run's identity **cannot** use `call_import` — that seam has
none of those. Instead it is serviced by a **host bridge**: a wrapper
around the sandbox's `ExtensionCallback` that captures the capabilities and
intercepts this extension's calls, delegating everything else to the inner
callback.

Two reference bridges exist, wired in `build_host_bridge` (`gecko-bin/src/main.rs`):

- **mem** — `epistemic_extension_callback` (`core/gecko-engine/src/sandbox/mem_host.rs`)
  routes `mem.*` to the `EpistemicWriter`/`EpistemicReader`.
- **cyber** — `cyber_extension_callback` (`extensions/cyber-gecko/src/detect/mod.rs`)
  routes the detect imports to a `GraphReader` and a `BeliefEdgeWriter`.

The shape (copy it):

```rust
pub fn my_extension_callback(
    graph: Arc<dyn GraphReader>,         // parameterized reads only
    edges: Arc<dyn BeliefEdgeWriter>,    // writes a belief with this extension's declared edges
    ctx: RunContext,                     // host-minted run identity (never sandbox-forged)
    base: ExtensionCallback,             // the inner callback to fall through to
) -> ExtensionCallback {
    let handle = tokio::runtime::Handle::current();
    Arc::new(move |ext, func, args| {
        if ext == MY_EXT && MY_FNS.contains(&func) {
            // Sync -> async: the callback is synchronous but the graph/writer are
            // async, so drive the future on the ambient multi-thread runtime.
            return tokio::task::block_in_place(|| handle.block_on(dispatch(/* … */)));
        }
        base(ext, func, args)   // not ours: delegate
    })
}
```

Wiring it (in `build_host_bridge`): construct the capabilities once, then fold your
bridge around the callback chain when your extension is activated. Bridges compose
— each wraps the previous, so a non-matching call falls through to the next and
finally to the base dispatcher.

Notes and invariants:

- **Run identity is the host's.** The `RunContext` (`run_id`/`actor`/`source`) is
  minted by the host per run; sandbox code cannot forge or omit it (invariant 2).
  Stamp every belief write with it.
- **The `GraphReader` seam** (`core/gecko-extension-api/src/graph.rs`): bridges get
  read-only graph access. `read(query, vars, row)` runs a parameterized `fetch` and
  returns JSON documents. Values cross as typed `GraphValue` params — never
  interpolate values into query text. Only fixed schema labels (types, roles,
  attributes) may be interpolated, and even those, if extension-supplied, should be
  validated as TypeQL identifiers first.
- **Belief edges.** A relation that attaches to a belief is declared in
  `belief_edges()` and written through the `BeliefEdgeWriter` the host binds to your
  extension (`assert_belief_with_edges`): the belief and its edges commit in one
  transaction or not at all. Only mem holds a `GraphStore`.
- **Import-time writes.** An import-time post-sync step (e.g. cyber's
  `cyber_post_sync`) returns a `Vec<GraphWrite>`, which the engine runs inside the
  sync transaction.
- **Multi-thread runtime required.** `block_in_place` panics on a current-thread
  runtime; the binary and sandbox both run multi-thread.
- **Still never fabricate.** Validate arguments and refuse to act on references that
  don't exist in the graph, rather than writing a meaningless record.

### Why the bridge is wired in the binary, not the trait

`ExtensionCallback` lives in `gecko-extension-api`, beside `GeckoExtension`, so an
extension can define its bridge without depending on the engine. The bridge is
still composed in `build_host_bridge`, because the host owns everything a bridge
captures: the `GraphReader`, the `BeliefEdgeWriter` bound to the extension, and the
`RunContext`. Adding a new bridged extension means adding one fold there, next to
mem's and cyber's.

## Checklist for a new extension

1. Crate under `extensions/<name>/`, depending only on `gecko-extension-api` —
   never `gecko-engine` or `typedb-driver`. CI enforces this with
   `scripts/check-extension-deps.sh`.
2. Implement `GeckoExtension`; return your schema fragment from `schema()`.
3. Validate the schema against live TypeDB (a schema-init test on a throwaway DB).
4. Plain imports → `call_import`. Graph-read/belief/identity imports → a host
   bridge wired in `build_host_bridge`, over a `GraphReader` and a
   `BeliefEdgeWriter`.
5. Persist queryable record fields as `typed_attributes`, not `metadata-json`.
6. Register in `build_extensions`, activate via `gecko.toml`, grant scopes in
   playbook frontmatter (`scopes: [<name>:<func>]`).
7. Tests: schema applies, imports validate bad input, and an end-to-end sync/run.
