//! Consolidation / "dreaming" daemon.
//!
//! * [`ConsolidationDaemon`] — the trait describing the scheduled, whole-graph jobs
//!   a daemon runs. Every job takes the [`RunContext`] of the tick it belongs to, so
//!   all of a tick's writes share one run. Every job beyond the scheduled entry point
//!   is a documented stub (a signature + doc-comment whose default body returns
//!   [`EpistemicError::NotYetImplemented`] rather than panicking), so a premature
//!   caller gets a graceful `Err`, not a crash.
//! * [`NoopConsolidationDaemon`] — a daemon whose scheduled [`tick`] does nothing.
//! * [`MemConsolidationDaemon`] — the shipped daemon: its [`tick`] dedups duplicate
//!   episodes under one run. Episodes are duplicates when they share an owner set,
//!   title and event-time and are plain episodes (not retrieval-events or prediction
//!   resolutions); the earliest ingested survives and each other member is
//!   tombstoned through [`MemWriter::dedup_episodes`](crate::MemWriter::dedup_episodes),
//!   which re-checks the rule inside one guarded write and drops the loser's vector.
//!
//! ## Invariant 8 (index interaction) binds every job here
//! On tombstone/archive the daemon calls the best-effort
//! [`SemanticIndex::remove`](gecko_extension_api::SemanticIndex::remove) hook for the
//! affected concept-ids. On consolidation that mints a new merged belief, that belief
//! rides the normal `assert_belief` embed+upsert path. The graph is the sole source of
//! record; the index is derived and rebuildable; an index error never fails a graph op.
//!
//! [`tick`]: ConsolidationDaemon::tick

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use gecko_extension_api::{
    ActorId, DateTime, EpistemicError, MemId, ProvenanceSource, RunContext, RunId,
};

use crate::writer::{DuplicateCandidate, IndexOutcome, MemWriter};

type Result<T> = std::result::Result<T, EpistemicError>;

/// A counting summary of what a scheduled [`tick`](ConsolidationDaemon::tick) did.
/// The no-op tick returns the default (all zeroes); an implemented tick fills it
/// in so a scheduler/operator can observe consolidation progress.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsolidationReport {
    /// The run the tick's writes were stamped with; `None` when the tick wrote nothing.
    pub run_id: Option<RunId>,
    /// Duplicate episodes deduped (loser tombstoned, vector removal attempted).
    pub episodes_deduped: usize,
    /// Provable typed-join links discovered (`derivation-method: type-join`).
    pub links_discovered: usize,
    /// Beliefs moved one step along `raw → candidate → consolidated → archived →
    /// tombstoned` this tick.
    pub state_transitions: usize,
    /// Duplicate pairs the guarded write refused (the pair no longer satisfied the
    /// duplicate rule when written); each was skipped and nothing was written for it.
    pub pairs_refused: usize,
    /// Concept-ids whose vector removal the index reported as succeeded.
    pub vectors_removed: usize,
    /// Concept-ids whose vector removal failed; the next startup reconciliation
    /// removes them.
    pub vectors_failed: usize,
}

/// The scheduled, whole-graph consolidation ("dreaming") interface.
///
/// Only [`tick`](Self::tick) has a real (no-op) default. Every job takes the
/// [`RunContext`] of its tick. Every other method is a documented stub — a
/// signature whose default body returns
/// [`EpistemicError::NotYetImplemented`] (never panics) so a no-op impl compiles
/// without carrying dead logic and a premature caller gets a graceful `Err`. Return
/// types are counts/id-lists so the bodies stay pure data-plane operations over the
/// graph + the best-effort index hook (invariant 8).
#[async_trait]
pub trait ConsolidationDaemon: Send + Sync {
    /// The scheduled entry point — one "sleep cycle" over the whole graph. A daemon
    /// runner calls this on a timer. **Default: a complete no-op** that touches
    /// nothing and reports zeroes, so the seam is wired and callable today while the
    /// real jobs land later.
    async fn tick(&self, _now: DateTime) -> Result<ConsolidationReport> {
        Ok(ConsolidationReport::default())
    }

    // ── Whole-graph reasoning jobs ───────────────────────────────────────────

    /// **Provable** episode dedup: scan for cohorts of episodes sharing an owner
    /// set, title and event-time, keep the earliest ingested of each, and tombstone
    /// and de-vector the rest via
    /// [`MemWriter::dedup_episodes`](crate::MemWriter::dedup_episodes). Returns the
    /// number of losers deduped.
    async fn dedup_content_hash(&self, _ctx: &RunContext) -> Result<usize> {
        Err(EpistemicError::NotYetImplemented(
            "whole-graph duplicate-episode scan → dedup_episodes per pair: not yet implemented",
        ))
    }

    /// **Provable** link discovery: materialize `derivation`s whose method is a
    /// type-enforced join (`derivation-method: type-join`, confidence 1.0 permitted —
    /// invariant 7). Structurally distinct from [`synthesize_links_llm`](Self::synthesize_links_llm).
    async fn discover_typed_join_links(&self, _ctx: &RunContext) -> Result<usize> {
        Err(EpistemicError::NotYetImplemented(
            "typed-join link discovery (provable, type-join): not yet implemented",
        ))
    }

