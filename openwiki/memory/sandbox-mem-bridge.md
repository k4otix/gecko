---
type: subsystem
title: Sandbox → mem Bridge (mem.* calls)
description: How a playbook's mem.* calls reach mem-gecko — the host-side epistemic_extension_callback, the reserved-key stripping that keeps run identity unforgeable, per-function payload validation, the recall reader bridge, and the shared run scratch that auto-links a derived belief to the retrieval that informed it.
tags: [mem, sandbox, host-bridge, provenance, recall, security]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-28T02:42:19.865Z
sources:
  - id: openwiki-source-d7abfb89abf530a5482f01bf
    resource: repo://core/gecko-engine/src/sandbox/mem_host.rs
  - id: openwiki-source-71302efdf2c88746c3bbc2a8
    resource: repo://core/mem-gecko/src/writer/beliefs.rs
  - id: openwiki-source-78aba063f87d0a556ddbeed7
    resource: repo://gecko-bin/tests/mem_recall_e2e_test.rs
generated: { by: "claude-code", at: "2026-11-28T02:42:19.865Z" }
---

# Sandbox → mem Bridge (mem.* calls)

Playbooks reach the epistemic substrate through the guest's `mem.*` namespace (`recall`, `remember`, `derive`, `supersede`, `contest`, `retract`, `restsOn`). Every one of these is a host call to extension `"mem"`, gated by an S3 scope such as `mem:recall` or `mem:derive` in the concept's frontmatter. The host side lives in `core/gecko-engine/src/sandbox/mem_host.rs`.

## Wiring

`build_host_bridge` (`gecko-bin/src/main.rs`) mints one `RunContext` per run (actor `system`, source `ExecutableDoc { concept_id }`) and calls:

```rust
epistemic_extension_callback(run_ctx, writer, reader, base_cb)
```

`writer` is mem's `MemWriter` as `Arc<dyn EpistemicWriter>`; `reader` comes from the same object via `as_epistemic_reader()`. The returned `ExtensionCallback` intercepts `ext == "mem"` with a known `MemHostFn` name and passes everything else to `base`. The callback is synchronous while the writer is async, so each call is driven with `block_in_place` + `Handle::block_on`; a `debug_assert` checks for a multi-thread runtime at wiring time, because `block_in_place` panics on a current-thread scheduler.

## Run identity cannot be forged

`bind_mem_call(ctx, func, args)` strips the reserved keys `run_id`, `actor`, `source` and `occurred_at` from the guest payload and stamps the call with the context's values (`StampedMemCall`). Guest code supplies only payload fields; it can neither supply nor override provenance (invariant 2). Beliefs drafted from a payload are owned by the run's actor, `Private`, with entrenchment left to the writer (`belief_draft_from_payload`).

## Function surface

`dispatch_mem_call` first validates that the payload is a JSON object containing **only** the function's allowed keys (`check_payload`). The guest's own checks can be bypassed with a raw `gecko.callExtension("mem", …)`, so the host enforces this itself.

| Host fn | Allowed keys | Writer call | Reply |
| --- | --- | --- | --- |
| `remember` | `text`, `event_time` (RFC 3339; default `occurred_at`) | `observe` | `{id}` |
| `derive` | `text`, `evidence`, `method` (default `llm-synthesis`), `confidence` | `assert_belief` | `{id}` |
| `supersede` | `old`, `text`, `reason`, `confidence` | `supersede` | `{id}` |
| `contest` | `claims` (required, non-empty) | `contest` | `{anomaly}` |
| `retract` | `id`, `reason` | `retract` | `{id, blast_radius}` |
| `rests_on` | `resting`, `assumptions` (required, non-empty ids) | `rests_on` | `{resting, assumptions}` |
| `recall` | `text`, `max_chunks` (default 8), `as_of` | reader `recall` | `{chunks: [{id, text, score}]}` |

<!-- openwiki: broken internal link [/openwiki/memory/epistemic-substrate.md] link "/openwiki/memory/epistemic-substrate.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Malformed values are errors and are never defaulted. A malformed `as_of`, for example, fails instead of silently falling back to present-state recall. `recall` is routed to `dispatch_mem_recall` on the reader bridge; it errors when no reader is wired, and `dispatch_mem_call` refuses it on the write bridge. Writer-side refusals (visibility, entrenchment, already-retracted, …) come back as the error string; see [Epistemic Memory Substrate](/openwiki/memory/epistemic-substrate.md).

## Recall → derive provenance

The bridge threads the same `RunContext` (and its shared `RunScratch`) through every mem call in a run:

1. `mem.recall` with retrieval provenance enabled (`[semantic_index] record_retrieval_provenance`, defaulting to `enabled`) mints a `retrieval-event` with `surfaced` edges and pushes `(event, gated candidates)` into the scratch.
2. A later `mem.derive(text, evidenceIds)` calls `assert_belief`, which uses `ctx.matched_retrievals(evidence)` to find retrievals whose candidates overlap the evidence. It writes `informs-synthesis` edges, flags those `surfaced` links `was-used`, and sets the belief's `retrieval-provenance`, all atomically with the belief (`core/mem-gecko/src/writer/beliefs.rs`).

The playbook only passes recalled ids as evidence; the linking is automatic. `sample-bundle/playbooks/mem_recall_demo.md` (scopes `[mem:recall, mem:derive]`) is the runnable demo.

## Tests

- `core/gecko-engine/src/sandbox/mem_host.rs` unit tests — forgery stripping, payload validation, dispatch over fake writers.
- `gecko-bin/tests/epistemic_bridge_test.rs` — the bridge against live TypeDB.
- `gecko-bin/tests/mem_recall_e2e_test.rs` — a real guest program runs recall → derive through the wasm sandbox; asserts `retrieval-provenance = "semantic"`, the `informs-synthesis` edge, and `was-used = true`.

<!-- openwiki: broken internal link [/openwiki/sandbox/wasm-executor.md] link "/openwiki/sandbox/wasm-executor.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
<!-- openwiki: broken internal link [/openwiki/sandbox/js-guest.md] link "/openwiki/sandbox/js-guest.md" is root-absolute, which no real consumer resolves against the repository root (not a coding agent reading the page, not GitHub's Markdown renderer, not a local viewer); use a path relative to this file instead. Fix the href or restore the target, then delete this comment. -->
Related: [WASM Sandbox Executor and Host ABI](/openwiki/sandbox/wasm-executor.md), [JavaScript Guest](/openwiki/sandbox/js-guest.md).
