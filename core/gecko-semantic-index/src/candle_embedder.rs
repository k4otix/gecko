//! The real, candle-backed [`Embedder`]: `bge-large-en-v1.5` (BERT, 1024-dim),
//! run in-process on CPU. Behind the NON-default `real-embedder` feature so the
//! fast/default build never compiles candle (plan P2 / P4).
//!
//! ## Load-bearing rule #1 — the model is a runtime asset
//! No weights enter cargo's world: the struct holds only a `model_dir` PATH
//! (resolved from `model_id` by the caller, always OUTSIDE the build tree). The
//! ~1.3GB is loaded from disk, never `include_bytes!`'d.
//!
//! ## Lazy load
//! Construction touches NO files. The model + tokenizer load on the FIRST embed,
//! once, and are reused for the rest of the process (a `Mutex<Option<_>>` cell). A
//! documentation-only run (`enabled=[]`, no beliefs → no embed) never loads it.
//! On first embed, if the model dir is absent: `auto_fetch=false` (default) yields
//! an actionable error pointing at `gecko model fetch` — never a silent 1.3GB
//! pull; `auto_fetch=true` fetches then loads.
//!
//! ## BGE specifics (A5.2)
//! - Query/document asymmetry is enforced on the trait: [`Embedder::embed_query`]
//!   prepends the BGE search prefix; [`Embedder::embed_document`] does not; both
//!   run the same forward pass. [`Embedder::embed`] defaults to the document side.
//! - Pooling is **CLS-token** (BGE's recommended pooling), then **L2-normalize**
//!   so downstream cosine is a plain dot product.

use std::path::PathBuf;
use std::sync::Mutex;

use candle_core::{Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig, DTYPE};
use gecko_extension_api::{Embedder, EpistemicError};
use tokenizers::Tokenizer;

use crate::fetch;

/// The BGE query-side prefix (plan A5.2). Prepended by [`Embedder::embed_query`]
/// so the query and document embeddings of the same text differ, as BGE requires.
const QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";

/// `bge-large-en-v1.5` output dimensionality.
const BGE_DIM: usize = 1024;

/// The real candle/BERT embedder. See the module docs for the lazy-load and
/// runtime-asset contracts.
pub struct CandleEmbedder {
    /// `"<model>@<revision>"` — the model identity recorded in the index header.
    model_id: String,
    /// The resolved on-disk model directory (weights + tokenizer + config). Always
    /// OUTSIDE the build tree; never dereferenced until the first embed.
    model_dir: PathBuf,
    /// Whether a missing model may be auto-downloaded on first embed. Default
    /// false — GECKO never silently pulls ~1.3GB.
    auto_fetch: bool,
    /// Optional mirror base URL for acquisition (`None` ⇒ HuggingFace).
    source: Option<String>,
    /// The lazily-loaded model+tokenizer, populated on first embed and reused.
    loaded: Mutex<Option<Loaded>>,
}

/// The loaded state: constructed once on first embed.
struct Loaded {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl CandleEmbedder {
    /// Constructs the embedder WITHOUT loading anything (lazy). No file access
    /// happens here — this is what lets a doc-only run skip the 1.3GB entirely.
    pub fn new(
        model_id: impl Into<String>,
        model_dir: impl Into<PathBuf>,
        auto_fetch: bool,
        source: Option<String>,
    ) -> Self {
        Self {
            model_id: model_id.into(),
            model_dir: model_dir.into(),
            auto_fetch,
            source,
            loaded: Mutex::new(None),
        }
    }

    /// Whether the model has been loaded yet (for the lazy-load assertion in
    /// tests: constructing + a doc-only run must leave this `false`).
    pub fn is_loaded(&self) -> bool {
        self.loaded
            .lock()
            .expect("embedder mutex poisoned")
            .is_some()
    }

