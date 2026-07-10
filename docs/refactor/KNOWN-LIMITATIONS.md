# Known limitations (gecko-core-substrate-refactor)

Short, load-bearing caveats so nothing in this branch is described as shipped
end-to-end when it is not.

## `mem.*` sandbox namespace — RESOLVED (recall proven end-to-end)

**Status: shipped end-to-end.** The guest binding landed, so the full `mem.*`
surface (`recall`/`remember`/`derive`/`supersede`/`contest`) is now callable from
exec-doc JS in a live `gecko run`, and A5.7 retrieval-provenance correlation is
proven through the real sandbox against live TypeDB.

- `EpistemicReader::recall` (present-state, index-seeded, and as-of-T paths) and the
  A5.7 retrieval-provenance write path (`retrieval-event` stamping +
  `informs-synthesis` correlation on a later `assert_belief`) are implemented in
  `core/mem-gecko` and covered by that crate's tests (`writer_reader_integration`,
  `semantic_index_integration`).
- The host reader bridge (`core/gecko-engine/src/sandbox/mem_host.rs`):
  `dispatch_mem_recall` binds the host `RunContext`, parses the payload into a
  `RecallQuery`/`ContextBudget`, calls the injected `EpistemicReader`, and serializes
  the chunks; `epistemic_extension_callback` routes `MemHostFn::Recall` to it through
  the same async→sync bridge and the SAME per-run scratch as the writer. `MemWriter`
  is handed over as the reader via `EpistemicWriter::as_epistemic_reader`.
- **The guest binding now exists** (`core/js-sandbox/src/main.rs`, compiled into the
  committed `core/gecko-engine/src/sandbox/quickjs.wasm`): a `mem` global exposes the
  five functions, each marshalling the exact payload the host parses and (for
  `recall`) unwrapping the `{ chunks }` envelope to hand JS the array directly. Every
  call is S3 default-deny gated on the concept's granted `mem:<fn>` scope.
- **Proven live:** `gecko-bin/tests/mem_recall_e2e_test.rs` drives a real guest
  program (`mem.recall(...)` → `mem.derive(..., chunks.map(c => c.id))`) through
  `WasmExecutor` against live TypeDB and asserts the synthesized belief carries
  `retrieval-provenance = "semantic"`, an `informs-synthesis` edge, and a `surfaced`
  link with `was-used = true` — the first end-to-end demonstration of A5.7. A
  negative test proves an un-scoped `mem.recall` is S3-denied before any host
  dispatch. A worked example ships at `sample-bundle/playbooks/mem_recall_demo.md`.

**Residual caveats (not blocking):**
- The guest reads a host reply into a grow-and-retry buffer capped at 16 MiB; a
  single `mem.recall` returning more than that throws a clear "response exceeded
  16 MiB" error rather than truncating. A host-reported required-size ABI would let
  the guest size the buffer exactly — a noted follow-up, not needed for current
  recall sizes.
- The guest is a prebuilt binary kept out of CI; after editing
  `core/js-sandbox/src/main.rs` you must `just build-guest` and re-commit the wasm
  (see `docs/refactor/guest-wasm.md`).

## Consolidation ("dreaming") daemon is a scaffold

`core/mem-gecko/src/consolidation.rs` ships the `ConsolidationDaemon` trait + a wired
no-op `tick`. Every other job is a build-later stub that returns
`EpistemicError::NotYetImplemented` (no panic). The one real consolidation operation
that ships is per-pair `MemWriter::dedup_episodes`.
