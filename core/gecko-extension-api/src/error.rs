//! The crate error type for the async epistemic traits and the (sync) semantic
//! index/embedder traits.
//!
//! A concrete crate error (`EpistemicError`) is used over `Box<dyn Error>` so
//! callers can match on the failure mode — in particular the
//! [`NotYetImplemented`](EpistemicError::NotYetImplemented) seam that lets
//! mem-gecko land graph-coupled bodies in a later phase without changing this
//! contract.

use thiserror::Error;

/// Errors surfaced by the epistemic write/read path and the semantic index.
#[derive(Debug, Error)]
pub enum EpistemicError {
    /// The method is part of the stable contract but its body ships in a later
    /// phase: mem returns this for graph-coupled methods it cannot yet satisfy.
    #[error("not yet implemented: {0}")]
    NotYetImplemented(&'static str),

    /// The authoritative graph write failed.
    #[error("storage error: {0}")]
    Storage(String),

    /// The embedder failed to produce a vector.
    #[error("embedding error: {0}")]
    Embedding(String),

    /// The semantic index rejected an upsert/remove/query.
    #[error("index error: {0}")]
    Index(String),

    /// The caller supplied an invalid draft/query.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Invariant 7: a lower-entrenchment belief attempted to supersede a
    /// higher-entrenchment one. The write is rejected and nothing is committed —
    /// an `llm`/`inferred` synthesis is structurally forbidden from overwriting an
    /// `axiom`/`user-stated` belief.
    #[error("entrenchment violation: {0}")]
    EntrenchmentViolation(String),

    /// An id or enum string could not be parsed against the schema contract.
    #[error("parse error: {0}")]
    Parse(String),
}
