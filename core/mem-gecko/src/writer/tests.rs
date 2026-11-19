use super::beliefs::validate_confidence;
use super::reader::{DEFAULT_HALF_LIFE_HOURS, recency_decay};
use super::*;
use async_trait::async_trait;
use gecko_extension_api::GraphWrite;
use gecko_extension_api::{
    ActorId, BeliefDraft, DerivationMethod, EpistemicWriter, FilterMeta, ProvenanceSource,
    RunContext, Visibility,
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn confidence_validation_enforces_range_and_invariant_7() {
    use DerivationMethod::*;
    for c in [f64::NAN, -0.1, 1.5, f64::INFINITY] {
        assert!(validate_confidence(Some(c), Some(TypeJoin)).is_err(), "{c}");
        assert!(validate_confidence(Some(c), None).is_err(), "{c}");
    }
    assert!(validate_confidence(Some(1.0), Some(LlmSynthesis)).is_err());
    assert!(validate_confidence(Some(0.99), Some(LlmSynthesis)).is_ok());
    assert!(validate_confidence(Some(1.0), Some(TypeJoin)).is_ok());
    assert!(validate_confidence(Some(0.0), Some(HumanAssertion)).is_ok());
    assert!(validate_confidence(None, Some(LlmSynthesis)).is_ok());
}

fn ctx() -> RunContext {
    RunContext::new(
        ActorId::new("agent-1"),
        ProvenanceSource::Manual,
        chrono::Utc::now(),
    )
}

fn belief() -> BeliefDraft {
    BeliefDraft {
        text: "the host is compromised".to_string(),
        owner: ActorId::new("agent-1"),
        visibility: Visibility::Private,
        confidence: Some(0.7),
        entrenchment: None,
    }
}

/// A fake [`GraphStore`] that records committed writes and can be forced to
/// fail — enough to unit-test the best-effort/dirty wrapper without a live DB.
#[derive(Default)]
struct RecordingGraphStore {
    writes: Mutex<Vec<GraphWrite>>,
    fail_write: bool,
}
impl RecordingGraphStore {
    fn failing() -> Self {
        Self {
            fail_write: true,
            ..Default::default()
        }
    }
}
#[async_trait]
impl GraphStore for RecordingGraphStore {
    async fn write(&self, ops: &[GraphWrite]) -> Result<()> {
        if self.fail_write {
            return Err(EpistemicError::Storage("db down".into()));
        }
        self.writes.lock().unwrap().extend_from_slice(ops);
        Ok(())
    }
    async fn read(
        &self,
        _query: &str,
        _vars: &[String],
        _row: &[gecko_extension_api::GraphValue],
    ) -> Result<Vec<Value>> {
        Ok(vec![])
    }
}

struct OkEmbedder;
impl Embedder for OkEmbedder {
    fn model_id(&self) -> &str {
        "test-embedder"
    }
    fn dim(&self) -> usize {
        3
    }
    fn embed_query(&self, _t: &str) -> Result<Vec<f32>> {
        Ok(vec![0.1, 0.2, 0.3])
    }
    fn embed_document(&self, _t: &str) -> Result<Vec<f32>> {
        Ok(vec![0.1, 0.2, 0.3])
    }
}

struct FailingEmbedder;
impl Embedder for FailingEmbedder {
    fn model_id(&self) -> &str {
        "failing-embedder"
    }
    fn dim(&self) -> usize {
        3
    }
    fn embed_query(&self, _t: &str) -> Result<Vec<f32>> {
        Err(EpistemicError::Embedding("boom".into()))
    }
    fn embed_document(&self, _t: &str) -> Result<Vec<f32>> {
        Err(EpistemicError::Embedding("boom".into()))
    }
}

#[derive(Default)]
struct FailingUpsertIndex {
    upserts: AtomicUsize,
    removes: AtomicUsize,
}
impl SemanticIndex for FailingUpsertIndex {
    fn upsert(&self, _id: ConceptId, _v: &[f32], _m: FilterMeta) -> Result<()> {
        self.upserts.fetch_add(1, Ordering::SeqCst);
        Err(EpistemicError::Index("upsert failed".into()))
    }
    fn remove(&self, _id: ConceptId) -> Result<()> {
        self.removes.fetch_add(1, Ordering::SeqCst);
        Err(EpistemicError::Index("remove failed".into()))
    }
    fn query(&self, _v: &[f32], _k: usize, _p: &FilterMeta) -> Result<Vec<(ConceptId, f32)>> {
        Ok(vec![])
    }
    fn model_id(&self) -> &str {
        "test-embedder"
    }
}

#[derive(Default)]
struct OkIndex {
    upserts: Mutex<Vec<ConceptId>>,
}
impl SemanticIndex for OkIndex {
    fn upsert(&self, id: ConceptId, _v: &[f32], _m: FilterMeta) -> Result<()> {
        self.upserts.lock().unwrap().push(id);
        Ok(())
    }
    fn remove(&self, _id: ConceptId) -> Result<()> {
        Ok(())
    }
    fn query(&self, _v: &[f32], _k: usize, _p: &FilterMeta) -> Result<Vec<(ConceptId, f32)>> {
        Ok(vec![])
    }
    fn model_id(&self) -> &str {
        "test-embedder"
    }
}

#[tokio::test]
async fn assert_belief_survives_failing_index_and_marks_dirty() {
    let idx = Arc::new(FailingUpsertIndex::default());
    let w = MemWriter::new(
        Arc::new(RecordingGraphStore::default()),
        Some(Arc::new(OkEmbedder)),
        Some(idx.clone()),
    );
    let id = w
        .assert_belief(&ctx(), belief(), &[], DerivationMethod::LlmSynthesis)
        .await
        .expect("belief write must succeed even when the index fails");
    assert_eq!(idx.upserts.load(Ordering::SeqCst), 1);
    assert!(w.is_dirty(&ConceptId(id.0.clone())));
    assert_eq!(w.dirty_len(), 1);
}

#[tokio::test]
async fn assert_belief_survives_failing_embed_and_marks_dirty() {
    let idx = Arc::new(FailingUpsertIndex::default());
    let w = MemWriter::new(
        Arc::new(RecordingGraphStore::default()),
        Some(Arc::new(FailingEmbedder)),
        Some(idx.clone()),
    );
    let id = w
        .assert_belief(&ctx(), belief(), &[], DerivationMethod::LlmSynthesis)
        .await
        .expect("belief write must succeed even when embedding fails");
    assert_eq!(
        idx.upserts.load(Ordering::SeqCst),
        0,
        "embed failed ⇒ no upsert attempt"
    );
    assert!(w.is_dirty(&ConceptId(id.0)));
}

#[tokio::test]
async fn assert_belief_with_working_index_is_not_dirty() {
    let idx = Arc::new(OkIndex::default());
    let w = MemWriter::new(
        Arc::new(RecordingGraphStore::default()),
        Some(Arc::new(OkEmbedder)),
        Some(idx.clone()),
    );
    let id = w
        .assert_belief(&ctx(), belief(), &[], DerivationMethod::TypeJoin)
        .await
        .unwrap();
    assert_eq!(w.dirty_len(), 0);
    assert_eq!(idx.upserts.lock().unwrap().as_slice(), &[ConceptId(id.0)]);
}

#[tokio::test]
async fn no_index_configured_skips_embed_and_never_dirties() {
    let w = MemWriter::without_index(Arc::new(RecordingGraphStore::default()));
    let id = w
        .assert_belief(&ctx(), belief(), &[], DerivationMethod::TypeJoin)
        .await
        .unwrap();
    assert!(id.0.starts_with("mem/bel/"));
    assert_eq!(w.dirty_len(), 0);
}

#[tokio::test]
async fn authoritative_commit_failure_fails_the_write_and_dirties_nothing() {
    let idx = Arc::new(OkIndex::default());
    let w = MemWriter::new(
        Arc::new(RecordingGraphStore::failing()),
        Some(Arc::new(OkEmbedder)),
        Some(idx.clone()),
    );
    let err = w
        .assert_belief(&ctx(), belief(), &[], DerivationMethod::TypeJoin)
        .await
        .expect_err("a failed graph commit must fail the write");
    assert!(matches!(err, EpistemicError::Storage(_)));
    assert_eq!(idx.upserts.lock().unwrap().len(), 0);
    assert_eq!(w.dirty_len(), 0);
}

#[tokio::test]
async fn supersede_marks_old_dirty_when_remove_fails() {
    let idx = Arc::new(FailingUpsertIndex::default());
    let w = MemWriter::new(
        Arc::new(RecordingGraphStore::default()),
        Some(Arc::new(OkEmbedder)),
        Some(idx.clone()),
    );
    let old = MemId::new("mem/bel/OLD");
    let new_id = w
        .supersede(
            &ctx(),
            old.clone(),
            belief(),
            "found contradicting evidence",
        )
        .await
        .expect("supersede must succeed even when the index errors");
    assert_eq!(idx.removes.load(Ordering::SeqCst), 1);
    assert!(w.is_dirty(&ConceptId(old.0)));
    assert!(w.is_dirty(&ConceptId(new_id.0)));
}

#[tokio::test]
async fn supersede_without_embedder_never_removes() {
    // Old-vector remove is gated on BOTH embedder+index.
    let idx = Arc::new(FailingUpsertIndex::default());
    let w = MemWriter::new(
        Arc::new(RecordingGraphStore::default()),
        None, // no embedder
        Some(idx.clone()),
    );
    w.supersede(&ctx(), MemId::new("mem/bel/OLD"), belief(), "reason")
        .await
        .unwrap();
    assert_eq!(
        idx.removes.load(Ordering::SeqCst),
        0,
        "no embedder ⇒ no remove attempt (matches upsert gating)"
    );
    assert_eq!(w.dirty_len(), 0);
}

#[tokio::test]
async fn contest_rejects_fewer_than_two_claims() {
    let w = MemWriter::without_index(Arc::new(RecordingGraphStore::default()));
    let e = w
        .contest(&ctx(), &[MemId::new("mem/bel/x")])
        .await
        .unwrap_err();
    assert!(matches!(e, EpistemicError::InvalidInput(_)));
}

#[test]
fn recency_decay_halves_each_half_life() {
    let t0 = chrono::Utc::now();
    let one_week = t0 + chrono::Duration::hours(168);
    let d = recency_decay(t0, one_week, DEFAULT_HALF_LIFE_HOURS);
    assert!((d - 0.5).abs() < 1e-6, "one half-life ⇒ 0.5, got {d}");
    assert_eq!(recency_decay(one_week, t0, DEFAULT_HALF_LIFE_HOURS), 1.0);
}
