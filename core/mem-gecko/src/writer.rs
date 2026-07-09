//! The mem-gecko [`EpistemicWriter`] + [`EpistemicReader`] implementation.
//!
//! This is the load-bearing substrate-reasoning piece. Writes are **real TypeDB
//! writes** through the neutral [`GraphStore`] seam (mem owns its TQL; the engine
//! owns the driver). The graph write is authoritative and commits *first*;
//! embedding + index upsert (and remove-on-supersede) is **best-effort after
//! commit** — any embed/upsert/remove failure calls [`MemWriter::mark_dirty`] and
//! **never** rolls back or fails the graph write (invariant 8).
//!
//! Reads invoke the persisted substrate functions (`is-superseded`, `believed-at`,
//! `derivation-chain`, `gate`, `retrieval-score`, …) from read transactions and
//! layer the recency/ACT-R decay that TypeQL cannot express **here, in the Rust
//! Reader**. [`recall`](EpistemicReader::recall) is index-seeded: the ANN only
//! *generates candidates*; every candidate is fetched authoritatively and **gated**
//! so a superseded/out-of-scope id can never leak (invariant 8, the crux).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gecko_extension_api::{
    ActorId, AnomalyId, BeliefDraft, BeliefQuery, BeliefState, Chunk, ConceptId, ContextBudget,
    DateTime, DerivationMethod, Embedder, EpisodeDraft, EpistemicError, EpistemicReader,
    EpistemicWriter, FilterMeta, GraphStore, MemId, Outcome, RecallQuery, RunContext,
    SemanticIndex, Visibility,
};
use serde_json::Value;

use crate::tql::{self, Params};

type Result<T> = std::result::Result<T, EpistemicError>;

/// Default belief half-life for the recency-decay layer (one week). Episodes carry
/// their own `half-life-hours`; beliefs use this substrate default.
const DEFAULT_HALF_LIFE_HOURS: f64 = 168.0;

/// Mints a fresh mem-id under `prefix` (e.g. `mem/ep`, `mem/bel`, `mem/anom`).
fn mint(prefix: &str) -> MemId {
    MemId(format!("{prefix}/{}", ulid::Ulid::new()))
}

/// mem-gecko's epistemic writer + reader.
///
/// Holds the [`GraphStore`] (the sole production coupling to a graph backend) plus
/// an optional [`Embedder`] + [`SemanticIndex`]: when either is `None` the
/// embed/upsert path is skipped and `recall` uses the non-vector fallback. The
/// dirty set records concept-ids whose index vector is stale and awaits background
/// reindex.
pub struct MemWriter {
    graph: Arc<dyn GraphStore>,
    embedder: Option<Arc<dyn Embedder>>,
    index: Option<Arc<dyn SemanticIndex>>,
    dirty: Mutex<HashSet<ConceptId>>,
}

impl MemWriter {
    /// Constructs a writer over the given graph seam and optional index stack.
    pub fn new(
        graph: Arc<dyn GraphStore>,
        embedder: Option<Arc<dyn Embedder>>,
        index: Option<Arc<dyn SemanticIndex>>,
    ) -> Self {
        Self {
            graph,
            embedder,
            index,
            dirty: Mutex::new(HashSet::new()),
        }
    }

    /// A writer over `graph` with no index configured (the A5-disabled shape).
    pub fn without_index(graph: Arc<dyn GraphStore>) -> Self {
        Self::new(graph, None, None)
    }

    // ── Dirty-set (invariant 8: the accelerator can never gate truth) ────────

    /// Records `id` as needing background reindex. Called on any best-effort
    /// embed/upsert/remove failure — never propagated to the caller.
    pub fn mark_dirty(&self, id: ConceptId) {
        self.dirty
            .lock()
            .expect("dirty set mutex poisoned")
            .insert(id);
    }

    /// Snapshot of the current dirty set (for the A5 background reindexer / tests).
    pub fn dirty_snapshot(&self) -> HashSet<ConceptId> {
        self.dirty.lock().expect("dirty set mutex poisoned").clone()
    }

