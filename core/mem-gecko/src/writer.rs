//! The mem-gecko [`EpistemicWriter`] implementation.
//!
//! This is the load-bearing A1 piece: it fixes the **call structure** that makes
//! the semantic index a pure accelerator (invariant 8). The graph write is
//! authoritative and commits *first*; embedding + index upsert (and
//! remove-on-supersede) is **best-effort after commit**. Any embed/upsert/remove
//! failure calls [`MemWriter::mark_dirty`] and **never** rolls back or fails the
//! graph write.
//!
//! The graph-commit step sits behind the [`BeliefCommitter`] seam so A2 can drop
//! in the real TypeDB write without touching this wrapper. A1 ships
//! [`StubCommitter`], which returns synthetic mem-ids — enough to fully unit-test
//! the best-effort/dirty-set behaviour now.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gecko_extension_api::{
    AnomalyId, BeliefDraft, BeliefQuery, BeliefState, Chunk, ConceptId, ContextBudget, DateTime,
    DerivationMethod, Embedder, EpisodeDraft, EpistemicError, EpistemicReader, EpistemicWriter,
    FilterMeta, MemId, Outcome, RecallQuery, RunContext, SemanticIndex, Visibility,
};

type Result<T> = std::result::Result<T, EpistemicError>;

/// The authoritative graph-commit seam.
///
/// A1 fixes the *call structure* around this; A2 fills in the real TypeDB writes.
/// Each method performs the AUTHORITATIVE write and commits before the caller
/// runs any best-effort indexing.
#[async_trait]
pub trait BeliefCommitter: Send + Sync {
    /// Commits an episode to the episodic tier, returning its mem-id.
    async fn commit_episode(&self, ctx: &RunContext, ep: &EpisodeDraft) -> Result<MemId>;

    /// Commits a belief + its evidence/derivation + run-id stamp, returning its
    /// mem-id.
    async fn commit_belief(
        &self,
        ctx: &RunContext,
        b: &BeliefDraft,
        evidence: &[MemId],
        method: DerivationMethod,
    ) -> Result<MemId>;

    /// Commits a supersession (old → new lineage) and returns the new mem-id.
    async fn commit_supersede(
        &self,
        ctx: &RunContext,
        old: &MemId,
        new: &BeliefDraft,
        reason: &str,
    ) -> Result<MemId>;
}

/// A1 placeholder committer: returns synthetic mem-ids without touching TypeDB.
///
/// Lets the best-effort/dirty-set wrapper be exercised end-to-end before the A2
/// schema lands. **Not** for production — A2 replaces this with a router-backed
/// committer.
pub struct StubCommitter;

impl StubCommitter {
    fn mint(prefix: &str) -> MemId {
        MemId(format!("{prefix}/{}", ulid::Ulid::new()))
    }
}

#[async_trait]
impl BeliefCommitter for StubCommitter {
    async fn commit_episode(&self, _ctx: &RunContext, _ep: &EpisodeDraft) -> Result<MemId> {
        Ok(Self::mint("mem/ep"))
    }

    async fn commit_belief(
        &self,
        _ctx: &RunContext,
        _b: &BeliefDraft,
        _evidence: &[MemId],
        _method: DerivationMethod,
    ) -> Result<MemId> {
        Ok(Self::mint("mem/bel"))
    }

    async fn commit_supersede(
        &self,
        _ctx: &RunContext,
        _old: &MemId,
        _new: &BeliefDraft,
        _reason: &str,
    ) -> Result<MemId> {
        Ok(Self::mint("mem/bel"))
    }
}

/// mem-gecko's epistemic writer.
///
/// Holds an optional [`Embedder`] + [`SemanticIndex`]: when either is `None` the
/// embed/upsert path is skipped entirely (the A5-disabled configuration). The
/// dirty set records concept-ids whose index vector is stale and awaits
/// background reindex.
pub struct MemWriter {
    committer: Arc<dyn BeliefCommitter>,
    embedder: Option<Arc<dyn Embedder>>,
    index: Option<Arc<dyn SemanticIndex>>,
    dirty: Mutex<HashSet<ConceptId>>,
}

impl MemWriter {
    /// Constructs a writer over the given commit seam and optional index stack.
    pub fn new(
        committer: Arc<dyn BeliefCommitter>,
        embedder: Option<Arc<dyn Embedder>>,
        index: Option<Arc<dyn SemanticIndex>>,
    ) -> Self {
        Self {
            committer,
            embedder,
            index,
            dirty: Mutex::new(HashSet::new()),
        }
    }

    /// Convenience: an A1 writer backed by [`StubCommitter`] with no index.
    pub fn with_stub_committer() -> Self {
        Self::new(Arc::new(StubCommitter), None, None)
    }

    /// Records `id` as needing background reindex (invariant 8). Called on any
    /// best-effort embed/upsert/remove failure — never propagated to the caller.
    pub fn mark_dirty(&self, id: ConceptId) {
        self.dirty
            .lock()
            .expect("dirty set mutex poisoned")
            .insert(id);
    }

