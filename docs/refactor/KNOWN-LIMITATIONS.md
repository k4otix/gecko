# Known limitations (gecko-core-substrate-refactor)

Short, load-bearing caveats so nothing in this branch is described as shipped
end-to-end when it is not.

## `mem.recall` host-bridge exists, but the QuickJS-guest binding does not

**Status: implemented + tested at the library level; the host-side reader bridge is
now wired and unit-tested; the guest binding that lets exec-doc JS call it is still
missing, so it is NOT yet reachable end-to-end from a live `gecko run`.**

- `EpistemicReader::recall` (present-state, index-seeded, and as-of-T paths) and the
  A5.7 retrieval-provenance write path (`retrieval-event` stamping +
  `informs-synthesis` correlation on a later `assert_belief`) are fully implemented in
  `core/mem-gecko` and covered by that crate's own tests
  (`writer_reader_integration`, `semantic_index_integration`).
- **The host-side reader bridge is now wired** (`core/gecko-engine/src/sandbox/mem_host.rs`):
  `dispatch_mem_recall` binds the host `RunContext`, parses the payload into a
  `RecallQuery`/`ContextBudget`, calls the injected `EpistemicReader`, and serializes
  the chunks to JSON; `epistemic_extension_callback` routes `MemHostFn::Recall` to it
  through the same async→sync bridge and the SAME per-run scratch as the writer (so
  A5.7 correlation would fire in a live run). `MemWriter` is handed over as the reader
  via `EpistemicWriter::as_epistemic_reader`. Covered by the `recall_*` unit tests in
  `mem_host.rs`.
- **What is still missing:** the QuickJS-**guest** FFI binding — the `mem.recall(...)`
  function exposed *inside* the sandbox that lets exec-doc JS call it and receive the
  returned chunk array. No in-tree guest binding exists, so although the host bridge is
  reachable and tested, nothing in a live `gecko run` calls it yet.
- Consequence: **A5.7 retrieval-provenance correlation still runs only in mem-gecko's
  library tests and the host-bridge unit tests, never yet in a live run.** Do not
  describe `mem.recall` as shipped end-to-end until the guest binding lands.

**Deferred work (planned separately):** the QuickJS-guest binding for `mem.recall`
(return-value FFI surface) + an end-to-end `gecko run` test that recalls and then
asserts, proving live A5.7 correlation.

## Consolidation ("dreaming") daemon is a scaffold

`core/mem-gecko/src/consolidation.rs` ships the `ConsolidationDaemon` trait + a wired
no-op `tick`. Every other job is a build-later stub that returns
`EpistemicError::NotYetImplemented` (no panic). The one real consolidation operation
that ships is per-pair `MemWriter::dedup_episodes`.
