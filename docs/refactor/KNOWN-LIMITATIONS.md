# Known limitations (gecko-core-substrate-refactor)

Short, load-bearing caveats so nothing in this branch is described as shipped
end-to-end when it is not.

## `mem.recall` and the retrieval-provenance write path are NOT reachable from `gecko run`

**Status: implemented + tested at the library level; NOT wired into a sandboxed run.**

- `EpistemicReader::recall` (present-state, index-seeded, and as-of-T paths) and the
  A5.7 retrieval-provenance write path (`retrieval-event` stamping +
  `informs-synthesis` correlation on a later `assert_belief`) are fully implemented in
  `core/mem-gecko` and covered by that crate's own tests
  (`writer_reader_integration`, `semantic_index_integration`).
- They are **not** reachable from a sandboxed `gecko run`. The sandbox → mem bridge
  (`core/gecko-engine/src/sandbox/mem_host.rs`) exposes only the **writer** host fns
  (`remember`/`derive`/`supersede`/`contest`). `MemHostFn::Recall` is deliberately
  rejected there: the async **reader** bridge and the QuickJS-guest FFI binding are
  out of scope for this refactor, and no in-tree guest binding exists.
- Consequence: **A5.7 retrieval-provenance correlation runs only in mem-gecko's
  library tests, never in a live run.** Do not describe `mem.recall` or the
  retrieval-provenance write path as shipped end-to-end.

**Deferred work (not started here):** the async reader bridge in `mem_host.rs` plus a
QuickJS-guest binding that lets exec-docs call `mem.recall`. Wiring these is the only
thing that makes the reader path reachable end-to-end.

## Consolidation ("dreaming") daemon is a scaffold

`core/mem-gecko/src/consolidation.rs` ships the `ConsolidationDaemon` trait + a wired
no-op `tick`. Every other job is a build-later stub that returns
`EpistemicError::NotYetImplemented` (no panic). The one real consolidation operation
that ships is per-pair `MemWriter::dedup_episodes`.
