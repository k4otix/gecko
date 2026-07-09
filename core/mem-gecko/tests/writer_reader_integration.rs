//! A3 writer/reader acceptance tests (live TypeDB 3.12).
//!
//! These drive the **real** [`MemWriter`] over a [`RouterGraphStore`] against a
//! live server (`localhost:1729`, admin/password): every write is a real TypeDB
//! write; every read invokes the persisted substrate functions. Each test
//! provisions its own throwaway database, applies core + mem schema, and drops it.
//! The real `gecko` database is never touched.
//!
//! Requires a running TypeDB 3.12 service (CI provides one).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::StreamExt;
use tokio::sync::Mutex;

use gecko_engine::db::RouterGraphStore;
use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_extension_api::GeckoExtension;
use gecko_extension_api::{
    ActorId, BeliefDraft, BeliefQuery, ConceptId, ContextBudget, DateTime, DerivationMethod,
    Embedder, EpisodeDraft, EpistemicError, EpistemicReader, EpistemicWriter, FilterMeta, MemId,
    ProvenanceSource, RecallQuery, RunContext, SemanticIndex, Visibility,
};
use mem_gecko::MemWriter;
use ulid::Ulid;

const CORE_SCHEMA: &str = include_str!("../../gecko-engine/schema/core_schema.tql");

/// A test harness: a shared router (schema applied) + the mem writer over it.
struct Fixture {
    shared: Arc<Mutex<TypeDbRouter>>,
    name: String,
}

impl Fixture {
    async fn new() -> Self {
        let name = format!("a3test_{}", Ulid::new().to_string().to_lowercase());
        let mut router = TypeDbRouter::new(DbConfig {
            address: "localhost:1729".to_string(),
            database: name.clone(),
            username: "admin".to_string(),
            password: "password".to_string(),
            tls: TlsMode::Disabled,
        });
        router
            .apply_schema(CORE_SCHEMA)
            .await
            .expect("core schema applies");
        router
            .apply_schema(mem_gecko::MemGecko::new().schema())
            .await
            .expect("mem substrate schema applies");
        Fixture {
            shared: Arc::new(Mutex::new(router)),
            name,
        }
    }

    fn writer(
        &self,
        embedder: Option<Arc<dyn Embedder>>,
        index: Option<Arc<dyn SemanticIndex>>,
    ) -> MemWriter {
        let store = Arc::new(RouterGraphStore::new(self.shared.clone()));
        MemWriter::new(store, embedder, index)
    }

    /// Low-level fetch (bypasses the writer) for verifying stored graph state.
    async fn raw_fetch(&self, tql: &str) -> Vec<serde_json::Value> {
        let mut router = self.shared.lock().await;
        let tx = router.begin_read().await.expect("read tx");
        let answer = tx.query(tql).await.expect("read query");
        let mut out = Vec::new();
        if answer.is_document_stream() {
            let mut stream = answer.into_documents();
            while let Some(Ok(doc)) = stream.next().await {
                out.push(serde_json::from_str(&doc.into_json().to_string()).unwrap());
            }
        }
        out
    }

    /// Low-level write (bypasses the writer) for seeding fixtures the A3 writer
    /// does not yet mint (e.g. retrieval-events — that path is A5).
    async fn raw_write(&self, tql: &str) {
        let mut router = self.shared.lock().await;
        let tx = router.begin_write().await.expect("write tx");
        tx.query(tql).await.expect("write query");
        tx.commit().await.expect("commit");
    }

    async fn drop_db(&self) {
        let mut router = self.shared.lock().await;
        router.delete_database(&self.name).await.expect("drop db");
    }
}

fn dt(s: &str) -> DateTime {
    chrono::DateTime::parse_from_rfc3339(s)
        .unwrap()
        .with_timezone(&chrono::Utc)
}

fn ctx_at(actor: &str, at: DateTime) -> RunContext {
    RunContext::new(ActorId::new(actor), ProvenanceSource::Manual, at)
}

