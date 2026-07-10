//! # gecko-semantic-index
//!
//! The **concrete retrieval accelerator** for the GECKO epistemic substrate: a
//! native-Rust HNSW approximate-nearest-neighbour index ([`HnswIndex`], wrapping
//! [`hnsw_rs`]) plus a deterministic hash-based stub [`StubEmbedder`], both
//! implementing the abstract [`SemanticIndex`] / [`Embedder`] contracts from
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
//! ## Swap-to-native contract (this crate is the rip-out surface)
//! A `TypeDbNativeIndex` implementing the same [`SemanticIndex`] trait over TypeDB
//! native vector search (issue #6911) is a **drop-in**: flip the `backend` config
//! from `"hnsw"` to `"typedb-native"`, delete this crate and its path dependency.
//! Nothing above the trait (mem's recall/write path) knows which backend answered —
//! callers only ever see `Vec<(ConceptId, f32)>` → TypeDB fetch → `gate`. The trait
//! **is** the seam; there is deliberately no shared code to disentangle.

mod embedder;
pub mod fetch;
mod index;

#[cfg(feature = "real-embedder")]
mod candle_embedder;

#[cfg(feature = "real-embedder")]
pub use candle_embedder::CandleEmbedder;
pub use embedder::StubEmbedder;
pub use index::{HnswIndex, NoopIndex};

/// The BGE query-side prefix. Prepended on the query side only (never the
/// document side) so query and document embeddings of the same text
/// deterministically differ, mirroring the asymmetry BGE-style models require.
/// Shared by [`StubEmbedder`] and, under the `real-embedder` feature,
/// `CandleEmbedder` — hoisted here (always compiled) so the exact wording can't
/// drift between the two.
pub(crate) const QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";

/// L2-normalizes `v`, returning a unit vector (cosine-ready). A degenerate
/// all-zero vector maps to a fixed unit vector so downstream cosine math never
/// sees NaN. Shared by [`StubEmbedder`] and, under the `real-embedder` feature,
/// `CandleEmbedder`.
pub(crate) fn l2_normalize(mut v: Vec<f32>) -> Vec<f32> {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    } else if !v.is_empty() {
        v[0] = 1.0;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::l2_normalize;

    #[test]
    fn l2_normalize_yields_unit_vectors() {
        let v = l2_normalize(vec![3.0, 4.0]);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        // Zero vector → fixed unit vector (NaN-free).
        let z = l2_normalize(vec![0.0, 0.0, 0.0]);
        assert_eq!(z[0], 1.0);
        assert!(z.iter().all(|x| x.is_finite()));
    }
}

/// HNSW construction/search parameters, tuned for ~10⁶ vectors.
///
/// The caller's `k*4` over-fetch-then-gate gives recall headroom, so `EF_SEARCH`
/// can stay modest — gating + re-rank cleans up the approximate result.
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
