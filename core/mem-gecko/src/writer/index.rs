//! The semantic-index side of the writer: startup reconciliation with the graph and
//! the best-effort post-commit index hooks (invariant 8).

use std::collections::HashSet;
use std::sync::Arc;

use gecko_extension_api::{
    ActorId, BeliefDraft, BeliefState, ConceptId, Embedder, FilterMeta, MemId, RunContext,
    Visibility,
};
use serde_json::Value;
use tracing::warn;

use super::{MemWriter, Result, dt_field, str_field};
use crate::tql;

/// What a startup reconciliation changed in the index.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexReconciliation {
    /// Ids the index holds afterwards that it did not hold before.
    pub added: usize,
    /// Ids the index held before that it no longer holds.
    pub removed: usize,
}

/// One embeddable graph item: its id, text and pre-filter.
type Embeddable = (ConceptId, String, FilterMeta);
/// One embedded item, ready for `upsert_many`.
type Embedded = (ConceptId, Vec<f32>, FilterMeta);

impl MemWriter {
    /// Enumerates the graph's current-state embeddables — asserted beliefs and
    /// episodes other than retrieval-events — as `(id, text, FilterMeta)`,
    /// sorted and de-duplicated by concept-id. The graph is the source of truth; the
    /// index is reconstructible from this (invariant 8).
    pub async fn enumerate_current_state_embeddables(&self) -> Result<Vec<Embeddable>> {
        let mut out = Vec::new();
        for d in self
            .graph
            .read(tql::enumerate_beliefs_read(), &[], &[])
            .await?
        {
            if let Some(e) = row_to_embeddable(&d, Visibility::Private) {
                out.push(e);
            }
        }
        for d in self
            .graph
            .read(tql::enumerate_episodes_read(), &[], &[])
            .await?
        {
            // Episodes have no visibility of their own; `observe` indexes them as
            // private, so enumerate them the same way.
            if let Some(e) = row_to_embeddable(&d, Visibility::Private) {
                out.push(e);
            }
        }
        out.sort_by(|a, b| a.0.0.cmp(&b.0.0));
        out.dedup_by(|a, b| a.0 == b.0);
        Ok(out)
    }

    /// Brings the index in step with the graph's current state; call once at startup
    /// before serving recalls. A model change (or an empty self-managed index)
    /// rebuilds from scratch; otherwise stale ids are removed and missing ones
    /// embedded, so matching id sets embed nothing. The report counts what the index
    /// holds afterwards, in memory, whether or not saving it succeeded. Index
    /// failures only warn: the graph read is the one error this returns. A no-op
    /// without embedder and index.
    pub async fn reconcile_index(&self) -> Result<IndexReconciliation> {
        let (Some(emb), Some(idx)) = (&self.embedder, &self.index) else {
            return Ok(IndexReconciliation::default());
        };
        let graph = self.enumerate_current_state_embeddables().await?;
        let mut report = IndexReconciliation::default();

        let held = if idx.model_id() == emb.model_id() {
            idx.ids()
        } else {
            None
        };
        match held {
            Some(held) => {
                let current: HashSet<&ConceptId> = graph.iter().map(|(id, _, _)| id).collect();
                let stale: Vec<ConceptId> = held
                    .iter()
                    .filter(|id| !current.contains(id))
                    .cloned()
                    .collect();
                let missing: Vec<Embeddable> = graph
                    .into_iter()
                    .filter(|(id, _, _)| !held.contains(id))
                    .collect();
                if stale.is_empty() && missing.is_empty() {
                    return Ok(report);
                }
                // Remove first, so a later embed or upsert failure never leaves
                // another graph's vectors behind.
                if !stale.is_empty() {
                    let index = idx.clone();
                    on_blocking_pool("stale-vector removal", move || index.remove_many(stale))
                        .await;
                }
                self.add_embedded(emb, idx, missing).await;
                let after = idx.ids().unwrap_or_default();
                report.removed = held.difference(&after).count();
                report.added = after.difference(&held).count();
            }
            None if idx.model_id() != emb.model_id() || idx.is_empty() => {
                report.removed = idx.len();
                let index = idx.clone();
                on_blocking_pool("index reset", move || {
                    index.reset();
                    Ok(())
                })
                .await;
                self.add_embedded(emb, idx, graph).await;
                report.added = idx.len();
            }
            None => {}
        }
        Ok(report)
    }