    /// Resolves + verifies the model dir (fetching if allowed), then loads the
    /// weights, tokenizer, and config. Called once, under the lock.
    fn load(&self) -> Result<Loaded, EpistemicError> {
        let weights = self.model_dir.join("model.safetensors");
        if !fetch::is_cached(&self.model_dir) {
            if self.auto_fetch {
                fetch::fetch_model(
                    &self.model_id,
                    &self.model_dir,
                    self.source.as_deref(),
                    false,
                    &[],
                )
                .map_err(|e| {
                    EpistemicError::Embedding(format!(
                        "auto-fetch of '{}' failed: {e:#}",
                        self.model_id
                    ))
                })?;
            } else {
                return Err(EpistemicError::Embedding(format!(
                    "embedding model '{}' is not staged at {}. Run `gecko model fetch` to \
                     download it (~1.3GB), point [semantic_index] model_path at a \
                     pre-provisioned dir, or set auto_fetch=true to fetch on first use.",
                    self.model_id,
                    self.model_dir.display()
                )));
            }
        }

        let device = Device::Cpu;
        let config_path = self.model_dir.join("config.json");
        let tok_path = self.model_dir.join("tokenizer.json");

        let config_str = std::fs::read_to_string(&config_path).map_err(|e| {
            EpistemicError::Embedding(format!("cannot read {}: {e}", config_path.display()))
        })?;
        let config: BertConfig = serde_json::from_str(&config_str).map_err(|e| {
            EpistemicError::Embedding(format!("cannot parse {}: {e}", config_path.display()))
        })?;

        let tokenizer = Tokenizer::from_file(&tok_path)
            .map_err(|e| EpistemicError::Embedding(format!("cannot load tokenizer: {e}")))?;

        // SAFETY: mmap of a read-only weights file we just verified is present.
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights.clone()], DTYPE, &device).map_err(
                |e| EpistemicError::Embedding(format!("cannot mmap {}: {e}", weights.display())),
            )?
        };
        let model = BertModel::load(vb, &config)
            .map_err(|e| EpistemicError::Embedding(format!("cannot build BERT model: {e}")))?;

        Ok(Loaded {
            model,
            tokenizer,
            device,
        })
    }

    /// Runs the BERT forward pass over `text`, CLS-pools, and L2-normalizes.
    /// Loads the model on first call (lazy), reusing it thereafter.
    fn embed_pooled(&self, text: &str) -> Result<Vec<f32>, EpistemicError> {
        let mut guard = self.loaded.lock().expect("embedder mutex poisoned");
        if guard.is_none() {
            *guard = Some(self.load()?);
        }
        let loaded = guard.as_ref().expect("just loaded");

        let encoding = loaded
            .tokenizer
            .encode(text, true)
            .map_err(|e| EpistemicError::Embedding(format!("tokenization failed: {e}")))?;

        let ids = encoding.get_ids();
        let mask = encoding.get_attention_mask();

        let to_err = |e: candle_core::Error| EpistemicError::Embedding(format!("tensor op: {e}"));

        let input_ids = Tensor::new(ids, &loaded.device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(to_err)?;
        let token_type_ids = input_ids.zeros_like().map_err(to_err)?;
        let attention_mask = Tensor::new(mask, &loaded.device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(to_err)?;

        // sequence_output: [1, seq_len, hidden]
        let sequence = loaded
            .model
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))
            .map_err(to_err)?;

        // CLS pooling: token 0 of the single batch row → [hidden].
        let cls = sequence.i((0, 0)).map_err(to_err)?;
        let vec: Vec<f32> = cls.to_vec1().map_err(to_err)?;
        Ok(l2_normalize(vec))
    }
}

/// The document input is the raw text; the query input prepends the BGE prefix.
/// Factored out (and unit-tested) so the query/document asymmetry can't drift.
fn query_input(text: &str) -> String {
    format!("{QUERY_PREFIX}{text}")
}

