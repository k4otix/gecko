//! Belief and episode writes: the [`EpistemicWriter`] implementation and its
//! invariant-7 guards.

use std::sync::Arc;

use async_trait::async_trait;
use gecko_extension_api::{
    AnomalyId, BeliefDraft, BeliefState, ConceptId, DerivationMethod, EpisodeDraft, EpistemicError,
    EpistemicReader, EpistemicWriter, FilterMeta, GraphWrite, MemId, Outcome, RunContext,
    Visibility,
};

use super::{MemWriter, Result, mint, str_field};
use crate::tql;

impl MemWriter {
    /// Reads a belief's stored `entrenchment` tier (invariant-7 supersede guard input).
    /// `None` when the belief carries no entrenchment attribute.
    async fn read_entrenchment(&self, b: &MemId) -> Result<Option<String>> {
        let (q, v, r) = tql::read_entrenchment_read(b);
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(docs
            .first()
            .map(|d| str_field(d, "e"))
            .filter(|s| !s.is_empty()))
    }

    /// The ops that write belief `id`: the belief with its run and actor stamps, its
    /// derivation from `evidence`, and, when retrieval provenance is on, its links to
    /// the run's retrievals whose candidates overlap that evidence.
    pub(super) fn belief_ops(
        &self,
        ctx: &RunContext,
        b: &BeliefDraft,
        id: &MemId,
        evidence: &[MemId],
        method: DerivationMethod,
    ) -> Vec<GraphWrite> {
        let mut ops = tql::insert_belief_ops(ctx, b, id, tql::entrenchment_for(method));
        if let Some(deriv) = tql::derivation_op(id, evidence, method, b.confidence) {
            ops.push(deriv);
        }
        // Correlate this belief with the run's prior semantic retrievals by
        // evidence overlap (auto-linked; the agent passes nothing). Same transaction
        // as the belief so the retrieval-provenance axis lands atomically with it.
        if self.record_retrieval_provenance {
            let matched = ctx.matched_retrievals(evidence);
            for (ev, used) in &matched {
                ops.push(tql::informs_synthesis_op(ev, id));
                for item in used {
                    ops.push(tql::set_was_used_op(ev, item));
                }
            }
            // Only semantic retrievals are recorded today ⇒ single method or none.
            let provenance = if matched.is_empty() {
                "none"
            } else {
                "semantic"
            };
            ops.push(tql::set_retrieval_provenance_op(id, provenance));
        }
        ops
    }
}

#[async_trait]
impl EpistemicWriter for MemWriter {
    async fn observe(&self, ctx: &RunContext, ep: EpisodeDraft) -> Result<MemId> {
        let id = mint("mem/ep");
        // AUTHORITATIVE: commit the episode (both times + run/actor stamp) first.
        self.graph.write(&tql::observe_ops(ctx, &ep, &id)).await?;
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
        validate_confidence(b.confidence, Some(method))?;
        let id = mint("mem/bel");
        // AUTHORITATIVE: belief + provenance stamp + (evidence⇒derivation) commit first.
        let ops = self.belief_ops(ctx, &b, &id, evidence, method);
        self.graph.write(&ops).await?;
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
        validate_confidence(new.confidence, None)?;
        // Invariant 7 entrenchment guard: a lower-entrenchment belief is STRUCTURALLY
        // forbidden from superseding a higher-entrenchment one. Read the OLD belief's
        // entrenchment; the NEW belief's entrenchment is its explicit tier when the
        // draft carries one, else it INHERITS the old belief's tier (a revision
        // preserves entrenchment) — falling back to "inferred" only when the old tier
        // is unknown. Reject (writing NOTHING) when the new tier ranks below the old.
        let old_entrenchment = self.read_entrenchment(&old).await?;
        let new_entrenchment: &str = match new.entrenchment {
            Some(e) => e.as_str(),
            None => old_entrenchment.as_deref().unwrap_or("inferred"),
        };
        if let Some(old_ent) = &old_entrenchment
            && tql::entrenchment_rank(new_entrenchment) < tql::entrenchment_rank(old_ent)
        {
            return Err(EpistemicError::EntrenchmentViolation(format!(
                "a '{new_entrenchment}' belief cannot supersede a more-entrenched \
                     '{old_ent}' belief (invariant 7)"
            )));
        }
        let new_id = mint("mem/bel");
        // AUTHORITATIVE: new belief + supersession lineage + old→superseded commit first.
        let mut ops = tql::insert_belief_ops(ctx, &new, &new_id, new_entrenchment);
        ops.extend(tql::supersession_ops(
            &old,
            &new_id,
            reason,
            ctx.occurred_at,
        ));
        self.graph.write(&ops).await?;
        // Best-effort: drop the old vector via the existing remove hook (gated on
        // BOTH embedder+index present, the same condition as upsert).
        self.best_effort_remove(ConceptId(old.0.clone()));
        // ...and index the new one.
        let meta = self.belief_meta(&new, ctx, BeliefState::Asserted);
        self.best_effort_index(&new_id, &new.text, meta);
        Ok(new_id)
    }

