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

/// What a best-effort vector removal did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexOutcome {
    /// The vector was removed from the index.
    Removed,
    /// The removal failed; the next startup reconciliation removes it.
    Failed,
    /// No embedder or index is configured, so there was nothing to remove.
    NoIndex,
}

/// What a startup reconciliation changed in the index.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexReconciliation {
    /// Vectors embedded and added for graph items the index lacked.
    pub added: usize,
    /// Vectors removed for ids the graph no longer holds.
    pub removed: usize,
    /// Whether the index was rebuilt from scratch (a model change, or an empty
    /// self-managed index).
    pub full_rebuild: bool,
}

/// One embeddable graph item: its id, text and pre-filter.
type Embeddable = (ConceptId, String, FilterMeta);
/// One embedded item, ready for `upsert_many`.
type Embedded = (ConceptId, Vec<f32>, FilterMeta);

impl MemWriter {
    /// Enumerates the graph's current-state embeddables — asserted beliefs and
    /// (non-retrieval-event, non-tombstoned) episodes — as `(id, text, FilterMeta)`,
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
    /// embedded, so matching id sets embed nothing. Index failures only warn: the
    /// graph read is the one error this returns. A no-op without embedder and index.
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
        let missing = match held {
            Some(held) => {
                let current: HashSet<&ConceptId> = graph.iter().map(|(id, _, _)| id).collect();
                let stale: Vec<ConceptId> = held
                    .iter()
                    .filter(|id| !current.contains(id))
                    .cloned()
                    .collect();
                // Remove first, so a later embed or upsert failure never leaves
                // another graph's vectors behind.
                if !stale.is_empty() {
                    let n = stale.len();
                    match idx.remove_many(stale) {
                        Ok(()) => report.removed = n,
                        Err(e) => warn!(error = %e, "semantic index: stale-vector removal failed"),
                    }
                }
                graph
                    .into_iter()
                    .filter(|(id, _, _)| !held.contains(id))
                    .collect()
            }
            None if idx.model_id() != emb.model_id() || idx.is_empty() => {
                idx.reset();
                report.full_rebuild = true;
                graph
            }
            None => return Ok(report),
        };

        let items = embed_batch(emb.clone(), missing).await;
        if !items.is_empty() {
            let n = items.len();
            match idx.upsert_many(items) {
                Ok(()) => report.added = n,
                Err(e) => warn!(error = %e, "semantic index: batch upsert failed"),
            }
        }
        Ok(report)
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
    pub(super) fn best_effort_remove(&self, id: ConceptId) -> IndexOutcome {
        let (true, Some(idx)) = (self.embedder.is_some(), &self.index) else {
            return IndexOutcome::NoIndex;
        };
        if let Err(e) = idx.remove(id.clone()) {
            warn!(id = %id.0, error = %e, "semantic index: remove failed");
            return IndexOutcome::Failed;
        }
        IndexOutcome::Removed
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
