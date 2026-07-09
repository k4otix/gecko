//! # gecko-semantic-index
//!
//! The **concrete retrieval accelerator** for the GECKO epistemic substrate (plan
//! A5): a native-Rust HNSW approximate-nearest-neighbour index ([`HnswIndex`],
//! wrapping [`hnsw_rs`]) plus a deterministic hash-based stub [`StubEmbedder`],
//! both implementing the abstract [`SemanticIndex`] / [`Embedder`] contracts from
//! `gecko-extension-api`.
//!
//! ## Invariant 8 — a pure accelerator, never a source of truth
//! This crate stores only `(embedding, concept_id, coarse FilterMeta)` and answers
//! exactly one question: *which concept-ids are semantically near this query.*
//! Every hit is fetched from TypeDB, **gated**, and provenance-checked upstream in
//! `mem-gecko` — the graph is the only system of record. Consequences baked in
//! here:
//! - **Present-state only.** The index carries no bitemporal state; as-of-T recall
//!   never touches it (that gate lives in `mem-gecko`).
//! - **Best-effort.** An `upsert`/`remove` failure never fails a graph write; the
//!   writer marks the concept-id dirty for background repair.
//! - **Fully rebuildable.** The persisted file is derived state; deleting it (or a
//!   header/model-id mismatch) triggers a full reconstruction from the graph. No
//!   vector storage is ever added to the graph schema.
//!
//! ## A5.6 — swap-to-native contract (this crate is the rip-out surface)
//! A `TypeDbNativeIndex` implementing the same [`SemanticIndex`] trait over TypeDB
//! native vector search (issue #6911) is a **drop-in**: flip the `backend` config
//! from `"hnsw"` to `"typedb-native"`, delete this crate and its path dependency.
//! Nothing above the trait (mem's recall/write path) knows which backend answered —
//! callers only ever see `Vec<(ConceptId, f32)>` → TypeDB fetch → `gate`. The trait
//! **is** the seam; there is deliberately no shared code to disentangle.

mod embedder;
mod index;

pub use embedder::StubEmbedder;
pub use index::{HnswIndex, NoopIndex};

/// HNSW construction/search parameters (plan A5.1, tuned for ~10⁶ vectors).
///
/// The A3 `k*4` over-fetch-then-gate gives recall headroom, so `EF_SEARCH` can stay
/// modest — gating + re-rank cleans up the approximate result.
pub mod params {
    /// `M`: max neighbour connections per node.
    pub const M: usize = 16;
    /// Max HNSW layer count.
    pub const MAX_LAYER: usize = 16;
    /// `ef_construction`: build-time candidate breadth.
    pub const EF_CONSTRUCTION: usize = 200;
    /// `ef_search`: query-time recall/latency dial.
    pub const EF_SEARCH: usize = 100;
    /// Capacity hint handed to `hnsw_rs` (it grows past this as needed).
    pub const MAX_ELEMENTS_HINT: usize = 100_000;
}