    /// Whether `id` is currently marked dirty.
    pub fn is_dirty(&self, id: &ConceptId) -> bool {
        self.dirty
            .lock()
            .expect("dirty set mutex poisoned")
            .contains(id)
    }

    /// Number of concept-ids awaiting reindex.
    pub fn dirty_len(&self) -> usize {
        self.dirty.lock().expect("dirty set mutex poisoned").len()
    }

    // ── A5.4 rebuild-from-graph (cold start / version bump / corruption) ─────

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
    /// no longer matches the embedder (A5.4), then drains the dirty set. A no-op when
    /// no index/embedder is configured. Call once at startup before serving recalls.
    pub async fn rebuild_index_from_graph(&self) -> Result<()> {
        let (Some(emb), Some(idx)) = (&self.embedder, &self.index) else {
            return Ok(());
        };
        if idx.model_id() != emb.model_id() || idx.is_empty() {
            idx.reset();
            for (id, text, meta) in self.enumerate_current_state_embeddables().await? {
                match emb.embed_document(&text) {
                    Ok(v) => {
                        if idx.upsert(id.clone(), &v, meta).is_err() {
                            self.mark_dirty(id);
                        }
                    }
                    Err(_) => self.mark_dirty(id),
                }
            }
        }
        self.drain_dirty_set().await?;
        Ok(())
    }

    /// Repairs the dirty set: for each dirty concept-id, re-fetch its current-state
    /// text + meta and re-upsert (clearing it on success). An id that is no longer
    /// current-state (superseded/removed) is dropped from the index and the set.
    pub async fn drain_dirty_set(&self) -> Result<()> {
        let (Some(emb), Some(idx)) = (&self.embedder, &self.index) else {
            return Ok(());
        };
        for cid in self.dirty_snapshot() {
            let (q, v, r) = tql::fetch_belief_embeddable(&cid);
            let rows = self.graph.read(&q, &v, &r).await?;
            match rows
                .first()
                .and_then(|d| row_to_embeddable(d, Visibility::Private))
            {
                Some((id, text, meta)) => {
                    if let Ok(vec) = emb.embed_document(&text) {
                        if idx.upsert(id, &vec, meta).is_ok() {
                            self.clear_dirty(&cid);
                        }
                    }
                }
                None => {
                    // No longer current-state: drop from the accelerator and clear.
                    let _ = idx.remove(cid.clone());
                    self.clear_dirty(&cid);
                }
            }
        }
        Ok(())
    }

    /// Removes `id` from the dirty set (a successful repair / drop).
    fn clear_dirty(&self, id: &ConceptId) {
        self.dirty
            .lock()
            .expect("dirty set mutex poisoned")
            .remove(id);
    }

    // ── Best-effort index hooks (post-commit; never fail the graph write) ────