    /// Snapshot of the current dirty set (for the A5 background reindexer / tests).
    pub fn dirty_snapshot(&self) -> HashSet<ConceptId> {
        self.dirty.lock().expect("dirty set mutex poisoned").clone()
    }

    /// Whether `id` is currently marked dirty.
    pub fn is_dirty(&self, id: &ConceptId) -> bool {
        self.dirty
            .lock()
            .expect("dirty set mutex poisoned")
            .contains(id)
    }

    /// Number of concept-ids awaiting reindex.
    pub fn dirty_len(&self) -> usize {
        self.dirty.lock().expect("dirty set mutex poisoned").len()
    }

    /// Best-effort embed + upsert for a freshly-committed node. Runs only when
    /// both an embedder and an index are configured; on any failure marks the id
    /// dirty and returns without error (invariant 8: the accelerator can never
    /// gate truth).
    fn best_effort_index(&self, id: &MemId, text: &str, meta: FilterMeta) {
        let (Some(emb), Some(idx)) = (&self.embedder, &self.index) else {
            return; // A5-disabled: skip embed/upsert entirely.
        };
        let cid = ConceptId(id.0.clone());
        match emb.embed(text) {
            Ok(v) => {
                if idx.upsert(cid.clone(), &v, meta).is_err() {
                    self.mark_dirty(cid);
                }
            }
            Err(_) => self.mark_dirty(cid),
        }
    }

    fn belief_meta(&self, b: &BeliefDraft, ctx: &RunContext, state: BeliefState) -> FilterMeta {
        FilterMeta {
            owner: b.owner.clone(),
            visibility: b.visibility,
            belief_state: state,
            valid_from: ctx.occurred_at,
        }
    }
}

#[async_trait]
impl EpistemicWriter for MemWriter {
    async fn observe(&self, ctx: &RunContext, ep: EpisodeDraft) -> Result<MemId> {
        // AUTHORITATIVE: commit the episode first.
        let id = self.committer.commit_episode(ctx, &ep).await?;
        // Best-effort index after commit.
        let meta = FilterMeta {
            owner: ctx.actor.clone(),
            visibility: Visibility::Private,
            belief_state: BeliefState::Asserted,
            valid_from: ctx.occurred_at,
        };
        self.best_effort_index(&id, &ep.text, meta);
        Ok(id)
    }

    async fn assert_belief(
        &self,
        ctx: &RunContext,
        b: BeliefDraft,
        evidence: &[MemId],
        method: DerivationMethod,
    ) -> Result<MemId> {
        // AUTHORITATIVE: commits first.
        let id = self
            .committer
            .commit_belief(ctx, &b, evidence, method)
            .await?;
        // Best-effort embed + upsert; failure marks dirty, never fails the write.
        let meta = self.belief_meta(&b, ctx, BeliefState::Asserted);
        self.best_effort_index(&id, &b.text, meta);
        Ok(id)
    }

    async fn supersede(
        &self,
        ctx: &RunContext,
        old: MemId,
        new: BeliefDraft,
        reason: &str,
    ) -> Result<MemId> {
        // AUTHORITATIVE: graph supersession commits first.
        let new_id = self
            .committer
            .commit_supersede(ctx, &old, &new, reason)
            .await?;
        // Best-effort: drop the old vector...
        if let Some(idx) = &self.index {
            let old_cid = ConceptId(old.0.clone());
            if idx.remove(old_cid.clone()).is_err() {
                self.mark_dirty(old_cid);
            }
        }
        // ...and index the new one.
        let meta = self.belief_meta(&new, ctx, BeliefState::Asserted);
        self.best_effort_index(&new_id, &new.text, meta);
        Ok(new_id)
    }

    // ── Graph-coupled methods: bodies land in A2/A3 (seam). ──────────────────
    async fn contest(&self, _ctx: &RunContext, _claims: &[MemId]) -> Result<AnomalyId> {
        Err(EpistemicError::NotYetImplemented(
            "EpistemicWriter::contest",
        ))
    }

    async fn rests_on(
        &self,
        _ctx: &RunContext,
        _resting: MemId,
        _assumptions: &[MemId],
    ) -> Result<()> {
        Err(EpistemicError::NotYetImplemented(
            "EpistemicWriter::rests_on",
        ))
    }

    async fn record_prediction(&self, _ctx: &RunContext, _b: MemId, _p: f64) -> Result<()> {
        Err(EpistemicError::NotYetImplemented(
            "EpistemicWriter::record_prediction",
        ))
    }

    async fn resolve_prediction(
        &self,
        _ctx: &RunContext,
        _b: MemId,
        _outcome: Outcome,
    ) -> Result<()> {
        Err(EpistemicError::NotYetImplemented(
            "EpistemicWriter::resolve_prediction",
        ))
    }
}

#[async_trait]
impl EpistemicReader for MemWriter {
    async fn recall(
        &self,
        _ctx: &RunContext,
        _q: RecallQuery,
        _budget: ContextBudget,
    ) -> Result<Vec<Chunk>> {
        Err(EpistemicError::NotYetImplemented("EpistemicReader::recall"))
    }

    async fn derivation_chain(&self, _b: MemId) -> Result<Vec<MemId>> {
        Err(EpistemicError::NotYetImplemented(
            "EpistemicReader::derivation_chain",
        ))
    }

