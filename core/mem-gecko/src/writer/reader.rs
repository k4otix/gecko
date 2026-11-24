//! Reads: Rust callers for the persisted substrate functions, the recall paths,
//! and the [`EpistemicReader`] implementation. The recency/ACT-R decay TypeQL
//! cannot express is layered on here.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream::{self, StreamExt, TryStreamExt};
use gecko_extension_api::{
    ActorId, BeliefState, Chunk, ConceptId, ContextBudget, DateTime, Embedder, EpistemicError,
    EpistemicReader, FilterMeta, MemId, RecallQuery, RowCountMismatch, RunContext, SemanticIndex,
    Visibility,
};
use serde_json::Value;

use super::{MemWriter, Result, dedup_mapped, dt_field, f64_field, mint, str_field};
use crate::tql;

/// Belief half-life for the recency-decay layer (one week). The decay layer applies
/// this single substrate-wide half-life to every item; per-item half-lives are not
/// currently read.
pub(super) const DEFAULT_HALF_LIFE_HOURS: f64 = 168.0;

/// Bounds how many gated-candidate fetches `recall` runs concurrently.
const RECALL_FETCH_CONCURRENCY: usize = 8;

/// Clamps an index similarity to the cosine range `[-1, 1]` the schema's
/// `similarity-score` allows; the index's f32 `1 - distance` can land just outside.
pub(super) fn clamp_similarity(similarity: f32) -> f32 {
    similarity.clamp(-1.0, 1.0)
}

impl MemWriter {
    // ── Persisted-function readers ───────────────────────────────────────────

    /// `is-superseded($b)` — supersession lineage OR a retracted/superseded state.
    pub async fn is_superseded(&self, b: &MemId) -> Result<bool> {
        let (q, v, r) = tql::is_superseded_read(b);
        Ok(!self.graph.read(&q, &v, &r).await?.is_empty())
    }

    /// `gate($n,$agent,$now)` — supersession · validity · scope, all at once. Every
    /// retrieved candidate passes through this.
    pub async fn gate(&self, n: &MemId, agent: &ActorId, now: DateTime) -> Result<bool> {
        let (q, v, r) = tql::gate_read(n, agent, now);
        Ok(!self.graph.read(&q, &v, &r).await?.is_empty())
    }

    /// `contradicts($b1,$b2)` — the two beliefs share a `contradiction` hub.
    pub async fn contradicts(&self, b1: &MemId, b2: &MemId) -> Result<bool> {
        let (q, v, r) = tql::contradicts_read(b1, b2);
        Ok(!self.graph.read(&q, &v, &r).await?.is_empty())
    }

    /// `retrieval-score($m,$now)` with the **Rust-side recency/ACT-R decay layered
    /// on top** (TQL cannot express continuous half-life decay — see the function's
    /// header in `mem_functions.tql`). Returns `None` when the item lacks the decay
    /// attributes (base-activation/salience/last-access).
    pub async fn retrieval_score(&self, m: &MemId, now: DateTime) -> Result<Option<f64>> {
        let (q, v, r) = tql::retrieval_score_read(m, now);
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(docs
            .first()
            .and_then(|d| f64_field(d, "s").map(|base| base * decay_of(d, now))))
    }

    /// `select-for-context($agent,$now)` — the gated + scored belief stream, with
    /// the Rust recency decay applied (the non-vector recall fallback source).
    pub async fn select_for_context(
        &self,
        agent: &ActorId,
        now: DateTime,
    ) -> Result<Vec<(MemId, f64)>> {
        let (q, v, r) = tql::select_for_context_read(agent, now);
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(docs
            .iter()
            .map(|d| {
                let base = f64_field(d, "score").unwrap_or(0.0);
                (MemId::new(str_field(d, "id")), base * decay_of(d, now))
            })
            .collect())
    }

    /// `population-members($pop)` — reads the materialized `population-member`
    /// view for the population identified by `spec_hash` (its `@key`).
    pub async fn population_members(&self, spec_hash: &str) -> Result<Vec<ConceptId>> {
        let (q, v, r) = tql::population_members_read(spec_hash);
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(dedup_mapped(&docs, ConceptId))
    }

    /// `canonical-entity($rec)` — the transitive closure over **provable**
    /// resolutions (probabilistic ones are never collapsed).
    pub async fn canonical_entity(&self, rec: &ConceptId) -> Result<Vec<ConceptId>> {
        let (q, v, r) = tql::canonical_entity_read(rec);
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(dedup_mapped(&docs, ConceptId))
    }

