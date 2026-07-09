//! A6 — Consolidation / "dreaming" daemon **scaffold** (build later).
//!
//! This module lays the hooks so the retention/consolidation daemon can slot in
//! LATER without schema churn — it does **not** build the full daemon. What ships
//! today is:
//!
//! * [`ConsolidationDaemon`] — the trait describing the scheduled, whole-graph jobs
//!   the future daemon will run. Every job beyond the scheduled entry point is a
//!   **documented stub** (a signature + doc-comment with an `unimplemented!` default
//!   body) — the deliverable is the seam, not the body.
//! * [`NoopConsolidationDaemon`] — a concrete daemon whose scheduled [`tick`] does
//!   **nothing** today. It exists and is callable so the wiring is real now.
//!
//! The ONE real consolidation operation — content-hash episode dedup that also drops
//! the loser's vector — lives on [`MemWriter::dedup_episodes`](crate::MemWriter::dedup_episodes),
//! because it needs the writer's graph seam + best-effort index hook. The future
//! daemon's [`dedup_content_hash`](ConsolidationDaemon::dedup_content_hash) job will
//! scan the whole graph for identical-hash cohorts and call that method per pair.
//!
//! ## Invariant 8 (index interaction) binds every job here
//! On tombstone/archive the daemon calls the **existing** best-effort
//! [`SemanticIndex::remove`](gecko_extension_api::SemanticIndex::remove) hook for the
//! affected concept-ids — no new index API. On consolidation that mints a new merged
//! belief, that belief rides the normal `assert_belief` embed+upsert path. The graph
//! is the sole source of record; the index is derived and rebuildable; an index error
//! never fails a graph op.
//!
//! [`tick`]: ConsolidationDaemon::tick

use async_trait::async_trait;

use gecko_extension_api::{DateTime, EpistemicError, MemId};

type Result<T> = std::result::Result<T, EpistemicError>;

/// A counting summary of what a scheduled [`tick`](ConsolidationDaemon::tick) did.
/// Today's no-op tick returns the default (all zeroes); the future daemon fills it
/// in so a scheduler/operator can observe consolidation progress.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsolidationReport {
    /// Identical-content episodes deduped (loser tombstoned + vector removed).
    pub episodes_deduped: usize,
    /// Provable typed-join links discovered (`derivation-method: type-join`).
    pub links_discovered: usize,
    /// Beliefs moved one step along `raw → candidate → consolidated → archived →
    /// tombstoned` this tick.
    pub state_transitions: usize,
    /// Concept-ids whose vector was removed from the index (best-effort).
    pub vectors_removed: usize,
}

/// The scheduled, whole-graph consolidation ("dreaming") interface.
///
/// SCAFFOLD: only [`tick`](Self::tick) has a real (no-op) default. Every other
/// method is a documented **stub** — a signature the future daemon will implement,
/// with an `unimplemented!` default body so a no-op impl compiles without carrying
/// dead logic. Return types are counts/id-lists so the eventual bodies stay pure
/// data-plane operations over the graph + the best-effort index hook (invariant 8).
#[async_trait]
pub trait ConsolidationDaemon: Send + Sync {
    /// The scheduled entry point — one "sleep cycle" over the whole graph. A daemon
    /// runner calls this on a timer. **Default: a complete no-op** that touches
    /// nothing and reports zeroes, so the seam is wired and callable today while the
    /// real jobs land later.
    async fn tick(&self, _now: DateTime) -> Result<ConsolidationReport> {
        Ok(ConsolidationReport::default())
    }

    // ── Whole-graph reasoning jobs (build later) ─────────────────────────────

    /// **Provable** content-hash dedup: scan for cohorts of memory-items sharing an
    /// identical content-hash, keep one per cohort, and tombstone + de-vector the
    /// rest via [`MemWriter::dedup_episodes`](crate::MemWriter::dedup_episodes).
    /// Returns the number of losers deduped. (The per-pair operation is the ONE real
    /// body shipped in A6; this whole-graph scan is the build-later wrapper.)
    async fn dedup_content_hash(&self) -> Result<usize> {
        unimplemented!("A6 build-later: whole-graph identical-hash scan → dedup_episodes per pair")
    }

    /// **Provable** link discovery: materialize `derivation`s whose method is a
    /// type-enforced join (`derivation-method: type-join`, confidence 1.0 permitted —
    /// invariant 7). Structurally distinct from [`synthesize_links_llm`](Self::synthesize_links_llm).
    async fn discover_typed_join_links(&self) -> Result<usize> {
        unimplemented!("A6 build-later: typed-join link discovery (provable, type-join)")
    }