fn belief(text: &str, owner: &str, conf: f64) -> BeliefDraft {
    BeliefDraft {
        text: text.to_string(),
        owner: ActorId::new(owner),
        visibility: Visibility::Private,
        confidence: Some(conf),
    }
}

// ── Test doubles for recall ──────────────────────────────────────────────────

struct StubEmbedder;
impl Embedder for StubEmbedder {
    fn model_id(&self) -> &str {
        "stub"
    }
    fn dim(&self) -> usize {
        3
    }
    fn embed_query(&self, _t: &str) -> Result<Vec<f32>, EpistemicError> {
        Ok(vec![0.1, 0.2, 0.3])
    }
    fn embed_document(&self, _t: &str) -> Result<Vec<f32>, EpistemicError> {
        Ok(vec![0.1, 0.2, 0.3])
    }
}

/// A stub index that returns a FIXED candidate list (whatever ids the test seeds,
/// including deliberately-gateable ones) and records whether `query` was called.
struct StubIndex {
    candidates: Vec<(ConceptId, f32)>,
    queried: AtomicBool,
}
impl StubIndex {
    fn with(candidates: Vec<(ConceptId, f32)>) -> Arc<Self> {
        Arc::new(Self {
            candidates,
            queried: AtomicBool::new(false),
        })
    }
}
impl SemanticIndex for StubIndex {
    fn upsert(&self, _id: ConceptId, _v: &[f32], _m: FilterMeta) -> Result<(), EpistemicError> {
        Ok(())
    }
    fn remove(&self, _id: ConceptId) -> Result<(), EpistemicError> {
        Ok(())
    }
    fn query(
        &self,
        _v: &[f32],
        _k: usize,
        _p: &FilterMeta,
    ) -> Result<Vec<(ConceptId, f32)>, EpistemicError> {
        self.queried.store(true, Ordering::SeqCst);
        Ok(self.candidates.clone())
    }
    fn model_id(&self) -> &str {
        "stub"
    }
}