/// L2-normalizes in place, returning a unit vector (cosine-ready). A degenerate
/// zero vector is mapped to a fixed unit vector so cosine never sees NaN.
fn l2_normalize(mut v: Vec<f32>) -> Vec<f32> {
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

impl Embedder for CandleEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dim(&self) -> usize {
        BGE_DIM
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EpistemicError> {
        self.embed_pooled(&query_input(text))
    }

    fn embed_document(&self, text: &str) -> Result<Vec<f32>, EpistemicError> {
        self.embed_pooled(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_input_prepends_the_bge_prefix() {
        assert_eq!(
            query_input("lateral movement via smb"),
            "Represent this sentence for searching relevant passages: lateral movement via smb"
        );
        // The document side is left untouched (the asymmetry).
        assert_ne!(query_input("x"), "x");
    }

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

    #[test]
    fn construction_is_lazy_and_touches_no_files() {
        // A deliberately-nonexistent dir: constructing must NOT load or fetch.
        let e = CandleEmbedder::new(
            "bge-large-en-v1.5@abc",
            "/nonexistent/gecko-model-dir",
            false,
            None,
        );
        assert!(!e.is_loaded(), "construction must not load the model");
        assert_eq!(e.model_id(), "bge-large-en-v1.5@abc");
        assert_eq!(e.dim(), BGE_DIM);
        assert!(!e.is_loaded(), "querying identity must not load the model");
    }

    #[test]
    fn missing_model_without_auto_fetch_is_an_actionable_error() {
        let e = CandleEmbedder::new(
            "bge-large-en-v1.5@abc",
            "/nonexistent/gecko-model-dir",
            false,
            None,
        );
        let err = e.embed_document("anything").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("gecko model fetch"),
            "error should point at `gecko model fetch`, got: {msg}"
        );
        // It must NOT have silently attempted a download / partial load.
        assert!(!e.is_loaded());
    }

    /// End-to-end real-weights smoke test (Tier-3-ish): validates the BERT forward
    /// pass, CLS pooling, L2-normalization, and the BGE query prefix against the
    /// actual ~1.3GB `bge-large-en-v1.5`. `#[ignore]`d so it NEVER runs on the
    /// fast/default path — opt in with the model staged and its dir in
    /// `GECKO_SMOKE_MODEL_DIR`:
    ///
    /// ```text
    /// gecko model fetch --model "BAAI/bge-large-en-v1.5@main"
    /// GECKO_SMOKE_MODEL_DIR=<cache>/models/BAAI/bge-large-en-v1.5@main \
    ///   cargo test -p gecko-semantic-index --features real-embedder -- --ignored real_weights
    /// ```
    #[test]
    #[ignore = "requires the real ~1.3GB bge-large-en-v1.5; set GECKO_SMOKE_MODEL_DIR"]
    fn real_weights_smoke_embed() {
        let Ok(dir) = std::env::var("GECKO_SMOKE_MODEL_DIR") else {
            eprintln!("GECKO_SMOKE_MODEL_DIR unset — skipping real-weights smoke test");
            return;
        };
        let e = CandleEmbedder::new("bge-large-en-v1.5@main", dir, false, None);
        // Lazy: not loaded until the first embed.
        assert!(!e.is_loaded());

        let query = e
            .embed_query("How do I persist an OAuth access token?")
            .unwrap();
        assert_eq!(query.len(), 1024, "bge-large is 1024-dim");
        assert!(e.is_loaded(), "first embed must load the model");

        // L2-normalized ⇒ unit norm.
        let norm: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "expected unit norm, got {norm}");

        let related = e
            .embed_document(
                "Securely store the OAuth token so it can be reused for later API calls.",
            )
            .unwrap();
        let unrelated = e
            .embed_document("The croissants at the Paris bakery were warm and buttery.")
            .unwrap();

        let cos = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        let sim_related = cos(&query, &related);
        let sim_unrelated = cos(&query, &unrelated);
        assert!(
            sim_related > sim_unrelated,
            "semantically related text ({sim_related:.4}) must score above unrelated \
             ({sim_unrelated:.4})"
        );
    }
}
