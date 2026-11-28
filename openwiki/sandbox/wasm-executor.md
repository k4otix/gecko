---
type: subsystem
title: WASM Sandbox Executor and Host ABI
description: The Wasmtime executor that runs every playbook — S1 resource limits and the epoch timeout, the virtual stdin/stdout/env WASI context with no filesystem or network, the two-import host-call ABI with its guest length caps, S3 default-deny scope checking, the lazily built SandboxPool, and how guest stdout becomes an ExecutionResult.
tags: [sandbox, wasm, wasmtime, security, abi, scopes]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-28T02:42:19.865Z
sources:
  - id: openwiki-source-8e6f1b70944b54af3591448e
    resource: repo://core/gecko-engine/src/sandbox/wasm_executor.rs
  - id: openwiki-source-4c88f8142f4e53f07bbf6368
    resource: repo://core/gecko-engine/src/sandbox/wasm_pool.rs
generated: { by: "claude-code", at: "2026-11-28T02:42:19.865Z" }
---

# WASM Sandbox Executor and Host ABI

<!-- openwiki: broken internal link [/openwiki/sandbox/js-guest.md] link "/openwiki/sandbox/js-guest.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
All synced code is untrusted. `core/gecko-engine/src/sandbox/wasm_executor.rs` runs it in Wasmtime inside a single guest module, the Boa JS engine embedded with `include_bytes!("quickjs.wasm")` (see [JavaScript Guest](/openwiki/sandbox/js-guest.md)). The pipeline reaches it through `SandboxPool::execute` (`wasm_pool.rs`), which lazily builds one `WasmExecutor` on first use via a `OnceLock` and caches an initialization failure as a failed `ExecutionResult`.

## Engine setup (`WasmExecutor::new`)

- Wasmtime `Config` with SIMD and relaxed SIMD off, bulk memory and multi-value on, and **epoch interruption** on.
- A background thread increments the engine epoch every `EPOCH_TICK_MS` = 10 ms until the executor drops.
- The embedded module is compiled once and reused for every run.

## One run (`evaluate`)

1. **Validate inputs.** `timeout_ms` must be in `1..=MAX_TIMEOUT_MS` (600 000); the default is 30 000. Run args serialize to `GECKO_ARGS` (default `{}`), capped at `MAX_RUN_ARGS_LEN` = 1 MiB.
2. **WASI context.** stdin is an in-memory pipe holding the program; stdout is an in-memory pipe capped at 10 MiB; the environment holds only `GECKO_ARGS`. There are no preopened directories and no sockets.
3. **S1 limits** via `StoreLimits`: linear memory 256 MiB, 100 000 table elements, one instance, one memory, one table, trap on grow failure. The store's epoch deadline is set from the timeout.
4. **Link** WASI Preview 1 (async) and the two host imports below. Link failures are propagated as run failures (S5).
5. **Instantiate and call `_start`** on the async path. An epoch `Trap::Interrupt` becomes `execution timed out after N ms`; a full stdout pipe becomes `guest output exceeded the 10 MiB limit`; other traps become `JS Execution trap: …`.
6. **Read the output** (`read_guest_output`): empty output → `guest wrote no output`; non-JSON → `guest output is not JSON: <first 1 KiB>`; top-level `"success": false` → failure with its `error` string (or a default message); any other JSON → success with that value as `output`.

`ExecutionResult { output, duration_ms, engine, success, error }` is returned to the pipeline; `gecko run` prints it and exits non-zero when `success` is false.

## Host-call ABI

Both imports come from module `env`, and both trap if the guest exports no linear memory.

### `host_call_extension(ext_ptr, ext_len, func_ptr, func_len, args_ptr, args_len) -> i32`

Order matters for security:

1. Clear any pending reply, so a reply is never read by a later call.
2. Read `ext` and `func`, each capped at `MAX_IMPORT_NAME_LEN` = 256 bytes **before** a host buffer is allocated.
3. **S3 default-deny**: require `"{ext}:{func}"` in the concept's `scopes`; otherwise return `scope '…' not granted to this concept`. This happens before the args are read, so an ungranted call learns nothing about whether its payload was well-formed.
4. Read args (capped at `MAX_ARGS_JSON_LEN` = 8 MiB) and parse them as JSON.
5. Invoke the run's `ExtensionCallback` **at most once** (`No extensions loaded` when there is none).
6. `stash_outcome` stores the result host-side and returns `+len` for a JSON reply or `-len` for a UTF-8 error. A reply over 16 MiB becomes `host response exceeded 16 MiB`, an over-long error is truncated on a char boundary, and empty replies and errors get messages, so the return value is never 0.

### `host_read_reply(out_ptr, out_cap) -> i32`

Takes the pending outcome (readable once) and copies it into the guest buffer, returning its length, or `-1` (nothing pending), `-2` (buffer too small), or `-3` (out of bounds). The pending outcome is cleared in every case. The guest allocates exactly `|len|` bytes.

## Where calls go

<!-- openwiki: broken internal link [/openwiki/architecture/assembler-and-extensions.md] link "/openwiki/architecture/assembler-and-extensions.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
<!-- openwiki: broken internal link [/openwiki/memory/sandbox-mem-bridge.md] link "/openwiki/memory/sandbox-mem-bridge.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
The `ExtensionCallback` is the chain composed by `build_host_bridge`: cyber's detect bridge → mem's `epistemic_extension_callback` → the base `call_import` dispatcher ([Assembler Pattern](/openwiki/architecture/assembler-and-extensions.md), [Sandbox → mem Bridge](/openwiki/memory/sandbox-mem-bridge.md)). Because the scope gate runs first, no bridge or handler needs to re-check scopes. Callbacks are synchronous; bridges drive async work with `block_in_place`, so the executor must run on a multi-thread Tokio runtime.

## Security tiers in this file

- **S1** — bounded memory, tables, instances, and an epoch timeout.
- **S3** — default-deny `ext:func` scopes from frontmatter.
- **S5** — link and instantiation errors surface as failures rather than being ignored.
- No ambient authority: no filesystem, network, or environment beyond `GECKO_ARGS`.

## Tests

- `core/gecko-engine/tests/wasm_tests.rs` — limits, timeouts, the ABI return codes, scope denial before args are parsed, output parsing.
- `core/gecko-engine/tests/pipeline_tests.rs` — parse → execute through the pipeline.
- `gecko-bin/tests/sandbox_test.rs`, `run_args_cli_test.rs` — the binary end to end, including `--args` limits.
- Unit tests in `wasm_executor.rs` (`timeout_epochs` rounding, `stash_outcome`, `read_guest_output`).
