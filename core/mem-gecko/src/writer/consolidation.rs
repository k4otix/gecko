//! Consolidation operations on the graph ("dreaming"): provable episode dedup and
//! retrieval-event retention.

use std::collections::{BTreeMap, BTreeSet};

use gecko_extension_api::{
    ConceptId, DateTime, EpistemicError, MemId, RowCountMismatch, RunContext,
};

use super::{IndexOutcome, MemWriter, Result, dedup_mapped, parse_dt};
use crate::tql;

/// The index of the guarded write in the ops [`tql::dedup_episodes_ops`] returns.
const DEDUP_GUARD_OP: usize = 2;

/// The string field `key` of a cohort row; a missing or non-string field is an error.
fn required_str(d: &serde_json::Value, key: &str) -> Result<String> {
    d.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| EpistemicError::Storage(format!("cohort row lacks `{key}`: {d}")))
}

/// The datetime field `key` of a cohort row; a missing or unparseable field is an error.
fn required_dt(d: &serde_json::Value, key: &str) -> Result<DateTime> {
    parse_dt(&required_str(d, key)?)
        .ok_or_else(|| EpistemicError::Storage(format!("cohort row has a bad `{key}`: {d}")))
}

/// An episode that shares its title and event-time with at least one other episode,
/// with the ingest time and owner set the duplicate rule compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateCandidate {
    /// The episode's concept-id.
    pub id: MemId,
    /// The episode's title.
    pub title: String,
    /// When the observed event occurred.
    pub event_time: DateTime,
    /// When the episode entered the system.
    pub ingest_time: DateTime,
    /// The agent-ids that own the episode.
    pub owners: BTreeSet<String>,
}

impl MemWriter {
    /// The owned, untombstoned, non-prediction-resolving plain episodes that share a
    /// title and event-time with another such episode, one candidate per episode.
    pub async fn duplicate_episode_candidates(&self) -> Result<Vec<DuplicateCandidate>> {
        let docs = self
            .graph
            .read(tql::duplicate_episode_candidates_read(), &[], &[])
            .await?;
        let mut by_id: BTreeMap<String, DuplicateCandidate> = BTreeMap::new();
        for d in &docs {
            let id = required_str(d, "id")?;
            let owner = required_str(d, "owner")?;
            match by_id.get_mut(&id) {
                Some(c) => {
                    c.owners.insert(owner);
                }
                None => {
                    let candidate = DuplicateCandidate {
                        id: MemId(id.clone()),
                        title: required_str(d, "title")?,
                        event_time: required_dt(d, "et")?,
                        ingest_time: required_dt(d, "it")?,
                        owners: BTreeSet::from([owner]),
                    };
                    by_id.insert(id, candidate);
                }
            }
        }
        Ok(by_id.into_values().collect())
    }

    /// Collapses a duplicate episode pair: tombstones `loser` (its
    /// `consolidation-state` becomes "tombstoned"; the row stays queryable), records
    /// a `consolidation-step` stamped with `ctx`'s run and actor, then removes the
    /// loser's vector from the index on a best-effort basis (invariant 8: an index
    /// failure warns and never fails the graph write).
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
