//! The crate error type for the async epistemic traits and the (sync) semantic
//! index/embedder traits.
//!
//! The plan mandates a concrete crate error (`EpistemicError`) over
//! `Box<dyn Error>` so callers can match on the failure mode — in particular the
//! [`NotYetImplemented`](EpistemicError::NotYetImplemented) seam that lets
//! mem-gecko land graph-coupled bodies in a later phase without changing this
//! contract.

use thiserror::Error;

/// Errors surfaced by the epistemic write/read path and the semantic index.
#[derive(Debug, Error)]
pub enum EpistemicError {
    /// The method is part of the stable contract but its body ships in a later
    /// phase (A2/A3). This is the A1↔A2 seam: mem returns this for graph-coupled
    /// methods it cannot yet satisfy.
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

    /// An id or enum string could not be parsed against the schema contract.
    #[error("parse error: {0}")]
    Parse(String),
}
