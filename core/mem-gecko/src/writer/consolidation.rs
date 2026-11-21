//! Consolidation operations on the graph ("dreaming"): provable episode dedup and
//! retrieval-event retention.

use gecko_extension_api::{ConceptId, EpistemicError, MemId, RowCountMismatch, RunContext};

use super::{IndexOutcome, MemWriter, Result, dedup_mapped};
use crate::tql;

/// The index of the guarded write in the ops [`tql::dedup_episodes_ops`] returns.
const DEDUP_GUARD_OP: usize = 2;

impl MemWriter {
    /// Collapses a duplicate episode pair: tombstones `loser` (its
    /// `consolidation-state` becomes "tombstoned"; the row stays queryable), records
    /// a `consolidation-step` stamped with `ctx`'s run and actor, then removes the
    /// loser's vector from the index on a best-effort basis (invariant 8: an index
    /// failure marks the id dirty and never fails the graph write).
    ///
    /// The duplicate rule is enforced inside the one guarded write: both are exactly
    /// `episode`s with the same title, event-time and owner set, neither resolves a
    /// prediction or is already tombstoned, and `keeper` ingested first (ties broken
    /// by lowest concept-id). Any other pair is refused with
    /// [`EpistemicError::InvalidInput`] and nothing is written.
    pub async fn dedup_episodes(
        &self,
        ctx: &RunContext,
        keeper: &MemId,
        loser: &MemId,
    ) -> Result<IndexOutcome> {
        if keeper.0 == loser.0 {
            return Err(EpistemicError::InvalidInput(format!(
                "dedup_episodes: keeper and loser are the same episode {}",
                keeper.0
            )));
        }
        self.graph
            .write(&tql::dedup_episodes_ops(ctx, keeper, loser))
            .await
            .map_err(|e| match (&e, RowCountMismatch::from_error(&e)) {
                (EpistemicError::Storage(_), Some(m)) if m.op == DEDUP_GUARD_OP => {
                    EpistemicError::InvalidInput(format!(
                        "dedup_episodes: {} is not a duplicate of keeper {}: the pair must be \
                         untombstoned episodes with the same title, event-time and owners, \
                         neither resolving a prediction, with the keeper ingested first",
                        loser.0, keeper.0
                    ))
                }
                _ => e,
            })?;
        Ok(self.best_effort_remove(ConceptId(loser.0.clone())))
    }

    /// Retrieval-provenance retention (concrete helper): the `retrieval-event`s
    /// whose `informs-synthesis` belief IS superseded — i.e. those safe to
    /// tombstone. Enforces the retention RULE: a retrieval-event is NEVER surfaced
    /// here while its synthesized belief is still non-superseded; only after the
    /// belief is superseded may the event decay. The belief's write-once
    /// `retrieval-provenance` flag persists regardless (the axis is retained on the
    /// belief, not the event). The `ttl_prune_record_tier` job reads
    /// this to know which episodic records it may prune.
    pub async fn retrieval_events_safe_to_tombstone(&self) -> Result<Vec<MemId>> {
        let docs = self
            .graph
            .read(tql::retrieval_events_safe_to_tombstone_read(), &[], &[])
            .await?;
        Ok(dedup_mapped(&docs, MemId))
    }
}