    async fn contest(&self, ctx: &RunContext, claims: &[MemId]) -> Result<AnomalyId> {
        if claims.len() < 2 {
            return Err(EpistemicError::InvalidInput(
                "contest requires at least two claims".into(),
            ));
        }
        let anomaly = mint("mem/anom");
        self.graph
            .write(&tql::contest_ops(ctx, &anomaly, claims))
            .await?;
        Ok(AnomalyId(anomaly.0))
    }

    async fn rests_on(
        &self,
        _ctx: &RunContext,
        resting: MemId,
        assumptions: &[MemId],
    ) -> Result<()> {
        if assumptions.is_empty() {
            return Err(EpistemicError::InvalidInput(
                "rests_on requires at least one assumption".into(),
            ));
        }
        self.graph
            .write(&[tql::rests_on_op(&resting, assumptions)])
            .await
    }

    async fn record_prediction(&self, _ctx: &RunContext, b: MemId, p: f64) -> Result<()> {
        self.graph.write(&[tql::record_prediction_op(&b, p)]).await
    }

    async fn resolve_prediction(&self, ctx: &RunContext, b: MemId, outcome: Outcome) -> Result<()> {
        let ep_id = mint("mem/ep");
        let oc = match outcome {
            Outcome::Confirmed => "confirmed",
            Outcome::Refuted => "refuted",
            Outcome::Inconclusive => "inconclusive",
        };
        self.graph
            .write(&tql::resolve_prediction_ops(ctx, &b, &ep_id, oc))
            .await
    }

    /// `MemWriter` is both writer and [`EpistemicReader`], so hand it back as one:
    /// this is what lets the host wire the sandbox `mem.recall` reader bridge from
    /// the same object (and share the per-run scratch for retrieval correlation).
    fn as_epistemic_reader(self: Arc<Self>) -> Option<Arc<dyn EpistemicReader>> {
        Some(self)
    }
}

/// Rejects a confidence that is not a number in `[0.0, 1.0]`, and a confidence of
/// 1.0 from a probabilistic derivation method (invariant 7).
pub(super) fn validate_confidence(
    confidence: Option<f64>,
    method: Option<DerivationMethod>,
) -> Result<()> {
    let Some(c) = confidence else {
        return Ok(());
    };
    if !(0.0..=1.0).contains(&c) {
        return Err(EpistemicError::InvalidInput(format!(
            "confidence must be a number in [0.0, 1.0], got {c}"
        )));
    }
    if let Some(m) = method
        && m.probabilistic()
        && c >= 1.0
    {
        return Err(EpistemicError::InvalidInput(format!(
            "a '{}' belief must carry confidence below 1.0 (invariant 7)",
            m.as_str()
        )));
    }
    Ok(())
}