    /// **Probabilistic** link synthesis: propose links via LLM synthesis
    /// (`derivation-method: llm-synthesis`, confidence < 1.0, MARKED, structurally
    /// forbidden from superseding higher-entrenchment beliefs — invariant 7). Kept
    /// separate from the provable path so the two never blur.
    async fn synthesize_links_llm(&self) -> Result<usize> {
        unimplemented!("A6 build-later: LLM link synthesis (probabilistic, marked llm-synthesis)")
    }

    /// Contradiction sweep: find belief pairs that should share a `contradiction`
    /// hub and reify the anomaly for the JTMS/argumentation layer to resolve.
    async fn contradiction_sweep(&self) -> Result<usize> {
        unimplemented!("A6 build-later: whole-graph contradiction detection → anomaly hubs")
    }

    /// Lifecycle advance: walk `consolidation-state` forward
    /// `raw → candidate → consolidated → archived → tombstoned` for decayed items
    /// (calling the best-effort index remove hook on archive/tombstone — invariant 8).
    async fn decay_archive_tombstone(&self, _now: DateTime) -> Result<usize> {
        unimplemented!("A6 build-later: decay → archive → tombstone with best-effort de-vector")
    }

    /// Summarize the provenance of archived beliefs (collapse detail into a summary
    /// while retaining the derivation/retrieval axes the retention rules protect).
    async fn summarize_archived_provenance(&self) -> Result<usize> {
        unimplemented!("A6 build-later: summarize archived-belief provenance")
    }

    // ── Tiering / materialization / retention hooks (build later) ────────────

    /// TTL-prune the record/episodic tier: tombstone episodic records past their
    /// retention window. MUST honour the retrieval-provenance retention rule (see
    /// [`retrieval_events_safe_to_tombstone`](crate::MemWriter::retrieval_events_safe_to_tombstone)):
    /// never tombstone a `retrieval-event` whose `informs-synthesis` belief is still
    /// non-superseded.
    async fn ttl_prune_record_tier(&self, _now: DateTime) -> Result<usize> {
        unimplemented!(
            "A6 build-later: TTL-prune record tier (honour retrieval-provenance retention)"
        )
    }

    /// Roll up high-volume episodes into aggregate summaries (detail lost, axis
    /// retained), reducing the episodic tier's footprint.
    async fn rollup_high_volume_episodes(&self) -> Result<usize> {
        unimplemented!("A6 build-later: roll up high-volume episodes to aggregates")
    }

    /// Collapse a superseded belief's provenance chain into a summary once the belief
    /// is no longer current-state. The write-once `retrieval-provenance` flag on the
    /// belief persists (axis retained) even as episodic detail decays.
    async fn collapse_superseded_provenance(&self) -> Result<usize> {
        unimplemented!("A6 build-later: collapse superseded-belief provenance to summaries")
    }

    /// Precompute (materialize) `blast-radius` for a critical assumption so a later
    /// retraction is O(read). The materialization is a cache: the graph stays SoR.
    async fn precompute_blast_radius(&self, _assumption: &MemId) -> Result<()> {
        unimplemented!("A6 build-later: precompute blast-radius for critical assumptions")
    }

    /// Invalidate a precomputed `blast-radius` when a host-mediated write touches the
    /// assumption's neighbourhood (the cache is only ever as good as the last write).
    async fn invalidate_blast_radius(&self, _assumption: &MemId) -> Result<()> {
        unimplemented!("A6 build-later: invalidate blast-radius on host-mediated write")
    }
}

/// The wired **no-op** consolidation daemon: its scheduled [`tick`] does nothing and
/// reports zeroes (all other jobs inherit the trait's `unimplemented!` stubs and are
/// never invoked today). This is the "daemon trait + no-op scheduled job wired" A6
/// acceptance: it compiles, is callable, and does nothing.
///
/// [`tick`]: ConsolidationDaemon::tick
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopConsolidationDaemon;

impl ConsolidationDaemon for NoopConsolidationDaemon {
    // Inherits the trait default `tick` (a true no-op) and the `unimplemented!`
    // job stubs. Nothing to override until the real daemon lands.
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The no-op scheduled job is wired, callable, and does nothing (returns the
    /// zeroed report). This is the A6 "daemon trait + no-op scheduled job" acceptance.
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