    async fn believed_at(&self, _at: DateTime, _q: BeliefQuery) -> Result<Vec<MemId>> {
        Err(EpistemicError::NotYetImplemented(
            "EpistemicReader::believed_at",
        ))
    }

    async fn blast_radius(&self, _retracted: MemId) -> Result<Vec<MemId>> {
        Err(EpistemicError::NotYetImplemented(
            "EpistemicReader::blast_radius",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gecko_extension_api::ActorId;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn ctx() -> RunContext {
        use gecko_extension_api::ProvenanceSource;
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
        }
    }

    /// Embedder that always succeeds with a fixed vector.
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

    /// Embedder that always fails.
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

    /// Index whose `upsert` always errors (and counts calls).
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

    /// Index that records upserts and succeeds.
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

    /// Committer that always fails the authoritative write.
    struct FailingCommitter;
    #[async_trait]
    impl BeliefCommitter for FailingCommitter {
        async fn commit_episode(&self, _c: &RunContext, _e: &EpisodeDraft) -> Result<MemId> {
            Err(EpistemicError::Storage("db down".into()))
        }
        async fn commit_belief(
            &self,
            _c: &RunContext,
            _b: &BeliefDraft,
            _e: &[MemId],
            _m: DerivationMethod,
        ) -> Result<MemId> {
            Err(EpistemicError::Storage("db down".into()))
        }
        async fn commit_supersede(
            &self,
            _c: &RunContext,
            _o: &MemId,
            _n: &BeliefDraft,
            _r: &str,
        ) -> Result<MemId> {
            Err(EpistemicError::Storage("db down".into()))
        }
    }

    // ── Acceptance bullet 4: failing upsert ⇒ Ok + id in dirty set ───────────
    #[tokio::test]
    async fn assert_belief_survives_failing_index_and_marks_dirty() {
        let idx = Arc::new(FailingUpsertIndex::default());
        let w = MemWriter::new(
            Arc::new(StubCommitter),
            Some(Arc::new(OkEmbedder)),
            Some(idx.clone()),
        );

        let id = w
            .assert_belief(&ctx(), belief(), &[], DerivationMethod::LlmSynthesis)
            .await
            .expect("belief write must succeed even when the index fails");

        assert_eq!(
            idx.upserts.load(Ordering::SeqCst),
            1,
            "upsert was attempted"
        );
        assert!(
            w.is_dirty(&ConceptId(id.0.clone())),
            "failed upsert must land the id in the dirty set"
        );
        assert_eq!(w.dirty_len(), 1);
    }

    #[tokio::test]
    async fn assert_belief_survives_failing_embed_and_marks_dirty() {
        let idx = Arc::new(FailingUpsertIndex::default());
        let w = MemWriter::new(
            Arc::new(StubCommitter),
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
            "embed failed, so upsert must not be attempted"
        );
        assert!(w.is_dirty(&ConceptId(id.0)));
    }

    #[tokio::test]
    async fn assert_belief_with_working_index_is_not_dirty() {
        let idx = Arc::new(OkIndex::default());
        let w = MemWriter::new(
            Arc::new(StubCommitter),
            Some(Arc::new(OkEmbedder)),
            Some(idx.clone()),
        );

        let id = w
            .assert_belief(&ctx(), belief(), &[], DerivationMethod::TypeJoin)
            .await
            .unwrap();

        assert_eq!(w.dirty_len(), 0, "successful upsert leaves nothing dirty");
        assert_eq!(idx.upserts.lock().unwrap().as_slice(), &[ConceptId(id.0)]);
    }

    #[tokio::test]
    async fn no_index_configured_skips_embed_and_never_dirties() {
        let w = MemWriter::with_stub_committer(); // no embedder, no index
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
            Arc::new(FailingCommitter),
            Some(Arc::new(OkEmbedder)),
            Some(idx.clone()),
        );

        let err = w
            .assert_belief(&ctx(), belief(), &[], DerivationMethod::TypeJoin)
            .await
            .expect_err("a failed graph commit must fail the write");
        assert!(matches!(err, EpistemicError::Storage(_)));
        assert_eq!(
            idx.upserts.lock().unwrap().len(),
            0,
            "no index side effects"
        );
        assert_eq!(w.dirty_len(), 0);
    }

    #[tokio::test]
    async fn supersede_marks_old_dirty_when_remove_fails() {
        let idx = Arc::new(FailingUpsertIndex::default());
        let w = MemWriter::new(
            Arc::new(StubCommitter),
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
        assert!(
            w.is_dirty(&ConceptId(old.0)),
            "failed remove dirties the old id"
        );
        assert!(
            w.is_dirty(&ConceptId(new_id.0)),
            "failed upsert dirties the new id too"
        );
    }

    #[tokio::test]
    async fn graph_coupled_methods_return_not_yet_implemented() {
        let w = MemWriter::with_stub_committer();
        let e = w.contest(&ctx(), &[]).await.unwrap_err();
        assert!(matches!(e, EpistemicError::NotYetImplemented(_)));
    }
}
