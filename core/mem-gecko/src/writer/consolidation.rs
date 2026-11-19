//! Consolidation operations on the graph ("dreaming"): provable episode dedup and
//! retrieval-event retention.

use gecko_extension_api::{ConceptId, EpistemicError, MemId};

use super::{MemWriter, Result, dedup_mapped};
use crate::tql;

impl MemWriter {
    /// The one real consolidation operation — the seam proof. Given two episodes
    /// the daemon's content-hash scan found to be byte-identical, keep `keeper`,
    /// **tombstone** `loser` (its `consolidation-state` → "tombstoned"; the graph is
    /// the source of record, so the tombstoned row stays queryable), AND best-effort
    /// **remove** the loser's vector from the index via the
    /// [`SemanticIndex::remove`](gecko_extension_api::SemanticIndex::remove) hook (invariant 8: never fail the graph op on an
    /// index error; a failed remove marks the id dirty for background repair — no new
    /// index API).
    ///
    /// **Provable, not probabilistic:** it refuses to dedup unless `keeper` and
    /// `loser` genuinely share an identical content-hash, so it can never collapse two
    /// distinct episodes. Passing an episode as both keeper and loser, or two episodes
    /// with differing content, is rejected with [`EpistemicError::InvalidInput`].
    pub async fn dedup_episodes(&self, keeper: &MemId, loser: &MemId) -> Result<()> {
        if keeper.0 == loser.0 {
            return Err(EpistemicError::InvalidInput(
                "dedup_episodes: keeper and loser are the same episode".into(),
            ));
        }
        // Provable guard: both episodes exist AND share an identical content-hash.
        if !self.episodes_share_content(keeper, loser).await? {
            return Err(EpistemicError::InvalidInput(
                "dedup_episodes: episodes do not share an identical content-hash".into(),
            ));
        }
        // AUTHORITATIVE: tombstone the loser first (the graph is the SoR).
        self.graph
            .write(&[tql::tombstone_episode_op(loser)])
            .await?;
        // Best-effort: drop the loser's vector via the existing remove hook. A remove
        // failure only marks the id dirty — the dedup (graph op) already succeeded.
        self.best_effort_remove(ConceptId(loser.0.clone()));
        Ok(())
    }

    /// The provable-dedup guard read: true iff both episodes exist and share an
    /// identical content-hash (see [`tql::episodes_share_content_read`]).
    async fn episodes_share_content(&self, a: &MemId, b: &MemId) -> Result<bool> {
        let (q, v, r) = tql::episodes_share_content_read(a, b);
        Ok(!self.graph.read(&q, &v, &r).await?.is_empty())
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