    fn best_effort_index(&self, id: &MemId, text: &str, meta: FilterMeta) {
        let (Some(emb), Some(idx)) = (&self.embedder, &self.index) else {
            return; // A5-disabled: skip embed/upsert entirely.
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

    fn belief_meta(&self, b: &BeliefDraft, ctx: &RunContext, state: BeliefState) -> FilterMeta {
        FilterMeta {
            owner: b.owner.clone(),
            visibility: b.visibility,
            belief_state: state,
            valid_from: ctx.occurred_at,
        }
    }

    // ── Reader helpers: Rust callers for the remaining substrate functions ───

    /// `is-superseded($b)` — supersession lineage OR a retracted/superseded state.
    pub async fn is_superseded(&self, b: &MemId) -> Result<bool> {
        let mut p = Params::new();
        p.s("bid", b.0.clone());
        let (q, v, r) = p.read(
            "match $b isa memory-item, has concept-id $bc; $bc == $bid;
                   true == is-superseded($b);
             fetch { \"ok\": $bc };",
        );
        Ok(!self.graph.read(&q, &v, &r).await?.is_empty())
    }

    /// `gate($n,$agent,$now)` — supersession · validity · scope, all at once. Every
    /// retrieved candidate passes through this.
    pub async fn gate(&self, n: &MemId, agent: &ActorId, now: DateTime) -> Result<bool> {
        let mut p = Params::new();
        p.s("nid", n.0.clone());
        p.s("aid", agent.0.clone());
        p.dt("now", now);
        let (q, v, r) = p.read(
            "match $b isa belief, has concept-id $bc; $bc == $nid;
                   $ag isa agent, has agent-id $aa; $aa == $aid;
                   true == gate($b, $ag, $now);
             fetch { \"ok\": $bc };",
        );
        Ok(!self.graph.read(&q, &v, &r).await?.is_empty())
    }

    /// `contradicts($b1,$b2)` — the two beliefs share a `contradiction` hub.
    pub async fn contradicts(&self, b1: &MemId, b2: &MemId) -> Result<bool> {
        let mut p = Params::new();
        p.s("id1", b1.0.clone());
        p.s("id2", b2.0.clone());
        let (q, v, r) = p.read(
            "match $b1 isa belief, has concept-id $c1; $c1 == $id1;
                   $b2 isa belief, has concept-id $c2; $c2 == $id2;
                   true == contradicts($b1, $b2);
             fetch { \"ok\": $c1 };",
        );
        Ok(!self.graph.read(&q, &v, &r).await?.is_empty())
    }

    /// `retrieval-score($m,$now)` with the **Rust-side recency/ACT-R decay layered
    /// on top** (TQL cannot express continuous half-life decay — see the function's
    /// header in `mem_functions.tql`). Returns `None` when the item lacks the decay
    /// attributes (base-activation/salience/last-access).
    pub async fn retrieval_score(&self, m: &MemId, now: DateTime) -> Result<Option<f64>> {
        let mut p = Params::new();
        p.s("mid", m.0.clone());
        p.dt("now", now);
        let (q, v, r) = p.read(
            "match $m isa memory-item, has concept-id $mc; $mc == $mid;
                   let $s in retrieval-score($m, $now);
             fetch { \"s\": $s, \"la\": $m.last-access };",
        );
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
        let mut p = Params::new();
        p.s("aid", agent.0.clone());
        p.dt("now", now);
        let (q, v, r) = p.read(
            "match $ag isa agent, has agent-id $aa; $aa == $aid;
                   let $b, $score in select-for-context($ag, $now);
                   $b has concept-id $bid;
             fetch { \"id\": $bid, \"score\": $score, \"la\": $b.last-access };",
        );
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
        let mut p = Params::new();
        p.s("h", spec_hash);
        let (q, v, r) = p.read(
            "match $p isa population, has spec-hash $ph; $ph == $h;
                   let $m in population-members($p);
                   $m has concept-id $mid;
             fetch { \"id\": $mid };",
        );
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(dedup_ids(&docs).into_iter().map(ConceptId).collect())
    }

    /// `canonical-entity($rec)` — the transitive closure over **provable**
    /// resolutions (probabilistic ones are never collapsed).
    pub async fn canonical_entity(&self, rec: &ConceptId) -> Result<Vec<ConceptId>> {
        let mut p = Params::new();
        p.s("rid", rec.0.clone());
        let (q, v, r) = p.read(
            "match $r isa concept, has concept-id $rc; $rc == $rid;
                   let $o in canonical-entity($r);
                   $o has concept-id $oid;
             fetch { \"id\": $oid };",
        );
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(dedup_ids(&docs).into_iter().map(ConceptId).collect())
    }

    /// `retrieval-provenance-of($b)` — walks `informs-synthesis` (the retrieval
    /// ledger). This is the OTHER ledger: it is **not** reachable from
    /// `derivation-chain` (invariant 8 — two ledgers, never crossed).
    pub async fn retrieval_provenance_of(&self, b: &MemId) -> Result<Vec<MemId>> {
        let mut p = Params::new();
        p.s("bid", b.0.clone());
        let (q, v, r) = p.read(
            "match $b isa belief, has concept-id $bc; $bc == $bid;
                   let $rv in retrieval-provenance-of($b);
                   $rv has concept-id $rid;
             fetch { \"id\": $rid };",
        );
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(dedup_ids(&docs).into_iter().map(MemId).collect())
    }

    // ── Recall paths (invariant 8) ───────────────────────────────────────────

    /// As-of-time-T recall: routes through `believed-at`, **never touches the
    /// index** (present-state only).
    async fn recall_as_of(&self, at: DateTime, budget: ContextBudget) -> Result<Vec<Chunk>> {
        let mut p = Params::new();
        p.dt("t", at);
        let (q, v, r) = p.read(
            "match let $b in believed-at($t);
                   $b has concept-id $bid;
             fetch { \"id\": $bid, \"title\": $b.title };",
        );
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
        let mut p = Params::new();
        p.s("cid", cid.0.clone());
        p.s("aid", actor.0.clone());
        p.dt("now", now);
        let (q, v, r) = p.read(
            "match $b isa belief, has concept-id $bc; $bc == $cid;
                   $ag isa agent, has agent-id $aa; $aa == $aid;
                   true == gate($b, $ag, $now);
                   let $act in retrieval-score($b, $now);
             fetch { \"title\": $b.title, \"act\": $act, \"la\": $b.last-access };",
        );
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(docs.first().map(|d| {
            let rank = f64_field(d, "act").unwrap_or(0.0) * decay_of(d, now);
            (str_field(d, "title"), rank)
        }))
    }
}

// ── JSON extraction + decay helpers (the Rust Reader's math) ─────────────────

fn str_field(d: &Value, k: &str) -> String {
    d.get(k)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn f64_field(d: &Value, k: &str) -> Option<f64> {
    d.get(k).and_then(|v| v.as_f64())
}

fn dt_field(d: &Value, k: &str) -> Option<DateTime> {
    d.get(k).and_then(|v| v.as_str()).and_then(parse_dt)
}

/// Parses a datetime from a TypeDB fetch result (ISO, with or without fractional
/// seconds / offset), normalising to UTC.
fn parse_dt(s: &str) -> Option<DateTime> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(chrono::DateTime::from_naive_utc_and_offset(
                ndt,
                chrono::Utc,
            ));
        }
    }
    None
}

/// The ACT-R-style continuous recency multiplier `0.5^(age_hours / half_life)`.
/// Lives in Rust because TypeQL 3.12 cannot do duration→scalar / ln / pow.
fn recency_decay(last_access: DateTime, now: DateTime, half_life_hours: f64) -> f64 {
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

/// Collects the `"id"` field from each doc, de-duplicated, order-preserving.
fn dedup_ids(docs: &[Value]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for d in docs {
        let id = str_field(d, "id");
        if !id.is_empty() && seen.insert(id.clone()) {
            out.push(id);
        }
    }
    out
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
        let id = mint("mem/bel");
        // AUTHORITATIVE: belief + provenance stamp + (evidence⇒derivation) commit first.
        let mut ops = tql::insert_belief_ops(ctx, &b, &id, tql::entrenchment_for(method));
        if let Some(deriv) = tql::derivation_op(&id, evidence, method, b.confidence) {
            ops.push(deriv);
        }
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
        let new_id = mint("mem/bel");
        // AUTHORITATIVE: new belief + supersession lineage + old→superseded commit first.
        let mut ops = tql::insert_belief_ops(ctx, &new, &new_id, "inferred");
        ops.extend(tql::supersession_ops(
            &old,
            &new_id,
            reason,
            ctx.occurred_at,
        ));
        self.graph.write(&ops).await?;
        // Best-effort: drop the old vector (gated on BOTH embedder+index present,
        // the same condition as upsert — a NoopIndex-only config must not attempt
        // a remove either).
        if let (true, Some(idx)) = (self.embedder.is_some(), &self.index) {
            let old_cid = ConceptId(old.0.clone());
            if idx.remove(old_cid.clone()).is_err() {
                self.mark_dirty(old_cid);
            }
        }
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
}

#[async_trait]
impl EpistemicReader for MemWriter {
    async fn recall(
        &self,
        ctx: &RunContext,
        q: RecallQuery,
        budget: ContextBudget,
    ) -> Result<Vec<Chunk>> {
        let now = ctx.occurred_at;

        // As-of-T ⇒ temporal path; SKIP the index entirely (invariant 8).
        if let Some(t) = q.as_of {
            return self.recall_as_of(t, budget).await;
        }

        // Present-state: index-seeded when configured, else non-vector fallback.
        if let (Some(emb), Some(idx)) = (&self.embedder, &self.index) {
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

            let mut scored: Vec<(Chunk, f64)> = Vec::new();
            for (cid, sim) in cand {
                // AUTHORITATIVE fetch + gate: an ANN-surfaced but gateable id
                // yields no row here and is therefore dropped.
                if let Some((title, rank)) = self.fetch_gated(&cid, &ctx.actor, now).await? {
                    scored.push((
                        Chunk {
                            id: MemId(cid.0),
                            text: title,
                            score: sim,
                        },
                        rank,
                    ));
                }
            }
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            Ok(scored
                .into_iter()
                .take(budget.max_chunks)
                .map(|(c, _)| c)
                .collect())
        } else {
            // A5 disabled / NoopIndex: recent+salient+scoped via select-for-context.
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

    async fn derivation_chain(&self, b: MemId) -> Result<Vec<MemId>> {
        // Walks `derivation` ONLY — never the retrieval ledger (invariant 8).
        let mut p = Params::new();
        p.s("bid", b.0.clone());
        let (q, v, r) = p.read(
            "match $b isa memory-item, has concept-id $bc; $bc == $bid;
                   let $m in derivation-chain($b);
                   $m has concept-id $mid;
             fetch { \"id\": $mid };",
        );
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(dedup_ids(&docs).into_iter().map(MemId).collect())
    }

    async fn believed_at(&self, at: DateTime, _q: BeliefQuery) -> Result<Vec<MemId>> {
        // Temporal path; the index is NEVER consulted (invariant 8).
        let mut p = Params::new();
        p.dt("t", at);
        let (q, v, r) = p.read(
            "match let $b in believed-at($t);
                   $b has concept-id $bid;
             fetch { \"id\": $bid };",
        );
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(dedup_ids(&docs).into_iter().map(MemId).collect())
    }

    async fn blast_radius(&self, retracted: MemId) -> Result<Vec<MemId>> {
        let mut p = Params::new();
        p.s("rid", retracted.0.clone());
        let (q, v, r) = p.read(
            "match $r isa memory-item, has concept-id $rc; $rc == $rid;
                   let $m in blast-radius($r);
                   $m has concept-id $mid;
             fetch { \"id\": $mid };",
        );
        let docs = self.graph.read(&q, &v, &r).await?;
        Ok(dedup_ids(&docs).into_iter().map(MemId).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gecko_extension_api::{GraphWrite, ProvenanceSource};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn ctx() -> RunContext {
        RunContext::new(
            ActorId::new("agent-1"),
            ProvenanceSource::Manual,
            chrono::Utc::now(),
        )
    }

    fn belief() -> BeliefDraft {
        BeliefDraft {
            text: "the host is compromised".to_string(),
            owner: ActorId::new("agent-1"),
            visibility: Visibility::Private,
            confidence: Some(0.7),
        }
    }

    /// A fake [`GraphStore`] that records committed writes and can be forced to
    /// fail — enough to unit-test the best-effort/dirty wrapper without a live DB.
    #[derive(Default)]
    struct RecordingGraphStore {
        writes: Mutex<Vec<GraphWrite>>,
        fail_write: bool,
    }
    impl RecordingGraphStore {
        fn failing() -> Self {
            Self {
                fail_write: true,
                ..Default::default()
            }
        }
    }
    #[async_trait]
    impl GraphStore for RecordingGraphStore {
        async fn write(&self, ops: &[GraphWrite]) -> Result<()> {
            if self.fail_write {
                return Err(EpistemicError::Storage("db down".into()));
            }
            self.writes.lock().unwrap().extend_from_slice(ops);
            Ok(())
        }
        async fn read(
            &self,
            _query: &str,
            _vars: &[String],
            _row: &[gecko_extension_api::GraphValue],
        ) -> Result<Vec<Value>> {
            Ok(vec![])
        }
    }

    struct OkEmbedder;
    impl Embedder for OkEmbedder {
        fn model_id(&self) -> &str {
            "test-embedder"
        }
        fn dim(&self) -> usize {
            3
        }
        fn embed_query(&self, _t: &str) -> Result<Vec<f32>> {
            Ok(vec![0.1, 0.2, 0.3])
        }
        fn embed_document(&self, _t: &str) -> Result<Vec<f32>> {
            Ok(vec![0.1, 0.2, 0.3])
        }
    }

    struct FailingEmbedder;
    impl Embedder for FailingEmbedder {
        fn model_id(&self) -> &str {
            "failing-embedder"
        }
        fn dim(&self) -> usize {
            3
        }
        fn embed_query(&self, _t: &str) -> Result<Vec<f32>> {
            Err(EpistemicError::Embedding("boom".into()))
        }
        fn embed_document(&self, _t: &str) -> Result<Vec<f32>> {
            Err(EpistemicError::Embedding("boom".into()))
        }
    }

    #[derive(Default)]
    struct FailingUpsertIndex {
        upserts: AtomicUsize,
        removes: AtomicUsize,
    }
    impl SemanticIndex for FailingUpsertIndex {
        fn upsert(&self, _id: ConceptId, _v: &[f32], _m: FilterMeta) -> Result<()> {
            self.upserts.fetch_add(1, Ordering::SeqCst);
            Err(EpistemicError::Index("upsert failed".into()))
        }
        fn remove(&self, _id: ConceptId) -> Result<()> {
            self.removes.fetch_add(1, Ordering::SeqCst);
            Err(EpistemicError::Index("remove failed".into()))
        }
        fn query(&self, _v: &[f32], _k: usize, _p: &FilterMeta) -> Result<Vec<(ConceptId, f32)>> {
            Ok(vec![])
        }
        fn model_id(&self) -> &str {
            "test-embedder"
        }
    }

    #[derive(Default)]
    struct OkIndex {
        upserts: Mutex<Vec<ConceptId>>,
    }
    impl SemanticIndex for OkIndex {
        fn upsert(&self, id: ConceptId, _v: &[f32], _m: FilterMeta) -> Result<()> {
            self.upserts.lock().unwrap().push(id);
            Ok(())
        }
        fn remove(&self, _id: ConceptId) -> Result<()> {
            Ok(())
        }
        fn query(&self, _v: &[f32], _k: usize, _p: &FilterMeta) -> Result<Vec<(ConceptId, f32)>> {
            Ok(vec![])
        }
        fn model_id(&self) -> &str {
            "test-embedder"
        }
    }

    #[tokio::test]
    async fn assert_belief_survives_failing_index_and_marks_dirty() {
        let idx = Arc::new(FailingUpsertIndex::default());
        let w = MemWriter::new(
            Arc::new(RecordingGraphStore::default()),
            Some(Arc::new(OkEmbedder)),
            Some(idx.clone()),
        );
        let id = w
            .assert_belief(&ctx(), belief(), &[], DerivationMethod::LlmSynthesis)
            .await
            .expect("belief write must succeed even when the index fails");
        assert_eq!(idx.upserts.load(Ordering::SeqCst), 1);
        assert!(w.is_dirty(&ConceptId(id.0.clone())));
        assert_eq!(w.dirty_len(), 1);
    }

    #[tokio::test]
    async fn assert_belief_survives_failing_embed_and_marks_dirty() {
        let idx = Arc::new(FailingUpsertIndex::default());
        let w = MemWriter::new(
            Arc::new(RecordingGraphStore::default()),
            Some(Arc::new(FailingEmbedder)),
            Some(idx.clone()),
        );
        let id = w
            .assert_belief(&ctx(), belief(), &[], DerivationMethod::LlmSynthesis)
            .await
            .expect("belief write must succeed even when embedding fails");
        assert_eq!(
            idx.upserts.load(Ordering::SeqCst),
            0,
            "embed failed ⇒ no upsert attempt"
        );
        assert!(w.is_dirty(&ConceptId(id.0)));
    }

    #[tokio::test]
    async fn assert_belief_with_working_index_is_not_dirty() {
        let idx = Arc::new(OkIndex::default());
        let w = MemWriter::new(
            Arc::new(RecordingGraphStore::default()),
            Some(Arc::new(OkEmbedder)),
            Some(idx.clone()),
        );
        let id = w
            .assert_belief(&ctx(), belief(), &[], DerivationMethod::TypeJoin)
            .await
            .unwrap();
        assert_eq!(w.dirty_len(), 0);
        assert_eq!(idx.upserts.lock().unwrap().as_slice(), &[ConceptId(id.0)]);
    }

    #[tokio::test]
    async fn no_index_configured_skips_embed_and_never_dirties() {
        let w = MemWriter::without_index(Arc::new(RecordingGraphStore::default()));
        let id = w
            .assert_belief(&ctx(), belief(), &[], DerivationMethod::TypeJoin)
            .await
            .unwrap();
        assert!(id.0.starts_with("mem/bel/"));
        assert_eq!(w.dirty_len(), 0);
    }

    #[tokio::test]
    async fn authoritative_commit_failure_fails_the_write_and_dirties_nothing() {
        let idx = Arc::new(OkIndex::default());
        let w = MemWriter::new(
            Arc::new(RecordingGraphStore::failing()),
            Some(Arc::new(OkEmbedder)),
            Some(idx.clone()),
        );
        let err = w
            .assert_belief(&ctx(), belief(), &[], DerivationMethod::TypeJoin)
            .await
            .expect_err("a failed graph commit must fail the write");
        assert!(matches!(err, EpistemicError::Storage(_)));
        assert_eq!(idx.upserts.lock().unwrap().len(), 0);
        assert_eq!(w.dirty_len(), 0);
    }

    #[tokio::test]
    async fn supersede_marks_old_dirty_when_remove_fails() {
        let idx = Arc::new(FailingUpsertIndex::default());
        let w = MemWriter::new(
            Arc::new(RecordingGraphStore::default()),
            Some(Arc::new(OkEmbedder)),
            Some(idx.clone()),
        );
        let old = MemId::new("mem/bel/OLD");
        let new_id = w
            .supersede(
                &ctx(),
                old.clone(),
                belief(),
                "found contradicting evidence",
            )
            .await
            .expect("supersede must succeed even when the index errors");
        assert_eq!(idx.removes.load(Ordering::SeqCst), 1);
        assert!(w.is_dirty(&ConceptId(old.0)));
        assert!(w.is_dirty(&ConceptId(new_id.0)));
    }

    #[tokio::test]
    async fn supersede_without_embedder_never_removes() {
        // Carry-forward fix: old-vector remove is gated on BOTH embedder+index.
        let idx = Arc::new(FailingUpsertIndex::default());
        let w = MemWriter::new(
            Arc::new(RecordingGraphStore::default()),
            None, // no embedder
            Some(idx.clone()),
        );
        w.supersede(&ctx(), MemId::new("mem/bel/OLD"), belief(), "reason")
            .await
            .unwrap();
        assert_eq!(
            idx.removes.load(Ordering::SeqCst),
            0,
            "no embedder ⇒ no remove attempt (matches upsert gating)"
        );
        assert_eq!(w.dirty_len(), 0);
    }

    #[tokio::test]
    async fn contest_rejects_fewer_than_two_claims() {
        let w = MemWriter::without_index(Arc::new(RecordingGraphStore::default()));
        let e = w
            .contest(&ctx(), &[MemId::new("mem/bel/x")])
            .await
            .unwrap_err();
        assert!(matches!(e, EpistemicError::InvalidInput(_)));
    }

    #[test]
    fn recency_decay_halves_each_half_life() {
        let t0 = chrono::Utc::now();
        let one_week = t0 + chrono::Duration::hours(168);
        let d = recency_decay(t0, one_week, DEFAULT_HALF_LIFE_HOURS);
        assert!((d - 0.5).abs() < 1e-6, "one half-life ⇒ 0.5, got {d}");
        assert_eq!(recency_decay(one_week, t0, DEFAULT_HALF_LIFE_HOURS), 1.0);
    }
}
