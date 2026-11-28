---
type: subsystem
title: JavaScript Guest (Boa) and Playbook API
description: The prebuilt Boa JavaScript engine compiled to wasm32-wasip1 that runs every playbook — the globals it exposes to playbook code (console.log, gecko.callExtension, frozen gecko.args, mem.*), how the program's last expression becomes the run output, and how to rebuild and re-commit the guest binary.
tags: [sandbox, javascript, boa, wasm, playbooks, guest]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-28T02:42:19.865Z
sources:
  - id: openwiki-source-8d0ae7f670f80081d8e3b8ba
    resource: repo://core/js-sandbox/src/main.rs
  - id: openwiki-source-ba97b6d28e0165aba47d1e12
    resource: repo://docs/refactor/guest-wasm.md
  - id: openwiki-source-c59fe4336a371ea1052a01dd
    resource: repo://justfile
generated: { by: "claude-code", at: "2026-11-28T02:42:19.865Z" }
---

# JavaScript Guest (Boa) and Playbook API

<!-- openwiki: broken internal link [/openwiki/sandbox/wasm-executor.md] link "/openwiki/sandbox/wasm-executor.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Every playbook runs inside one guest program: a **Boa** JS engine built from `core/js-sandbox/src/main.rs` for `wasm32-wasip1` and checked in prebuilt as `core/gecko-engine/src/sandbox/quickjs.wasm`. The name is historical; the engine is not QuickJS, but `engine: quickjs` (or `js`/`javascript`) in frontmatter selects it. The host side is described in [WASM Sandbox Executor and Host ABI](/openwiki/sandbox/wasm-executor.md); the authoritative ABI notes are `docs/refactor/guest-wasm.md`.

## Startup

`main()`:

1. Reads the program from stdin. An empty or unreadable program prints a failure line.
2. Reads `GECKO_ARGS` from the otherwise-empty environment. It must be a JSON object, else the run fails before any program code runs.
3. Creates a Boa `Context` and registers three globals, then evaluates the program.

## Globals

- **`console.log(...)`** — joins its arguments with spaces and writes to **stderr**, so it never corrupts the stdout result line.
- **`gecko.args`** — the run's `--args` object (`{}` when absent), deep-frozen (every nested object/array frozen), non-writable and non-configurable. It is not scope-gated: arguments come from the operator, not from a host capability.
- **`gecko.callExtension(ext, func, args)`** — the generic host call. `ext` and `func` must be strings (else `TypeError`). Absent, `undefined` or `null` `args` is sent as `{}`. Values that cannot become JSON (functions, non-finite numbers, Symbols, cycles, BigInt) throw `TypeError`. The reply is returned as a JS value; a host error is thrown.
- **`mem.*`** — sugar over `host_call("mem", …)`:

| Method | Wire payload |
| --- | --- |
| `mem.recall(query, {maxChunks, asOf}?)` | `{text, max_chunks?, as_of?}`; returns the `chunks` array directly. |
| `mem.remember(text, opts?)` | `{text, event_time?}` |
| `mem.derive(text, evidence?, opts?)` | `{text, evidence?, method?, confidence?}` |
| `mem.supersede(old, text, reason?, opts?)` | `{old, text, reason?, confidence?}` |
| `mem.retract(id, reason?)` | `{id, reason?}` → `{id, blast_radius}` |
| `mem.restsOn(resting, assumptions)` | host fn `rests_on` |
| `mem.contest(claims)` | `{claims}` |

<!-- openwiki: broken internal link [/openwiki/memory/sandbox-mem-bridge.md] link "/openwiki/memory/sandbox-mem-bridge.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Guest methods check argument shape and rename camelCase options to snake_case wire keys, but forward values unconverted: the host is the single authority on types and ranges, and it re-validates everything ([Sandbox → mem Bridge](/openwiki/memory/sandbox-mem-bridge.md)). The guest never sends provenance keys; the host strips and stamps them.

Every host call still needs the matching S3 scope (`mem:recall`, `cyber-gecko:claim`, …) in the concept's `scopes` frontmatter.

## Run output

The guest prints exactly **one JSON line** to stdout: the value of the program's last expression, serialized from Boa's `to_json`. If evaluation throws, or the value cannot be stringified, it prints `{"success": false, "error": "…"}` instead. The host interprets that line (see the executor page): a top-level `"success": false` fails the run, empty or non-JSON output fails it, and any other JSON is the successful output.

```javascript
const chunks = mem.recall("lateral movement", { maxChunks: 5 });
const belief = mem.derive("compromised via SMB", chunks.map((c) => c.id));
({ recalled: chunks.length, belief: belief.id });   // ← the run output
```

## Rebuilding

CI never builds `js-sandbox` (`--exclude js-sandbox`; it is not a default member and cannot run on the host because of its wasm imports). After editing `core/js-sandbox/src/main.rs`:

```bash
just build-guest      # rustup target add wasm32-wasip1; cargo build -p js-sandbox --target wasm32-wasip1 --release; copy over quickjs.wasm
cargo test -p gecko-engine --test wasm_tests --test pipeline_tests --no-default-features
cargo test -p gecko-bin --test sandbox_test --no-default-features   # live TypeDB
```

Commit the source and the regenerated `quickjs.wasm` together. The goal is behavioral equivalence (guest tests green), not a byte-identical binary. Only `cargo check`/`clippy -p js-sandbox` work on the host.

## Examples

- `sample-bundle/playbooks/hello_js.md` — minimal playbook, no scopes.
- `sample-bundle/playbooks/mem_recall_demo.md` — recall → derive with `scopes: [mem:recall, mem:derive]`.
- `extensions/cyber-gecko/sample-bundle/playbooks/*.md` — `gecko.args`-driven cyber playbooks.