    /// **Probabilistic** link synthesis: propose links via LLM synthesis
    /// (`derivation-method: llm-synthesis`, confidence < 1.0, MARKED, structurally
    /// forbidden from superseding higher-entrenchment beliefs — invariant 7). Kept
    /// separate from the provable path so the two never blur.
    async fn synthesize_links_llm(&self, _ctx: &RunContext) -> Result<usize> {
        Err(EpistemicError::NotYetImplemented(
            "LLM link synthesis (probabilistic, marked llm-synthesis): not yet implemented",
        ))
    }

    /// Contradiction sweep: find belief pairs that should share a `contradiction`
    /// hub and reify the anomaly for the JTMS/argumentation layer to resolve.
    async fn contradiction_sweep(&self, _ctx: &RunContext) -> Result<usize> {
        Err(EpistemicError::NotYetImplemented(
            "whole-graph contradiction detection → anomaly hubs: not yet implemented",
        ))
    }

    /// Lifecycle advance: walk `consolidation-state` forward
    /// `raw → candidate → consolidated → archived → tombstoned` for decayed items
    /// (calling the best-effort index remove hook on archive/tombstone — invariant 8).
    async fn decay_archive_tombstone(&self, _ctx: &RunContext, _now: DateTime) -> Result<usize> {
        Err(EpistemicError::NotYetImplemented(
            "decay → archive → tombstone with best-effort de-vector: not yet implemented",
        ))
    }

    /// Summarize the provenance of archived beliefs (collapse detail into a summary
    /// while retaining the derivation/retrieval axes the retention rules protect).
    async fn summarize_archived_provenance(&self, _ctx: &RunContext) -> Result<usize> {
        Err(EpistemicError::NotYetImplemented(
            "summarize archived-belief provenance: not yet implemented",
        ))
    }

    // ── Tiering / materialization / retention hooks ──────────────────────────

    /// TTL-prune the record/episodic tier: tombstone episodic records past their
    /// retention window. MUST honour the retrieval-provenance retention rule (see
    /// [`retrieval_events_safe_to_tombstone`](crate::MemWriter::retrieval_events_safe_to_tombstone)):
    /// never tombstone a `retrieval-event` whose `informs-synthesis` belief is still
    /// non-superseded.
    async fn ttl_prune_record_tier(&self, _ctx: &RunContext, _now: DateTime) -> Result<usize> {
        Err(EpistemicError::NotYetImplemented(
            "TTL-prune record tier (honour retrieval-provenance retention): not yet implemented",
        ))
    }

    /// Roll up high-volume episodes into aggregate summaries (detail lost, axis
    /// retained), reducing the episodic tier's footprint.
    async fn rollup_high_volume_episodes(&self, _ctx: &RunContext) -> Result<usize> {
        Err(EpistemicError::NotYetImplemented(
            "roll up high-volume episodes to aggregates: not yet implemented",
        ))
    }

    /// Collapse a superseded belief's provenance chain into a summary once the belief
    /// is no longer current-state. The write-once `retrieval-provenance` flag on the
    /// belief persists (axis retained) even as episodic detail decays.
    async fn collapse_superseded_provenance(&self, _ctx: &RunContext) -> Result<usize> {
        Err(EpistemicError::NotYetImplemented(
            "collapse superseded-belief provenance to summaries: not yet implemented",
        ))
    }

    /// Precompute (materialize) `blast-radius` for a critical assumption so a later
    /// retraction is O(read). The materialization is a cache: the graph stays SoR.
    async fn precompute_blast_radius(&self, _ctx: &RunContext, _assumption: &MemId) -> Result<()> {
        Err(EpistemicError::NotYetImplemented(
            "precompute blast-radius for critical assumptions: not yet implemented",
        ))
    }

    /// Invalidate a precomputed `blast-radius` when a host-mediated write touches the
    /// assumption's neighbourhood (the cache is only ever as good as the last write).
    async fn invalidate_blast_radius(&self, _ctx: &RunContext, _assumption: &MemId) -> Result<()> {
        Err(EpistemicError::NotYetImplemented(
            "invalidate blast-radius on host-mediated write: not yet implemented",
        ))
    }
}

/// The wired no-op consolidation daemon: its scheduled [`tick`] does nothing and
/// reports zeroes (all other jobs inherit the trait's `NotYetImplemented` stubs and are
/// never invoked). It compiles, is callable, and does nothing.
///
/// [`tick`]: ConsolidationDaemon::tick
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopConsolidationDaemon;

impl ConsolidationDaemon for NoopConsolidationDaemon {
    // Inherits the trait default `tick` (a true no-op) and the `NotYetImplemented`
    // job stubs. Nothing to override.
}

/// The shipped consolidation daemon: each [`tick`](ConsolidationDaemon::tick) mints
/// one run for the `consolidation-daemon` actor and dedups every duplicate episode
/// cohort under it. A tick over a graph with no duplicates writes nothing.
pub struct MemConsolidationDaemon {
    writer: Arc<MemWriter>,
}