/// An index that records every id passed to `remove` (and a fixed candidate list for
/// `query`) so the A6 dedup acceptance can assert the loser's vector was dropped.
#[derive(Default)]
struct RecordingIndex {
    removed: std::sync::Mutex<Vec<ConceptId>>,
}
impl SemanticIndex for RecordingIndex {
    fn upsert(&self, _id: ConceptId, _v: &[f32], _m: FilterMeta) -> Result<(), EpistemicError> {
        Ok(())
    }
    fn remove(&self, id: ConceptId) -> Result<(), EpistemicError> {
        self.removed.lock().unwrap().push(id);
        Ok(())
    }
    fn query(
        &self,
        _v: &[f32],
        _k: usize,
        _p: &FilterMeta,
    ) -> Result<Vec<(ConceptId, f32)>, EpistemicError> {
        Ok(vec![])
    }
    fn model_id(&self) -> &str {
        "stub"
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// A1 bullet 1 + bullet 3 (writer level): provenance is host-mediated + run-stamped.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn provenance_stamp_roundtrips_run_id_and_source() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));

    // observe → episode stamped with the run's run_id + both bitemporal times.
    let ep_id = w
        .observe(
            &ctx,
            EpisodeDraft {
                text: "saw the host beacon".into(),
                event_time: dt("2026-01-01T00:00:00Z"),
                ingest_time: dt("2026-01-01T00:05:00Z"),
            },
        )
        .await
        .expect("observe commits");

    let stamp = fx
        .raw_fetch(&format!(
            r#"match
                 $ep isa episode, has concept-id "{}";
                 (memory: $ep, origin: $run) isa source-link, has origin-kind $k;
                 $run isa doc-run, has run-id $rid;
               fetch {{ "rid": $rid, "kind": $k }};"#,
            ep_id.0
        ))
        .await;
    assert_eq!(
        stamp.len(),
        1,
        "episode carries a source-link → doc-run stamp"
    );
    assert_eq!(
        stamp[0]["rid"],
        ctx.run_id.to_string(),
        "run_id matches the RunContext"
    );
    assert_eq!(stamp[0]["kind"], "manual");

    // both event-time and ingest-time present (invariant 6).
    let times = fx
        .raw_fetch(&format!(
            r#"match $ep isa episode, has concept-id "{}", has event-time $et, has ingest-time $it;
               fetch {{ "et": $et, "it": $it }};"#,
            ep_id.0
        ))
        .await;
    assert_eq!(
        times.len(),
        1,
        "episode has BOTH event-time and ingest-time"
    );

    // assert_belief → same source-link → run-id → source path (A1 bullet 1).
    let bel_id = w
        .assert_belief(
            &ctx,
            belief("the host is compromised", "agent-1", 0.8),
            &[ep_id.clone()],
            DerivationMethod::LlmSynthesis,
        )
        .await
        .expect("assert commits");

    let bp = fx
        .raw_fetch(&format!(
            r#"match
                 $b isa belief, has concept-id "{}";
                 (memory: $b, origin: $run) isa source-link, has origin-kind $k;
                 $run isa doc-run, has run-id $rid;
               fetch {{ "rid": $rid, "kind": $k }};"#,
            bel_id.0
        ))
        .await;
    assert_eq!(bp.len(), 1);
    assert_eq!(bp[0]["rid"], ctx.run_id.to_string());
    assert_eq!(bp[0]["kind"], "manual");

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// derivation-chain walks evidence but NEVER the retrieval ledger (invariant 8).
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn derivation_chain_never_touches_the_retrieval_ledger() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));

    let ev = w
        .observe(
            &ctx,
            EpisodeDraft {
                text: "evidence episode".into(),
                event_time: dt("2026-01-01T00:00:00Z"),
                ingest_time: dt("2026-01-01T00:00:00Z"),
            },
        )
        .await
        .unwrap();

    let bel = w
        .assert_belief(
            &ctx,
            belief("synthesized claim", "agent-1", 0.6),
            &[ev.clone()],
            DerivationMethod::LlmSynthesis,
        )
        .await
        .unwrap();

    // Seed a retrieval-event + informs-synthesis edge (the OTHER ledger, A5.7 path).
    fx.raw_write(&format!(
        r#"match $b isa belief, has concept-id "{}";
           insert
             $re isa retrieval-event, has concept-id "mem/ep/re1",
                 has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00,
                 has retrieval-method "semantic", has candidate-count 3;
             (retrieval: $re, synthesized: $b) isa informs-synthesis;"#,
        bel.0
    ))
    .await;

    let chain = w.derivation_chain(bel.clone()).await.unwrap();
    assert!(
        chain.contains(&ev),
        "derivation-chain includes the evidence episode"
    );
    assert!(
        !chain.contains(&MemId::new("mem/ep/re1")),
        "derivation-chain must NOT include the retrieval-event (two ledgers, never crossed)"
    );

    // The retrieval ledger is walked by its OWN function.
    let prov = w.retrieval_provenance_of(&bel).await.unwrap();
    assert_eq!(prov, vec![MemId::new("mem/ep/re1")]);

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// blast-radius is transitive over rests-on ∪ derivation.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn blast_radius_propagates_transitively() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));

    let a = w
        .assert_belief(
            &ctx,
            belief("assumption", "agent-1", 0.9),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    let b = w
        .assert_belief(
            &ctx,
            belief("resting belief", "agent-1", 0.7),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    w.rests_on(&ctx, b.clone(), &[a.clone()]).await.unwrap();
    // b2 is derived FROM b (b is its evidence/source).
    let b2 = w
        .assert_belief(
            &ctx,
            belief("downstream belief", "agent-1", 0.5),
            &[b.clone()],
            DerivationMethod::TypeJoin,
        )
        .await
        .unwrap();

    let radius = w.blast_radius(a).await.unwrap();
    assert!(
        radius.contains(&b),
        "retracting the assumption blasts the resting belief"
    );
    assert!(
        radius.contains(&b2),
        "…and transitively its downstream derivation"
    );

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// believed-at is the temporal path (never the index).
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn believed_at_returns_the_as_of_set() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);

    let b1 = w
        .assert_belief(
            &ctx_at("agent-1", dt("2026-01-01T00:00:00Z")),
            belief("early belief", "agent-1", 0.8),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    let b2 = w
        .assert_belief(
            &ctx_at("agent-1", dt("2026-03-01T00:00:00Z")),
            belief("later belief", "agent-1", 0.8),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    let q = BeliefQuery {
        text: String::new(),
    };
    let feb = w
        .believed_at(dt("2026-02-01T00:00:00Z"), q.clone())
        .await
        .unwrap();
    assert!(
        feb.contains(&b1) && !feb.contains(&b2),
        "as-of Feb: only the early belief"
    );

    let apr = w.believed_at(dt("2026-04-01T00:00:00Z"), q).await.unwrap();
    assert!(
        apr.contains(&b1) && apr.contains(&b2),
        "as-of Apr: both beliefs"
    );

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// gate excludes a superseded belief; is-superseded agrees; supersession lineage.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn gate_and_is_superseded_exclude_superseded_beliefs() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));
    let now = dt("2026-01-02T00:00:00Z");

    let old = w
        .assert_belief(
            &ctx,
            belief("old view", "agent-1", 0.6),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    let new = w
        .supersede(
            &ctx,
            old.clone(),
            belief("revised view", "agent-1", 0.9),
            "new evidence",
        )
        .await
        .unwrap();

    assert!(w.is_superseded(&old).await.unwrap(), "old is superseded");
    assert!(!w.is_superseded(&new).await.unwrap(), "new is not");
    assert!(
        !w.gate(&old, &ActorId::new("agent-1"), now).await.unwrap(),
        "gate excludes superseded old"
    );
    assert!(
        w.gate(&new, &ActorId::new("agent-1"), now).await.unwrap(),
        "gate admits the new belief"
    );
    // scope: another actor does not own it → gate false.
    assert!(
        !w.gate(&new, &ActorId::new("other"), now).await.unwrap(),
        "gate enforces ownership scope"
    );

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// contest builds a contradiction→anomaly hub; contradicts() reads it back.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn contest_and_contradicts() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));

    let b1 = w
        .assert_belief(
            &ctx,
            belief("X is true", "agent-1", 0.7),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    let b2 = w
        .assert_belief(
            &ctx,
            belief("X is false", "agent-1", 0.7),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    let b3 = w
        .assert_belief(
            &ctx,
            belief("unrelated", "agent-1", 0.7),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    let anomaly = w.contest(&ctx, &[b1.clone(), b2.clone()]).await.unwrap();
    assert!(anomaly.0.starts_with("mem/anom/"));

    assert!(
        w.contradicts(&b1, &b2).await.unwrap(),
        "b1 and b2 share the contradiction hub"
    );
    assert!(!w.contradicts(&b1, &b3).await.unwrap(), "b1 and b3 do not");

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// THE CRUX (invariant 8): a stub index surfaces a SUPERSEDED belief's id, but
// recall gates AFTER the authoritative fetch, so it is never returned.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn recall_never_leaks_an_ann_surfaced_superseded_candidate() {
    let fx = Fixture::new().await;
    let ctx = ctx_at("agent-1", dt("2026-01-02T00:00:00Z"));

    // Write two beliefs with the writer that has no index (writes only).
    let writer_only = fx.writer(None, None);
    let kept = writer_only
        .assert_belief(
            &ctx_at("agent-1", dt("2026-01-01T00:00:00Z")),
            belief("still valid belief", "agent-1", 0.9),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    let to_supersede = writer_only
        .assert_belief(
            &ctx_at("agent-1", dt("2026-01-01T00:00:00Z")),
            belief("obsolete belief", "agent-1", 0.5),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    // Supersede it → it becomes gateable.
    let _new = writer_only
        .supersede(
            &ctx,
            to_supersede.clone(),
            belief("replacement", "agent-1", 0.9),
            "obsoleted",
        )
        .await
        .unwrap();

    // The index deliberately surfaces the SUPERSEDED id first, then the kept id.
    let index = StubIndex::with(vec![
        (ConceptId(to_supersede.0.clone()), 0.99), // superseded — must be gated OUT
        (ConceptId(kept.0.clone()), 0.42),
    ]);
    let recaller = fx.writer(Some(Arc::new(StubEmbedder)), Some(index.clone()));

    let chunks = recaller
        .recall(
            &ctx,
            RecallQuery::now("anything"),
            ContextBudget {
                max_chunks: 10,
                max_tokens: None,
            },
        )
        .await
        .unwrap();

    let ids: Vec<String> = chunks.iter().map(|c| c.id.0.clone()).collect();
    assert!(ids.contains(&kept.0), "the still-valid belief is recalled");
    assert!(
        !ids.contains(&to_supersede.0),
        "the ANN-surfaced SUPERSEDED belief is gated out and never leaks (invariant 8)"
    );

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// As-of-T recall routes through believed-at and NEVER calls index.query.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn recall_as_of_t_skips_the_index_entirely() {
    let fx = Fixture::new().await;
    let w0 = fx.writer(None, None);
    let b = w0
        .assert_belief(
            &ctx_at("agent-1", dt("2026-01-01T00:00:00Z")),
            belief("historical belief", "agent-1", 0.8),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    let index = StubIndex::with(vec![(ConceptId(b.0.clone()), 0.9)]);
    let w = fx.writer(Some(Arc::new(StubEmbedder)), Some(index.clone()));

    let q = RecallQuery {
        text: "historical".into(),
        as_of: Some(dt("2026-02-01T00:00:00Z")),
    };
    let chunks = w
        .recall(
            &ctx_at("agent-1", dt("2026-06-01T00:00:00Z")),
            q,
            ContextBudget {
                max_chunks: 5,
                max_tokens: None,
            },
        )
        .await
        .unwrap();

    assert!(
        !index.queried.load(Ordering::SeqCst),
        "an as-of-T recall must NOT touch the semantic index"
    );
    assert!(
        chunks.iter().any(|c| c.id == b),
        "believed-at returns the historical belief"
    );

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// No index configured → recall falls back to select-for-context cleanly.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn recall_falls_back_when_no_index() {
    let fx = Fixture::new().await;
    let now = dt("2026-01-02T00:00:00Z");
    let w = fx.writer(None, None); // no embedder, no index
    let b = w
        .assert_belief(
            &ctx_at("agent-1", dt("2026-01-01T00:00:00Z")),
            belief("recent salient belief", "agent-1", 0.9),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    let chunks = w
        .recall(
            &ctx_at("agent-1", now),
            RecallQuery::now("x"),
            ContextBudget {
                max_chunks: 5,
                max_tokens: None,
            },
        )
        .await
        .unwrap();
    assert!(
        chunks.iter().any(|c| c.id == b),
        "fallback recall surfaces the gated belief"
    );

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Fixture tests: retrieval-score (decayed), select-for-context, population,
// canonical-entity, prediction lifecycle.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn retrieval_score_and_select_for_context() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let at = dt("2026-01-01T00:00:00Z");
    let b = w
        .assert_belief(
            &ctx_at("agent-1", at),
            belief("scored belief", "agent-1", 0.5),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    // base-activation(1.0) + salience(0.5) = 1.5, near-zero age ⇒ decay ≈ 1.
    let score = w
        .retrieval_score(&b, at)
        .await
        .unwrap()
        .expect("belief has decay attrs");
    assert!(
        (score - 1.5).abs() < 1e-3,
        "retrieval-score ≈ 1.5, got {score}"
    );

    let selected = w
        .select_for_context(&ActorId::new("agent-1"), at)
        .await
        .unwrap();
    assert!(
        selected.iter().any(|(id, _)| id == &b),
        "select-for-context returns the gated belief"
    );

    fx.drop_db().await;
}

#[tokio::test]
async fn population_members_and_canonical_entity() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);

    // Seed a population + materialized members + a provable cross-source resolution.
    fx.raw_write(
        r#"insert
             $p isa population, has spec-hash "cohort-1", has spec-dialect "typeql-3",
                 has spec-text 'match $c isa concept, has tag "internet-facing";';
             $c1 isa concept, has concept-id "asset/c1";
             $c2 isa concept, has concept-id "asset/c2";
             (owning-population: $p, member: $c1) isa population-member, has computed-at 2026-01-01T00:00:00;
             (owning-population: $p, member: $c2) isa population-member, has computed-at 2026-01-01T00:00:00;
             $ra isa concept, has concept-id "src-a/evil.com";
             $rb isa concept, has concept-id "src-b/evil.com";
             $res isa resolution, has concept-id "mem/bel/res", has belief-state "asserted";
             $ep isa episode, has concept-id "mem/ep/rs",
                 has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00;
             (derived: $res, source: $ep) isa derivation, has derivation-method "type-join";
             (resolution-belief: $res, record-a: $ra, record-b: $rb) isa resolves;"#,
    )
    .await;

    let mut members = w.population_members("cohort-1").await.unwrap();
    members.sort();
    assert_eq!(
        members,
        vec![ConceptId::new("asset/c1"), ConceptId::new("asset/c2")]
    );

    let mut canon = w
        .canonical_entity(&ConceptId::new("src-a/evil.com"))
        .await
        .unwrap();
    canon.sort();
    assert_eq!(
        canon,
        vec![
            ConceptId::new("src-a/evil.com"),
            ConceptId::new("src-b/evil.com")
        ],
        "provable resolution collapses both seams into one canonical class"
    );

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// A6 ACCEPTANCE: a manual dedup of two identical-content-hash episodes tombstones
// the loser in the graph AND removes the loser's vector from the index (via the
// existing best-effort SemanticIndex::remove hook, invariant 8).
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn dedup_episodes_tombstones_loser_and_removes_its_vector() {
    let fx = Fixture::new().await;
    let index = Arc::new(RecordingIndex::default());
    // Embedder + index both present ⇒ observe upserts each episode's vector, and
    // dedup will attempt the (real) remove of the loser.
    let w = fx.writer(Some(Arc::new(StubEmbedder)), Some(index.clone()));
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));

    // Two episodes with an IDENTICAL content-hash (same observed text).
    let dup = EpisodeDraft {
        text: "the host phoned home at 03:14".into(),
        event_time: dt("2026-01-01T00:00:00Z"),
        ingest_time: dt("2026-01-01T00:00:00Z"),
    };
    let keeper = w.observe(&ctx, dup.clone()).await.unwrap();
    let loser = w.observe(&ctx, dup.clone()).await.unwrap();

    // The manual consolidation op.
    w.dedup_episodes(&keeper, &loser)
        .await
        .expect("dedup of two identical-hash episodes succeeds");

    // Graph: the loser is tombstoned (terminal consolidation-state)…
    let tomb = fx
        .raw_fetch(&format!(
            r#"match $l isa episode, has concept-id "{}", has consolidation-state $s;
               fetch {{ "s": $s }};"#,
            loser.0
        ))
        .await;
    assert_eq!(tomb.len(), 1, "loser carries a consolidation-state");
    assert_eq!(tomb[0]["s"], "tombstoned", "loser is tombstoned");

    // …and the keeper is NOT tombstoned (it survives, un-marked).
    let keep_state = fx
        .raw_fetch(&format!(
            r#"match $k isa episode, has concept-id "{}";
               try {{ $k has consolidation-state $s; }};
               fetch {{ "s": $s }};"#,
            keeper.0
        ))
        .await;
    assert_eq!(keep_state.len(), 1, "keeper still exists in the graph");
    assert!(
        keep_state[0]["s"].is_null(),
        "keeper is NOT tombstoned (survives the dedup)"
    );

    // Index: the loser's id was removed via the best-effort remove hook; the keeper's
    // was not.
    let removed = index.removed.lock().unwrap().clone();
    assert!(
        removed.contains(&ConceptId(loser.0.clone())),
        "the loser's vector was removed from the index"
    );
    assert!(
        !removed.contains(&ConceptId(keeper.0.clone())),
        "the keeper's vector is untouched"
    );

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// A6: dedup is PROVABLE — two episodes with DIFFERENT content are never collapsed.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn dedup_episodes_refuses_non_duplicates() {
    let fx = Fixture::new().await;
    let index = Arc::new(RecordingIndex::default());
    let w = fx.writer(Some(Arc::new(StubEmbedder)), Some(index.clone()));
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));

    let a = w
        .observe(
            &ctx,
            EpisodeDraft {
                text: "one observation".into(),
                event_time: dt("2026-01-01T00:00:00Z"),
                ingest_time: dt("2026-01-01T00:00:00Z"),
            },
        )
        .await
        .unwrap();
    let b = w
        .observe(
            &ctx,
            EpisodeDraft {
                text: "a DIFFERENT observation".into(),
                event_time: dt("2026-01-01T00:00:00Z"),
                ingest_time: dt("2026-01-01T00:00:00Z"),
            },
        )
        .await
        .unwrap();

    let err = w
        .dedup_episodes(&a, &b)
        .await
        .expect_err("distinct-content episodes must not be deduped");
    assert!(matches!(err, EpistemicError::InvalidInput(_)));

    // No tombstone was written and no vector removed (the guard fired first).
    let states = fx
        .raw_fetch(r#"match $e isa episode, has consolidation-state $s; fetch { "s": $s };"#)
        .await;
    assert!(states.is_empty(), "no episode was tombstoned");
    assert!(
        index.removed.lock().unwrap().is_empty(),
        "no vector was removed"
    );

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// A6 retrieval-provenance retention: a retrieval-event is safe to tombstone ONLY
// once its informs-synthesis belief is superseded.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn retrieval_events_safe_to_tombstone_only_after_supersession() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));

    // A belief fed by a retrieval-event (the retrieval ledger).
    let b = w
        .assert_belief(
            &ctx,
            belief("synthesized from retrieval", "agent-1", 0.7),
            &[],
            DerivationMethod::LlmSynthesis,
        )
        .await
        .unwrap();
    fx.raw_write(&format!(
        r#"match $b isa belief, has concept-id "{}";
           insert
             $re isa retrieval-event, has concept-id "mem/ep/re-ret",
                 has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00,
                 has retrieval-method "semantic", has candidate-count 1;
             (retrieval: $re, synthesized: $b) isa informs-synthesis;"#,
        b.0
    ))
    .await;

    // While the belief is non-superseded, the event is NOT safe to tombstone.
    let before = w.retrieval_events_safe_to_tombstone().await.unwrap();
    assert!(
        !before.contains(&MemId::new("mem/ep/re-ret")),
        "a retrieval-event is NOT prunable while its belief is non-superseded"
    );

    // Supersede the belief → the event becomes safe to tombstone.
    w.supersede(
        &ctx,
        b.clone(),
        belief("revised synthesis", "agent-1", 0.9),
        "newer evidence",
    )
    .await
    .unwrap();

    let after = w.retrieval_events_safe_to_tombstone().await.unwrap();
    assert!(
        after.contains(&MemId::new("mem/ep/re-ret")),
        "once the belief is superseded, the retrieval-event is safe to tombstone"
    );

    fx.drop_db().await;
}

#[tokio::test]
async fn prediction_lifecycle() {
    use gecko_extension_api::Outcome;
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));

    let b = w
        .assert_belief(
            &ctx,
            belief("it will rain", "agent-1", 0.6),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    w.record_prediction(&ctx, b.clone(), 0.6).await.unwrap();
    w.resolve_prediction(&ctx, b.clone(), Outcome::Confirmed)
        .await
        .unwrap();

    let rows = fx
        .raw_fetch(&format!(
            r#"match
                 $b isa belief, has concept-id "{}";
                 $pr isa prediction-resolution, links (predicted-belief: $b, resolving-episode: $ep);
                 $pr has predicted-probability $p, has outcome $o;
               fetch {{ "p": $p, "o": $o }};"#,
            b.0
        ))
        .await;
    assert_eq!(
        rows.len(),
        1,
        "prediction resolved with an episode + outcome"
    );
    assert_eq!(rows[0]["o"], "confirmed");

    fx.drop_db().await;
}