    /// Embeds `items` and adds them in one batch, both on the blocking pool.
    /// Failures only warn.
    async fn add_embedded(
        &self,
        emb: &Arc<dyn Embedder>,
        idx: &Arc<dyn gecko_extension_api::SemanticIndex>,
        items: Vec<Embeddable>,
    ) {
        let embedded = embed_batch(emb.clone(), items).await;
        if !embedded.is_empty() {
            let index = idx.clone();
            on_blocking_pool("batch upsert", move || index.upsert_many(embedded)).await;
        }
    }

    // ── Best-effort index hooks (post-commit; never fail the graph write) ────

    /// Embeds and upserts one committed item. A failure warns and never fails the
    /// caller; the next startup reconciliation indexes it.
    pub(super) fn best_effort_index(&self, id: &MemId, text: &str, meta: FilterMeta) {
        let (Some(emb), Some(idx)) = (&self.embedder, &self.index) else {
            return; // index disabled: skip embed/upsert entirely.
        };
        let result = emb
            .embed_document(text)
            .and_then(|v| idx.upsert(ConceptId(id.0.clone()), &v, meta));
        if let Err(e) = result {
            warn!(id = %id.0, error = %e, "semantic index: upsert failed");
        }
    }

    /// Best-effort vector removal via the [`SemanticIndex::remove`] hook.
    /// Gated on BOTH embedder+index present — a vector is upserted only when both are
    /// (see [`best_effort_index`](Self::best_effort_index)), so a remove is
    /// attempted exactly when an upsert would have happened. A failure warns and
    /// **never** fails the caller; the next startup reconciliation removes the id
    /// (invariant 8: the graph is the source of truth; the index is rebuildable).
    pub(super) fn best_effort_remove(&self, id: ConceptId) {
        let (true, Some(idx)) = (self.embedder.is_some(), &self.index) else {
            return;
        };
        if let Err(e) = idx.remove(id.clone()) {
            warn!(id = %id.0, error = %e, "semantic index: remove failed");
        }
    }

    pub(super) fn belief_meta(
        &self,
        b: &BeliefDraft,
        ctx: &RunContext,
        state: BeliefState,
    ) -> FilterMeta {
        FilterMeta {
            owner: b.owner.clone(),
            visibility: b.visibility,
            belief_state: state,
            valid_from: ctx.occurred_at,
        }
    }
}

/// Parses an enumerate row into `(ConceptId, text, FilterMeta)`. The
/// `belief-state` is treated as asserted (the enumerate queries already restrict to
/// current-state), and `visibility` falls back to `default_vis` when absent (e.g.
/// episodes). Returns `None` when the mandatory id/text are missing.
fn row_to_embeddable(
    d: &Value,
    default_vis: Visibility,
) -> Option<(ConceptId, String, FilterMeta)> {
    let id = str_field(d, "id");
    if id.is_empty() {
        return None;
    }
    let text = str_field(d, "text");
    let visibility = d
        .get("vis")
        .and_then(|v| v.as_str())
        .and_then(Visibility::from_str)
        .unwrap_or(default_vis);
    let meta = FilterMeta {
        owner: ActorId::new(str_field(d, "owner")),
        visibility,
        belief_state: BeliefState::Asserted,
        valid_from: dt_field(d, "vf").unwrap_or_else(chrono::Utc::now),
    };
    Some((ConceptId(id), text, meta))
}

/// Embeds `items` on the blocking pool (embedding is CPU-heavy). An item whose embed
/// fails is skipped with a warning; reconciliation retries it next startup.
async fn embed_batch(emb: Arc<dyn Embedder>, items: Vec<Embeddable>) -> Vec<Embedded> {
    if items.is_empty() {
        return Vec::new();
    }
    let joined = tokio::task::spawn_blocking(move || {
        items
            .into_iter()
            .filter_map(|(id, text, meta)| match emb.embed_document(&text) {
                Ok(v) => Some((id, v, meta)),
                Err(e) => {
                    warn!(id = %id.0, error = %e, "semantic index: embed failed");
                    None
                }
            })
            .collect()
    })
    .await;
    joined.unwrap_or_else(|e| {
        warn!(error = %e, "semantic index: embedding task failed");
        Vec::new()
    })
}

/// Runs one index operation on the blocking pool (it may rebuild the ANN graph and
/// write the snapshot file). A failure, or a panic in the task, warns.
async fn on_blocking_pool(what: &'static str, op: impl FnOnce() -> Result<()> + Send + 'static) {
    match tokio::task::spawn_blocking(op).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!(error = %e, "semantic index: {what} failed"),
        Err(e) => warn!(error = %e, "semantic index: {what} task failed"),
    }
}