impl MemConsolidationDaemon {
    /// A daemon that consolidates through `writer`.
    pub fn new(writer: Arc<MemWriter>) -> Self {
        Self { writer }
    }

    /// Dedups every duplicate episode cohort under `ctx`, returning one index outcome
    /// per tombstoned loser and a count of pairs the guarded write refused (skipped, not
    /// fatal). A failure of the cohort read is returned as is (nothing was written); any
    /// other failure of a dedup write names `ctx`'s run, even when it was the tick's
    /// first write, whose run was rolled back.
    pub async fn dedup_episodes_in(&self, ctx: &RunContext) -> Result<DedupSummary> {
        let mut cohorts: BTreeMap<_, Vec<DuplicateCandidate>> = BTreeMap::new();
        for c in self.writer.duplicate_episode_candidates().await? {
            let key = (c.title.clone(), c.event_time, c.owners.clone());
            cohorts.entry(key).or_default().push(c);
        }
        let mut summary = DedupSummary::default();
        for mut members in cohorts.into_values() {
            members.sort_by(|a, b| (a.ingest_time, &a.id.0).cmp(&(b.ingest_time, &b.id.0)));
            let Some((keeper, losers)) = members.split_first() else {
                continue;
            };
            for loser in losers {
                match self.writer.dedup_episodes(ctx, &keeper.id, &loser.id).await {
                    Ok(outcome) => summary.outcomes.push(outcome),
                    Err(EpistemicError::InvalidInput(_)) => summary.pairs_refused += 1,
                    Err(e) => {
                        return Err(EpistemicError::Storage(format!(
                            "consolidation run {}: {e}",
                            ctx.run_id
                        )));
                    }
                }
            }
        }
        Ok(summary)
    }
}

/// What [`MemConsolidationDaemon::dedup_episodes_in`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DedupSummary {
    /// One index outcome per tombstoned loser.
    pub outcomes: Vec<IndexOutcome>,
    /// Pairs the guarded write refused as no longer duplicates.
    pub pairs_refused: usize,
}

#[async_trait]
impl ConsolidationDaemon for MemConsolidationDaemon {
    async fn tick(&self, now: DateTime) -> Result<ConsolidationReport> {
        let ctx = RunContext::new(
            ActorId::new("consolidation-daemon"),
            ProvenanceSource::Tool {
                name: "consolidation-daemon".into(),
            },
            now,
        );
        let DedupSummary {
            outcomes,
            pairs_refused,
        } = self.dedup_episodes_in(&ctx).await?;
        let count = |wanted: IndexOutcome| outcomes.iter().filter(|o| **o == wanted).count();
        Ok(ConsolidationReport {
            run_id: (!outcomes.is_empty()).then_some(ctx.run_id),
            episodes_deduped: outcomes.len(),
            pairs_refused,
            vectors_removed: count(IndexOutcome::Removed),
            vectors_failed: count(IndexOutcome::Failed),
            ..Default::default()
        })
    }

    async fn dedup_content_hash(&self, ctx: &RunContext) -> Result<usize> {
        Ok(self.dedup_episodes_in(ctx).await?.outcomes.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The no-op scheduled job is wired, callable, and does nothing (returns the
    /// zeroed report).
    #[tokio::test]
    async fn noop_daemon_tick_does_nothing() {
        let daemon = NoopConsolidationDaemon;
        let report = daemon
            .tick(chrono::Utc::now())
            .await
            .expect("no-op tick never fails");
        assert_eq!(
            report,
            ConsolidationReport::default(),
            "the scheduled no-op tick reports zero work"
        );
    }

    /// A premature call to an unimplemented stub returns a graceful
    /// [`EpistemicError::NotYetImplemented`] — NOT a panic (invariant: a reachable
    /// stub must never crash a caller).
    #[tokio::test]
    async fn build_later_stub_returns_not_yet_implemented_not_panic() {
        let daemon = NoopConsolidationDaemon;
        let ctx = RunContext::new(
            ActorId::new("test"),
            ProvenanceSource::Manual,
            chrono::Utc::now(),
        );
        let err = daemon
            .dedup_content_hash(&ctx)
            .await
            .expect_err("stub is not yet implemented");
        assert!(matches!(err, EpistemicError::NotYetImplemented(_)));
        let unit_err = daemon
            .precompute_blast_radius(&ctx, &MemId::new("mem/bel/x"))
            .await
            .expect_err("unit-returning stub is also graceful");
        assert!(matches!(unit_err, EpistemicError::NotYetImplemented(_)));
    }

    /// A `dyn ConsolidationDaemon` is object-safe and dispatchable — the future
    /// daemon runner will hold one behind a trait object.
    #[tokio::test]
    async fn daemon_is_object_safe_and_dispatchable() {
        let daemon: Box<dyn ConsolidationDaemon> = Box::new(NoopConsolidationDaemon);
        assert_eq!(
            daemon
                .tick(chrono::Utc::now())
                .await
                .unwrap()
                .episodes_deduped,
            0
        );
    }
}