    /// `retrieval-provenance-of($b)` — walks `informs-synthesis` (the retrieval
    /// ledger). This is the OTHER ledger: it is **not** reachable from
    /// `derivation-chain` (invariant 8 — two ledgers, never crossed).
    pub async fn retrieval_provenance_of(&self, b: &MemId) -> Result<Vec<MemId>> {
        let (q, v, r) = tql::retrieval_provenance_of_read(b);
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(dedup_mapped(&docs, MemId))
    }

    // ── Retrieval-provenance write path ──────────────────────────────────────

    /// Mints a stamped `retrieval-event` for a semantic recall, creates a `surfaced`
    /// edge (with similarity score, `was-used=false`) per gated candidate, and
    /// records `(event, gated candidate ids)` on the run's scratch so a later
    /// `assert_belief` in the same run can correlate by evidence overlap.
    async fn record_retrieval(&self, ctx: &RunContext, gated: &[(Chunk, f64)]) -> Result<()> {
        let ev_id = mint("mem/ep");
        let surfaced: Vec<(ConceptId, f32)> = gated
            .iter()
            .map(|(c, _)| (ConceptId(c.id.0.clone()), c.score))
            .collect();
        self.graph
            .write(&tql::retrieval_event_ops(
                ctx,
                &ev_id,
                "semantic",
                surfaced.len() as i64,
                &surfaced,
            ))
            .await
            .map_err(|e| match (&e, RowCountMismatch::from_error(&e)) {
                (EpistemicError::Storage(msg), Some(m)) => EpistemicError::Storage(format!(
                    "retrieval-event {} matched {} row(s): one of its candidates no longer exists; nothing was committed: {msg}",
                    ev_id.0, m.got
                )),
                _ => e,
            })?;
        let candidates: HashSet<ConceptId> = surfaced.into_iter().map(|(c, _)| c).collect();
        ctx.push_retrieval(ev_id, candidates);
        Ok(())
    }

    // ── Recall paths (invariant 8) ───────────────────────────────────────────

    /// As-of-time-T recall: the beliefs `believed-at` `at` that are `visible-to` the
    /// run's actor, newest first and truncated to budget. It **never touches the
    /// index** (present-state only).
    async fn recall_as_of(
        &self,
        ctx: &RunContext,
        at: DateTime,
        budget: ContextBudget,
    ) -> Result<Vec<Chunk>> {
        let (q, v, r) = tql::believed_at_chunks_read(at, &ctx.actor);
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(docs
            .iter()
            .take(budget.max_chunks)
            .map(|d| Chunk {
                id: MemId::new(str_field(d, "id")),
                text: str_field(d, "title"),
                score: 0.0,
            })
            .collect())
    }

    /// Fetches a single gated candidate authoritatively. Returns `None` when the
    /// candidate does NOT pass `gate` (superseded · invalid · out-of-scope) — this
    /// is where an ANN-surfaced-but-gateable id is dropped.
    async fn fetch_gated(
        &self,
        cid: &ConceptId,
        actor: &ActorId,
        now: DateTime,
    ) -> Result<Option<(String, f64)>> {
        let (q, v, r) = tql::gated_candidate_read(cid, actor, now);
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(docs.first().map(|d| {
            let rank = f64_field(d, "act").unwrap_or(0.0) * decay_of(d, now);
            (str_field(d, "title"), rank)
        }))
    }

