//! The deterministic hash-based stub [`Embedder`].
//!
//! A stand-in for the real model (`bge-large-en-v1.5` run in-process via `candle`),
//! which is intentionally NOT pulled in by default so the build stays offline and
//! light. This stub maps text → a fixed-dim L2-normalized (cosine-ready) vector by
//! hashing tokens into dimensions, and it enforces the BGE **query/document
//! asymmetry** on the trait: [`embed_query`](Embedder::embed_query) prepends the BGE
//! search prefix before hashing while [`embed_document`](Embedder::embed_document)
//! hashes raw, so the two paths deterministically differ.
//!
//! The real embedder is [`crate::CandleEmbedder`], compiled only under the
//! `real-embedder` feature, implementing this same [`Embedder`] trait.

use gecko_extension_api::{Embedder, EpistemicError};

use crate::QUERY_PREFIX;

/// A deterministic, dependency-free stand-in [`Embedder`].
///
/// `model_id` is `"stub-hash-v1@<dim>"` (the `<model>@<rev>` shape) so a dimension
/// change forces a full index rebuild via the header mismatch.
pub struct StubEmbedder {
    dim: usize,
    model_id: String,
}

impl StubEmbedder {
    /// A stub embedder producing `dim`-dimensional vectors. `dim` must be > 0.
    pub fn new(dim: usize) -> Self {
        assert!(dim > 0, "embedder dim must be positive");
        Self {
            dim,
            model_id: format!("stub-hash-v1@{dim}"),
        }
    }

    /// The default 384-dimensional stub (a modest, BGE-small-ish width).
    pub fn default_dim() -> Self {
        Self::new(384)
    }

    /// FNV-1a 64-bit hash — small, stable across runs/platforms (unlike
    /// `DefaultHasher`), which is what makes rebuild-from-graph deterministic.
    fn fnv1a(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    /// Hashes text into an L2-normalized `dim`-vector. Tokens are lowercased and
    /// split on whitespace; each contributes a ±1 to one dimension (index and sign
    /// both drawn from its hash). Empty/degenerate text yields a fixed unit vector
    /// so cosine math never sees a zero vector (NaN-free).
    fn hash_embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; self.dim];
        for tok in text.split_whitespace() {
            let lower = tok.to_lowercase();
            let h = Self::fnv1a(lower.as_bytes());
            let idx = (h % self.dim as u64) as usize;
            let sign = if (h / self.dim as u64) & 1 == 0 {
                1.0
            } else {
                -1.0
            };
            v[idx] += sign;
        }
        crate::l2_normalize(v)
    }
}

impl Embedder for StubEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EpistemicError> {
        Ok(self.hash_embed(&format!("{QUERY_PREFIX}{text}")))
    }

    fn embed_document(&self, text: &str) -> Result<Vec<f32>, EpistemicError> {
        Ok(self.hash_embed(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_id_carries_the_dim_revision() {
        assert_eq!(StubEmbedder::new(256).model_id(), "stub-hash-v1@256");
        assert_eq!(StubEmbedder::default_dim().model_id(), "stub-hash-v1@384");
    }

    #[test]
    fn embeddings_are_l2_normalized() {
        let e = StubEmbedder::new(64);
        let v = e.embed_document("the host is compromised").unwrap();
        assert_eq!(v.len(), 64);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "expected unit norm, got {norm}");
    }

    #[test]
    fn deterministic_across_calls() {
        let e = StubEmbedder::new(128);
        assert_eq!(
            e.embed_document("alpha beta gamma").unwrap(),
            e.embed_document("alpha beta gamma").unwrap()
        );
    }

    #[test]
    fn query_and_document_paths_differ_asymmetrically() {
        let e = StubEmbedder::new(256);
        let text = "lateral movement via smb";
        assert_ne!(
            e.embed_query(text).unwrap(),
            e.embed_document(text).unwrap(),
            "BGE prefix must make the query embedding differ from the document one"
        );
        // embed() defaults to the document side.
        assert_eq!(e.embed(text).unwrap(), e.embed_document(text).unwrap());
    }

    #[test]
    fn empty_text_is_a_safe_unit_vector() {
        let e = StubEmbedder::new(32);
        let v = e.embed_document("").unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
        assert!(v.iter().all(|x| x.is_finite()));
    }
}
