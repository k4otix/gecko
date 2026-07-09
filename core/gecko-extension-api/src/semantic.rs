//! Embedder + semantic-index contract, and the coarse pre-filter metadata.
//!
//! These traits are **sync** and deliberately minimal so a native TypeDB vector
//! backend is a drop-in replacement later (invariant 8: the index is a pure,
//! rebuildable retrieval accelerator, never a source of truth).

use serde::{Deserialize, Serialize};

use crate::error::EpistemicError;
use crate::ids::{ActorId, ConceptId, DateTime};

// Module-local alias so the trait signatures read exactly like the plan.
type Result<T> = std::result::Result<T, EpistemicError>;

/// Kebab-cased, round-trippable enum mirroring a schema `@values(...)` set.
macro_rules! kebab_enum {
    ($(#[$meta:meta])* $name:ident { $( $variant:ident => $s:literal ),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $( #[doc = $s] $variant ),+
        }

        impl $name {
            /// The canonical kebab-case string — this is the contract the mem
            /// schema's `@values(...)` mirrors.
            pub fn as_str(&self) -> &'static str {
                match self {
                    $( Self::$variant => $s ),+
                }
            }

            /// Parses the canonical kebab-case string. `None` on an unknown value.
            pub fn from_str(s: &str) -> Option<Self> {
                match s {
                    $( $s => Some(Self::$variant), )+
                    _ => None,
                }
            }
        }
    };
}

kebab_enum! {
    /// Visibility scope of a belief (mirrors the schema `visibility @values`).
    Visibility {
        Private => "private",
        Team => "team",
        Shared => "shared",
    }
}

kebab_enum! {
    /// Lifecycle state of a belief (mirrors the schema `belief-state @values`).
    BeliefState {
        Asserted => "asserted",
        Retracted => "retracted",
        Superseded => "superseded",
        Contested => "contested",
    }
}

/// Coarse, denormalized pre-filter carried alongside a vector in the index so ANN
/// does not return obviously-gateable candidates. **Best-effort, never
/// authoritative** — real gating happens in TypeDB after the fetch (invariant 8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterMeta {
    pub owner: ActorId,
    pub visibility: Visibility,
    pub belief_state: BeliefState,
    pub valid_from: DateTime,
}

/// Produces embedding vectors for text.
///
/// BGE-style models require an asymmetric prefix on queries vs documents; the
/// plan (A5.2) mandates enforcing that asymmetry **on the trait** so the recall
/// and index-upsert paths cannot drift. Implementors override
/// [`embed_query`](Embedder::embed_query) / [`embed_document`](Embedder::embed_document);
/// [`embed`](Embedder::embed) defaults to the document side.
pub trait Embedder: Send + Sync {
    /// Model identity, recorded in the index header for versioning.
    fn model_id(&self) -> &str;

    /// Embedding dimensionality.
    fn dim(&self) -> usize;

    /// Embeds text as a **query** (applies the query-side prefix).
    fn embed_query(&self, text: &str) -> Result<Vec<f32>>;

    /// Embeds text as a **document** (applies the document-side prefix).
    fn embed_document(&self, text: &str) -> Result<Vec<f32>>;

    /// Embeds text for storage. Defaults to the document side; the index-upsert
    /// path always embeds documents.
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_document(text)
    }
}

/// A swappable approximate-nearest-neighbour index over belief embeddings.
///
/// Stores only `(embedding, concept_id, coarse FilterMeta)` and answers one
/// question: which concept-ids are semantically near a query. Every hit is then
/// fetched + gated in TypeDB (invariant 8).
pub trait SemanticIndex: Send + Sync {
    /// Inserts or replaces the vector + pre-filter for `id`.
    fn upsert(&self, id: ConceptId, v: &[f32], meta: FilterMeta) -> Result<()>;

    /// Removes `id`'s vector (e.g. on supersession).
    fn remove(&self, id: ConceptId) -> Result<()>;

    /// Returns up to `k` nearest concept-ids with similarity scores.
    fn query(&self, v: &[f32], k: usize, pre: &FilterMeta) -> Result<Vec<(ConceptId, f32)>>;

    /// Model identity; must match the [`Embedder`]'s or the index needs a rebuild.
    fn model_id(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_roundtrips() {
        for v in [Visibility::Private, Visibility::Team, Visibility::Shared] {
            assert_eq!(Visibility::from_str(v.as_str()), Some(v));
        }
        assert_eq!(Visibility::from_str("nonsense"), None);
    }

    #[test]
    fn belief_state_roundtrips() {
        for s in [
            BeliefState::Asserted,
            BeliefState::Retracted,
            BeliefState::Superseded,
            BeliefState::Contested,
        ] {
            assert_eq!(BeliefState::from_str(s.as_str()), Some(s));
        }
        assert_eq!(BeliefState::from_str("nonsense"), None);
    }

    #[test]
    fn belief_state_strings_are_the_schema_contract() {
        assert_eq!(BeliefState::Superseded.as_str(), "superseded");
        assert_eq!(Visibility::Private.as_str(), "private");
    }
}
