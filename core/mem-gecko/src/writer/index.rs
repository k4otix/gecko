//! The semantic-index side of the writer: the dirty set, rebuild-from-graph, and
//! the best-effort post-commit index hooks (invariant 8).

use std::collections::HashSet;

use gecko_extension_api::{
    ActorId, BeliefDraft, BeliefState, ConceptId, FilterMeta, MemId, RunContext, Visibility,
};
use serde_json::Value;

use super::{MemWriter, Result, dt_field, str_field};
use crate::tql;

/// What a best-effort vector removal did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexOutcome {
    /// The vector was removed from the index.
    Removed,
    /// The removal failed and the id was marked dirty for background repair.
    MarkedDirty,
    /// No embedder or index is configured, so there was nothing to remove.
    NoIndex,
}

impl MemWriter {
    // ── Dirty-set (invariant 8: the accelerator can never gate truth) ────────

    /// Records `id` as needing background reindex. Called on any best-effort
    /// embed/upsert/remove failure — never propagated to the caller.
    pub fn mark_dirty(&self, id: ConceptId) {
        self.dirty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id);
    }

    /// Snapshot of the current dirty set (for the background reindexer / tests).
    pub fn dirty_snapshot(&self) -> HashSet<ConceptId> {
        self.dirty.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Whether `id` is currently marked dirty.
    pub fn is_dirty(&self, id: &ConceptId) -> bool {
        self.dirty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(id)
    }

    /// Number of concept-ids awaiting reindex.
    pub fn dirty_len(&self) -> usize {
        self.dirty.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    // ── Rebuild-from-graph (cold start / version bump / corruption) ──────────

    /// Enumerates the graph's current-state embeddables — asserted beliefs and
    /// (non-retrieval-event) episodes — as `(id, embeddable text, FilterMeta)`,
    /// sorted by concept-id for deterministic replay. The graph is the SoR; the
    /// index is fully reconstructible from this (invariant 8).
    pub async fn enumerate_current_state_embeddables(
        &self,
    ) -> Result<Vec<(ConceptId, String, FilterMeta)>> {
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
            // Episodes have no visibility/belief-state of their own; they were
            // indexed as private/asserted (see `observe`), so replay them the same.
            if let Some(e) = row_to_embeddable(&d, Visibility::Private) {
                out.push(e);
            }
        }
        out.sort_by(|a, b| a.0.0.cmp(&b.0.0));
        Ok(out)
    }

    /// Rebuilds the index from the graph when it is missing/corrupt or its model-id
    /// differs from the embedder's, then drains the dirty set. A no-op when
    /// no index/embedder is configured. Call once at startup before serving recalls.
    pub async fn rebuild_index_from_graph(&self) -> Result<()> {
        let (Some(emb), Some(idx)) = (&self.embedder, &self.index) else {
            return Ok(());
        };
        if idx.model_id() != emb.model_id() || idx.is_empty() {
            idx.reset();
            // Embed the whole current-state batch, then persist it in a single
            // `upsert_many` (one persist for the batch, not one per item).
            let mut items: Vec<(ConceptId, Vec<f32>, FilterMeta)> = Vec::new();
            for (id, text, meta) in self.enumerate_current_state_embeddables().await? {
                match emb.embed_document(&text) {
                    Ok(v) => items.push((id, v, meta)),
                    Err(_) => self.mark_dirty(id),
                }
            }
            let queued: Vec<ConceptId> = items.iter().map(|(id, _, _)| id.clone()).collect();
            if idx.upsert_many(items).is_err() {
                // Batch persist failed: mark every embedded id dirty for repair.
                for id in queued {
                    self.mark_dirty(id);
                }
            }
        }
        self.drain_dirty_set().await?;
        Ok(())
    }

    /// Repairs the dirty set: for each dirty concept-id, re-fetch its current-state
    /// text + meta and re-upsert (clearing it on success). An id that has left
    /// current-state (superseded/removed) is dropped from the index and the set.
    pub async fn drain_dirty_set(&self) -> Result<()> {
        let (Some(emb), Some(idx)) = (&self.embedder, &self.index) else {
            return Ok(());
        };
        // Re-embed each dirty id, batching the successful ones into a single
        // `upsert_many`. Ids that have left current-state are dropped immediately;
        // ids that fail to embed stay dirty.
        let mut items: Vec<(ConceptId, Vec<f32>, FilterMeta)> = Vec::new();
        let mut queued: Vec<ConceptId> = Vec::new();
        for cid in self.dirty_snapshot() {
            let (q, v, r) = tql::fetch_belief_embeddable(&cid);
            let rows = self.graph.read(&q, &v, &r).await?;
            match rows
                .first()
                .and_then(|d| row_to_embeddable(d, Visibility::Private))
            {
                Some((id, text, meta)) => {
                    if let Ok(vec) = emb.embed_document(&text) {
                        items.push((id, vec, meta));
                        queued.push(cid);
                    }
                    // Embed failure: leave the id dirty for a later drain.
                }
                None => {
                    // Left current-state: drop from the accelerator and clear.
                    let _ = idx.remove(cid.clone());
                    self.clear_dirty(&cid);
                }
            }
        }
        if !items.is_empty() && idx.upsert_many(items).is_ok() {
            // Clear only the ids the batch persisted; on error keep them dirty.
            for cid in queued {
                self.clear_dirty(&cid);
            }
        }
        Ok(())
    }

    /// Removes `id` from the dirty set (a successful repair / drop).
    fn clear_dirty(&self, id: &ConceptId) {
        self.dirty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }

    // ── Best-effort index hooks (post-commit; never fail the graph write) ────

    pub(super) fn best_effort_index(&self, id: &MemId, text: &str, meta: FilterMeta) {
        let (Some(emb), Some(idx)) = (&self.embedder, &self.index) else {
            return; // index disabled: skip embed/upsert entirely.
        };
        let cid = ConceptId(id.0.clone());
        match emb.embed_document(text) {
            Ok(v) => {
                if idx.upsert(cid.clone(), &v, meta).is_err() {
                    self.mark_dirty(cid);
                }
            }
            Err(_) => self.mark_dirty(cid),
        }
    }

    /// Best-effort vector removal via the [`SemanticIndex::remove`] hook.
    /// Gated on BOTH embedder+index present — a vector is upserted only when both are
    /// (see [`best_effort_index`](Self::best_effort_index)), so a remove is
    /// attempted exactly when an upsert would have happened. A failure marks the id
    /// dirty for background repair and **never** fails the caller (invariant 8: the
    /// graph is the SoR; the index is rebuildable; no new index API).
    pub(super) fn best_effort_remove(&self, id: ConceptId) -> IndexOutcome {
        let (true, Some(idx)) = (self.embedder.is_some(), &self.index) else {
            return IndexOutcome::NoIndex;
        };
        if idx.remove(id.clone()).is_err() {
            self.mark_dirty(id);
            return IndexOutcome::MarkedDirty;
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

/// Parses a rebuild/repair fetch row into `(ConceptId, text, FilterMeta)`. The
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
