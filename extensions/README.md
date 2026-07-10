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
| `host_imports()` | Declares the callable host functions (`HostImportDef { name, description }`). |
| `call_import(name, args)` | **Synchronous** dispatch for host imports that need nothing but their arguments. |
| `inject_epistemic_host_fns(ctx, writer)` | Optional hook to bind the belief-write path into the sandbox (mem uses it). |

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

An import that must read or write the graph, write beliefs through the
`EpistemicWriter`, or know the run's identity **cannot** use `call_import` — that
seam has none of those. Instead it is serviced by a **host bridge**: a wrapper
around the sandbox's `ExtensionCallback` that captures the capabilities and
intercepts this extension's calls, delegating everything else to the inner
callback.

Two reference bridges exist, wired in `build_host_bridge` (`gecko-bin/src/main.rs`):

- **mem** — `epistemic_extension_callback` (`core/gecko-engine/src/sandbox/mem_host.rs`)
  routes `mem.*` to the `EpistemicWriter`/`EpistemicReader`.
- **cyber** — `cyber_extension_callback` (`extensions/cyber-gecko/src/detect/mod.rs`)
  routes the detect imports to a `GraphStore` + `EpistemicWriter`.

The shape (copy it):

```rust
pub fn my_extension_callback(
    graph: Arc<dyn GraphStore>,          // parameterized reads + atomic batched writes
    writer: Arc<dyn EpistemicWriter>,    // the exclusive belief-write path
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
- **The `GraphStore` seam** (`core/gecko-extension-api/src/graph.rs`): `write(&[GraphWrite])`
  runs a whole slice in one atomic transaction; `read(query, vars, row)` runs a
  parameterized `fetch` and returns JSON documents. Values cross as typed
  `GraphValue` params — never interpolate values into query text. Only fixed
  schema labels (types, roles, attributes) may be interpolated, and even those, if
  extension-supplied, should be validated as TypeQL identifiers first.
- **Multi-thread runtime required.** `block_in_place` panics on a current-thread
  runtime; the binary and sandbox both run multi-thread.
- **Still never fabricate.** Validate arguments and refuse to act on references that
  don't exist in the graph, rather than writing a meaningless record.

### Why the bridge is wired in the binary, not the trait

`ExtensionCallback` lives in `gecko-engine`, which depends on `gecko-extension-api`
(where `GeckoExtension` lives) — not the other way around. A trait method returning
`ExtensionCallback` would invert that dependency, so the bridge is composed in
`build_host_bridge` instead. Adding a new bridged extension means adding one fold
there, next to mem's and cyber's.

## Checklist for a new extension

1. Crate under `extensions/<name>/`, depending on `gecko-extension-api` and (if it
   needs the graph/writer bridge) `gecko-engine`.
2. Implement `GeckoExtension`; return your schema fragment from `schema()`.
3. Validate the schema against live TypeDB (a schema-init test on a throwaway DB).
4. Plain imports → `call_import`. Graph/belief/identity imports → a host bridge
   wired in `build_host_bridge`.
5. Persist queryable record fields as `typed_attributes`, not `metadata-json`.
6. Register in `build_extensions`, activate via `gecko.toml`, grant scopes in
   playbook frontmatter (`scopes: [<name>:<func>]`).
7. Tests: schema applies, imports validate bad input, and an end-to-end sync/run.