    /// Index-seeded present-state recall: the ANN generates candidates, then every
    /// candidate is fetched authoritatively + gated (invariant 8, the crux). The
    /// gated fetches run concurrently (bounded by [`RECALL_FETCH_CONCURRENCY`]); the
    /// hits are collected, then sorted by score descending and truncated to budget.
    async fn recall_indexed(
        &self,
        ctx: &RunContext,
        emb: &Arc<dyn Embedder>,
        idx: &Arc<dyn SemanticIndex>,
        q: &RecallQuery,
        budget: ContextBudget,
    ) -> Result<Vec<Chunk>> {
        let now = ctx.occurred_at;
        let qv = emb.embed_query(&q.text)?; // BGE query-prefix path
        let pre = FilterMeta {
            owner: ctx.actor.clone(),
            visibility: Visibility::Private,
            belief_state: BeliefState::Asserted,
            valid_from: now,
        };
        // Over-fetch (k*4) so gating has candidates to survive on.
        let k = budget.max_chunks.max(1).saturating_mul(4);
        let cand = idx.query(&qv, k, &pre)?; // ids + similarity scores

        // AUTHORITATIVE fetch + gate per candidate, run concurrently. An ANN-surfaced
        // but gateable id yields no row and is dropped. A fetch error aborts recall,
        // exactly as the sequential path did.
        let fetched: Vec<Option<(Chunk, f64)>> = stream::iter(cand)
            .map(|(cid, sim)| async move {
                let gated = self.fetch_gated(&cid, &ctx.actor, now).await?;
                Ok::<_, EpistemicError>(gated.map(|(title, rank)| {
                    (
                        Chunk {
                            id: MemId(cid.0),
                            text: title,
                            score: clamp_similarity(sim),
                        },
                        rank,
                    )
                }))
            })
            .buffer_unordered(RECALL_FETCH_CONCURRENCY)
            .try_collect()
            .await?;
        let mut scored: Vec<(Chunk, f64)> = fetched.into_iter().flatten().collect();

        // Record the retrieval as episodic provenance (authoritative, stamped) —
        // DISTINCT from the best-effort index upsert. Gated by config; off ⇒ no
        // retrieval-events, no flags (no-drag preserved).
        if self.record_retrieval_provenance && !scored.is_empty() {
            self.record_retrieval(ctx, &scored).await?;
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored
            .into_iter()
            .take(budget.max_chunks)
            .map(|(c, _)| c)
            .collect())
    }

    /// Non-vector present-state recall (no index / NoopIndex): recent + salient +
    /// scoped beliefs via `select-for-context`, sorted by score descending.
    async fn recall_fallback(&self, ctx: &RunContext, budget: ContextBudget) -> Result<Vec<Chunk>> {
        let now = ctx.occurred_at;
        let mut scored = self.select_for_context(&ctx.actor, now).await?;
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored
            .into_iter()
            .take(budget.max_chunks)
            .map(|(id, _)| Chunk {
                id,
                text: String::new(),
                score: 0.0,
            })
            .collect())
    }
}

#[async_trait]
impl EpistemicReader for MemWriter {
    async fn recall(
        &self,
        ctx: &RunContext,
        q: RecallQuery,
        budget: ContextBudget,
    ) -> Result<Vec<Chunk>> {
        // As-of-T ⇒ temporal path; SKIP the index entirely (invariant 8).
        if let Some(t) = q.as_of {
            return self.recall_as_of(ctx, t, budget).await;
        }
        // Present-state: index-seeded when configured, else non-vector fallback.
        match (&self.embedder, &self.index) {
            (Some(emb), Some(idx)) => self.recall_indexed(ctx, emb, idx, &q, budget).await,
            _ => self.recall_fallback(ctx, budget).await,
        }
    }

    async fn derivation_chain(&self, b: MemId) -> Result<Vec<MemId>> {
        // Walks `derivation` ONLY — never the retrieval ledger (invariant 8).
        let (q, v, r) = tql::derivation_chain_read(&b);
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(dedup_mapped(&docs, MemId))
    }

    async fn believed_at(&self, ctx: &RunContext, at: DateTime) -> Result<Vec<MemId>> {
        // Temporal path; the index is NEVER consulted (invariant 8).
        let (q, v, r) = tql::believed_at_chunks_read(at, &ctx.actor);
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(dedup_mapped(&docs, MemId))
    }

    async fn blast_radius(&self, retracted: MemId) -> Result<Vec<MemId>> {
        let (q, v, r) = tql::blast_radius_read(&retracted);
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(dedup_mapped(&docs, MemId))
    }
}

/// The ACT-R-style continuous recency multiplier `0.5^(age_hours / half_life)`.
/// Lives in Rust because TypeQL 3.12 cannot do duration→scalar / ln / pow.
pub(super) fn recency_decay(last_access: DateTime, now: DateTime, half_life_hours: f64) -> f64 {
    let age_secs = (now - last_access).num_seconds() as f64;
    if age_secs <= 0.0 {
        return 1.0;
    }
    0.5_f64.powf((age_secs / 3600.0) / half_life_hours)
}

/// Decay factor for a fetched row carrying `la` (last-access); `1.0` if absent.
fn decay_of(d: &Value, now: DateTime) -> f64 {
    match dt_field(d, "la") {
        Some(la) => recency_decay(la, now, DEFAULT_HALF_LIFE_HOURS),
        None => 1.0,
    }
}
