//! Extension trait for pluggable domain logic.
//!
//! The [`GeckoExtension`] contract now lives in the standalone
//! [`gecko-extension-api`] crate so that extensions can implement it without
//! depending on the whole engine. This module re-exports it for engine-internal
//! use and to preserve the `gecko_engine::extension::GeckoExtension` path.
//!
//! [`gecko-extension-api`]: gecko_extension_api

pub use gecko_extension_api::GeckoExtension;

// The additive epistemic-substrate contract also lives in `gecko-extension-api`
// (plan A0 carry-forward). Re-exported here for engine-internal plumbing —
// sandbox host-fn injection and any future domain extension — alongside the
// existing `GeckoExtension` re-export.
pub use gecko_extension_api::{
    ActorId, AnomalyId, BeliefDraft, BeliefQuery, BeliefState, Chunk, ConceptId, ContextBudget,
    DateTime, DerivationMethod, Embedder, EpisodeDraft, EpistemicError, EpistemicReader,
    EpistemicWriter, FilterMeta, MemId, Outcome, ProvenanceSource, RecallQuery, RunContext, RunId,
    SemanticIndex, Visibility,
};
