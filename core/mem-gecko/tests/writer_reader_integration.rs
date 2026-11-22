//! Writer/reader acceptance tests (live TypeDB).
//!
//! These drive the **real** [`MemWriter`] over a [`RouterGraphStore`] against a
//! live server (`localhost:1729`, admin/password): every write is a real TypeDB
//! write; every read invokes the persisted substrate functions. Each test
//! provisions its own throwaway database, applies core + mem schema, and drops it.
//! The real `gecko` database is never touched.
//!
//! Requires a running TypeDB service (CI provides one).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::StreamExt;
use tokio::sync::Mutex;

use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_engine::db::{RouterGraphStore, run_graph_writes};
use gecko_extension_api::GeckoExtension;
use gecko_extension_api::{
    ActorId, BeliefDraft, BeliefQuery, ConceptId, ContextBudget, DateTime, DerivationMethod,
    Embedder, Entrenchment, EpisodeDraft, EpistemicError, EpistemicReader, EpistemicWriter,
    FilterMeta, MemId, ProvenanceSource, RecallQuery, RunContext, SemanticIndex, Visibility,
};
use mem_gecko::{IndexOutcome, MemWriter};
use ulid::Ulid;

mod common;
#[path = "common/provenance.rs"]
mod provenance;

const CORE_SCHEMA: &str = include_str!("../../gecko-engine/schema/core_schema.tql");

/// A test harness: a shared router (schema applied) + the mem writer over it.
struct Fixture {
    shared: Arc<Mutex<TypeDbRouter>>,
    name: String,
}

impl Fixture {
    async fn new() -> Self {
        let name = format!("a3test_{}", Ulid::generate().to_string().to_lowercase());
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

    /// Low-level write (bypasses the writer) for seeding fixtures the writer
    /// itself does not mint (e.g. retrieval-events, which are seeded directly).
    async fn raw_write(&self, tql: &str) {
        let mut router = self.shared.lock().await;
        let tx = router.begin_write().await.expect("write tx");
        tx.query(tql).await.expect("write query");
        tx.commit().await.expect("commit");
    }

    /// Low-level write that returns the server error instead of panicking.
    async fn try_raw_write(&self, tql: &str) -> Result<(), String> {
        let mut router = self.shared.lock().await;
        let tx = router.begin_write().await.map_err(|e| e.to_string())?;
        tx.query(tql).await.map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())
    }

    /// Low-level write of raw memory items, each given its ownership and
    /// source-link by [`provenance::seed_provenance`] in the same transaction.
    async fn raw_seed(&self, tql: &str) {
        let mut router = self.shared.lock().await;
        let tx = router.begin_write().await.expect("write tx");
        tx.query(tql).await.expect("write query");
        run_graph_writes(&tx, &provenance::seed_provenance())
            .await
            .expect("seed provenance");
        tx.commit().await.expect("commit");
    }

    async fn drop_db(&self) {
        let mut router = self.shared.lock().await;
        common::drop_db_with_retry(&mut router, &self.name).await;
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
        entrenchment: None,
    }
}

/// A belief draft with an explicit visibility scope, for gate-visibility tests.
fn belief_vis(text: &str, owner: &str, conf: f64, vis: Visibility) -> BeliefDraft {
    BeliefDraft {
        visibility: vis,
        ..belief(text, owner, conf)
    }
}

/// A belief draft with an explicit entrenchment tier, for supersede-guard tests.
fn belief_ent(text: &str, owner: &str, conf: f64, ent: Entrenchment) -> BeliefDraft {
    BeliefDraft {
        entrenchment: Some(ent),
        ..belief(text, owner, conf)
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
/// Verifies that gating is applied after ANN retrieval.
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
/// `query`) so the dedup acceptance test can assert the loser's vector was dropped.
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
// Writer level: provenance is host-mediated + run-stamped.
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

    // assert_belief → same source-link → run-id → source path.
    let bel_id = w
        .assert_belief(
            &ctx,
            belief("the host is compromised", "agent-1", 0.8),
            std::slice::from_ref(&ep_id),
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
            std::slice::from_ref(&ev),
            DerivationMethod::LlmSynthesis,
        )
        .await
        .unwrap();

    // Seed a retrieval-event + informs-synthesis edge (the OTHER ledger).
    fx.raw_seed(&format!(
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
    w.rests_on(&ctx, b.clone(), std::slice::from_ref(&a))
        .await
        .unwrap();
    // b2 is derived FROM b (b is its evidence/source).
    let b2 = w
        .assert_belief(
            &ctx,
            belief("downstream belief", "agent-1", 0.5),
            std::slice::from_ref(&b),
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
// gate composes VISIBILITY, not owner-only scoping. A team/shared belief owned
// by a DIFFERENT agent is reachable; a private belief owned by another agent is not.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn gate_composes_visibility_across_owners() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let now = dt("2026-01-02T00:00:00Z");
    // The graph owner is the ctx actor (see `insert_belief_ops`), so beliefs written
    // under `owner_ctx` are owned by "owner-agent".
    let owner_ctx = ctx_at("owner-agent", dt("2026-01-01T00:00:00Z"));
    let other = ActorId::new("other-agent");
    // The querying agent must exist as a node (the `gate` wrapper matches `$ag isa
    // agent`); in a live run the actor always exists. Seed it for this fixture.
    fx.raw_write(r#"insert $a isa agent, has agent-id "other-agent";"#)
        .await;

    // TEAM-visibility belief owned by owner-agent ⇒ visible to a DIFFERENT agent.
    let team = w
        .assert_belief(
            &owner_ctx,
            belief_vis("team belief", "owner-agent", 0.7, Visibility::Team),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    assert!(
        w.gate(&team, &other, now).await.unwrap(),
        "a team-visibility belief passes gate for a non-owner (composes visibility)"
    );

    // SHARED-visibility belief owned by owner-agent ⇒ likewise visible.
    let shared = w
        .assert_belief(
            &owner_ctx,
            belief_vis("shared belief", "owner-agent", 0.7, Visibility::Shared),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    assert!(
        w.gate(&shared, &other, now).await.unwrap(),
        "a shared-visibility belief passes gate for a non-owner"
    );

    // PRIVATE belief owned by owner-agent ⇒ NOT visible to a different agent…
    let private = w
        .assert_belief(
            &owner_ctx,
            belief_vis("private belief", "owner-agent", 0.7, Visibility::Private),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    assert!(
        !w.gate(&private, &other, now).await.unwrap(),
        "a private belief owned by another agent is gated OUT"
    );
    // …but IS visible to its own owner.
    assert!(
        w.gate(&private, &ActorId::new("owner-agent"), now)
            .await
            .unwrap(),
        "a private belief passes gate for its owner"
    );

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// A superseded OR expired (valid-to < now) belief always fails gate,
// regardless of visibility/ownership.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn gate_excludes_superseded_and_expired_beliefs() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));
    let now = dt("2026-01-03T00:00:00Z");
    let agent = ActorId::new("agent-1");

    // Superseded belief fails gate even for its owner.
    let old = w
        .assert_belief(
            &ctx,
            belief("old view", "agent-1", 0.6),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    w.supersede(
        &ctx,
        old.clone(),
        belief("new view", "agent-1", 0.9),
        "moved on",
    )
    .await
    .unwrap();
    assert!(
        !w.gate(&old, &agent, now).await.unwrap(),
        "a superseded belief fails gate"
    );

    // Expired belief (valid-to in the past) fails gate.
    let expiring = w
        .assert_belief(
            &ctx,
            belief("time-boxed view", "agent-1", 0.6),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    fx.raw_write(&format!(
        r#"match $b isa belief, has concept-id "{}"; insert $b has valid-to 2026-01-02T00:00:00;"#,
        expiring.0
    ))
    .await;
    assert!(
        !w.gate(&expiring, &agent, now).await.unwrap(),
        "an expired belief (valid-to < now) fails gate"
    );

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// invariant-7 entrenchment guard — a lower-entrenchment belief is
// STRUCTURALLY forbidden from superseding a higher-entrenchment one (write NOTHING).
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn supersede_rejects_lower_entrenchment_and_leaves_graph_unchanged() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));

    // A user-stated belief (HumanAssertion ⇒ entrenchment "user-stated").
    let held = w
        .assert_belief(
            &ctx,
            belief("firmly held view", "agent-1", 0.9),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    // An llm-synthesis (lowest-entrenchment) supersede is rejected with a typed error.
    let err = w
        .supersede(
            &ctx,
            held.clone(),
            belief_ent("cheap guess", "agent-1", 0.3, Entrenchment::Llm),
            "llm wants to overwrite",
        )
        .await
        .expect_err("a lower-entrenchment belief cannot supersede a higher one");
    assert!(matches!(err, EpistemicError::EntrenchmentViolation(_)));

    // The graph is UNCHANGED: the protected belief is still asserted…
    assert!(
        !w.is_superseded(&held).await.unwrap(),
        "the protected belief is NOT superseded"
    );
    // …no supersession lineage was written…
    let supers = fx
        .raw_fetch(r#"match $s isa supersession; fetch { "ca": $s.created-at };"#)
        .await;
    assert!(supers.is_empty(), "no supersession relation was written");
    // …and no new belief was minted (only the original remains).
    let beliefs = fx
        .raw_fetch(r#"match $b isa belief, has concept-id $c; fetch { "c": $c };"#)
        .await;
    assert_eq!(beliefs.len(), 1, "no new belief was minted");

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Superseding a belief that does not exist errors and commits nothing: no orphan
// replacement belief is left behind.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn supersede_of_a_missing_belief_errors_and_commits_nothing() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));

    let err = w
        .supersede(
            &ctx,
            MemId::new("mem/bel/does-not-exist"),
            belief("orphan replacement", "agent-1", 0.5),
            "r",
        )
        .await
        .expect_err("superseding a missing belief must fail");
    let EpistemicError::Storage(msg) = &err else {
        panic!("expected Storage, got {err:?}");
    };
    assert!(msg.contains("mem/bel/does-not-exist"), "{msg}");

    let orphans = fx
        .raw_fetch(
            r#"match $b isa belief, has title $t; $t == "orphan replacement"; fetch { "t": $t };"#,
        )
        .await;
    assert!(orphans.is_empty(), "no replacement belief was committed");

    fx.drop_db().await;
}

fn storage_msg(err: EpistemicError) -> String {
    match err {
        EpistemicError::Storage(msg) => msg,
        other => panic!("expected Storage, got {other:?}"),
    }
}

/// The supersession edges recorded on belief `id`.
async fn supersessions_of(fx: &Fixture, id: &MemId) -> usize {
    fx.raw_fetch(&format!(
        r#"match $o isa belief, has concept-id "{}";
             $s isa supersession, links (superseded: $o);
           fetch {{ "k": "x" }};"#,
        id.0
    ))
    .await
    .len()
}

// ─────────────────────────────────────────────────────────────────────────────
// Only a currently asserted belief can be superseded: a second supersede of the
// same belief is refused and forks nothing.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn superseding_an_already_superseded_belief_is_refused_and_commits_nothing() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));
    let a = w
        .assert_belief(
            &ctx,
            belief("belief a", "agent-1", 0.5),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    w.supersede(&ctx, a.clone(), belief("belief b", "agent-1", 0.6), "r")
        .await
        .expect("the first supersede succeeds");

    let msg = storage_msg(
        w.supersede(&ctx, a.clone(), belief("belief c", "agent-1", 0.7), "r")
            .await
            .expect_err("a superseded belief cannot be superseded again"),
    );
    for want in [
        a.0.as_str(),
        "not a current belief visible to the caller",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "{want}: {msg}");
    }
    assert_eq!(supersessions_of(&fx, &a).await, 1, "no B/C fork exists");
    let c = fx
        .raw_fetch(r#"match $b isa belief, has title "belief c"; fetch { "k": "x" };"#)
        .await;
    assert!(c.is_empty(), "the refused replacement was not committed");

    fx.drop_db().await;
}

#[tokio::test]
async fn superseding_a_retracted_belief_is_refused() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));
    let a = w
        .assert_belief(
            &ctx,
            belief("retracted belief", "agent-1", 0.5),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    fx.raw_write(&format!(
        r#"match $b isa belief, has concept-id "{}";
           update $b has belief-state "retracted";"#,
        a.0
    ))
    .await;

    let msg = storage_msg(
        w.supersede(&ctx, a.clone(), belief("replacement", "agent-1", 0.6), "r")
            .await
            .expect_err("a retracted belief cannot be superseded"),
    );
    assert!(msg.contains(&a.0), "{msg}");
    assert!(msg.contains("nothing was committed"), "{msg}");
    let state = fx
        .raw_fetch(&format!(
            r#"match $b isa belief, has concept-id "{}", has belief-state $s; fetch {{ "s": $s }};"#,
            a.0
        ))
        .await;
    assert_eq!(state.len(), 1);
    assert_eq!(state[0]["s"], "retracted", "A stays retracted");
    assert_eq!(supersessions_of(&fx, &a).await, 0);

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// A caller may only act on beliefs visible to it.
// ─────────────────────────────────────────────────────────────────────────────

/// The belief-state of belief `id`.
async fn belief_state(fx: &Fixture, id: &MemId) -> String {
    let rows = fx
        .raw_fetch(&format!(
            r#"match $b isa belief, has concept-id "{}", has belief-state $s; fetch {{ "s": $s }};"#,
            id.0
        ))
        .await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    rows[0]["s"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn superseding_another_agents_private_belief_is_refused_and_commits_nothing() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let at = dt("2026-01-01T00:00:00Z");
    observe_as(&w, "agent-b", at).await;
    let private = w
        .assert_belief(
            &ctx_at("agent-a", at),
            belief_vis("a's private belief", "agent-a", 0.5, Visibility::Private),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    let msg = storage_msg(
        w.supersede(
            &ctx_at("agent-b", at),
            private.clone(),
            belief("b's replacement", "agent-b", 0.6),
            "r",
        )
        .await
        .expect_err("another agent's private belief cannot be superseded"),
    );
    for want in [
        private.0.as_str(),
        "not a current belief visible to the caller",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "{want}: {msg}");
    }
    assert_eq!(belief_state(&fx, &private).await, "asserted");
    assert_eq!(supersessions_of(&fx, &private).await, 0);
    let replacement = fx
        .raw_fetch(r#"match $b isa belief, has title "b's replacement"; fetch { "k": "x" };"#)
        .await;
    assert!(replacement.is_empty(), "no replacement was committed");

    fx.drop_db().await;
}

#[tokio::test]
async fn superseding_a_team_belief_of_another_agent_is_allowed() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let at = dt("2026-01-01T00:00:00Z");
    observe_as(&w, "agent-b", at).await;
    let team = w
        .assert_belief(
            &ctx_at("agent-a", at),
            belief_vis("a's team belief", "agent-a", 0.5, Visibility::Team),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    w.supersede(
        &ctx_at("agent-b", at),
        team.clone(),
        belief("b's revision", "agent-b", 0.6),
        "r",
    )
    .await
    .expect("a team belief is visible to another agent and may be superseded");
    assert_eq!(belief_state(&fx, &team).await, "superseded");
    assert_eq!(supersessions_of(&fx, &team).await, 1);

    fx.drop_db().await;
}

#[tokio::test]
async fn contesting_another_agents_private_belief_is_refused() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let at = dt("2026-01-01T00:00:00Z");
    let a = ctx_at("agent-a", at);
    let b = ctx_at("agent-b", at);
    let private = w
        .assert_belief(
            &a,
            belief_vis("a's private claim", "agent-a", 0.5, Visibility::Private),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    let own = w
        .assert_belief(
            &b,
            belief("b's own claim", "agent-b", 0.5),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    let msg = storage_msg(
        w.contest(&b, &[own.clone(), private.clone()])
            .await
            .expect_err("another agent's private belief cannot be contested"),
    );
    for want in [
        private.0.as_str(),
        "visible to the caller",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "{want}: {msg}");
    }
    let anomalies = fx
        .raw_fetch(r#"match $a isa anomaly, has concept-id $c; fetch { "c": $c };"#)
        .await;
    assert!(anomalies.is_empty(), "no anomaly was committed");

    fx.drop_db().await;
}

#[tokio::test]
async fn deriving_from_another_agents_private_belief_is_refused() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let at = dt("2026-01-01T00:00:00Z");
    observe_as(&w, "agent-b", at).await;
    let private = w
        .assert_belief(
            &ctx_at("agent-a", at),
            belief_vis("a's private evidence", "agent-a", 0.5, Visibility::Private),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    let msg = storage_msg(
        w.assert_belief(
            &ctx_at("agent-b", at),
            belief("b's derived belief", "agent-b", 0.5),
            std::slice::from_ref(&private),
            DerivationMethod::LlmSynthesis,
        )
        .await
        .expect_err("another agent's private belief cannot be evidence"),
    );
    for want in [
        private.0.as_str(),
        "visible to the caller",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "{want}: {msg}");
    }
    let stored = fx
        .raw_fetch(r#"match $b isa belief, has title "b's derived belief"; fetch { "k": "x" };"#)
        .await;
    assert!(stored.is_empty(), "no belief was committed");

    fx.drop_db().await;
}

#[tokio::test]
async fn deriving_from_another_agents_episode_is_refused() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let at = dt("2026-01-01T00:00:00Z");
    observe_as(&w, "agent-b", at).await;
    let episode = w
        .observe(
            &ctx_at("agent-a", at),
            EpisodeDraft {
                text: "a's observation".into(),
                event_time: at,
                ingest_time: at,
            },
        )
        .await
        .unwrap();

    let msg = storage_msg(
        w.assert_belief(
            &ctx_at("agent-b", at),
            belief("b's belief from a's episode", "agent-b", 0.5),
            std::slice::from_ref(&episode),
            DerivationMethod::LlmSynthesis,
        )
        .await
        .expect_err("another agent's episode cannot be evidence"),
    );
    for want in [
        episode.0.as_str(),
        "visible to the caller",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "{want}: {msg}");
    }

    fx.drop_db().await;
}

#[tokio::test]
async fn deriving_from_a_team_belief_and_an_own_episode_is_allowed() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let at = dt("2026-01-01T00:00:00Z");
    let b = ctx_at("agent-b", at);
    let episode = w
        .observe(
            &b,
            EpisodeDraft {
                text: "b's observation".into(),
                event_time: at,
                ingest_time: at,
            },
        )
        .await
        .unwrap();
    let team = w
        .assert_belief(
            &ctx_at("agent-a", at),
            belief_vis("a's team evidence", "agent-a", 0.5, Visibility::Team),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    w.assert_belief(
        &b,
        belief("b's synthesis", "agent-b", 0.5),
        &[team, episode],
        DerivationMethod::LlmSynthesis,
    )
    .await
    .expect("a team belief and the caller's own episode are visible evidence");

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// A write whose ids match nothing fails and commits nothing.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn derive_with_a_missing_evidence_id_errors_and_commits_nothing() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));
    let ep = w
        .observe(
            &ctx,
            EpisodeDraft {
                text: "real evidence".into(),
                event_time: ctx.occurred_at,
                ingest_time: ctx.occurred_at,
            },
        )
        .await
        .expect("observe");

    let msg = storage_msg(
        w.assert_belief(
            &ctx,
            belief("orphan derivation", "agent-1", 0.5),
            &[ep.clone(), MemId::new("mem/ep/does-not-exist")],
            DerivationMethod::LlmSynthesis,
        )
        .await
        .expect_err("a missing evidence id must fail the derive"),
    );
    for want in [
        ep.0.as_str(),
        "mem/ep/does-not-exist",
        "no memory item",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "{want}: {msg}");
    }
    let stored = fx
        .raw_fetch(
            r#"match $b isa belief, has title $t; $t == "orphan derivation"; fetch { "t": $t };"#,
        )
        .await;
    assert!(stored.is_empty(), "no belief was committed");

    fx.drop_db().await;
}

#[tokio::test]
async fn contest_with_a_missing_claim_errors_and_commits_nothing() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));
    let b = w
        .assert_belief(
            &ctx,
            belief("a real claim", "agent-1", 0.5),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .expect("assert");

    let msg = storage_msg(
        w.contest(&ctx, &[b.clone(), MemId::new("mem/bel/does-not-exist")])
            .await
            .expect_err("a missing claim must fail the contest"),
    );
    for want in [
        b.0.as_str(),
        "mem/bel/does-not-exist",
        "no belief",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "{want}: {msg}");
    }
    let anomalies = fx
        .raw_fetch(r#"match $a isa anomaly, has concept-id $c; fetch { "c": $c };"#)
        .await;
    assert!(anomalies.is_empty(), "no anomaly was committed");

    fx.drop_db().await;
}

#[tokio::test]
async fn rests_on_with_a_missing_assumption_errors_and_commits_nothing() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));
    let b = w
        .assert_belief(
            &ctx,
            belief("resting belief", "agent-1", 0.5),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .expect("assert");

    let msg = storage_msg(
        w.rests_on(&ctx, b.clone(), &[MemId::new("mem/bel/does-not-exist")])
            .await
            .expect_err("a missing assumption must fail rests_on"),
    );
    for want in [
        b.0.as_str(),
        "mem/bel/does-not-exist",
        "no memory item",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "{want}: {msg}");
    }
    let edges = fx
        .raw_fetch(r#"match $r isa rests-on; $r links (resting: $x); $x has concept-id $c; fetch { "c": $c };"#)
        .await;
    assert!(edges.is_empty(), "no rests-on was committed");

    fx.drop_db().await;
}

#[tokio::test]
async fn record_prediction_on_a_missing_belief_errors_and_commits_nothing() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));

    let msg = storage_msg(
        w.record_prediction(&ctx, MemId::new("mem/bel/does-not-exist"), 0.3)
            .await
            .expect_err("a missing belief must fail record_prediction"),
    );
    for want in [
        "mem/bel/does-not-exist",
        "does not exist",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "{want}: {msg}");
    }
    let preds = fx
        .raw_fetch(r#"match $p isa prediction-resolution, has predicted-probability $v; fetch { "v": $v };"#)
        .await;
    assert!(preds.is_empty(), "no prediction was committed");

    fx.drop_db().await;
}

#[tokio::test]
async fn recording_a_second_open_prediction_is_refused_and_commits_nothing() {
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

    let msg = storage_msg(
        w.record_prediction(&ctx, b.clone(), 0.7)
            .await
            .expect_err("a second open prediction must be refused"),
    );
    for want in [
        b.0.as_str(),
        "already has an open prediction",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "{want}: {msg}");
    }
    let preds = fx
        .raw_fetch(r#"match $p isa prediction-resolution, has predicted-probability $v; fetch { "v": $v };"#)
        .await;
    assert_eq!(preds.len(), 1, "exactly one prediction-resolution exists");
    w.resolve_prediction(&ctx, b, Outcome::Confirmed)
        .await
        .expect("the single open prediction still resolves");

    fx.drop_db().await;
}

#[tokio::test]
async fn recording_a_prediction_after_resolution_is_allowed() {
    use gecko_extension_api::Outcome;
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));
    let b = w
        .assert_belief(
            &ctx,
            belief("it will hail", "agent-1", 0.6),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    w.record_prediction(&ctx, b.clone(), 0.6).await.unwrap();
    w.resolve_prediction(&ctx, b.clone(), Outcome::Confirmed)
        .await
        .unwrap();
    w.record_prediction(&ctx, b.clone(), 0.8)
        .await
        .expect("a resolved prediction no longer blocks a new one");

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// An equal-or-higher entrenchment supersede SUCCEEDS.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn supersede_allows_equal_or_higher_entrenchment() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));

    // EQUAL: a user-stated belief superseded by another user-stated belief.
    let v1 = w
        .assert_belief(
            &ctx,
            belief("v1", "agent-1", 0.8),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    let v2 = w
        .supersede(
            &ctx,
            v1.clone(),
            belief_ent("v2", "agent-1", 0.9, Entrenchment::UserStated),
            "revised by the same authority",
        )
        .await
        .expect("equal-entrenchment supersede succeeds");
    assert!(w.is_superseded(&v1).await.unwrap(), "v1 is superseded");
    assert!(!w.is_superseded(&v2).await.unwrap(), "v2 is current");

    // HIGHER: an llm belief superseded by a more-entrenched inferred belief.
    let weak = w
        .assert_belief(
            &ctx,
            belief("weak llm claim", "agent-1", 0.4),
            &[],
            DerivationMethod::LlmSynthesis,
        )
        .await
        .unwrap();
    let stronger = w
        .supersede(
            &ctx,
            weak.clone(),
            belief_ent(
                "stronger inferred claim",
                "agent-1",
                0.6,
                Entrenchment::Inferred,
            ),
            "upgraded",
        )
        .await
        .expect("higher-entrenchment supersede succeeds");
    assert!(
        w.is_superseded(&weak).await.unwrap(),
        "the weak llm belief is superseded by a more-entrenched one"
    );
    assert!(!w.is_superseded(&stronger).await.unwrap());

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

/// Has `actor` write one episode at `at`, so its `agent` node exists.
async fn observe_as(w: &MemWriter, actor: &str, at: DateTime) {
    w.observe(
        &ctx_at(actor, at),
        EpisodeDraft {
            text: format!("{actor} was here"),
            event_time: at,
            ingest_time: at,
        },
    )
    .await
    .expect("observe commits");
}

/// An as-of recall by `actor` at `run_at`, asking for the state at `as_of`.
async fn recall_as_of(
    w: &MemWriter,
    actor: &str,
    run_at: DateTime,
    as_of: DateTime,
    max_chunks: usize,
) -> Vec<MemId> {
    w.recall(
        &ctx_at(actor, run_at),
        RecallQuery {
            text: String::new(),
            as_of: Some(as_of),
        },
        ContextBudget {
            max_chunks,
            max_tokens: None,
        },
    )
    .await
    .expect("as-of recall")
    .into_iter()
    .map(|c| c.id)
    .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// As-of recall applies the same visibility rule as present-state recall.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn as_of_recall_hides_another_agents_private_belief() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    observe_as(&w, "agent-b", dt("2026-01-01T00:00:00Z")).await;
    let private = w
        .assert_belief(
            &ctx_at("agent-a", dt("2026-01-01T00:00:00Z")),
            belief_vis("a's private belief", "agent-a", 0.8, Visibility::Private),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    let got = recall_as_of(
        &w,
        "agent-b",
        dt("2026-06-01T00:00:00Z"),
        dt("2026-02-01T00:00:00Z"),
        5,
    )
    .await;
    assert!(
        !got.contains(&private),
        "another agent's private belief must not surface in an as-of recall: {got:?}"
    );

    fx.drop_db().await;
}

#[tokio::test]
async fn as_of_recall_returns_a_team_belief_to_another_agent() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    observe_as(&w, "agent-b", dt("2026-01-01T00:00:00Z")).await;
    let team = w
        .assert_belief(
            &ctx_at("agent-a", dt("2026-01-01T00:00:00Z")),
            belief_vis("a's team belief", "agent-a", 0.8, Visibility::Team),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    let got = recall_as_of(
        &w,
        "agent-b",
        dt("2026-06-01T00:00:00Z"),
        dt("2026-02-01T00:00:00Z"),
        5,
    )
    .await;
    assert_eq!(got, vec![team], "a team belief is visible to another agent");

    fx.drop_db().await;
}

#[tokio::test]
async fn as_of_recall_returns_the_owners_private_belief() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    observe_as(&w, "agent-b", dt("2026-01-01T00:00:00Z")).await;
    let private = w
        .assert_belief(
            &ctx_at("agent-b", dt("2026-01-01T00:00:00Z")),
            belief_vis("b's private belief", "agent-b", 0.8, Visibility::Private),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    let got = recall_as_of(
        &w,
        "agent-b",
        dt("2026-06-01T00:00:00Z"),
        dt("2026-02-01T00:00:00Z"),
        5,
    )
    .await;
    assert_eq!(got, vec![private], "the owner sees its own private belief");

    fx.drop_db().await;
}

#[tokio::test]
async fn as_of_recall_is_newest_first_and_truncates_deterministically() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    observe_as(&w, "agent-b", dt("2026-01-01T00:00:00Z")).await;
    let mut ids = Vec::new();
    for (i, at) in [
        "2026-01-01T00:00:00Z",
        "2026-01-02T00:00:00Z",
        "2026-01-03T00:00:00Z",
    ]
    .iter()
    .enumerate()
    {
        let id = w
            .assert_belief(
                &ctx_at("agent-a", dt(at)),
                belief_vis(
                    &format!("team belief {i}"),
                    "agent-a",
                    0.8,
                    Visibility::Team,
                ),
                &[],
                DerivationMethod::HumanAssertion,
            )
            .await
            .unwrap();
        ids.push(id);
    }
    let want = vec![ids[2].clone(), ids[1].clone()];

    for call in 0..2 {
        let got = recall_as_of(
            &w,
            "agent-b",
            dt("2026-06-01T00:00:00Z"),
            dt("2026-02-01T00:00:00Z"),
            2,
        )
        .await;
        assert_eq!(
            got, want,
            "call {call}: the two newest valid-from, newest first"
        );
    }

    let tied_at = dt("2026-01-04T00:00:00Z");
    let mut tied = Vec::new();
    for i in 0..2 {
        tied.push(
            w.assert_belief(
                &ctx_at("agent-a", tied_at),
                belief_vis(
                    &format!("tied belief {i}"),
                    "agent-a",
                    0.8,
                    Visibility::Team,
                ),
                &[],
                DerivationMethod::HumanAssertion,
            )
            .await
            .unwrap(),
        );
    }
    tied.sort_by(|x, y| x.0.cmp(&y.0));
    let got = recall_as_of(
        &w,
        "agent-b",
        dt("2026-06-01T00:00:00Z"),
        dt("2026-02-01T00:00:00Z"),
        3,
    )
    .await;
    assert_eq!(
        got,
        vec![tied[0].clone(), tied[1].clone(), ids[2].clone()],
        "beliefs sharing a valid-from are ordered by concept id"
    );

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// An actor that has never written has no agent node, so its as-of recall is empty.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn as_of_recall_for_an_actor_with_no_history_is_empty() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    w.assert_belief(
        &ctx_at("agent-a", dt("2026-01-01T00:00:00Z")),
        belief_vis("a's shared belief", "agent-a", 0.8, Visibility::Shared),
        &[],
        DerivationMethod::HumanAssertion,
    )
    .await
    .unwrap();

    let got = recall_as_of(
        &w,
        "agent-never-wrote",
        dt("2026-06-01T00:00:00Z"),
        dt("2026-02-01T00:00:00Z"),
        5,
    )
    .await;
    assert!(got.is_empty(), "{got:?}");

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
    fx.raw_seed(
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
// Acceptance: a manual dedup of two duplicate episodes (same title, event-time and owners) tombstones
// the loser in the graph AND removes the loser's vector from the index (via the
// best-effort SemanticIndex::remove hook, invariant 8).
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn dedup_episodes_tombstones_loser_and_removes_its_vector() {
    let fx = Fixture::new().await;
    let index = Arc::new(RecordingIndex::default());
    // Embedder + index both present ⇒ observe upserts each episode's vector, and
    // dedup will attempt the (real) remove of the loser.
    let w = fx.writer(Some(Arc::new(StubEmbedder)), Some(index.clone()));
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));

    // Two duplicate episodes: same observed text, event-time and owner.
    let dup = EpisodeDraft {
        text: "the host phoned home at 03:14".into(),
        event_time: dt("2026-01-01T00:00:00Z"),
        ingest_time: dt("2026-01-01T00:00:00Z"),
    };
    let keeper = w.observe(&ctx, dup.clone()).await.unwrap();
    let loser = w
        .observe(
            &ctx,
            EpisodeDraft {
                ingest_time: dt("2026-01-01T00:00:01Z"),
                ..dup.clone()
            },
        )
        .await
        .unwrap();

    // The manual consolidation op.
    let outcome = w
        .dedup_episodes(&ctx, &keeper, &loser)
        .await
        .expect("dedup of two identical-hash episodes succeeds");
    assert_eq!(outcome, IndexOutcome::Removed);

    // One consolidation-step ties the loser to this run and actor.
    let steps = fx
        .raw_fetch(&format!(
            r#"match
                 $l isa episode, has concept-id "{}";
                 $r isa doc-run, has run-id "{}";
                 $a isa agent, has agent-id "agent-1";
                 $s isa consolidation-step, links (affected: $l, run: $r, performer: $a),
                     has consolidation-action $act, has created-at $at;
               fetch {{ "act": $act, "at": $at }};"#,
            loser.0, ctx.run_id
        ))
        .await;
    assert_eq!(steps.len(), 1, "exactly one consolidation-step");
    assert_eq!(steps[0]["act"], "tombstone");
    let at = steps[0]["at"].as_str().expect("created-at is a string");
    assert!(
        at.starts_with("2026-01-01T00:00:00"),
        "created-at is ctx.occurred_at: {at}"
    );
    let all_steps = fx
        .raw_fetch(r#"match $s isa consolidation-step; fetch { "k": "x" };"#)
        .await;
    assert_eq!(all_steps.len(), 1, "no other step was recorded");

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
               fetch {{ "s": $s? }};"#,
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
// Dedup is PROVABLE — two episodes with DIFFERENT content are never collapsed.
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
                ingest_time: dt("2026-01-01T00:00:01Z"),
            },
        )
        .await
        .unwrap();

    let err = w
        .dedup_episodes(&ctx, &a, &b)
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

/// Seeds an episode of `kind` straight into the graph, with the given optional
/// title and a non-empty owner set held by its one ownership (`observe` always
/// writes a title and exactly one owner).
async fn seed_episode(
    fx: &Fixture,
    kind: &str,
    id: &str,
    title: Option<&str>,
    event: &str,
    ingest: &str,
    owners: &[&str],
) {
    assert!(!owners.is_empty(), "the schema requires an owner for {id}");
    let mut owner_match = String::new();
    let mut owner_links = String::new();
    for (i, o) in owners.iter().enumerate() {
        fx.raw_write(&format!(r#"put $a isa agent, has agent-id "{o}";"#))
            .await;
        owner_match.push_str(&format!(r#"$a{i} isa agent, has agent-id "{o}"; "#));
        owner_links.push_str(&format!(", owner: $a{i}"));
    }
    let title = title
        .map(|t| format!(r#", has title "{t}""#))
        .unwrap_or_default();
    fx.raw_seed(&format!(
        r#"match {owner_match} insert $e isa {kind}, has concept-id "{id}"{title},
             has event-time {event}, has ingest-time {ingest};
             (owned: $e{owner_links}) isa ownership;"#
    ))
    .await;
}

/// Orders two ids as (earlier-concept-id, later-concept-id).
fn by_id(a: MemId, b: MemId) -> (MemId, MemId) {
    if a.0 < b.0 { (a, b) } else { (b, a) }
}

// ─────────────────────────────────────────────────────────────────────────────
// Every way a pair can fail the duplicate rule is refused as InvalidInput naming
// both ids, and the guarded write commits neither a tombstone nor a step.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn dedup_episodes_refuses_and_commits_nothing() {
    let fx = Fixture::new().await;
    let index = Arc::new(RecordingIndex::default());
    let w = fx.writer(Some(Arc::new(StubEmbedder)), Some(index.clone()));
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));
    let (t0, t1) = ("2026-01-01T00:00:00", "2026-01-01T00:00:01");
    let mut cases: Vec<(&str, MemId, MemId)> = Vec::new();
    let mid = |s: &str| MemId(s.to_string());

    // Different event time.
    seed_episode(&fx, "episode", "e-time-k", Some("t"), t0, t0, &["x"]).await;
    seed_episode(&fx, "episode", "e-time-l", Some("t"), t1, t1, &["x"]).await;
    cases.push(("event-time", mid("e-time-k"), mid("e-time-l")));

    // Owner sets {x} vs {x, y}.
    seed_episode(&fx, "episode", "e-own-k", Some("t"), t0, t0, &["x"]).await;
    seed_episode(&fx, "episode", "e-own-l", Some("t"), t0, t1, &["x", "y"]).await;
    cases.push(("owners-subset", mid("e-own-k"), mid("e-own-l")));
    seed_episode(&fx, "episode", "e-own2-k", Some("t"), t0, t0, &["x", "y"]).await;
    seed_episode(&fx, "episode", "e-own2-l", Some("t"), t0, t1, &["x"]).await;
    cases.push(("owners-superset", mid("e-own2-k"), mid("e-own2-l")));

    // An unowned episode cannot be seeded: the schema refuses it at commit.
    let unowned = fx
        .try_raw_write(&format!(
            r#"insert $run isa doc-run, has run-id "unowned-run";
                 $e isa episode, has concept-id "e-un-l", has title "t",
                     has event-time {t1}, has ingest-time {t1};
                 (memory: $e, origin: $run) isa source-link, has origin-kind "test";"#
        ))
        .await;
    let err = unowned.expect_err("an unowned episode commits");
    assert!(
        err.contains("ownership:owned"),
        "the refusal does not name ownership:owned: {err}"
    );

    // Without a title.
    seed_episode(&fx, "episode", "e-nt-k", None, t0, t0, &["x"]).await;
    seed_episode(&fx, "episode", "e-nt-l", None, t0, t1, &["x"]).await;
    cases.push(("untitled", mid("e-nt-k"), mid("e-nt-l")));

    // Reversed: the later ingest is the keeper.
    seed_episode(&fx, "episode", "e-rev-a", Some("t"), t0, t0, &["x"]).await;
    seed_episode(&fx, "episode", "e-rev-b", Some("t"), t0, t1, &["x"]).await;
    cases.push(("reversed", mid("e-rev-b"), mid("e-rev-a")));

    // Tied ingest time with the higher concept-id as keeper.
    seed_episode(&fx, "episode", "e-tie-a", Some("t"), t0, t0, &["x"]).await;
    seed_episode(&fx, "episode", "e-tie-b", Some("t"), t0, t0, &["x"]).await;
    cases.push(("tie-higher-keeper", mid("e-tie-b"), mid("e-tie-a")));

    // An already-tombstoned loser.
    seed_episode(&fx, "episode", "e-tomb-k", Some("t"), t0, t0, &["x"]).await;
    seed_episode(&fx, "episode", "e-tomb-l", Some("t"), t0, t1, &["x"]).await;
    fx.raw_write(
        r#"match $e isa episode, has concept-id "e-tomb-l";
           insert $e has consolidation-state "tombstoned";"#,
    )
    .await;
    cases.push(("tombstoned-loser", mid("e-tomb-k"), mid("e-tomb-l")));

    // Two titled execution-episodes with the same owner and times (isa! refuses).
    seed_episode(
        &fx,
        "execution-episode",
        "e-ex-k",
        Some("t"),
        t0,
        t0,
        &["x"],
    )
    .await;
    seed_episode(
        &fx,
        "execution-episode",
        "e-ex-l",
        Some("t"),
        t0,
        t1,
        &["x"],
    )
    .await;
    cases.push(("execution-episode", mid("e-ex-k"), mid("e-ex-l")));

    // Two prediction-resolving episodes resolved under ONE ctx (same title and
    // event time), keeper ordered first so only the resolution refuses them.
    let rctx = ctx_at("agent-1", dt("2026-01-02T00:00:00Z"));
    let mut resolving = Vec::new();
    for text in ["belief one", "belief two"] {
        let b = w
            .assert_belief(
                &rctx,
                belief(text, "agent-1", 0.6),
                &[],
                DerivationMethod::HumanAssertion,
            )
            .await
            .unwrap();
        w.record_prediction(&rctx, b.clone(), 0.6).await.unwrap();
        w.resolve_prediction(&rctx, b, gecko_extension_api::Outcome::Confirmed)
            .await
            .unwrap();
        let ids = fx
            .raw_fetch(
                r#"match $e isa episode, has title "prediction resolved: confirmed",
                         has concept-id $c; fetch { "c": $c };"#,
            )
            .await;
        resolving = ids
            .iter()
            .map(|d| MemId(d["c"].as_str().unwrap().to_string()))
            .collect();
    }
    assert_eq!(resolving.len(), 2, "two resolving episodes");
    let (rk, rl) = by_id(resolving[0].clone(), resolving[1].clone());
    cases.push(("prediction-resolving", rk, rl));

    for (name, keeper, loser) in &cases {
        let err = w.dedup_episodes(&ctx, keeper, loser).await.expect_err(name);
        let EpistemicError::InvalidInput(msg) = &err else {
            panic!("{name}: expected InvalidInput, got {err:?}");
        };
        assert!(
            msg.contains(&keeper.0) && msg.contains(&loser.0),
            "{name}: message must name both ids: {msg}"
        );
    }

    let steps = fx
        .raw_fetch(r#"match $s isa consolidation-step; fetch { "k": "x" };"#)
        .await;
    assert!(steps.is_empty(), "no consolidation-step was written");
    let tombstones = fx
        .raw_fetch(
            r#"match $e isa episode, has consolidation-state $s, has concept-id $c; fetch { "c": $c };"#,
        )
        .await;
    assert_eq!(
        tombstones.len(),
        1,
        "only the seeded tombstone exists: {tombstones:?}"
    );
    assert!(
        index.removed.lock().unwrap().is_empty(),
        "no vector was removed"
    );
    let runs = fx
        .raw_fetch(&format!(
            r#"match $r isa doc-run, has run-id "{}"; fetch {{ "k": "x" }};"#,
            ctx.run_id
        ))
        .await;
    assert!(runs.is_empty(), "the refused writes rolled back the run");

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// The seeded shapes the refusal test rejects are accepted when they do satisfy
// the rule: identical multi-owner sets, and a tied ingest time with the lowest
// concept-id as keeper.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn dedup_episodes_accepts_identical_owner_sets_and_id_tie_break() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));
    let t0 = "2026-01-01T00:00:00";
    seed_episode(&fx, "episode", "e-ok-a", Some("t"), t0, t0, &["x", "y"]).await;
    seed_episode(&fx, "episode", "e-ok-b", Some("t"), t0, t0, &["y", "x"]).await;

    let outcome = w
        .dedup_episodes(&ctx, &MemId("e-ok-a".into()), &MemId("e-ok-b".into()))
        .await
        .expect("identical owner sets and a lower-id keeper are a duplicate");
    assert_eq!(outcome, IndexOutcome::NoIndex);

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// A tombstoned episode is not current state: a rebuild does not re-embed it.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn index_rebuild_skips_a_tombstoned_episode() {
    let fx = Fixture::new().await;
    let w = fx.writer(
        Some(Arc::new(StubEmbedder)),
        Some(Arc::new(RecordingIndex::default())),
    );
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));
    let draft = EpisodeDraft {
        text: "seen twice".into(),
        event_time: dt("2026-01-01T00:00:00Z"),
        ingest_time: dt("2026-01-01T00:00:00Z"),
    };
    let keeper = w.observe(&ctx, draft.clone()).await.unwrap();
    let loser = w
        .observe(
            &ctx,
            EpisodeDraft {
                ingest_time: dt("2026-01-01T00:00:01Z"),
                ..draft
            },
        )
        .await
        .unwrap();

    let before: Vec<String> = w
        .enumerate_current_state_embeddables()
        .await
        .unwrap()
        .into_iter()
        .map(|(id, _, _)| id.0)
        .collect();
    assert!(before.contains(&keeper.0) && before.contains(&loser.0));

    w.dedup_episodes(&ctx, &keeper, &loser).await.unwrap();

    let after: Vec<String> = w
        .enumerate_current_state_embeddables()
        .await
        .unwrap()
        .into_iter()
        .map(|(id, _, _)| id.0)
        .collect();
    assert_eq!(
        after,
        vec![keeper.0.clone()],
        "only the keeper is embeddable"
    );

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Retrieval-provenance retention: a retrieval-event is safe to tombstone ONLY
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
    fx.raw_seed(&format!(
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

/// The prediction-resolving episodes in the graph.
async fn resolving_episodes(fx: &Fixture) -> usize {
    fx.raw_fetch(
        r#"match $e isa episode, has title $t; $t like "^prediction resolved"; fetch { "t": $t };"#,
    )
    .await
    .len()
}

#[tokio::test]
async fn resolve_prediction_without_a_recorded_prediction_is_refused_and_commits_nothing() {
    use gecko_extension_api::Outcome;
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));
    let b = w
        .assert_belief(
            &ctx,
            belief("never predicted", "agent-1", 0.6),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    let msg = storage_msg(
        w.resolve_prediction(&ctx, b.clone(), Outcome::Confirmed)
            .await
            .expect_err("no recorded prediction ⇒ nothing to resolve"),
    );
    for want in [
        b.0.as_str(),
        "matched 0 open prediction(s)",
        "exactly one is required",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "{want}: {msg}");
    }
    assert_eq!(resolving_episodes(&fx).await, 0, "no resolving episode");

    fx.drop_db().await;
}

#[tokio::test]
async fn resolving_a_prediction_twice_is_refused() {
    use gecko_extension_api::Outcome;
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));
    let b = w
        .assert_belief(
            &ctx,
            belief("it will snow", "agent-1", 0.6),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    w.record_prediction(&ctx, b.clone(), 0.6).await.unwrap();
    w.resolve_prediction(&ctx, b.clone(), Outcome::Confirmed)
        .await
        .expect("the first resolution succeeds");

    let msg = storage_msg(
        w.resolve_prediction(&ctx, b.clone(), Outcome::Refuted)
            .await
            .expect_err("a resolved prediction cannot be resolved again"),
    );
    for want in [
        b.0.as_str(),
        "matched 0 open prediction(s)",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "{want}: {msg}");
    }
    assert_eq!(
        resolving_episodes(&fx).await,
        1,
        "only the first resolution"
    );
    let outcomes = fx
        .raw_fetch(&format!(
            r#"match $b isa belief, has concept-id "{}";
                 $pr isa prediction-resolution, links (predicted-belief: $b), has outcome $o;
               fetch {{ "o": $o }};"#,
            b.0
        ))
        .await;
    assert_eq!(outcomes.len(), 1, "one outcome: {outcomes:?}");
    assert_eq!(outcomes[0]["o"], "confirmed");

    fx.drop_db().await;
}

/// A writer recording retrieval provenance whose recalls surface `evidence`.
fn recalling_writer(fx: &Fixture, evidence: &MemId) -> MemWriter {
    let index = StubIndex::with(vec![(ConceptId(evidence.0.clone()), 0.9)]);
    fx.writer(Some(Arc::new(StubEmbedder)), Some(index))
        .with_retrieval_provenance(true)
}

async fn recall_once(w: &MemWriter, ctx: &RunContext) {
    let chunks = w
        .recall(
            ctx,
            RecallQuery::now("anything"),
            ContextBudget {
                max_chunks: 10,
                max_tokens: None,
            },
        )
        .await
        .unwrap();
    assert!(!chunks.is_empty(), "the recall surfaced the evidence");
}

#[tokio::test]
async fn a_derive_that_used_two_retrievals_links_both() {
    let fx = Fixture::new().await;
    let ctx = ctx_at("agent-1", dt("2026-01-02T00:00:00Z"));
    let evidence = fx
        .writer(None, None)
        .assert_belief(
            &ctx_at("agent-1", dt("2026-01-01T00:00:00Z")),
            belief("shared evidence", "agent-1", 0.9),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    let w = recalling_writer(&fx, &evidence);
    recall_once(&w, &ctx).await;
    recall_once(&w, &ctx).await;

    let synth = w
        .assert_belief(
            &ctx,
            belief("synthesized from two recalls", "agent-1", 0.7),
            std::slice::from_ref(&evidence),
            DerivationMethod::LlmSynthesis,
        )
        .await
        .expect("a derive over two retrievals commits");

    let links = fx
        .raw_fetch(&format!(
            r#"match
                 $b isa belief, has concept-id "{}";
                 $re isa retrieval-event, has concept-id $rid;
                 (retrieval: $re, synthesized: $b) isa informs-synthesis;
                 (surfacer: $re, item: $ev) isa surfaced, has was-used $u;
                 $ev has concept-id "{}";
               fetch {{ "re": $rid, "u": $u }};"#,
            synth.0, evidence.0
        ))
        .await;
    assert_eq!(
        links.len(),
        2,
        "both retrievals inform the belief: {links:?}"
    );
    assert_ne!(links[0]["re"], links[1]["re"]);
    assert!(links.iter().all(|l| l["u"] == true), "{links:?}");

    fx.drop_db().await;
}

#[tokio::test]
async fn a_retrieval_event_feeding_a_live_belief_is_not_safe_to_tombstone() {
    let fx = Fixture::new().await;
    let ctx = ctx_at("agent-1", dt("2026-01-02T00:00:00Z"));
    let evidence = fx
        .writer(None, None)
        .assert_belief(
            &ctx_at("agent-1", dt("2026-01-01T00:00:00Z")),
            belief("evidence", "agent-1", 0.9),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();
    let w = recalling_writer(&fx, &evidence);
    recall_once(&w, &ctx).await;
    let mut informed = Vec::new();
    for text in ["synthesis one", "synthesis two"] {
        informed.push(
            w.assert_belief(
                &ctx,
                belief(text, "agent-1", 0.7),
                std::slice::from_ref(&evidence),
                DerivationMethod::LlmSynthesis,
            )
            .await
            .unwrap(),
        );
    }
    let events = fx
        .raw_fetch(r#"match $re isa retrieval-event, has concept-id $c; fetch { "c": $c };"#)
        .await;
    assert_eq!(events.len(), 1, "one retrieval-event: {events:?}");
    let event = MemId(events[0]["c"].as_str().unwrap().to_string());

    w.supersede(
        &ctx,
        informed[0].clone(),
        belief("revised one", "agent-1", 0.8),
        "r",
    )
    .await
    .unwrap();
    let partly = w.retrieval_events_safe_to_tombstone().await.unwrap();
    assert!(
        !partly.contains(&event),
        "an event still feeding a live belief is not safe to tombstone: {partly:?}"
    );

    w.supersede(
        &ctx,
        informed[1].clone(),
        belief("revised two", "agent-1", 0.8),
        "r",
    )
    .await
    .unwrap();
    let all = w.retrieval_events_safe_to_tombstone().await.unwrap();
    assert!(
        all.contains(&event),
        "once every informed belief is superseded the event is safe: {all:?}"
    );

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// One store fans reads out concurrently and serializes writers through the gate:
// many parallel recalls all succeed, and many parallel writes all commit atomically.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn store_reads_run_concurrently_and_writes_are_gated() {
    let fx = Fixture::new().await;
    let ctx = ctx_at("agent-1", dt("2026-01-01T00:00:00Z"));

    // Seed a few beliefs so recalls have current-state content to return.
    let seed = fx.writer(None, None);
    const N_SEED: usize = 5;
    for i in 0..N_SEED {
        seed.assert_belief(
            &ctx,
            belief(&format!("seed belief {i}"), "agent-1", 0.7),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .expect("seed commits");
    }

    // Many concurrent recalls through one shared store must all succeed and see the
    // committed state — the read path opens independent transactions with no lock.
    let reader = Arc::new(fx.writer(None, None));
    let reads: Vec<_> = (0..24)
        .map(|_| {
            let r = reader.clone();
            let c = ctx.clone();
            tokio::spawn(async move {
                r.recall(
                    &c,
                    RecallQuery::now("seed"),
                    ContextBudget {
                        max_chunks: 10,
                        max_tokens: None,
                    },
                )
                .await
            })
        })
        .collect();
    for h in reads {
        let chunks = h.await.expect("read task joins").expect("recall ok");
        assert_eq!(
            chunks.len(),
            N_SEED,
            "each concurrent recall sees every seed"
        );
    }

    // Many concurrent writers through one shared store must all commit. The
    // single-permit gate serializes them, so none conflict and each persists.
    let writer = Arc::new(fx.writer(None, None));
    const N_WRITES: usize = 12;
    let writes: Vec<_> = (0..N_WRITES)
        .map(|i| {
            let w = writer.clone();
            let c = ctx.clone();
            tokio::spawn(async move {
                w.assert_belief(
                    &c,
                    belief(&format!("concurrent write {i}"), "agent-2", 0.6),
                    &[],
                    DerivationMethod::HumanAssertion,
                )
                .await
            })
        })
        .collect();
    let mut ids = std::collections::HashSet::new();
    for h in writes {
        let id = h.await.expect("write task joins").expect("assert commits");
        ids.insert(id.0);
    }
    assert_eq!(
        ids.len(),
        N_WRITES,
        "every concurrent write committed a distinct belief"
    );

    // All seed + concurrent writes are durably present (no lost updates from overlap).
    let all = fx
        .raw_fetch(r#"match $b isa belief, has concept-id $c; fetch { "c": $c };"#)
        .await;
    assert_eq!(
        all.len(),
        N_SEED + N_WRITES,
        "all concurrently-written beliefs persisted alongside the seeds"
    );

    fx.drop_db().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Populations writer (SPEC-W2-W3 §3.5): materialization keeps the window and the
// freshness marker in step, closes departed members, and opens a fresh epoch on
// rejoin — so present-state and as-of-t reads both stay correct.
// ─────────────────────────────────────────────────────────────────────────────

const POP: &str = "pop/internet-facing";

async fn seed_concepts(fx: &Fixture, ids: &[&str]) {
    let lines: String = ids
        .iter()
        .enumerate()
        .map(|(i, id)| format!("$c{i} isa concept, has concept-id \"{id}\";\n"))
        .collect();
    fx.raw_write(&format!("insert {lines}")).await;
}

async fn members_at(fx: &Fixture, t: &str) -> Vec<String> {
    let docs = fx
        .raw_fetch(&format!(
            "match $p isa population, has spec-hash \"{POP}\";
                   let $m in population-members-at($p, {t});
                   $m has concept-id $id;
             fetch {{ \"id\": $id }};"
        ))
        .await;
    let mut ids: Vec<String> = docs
        .iter()
        .map(|d| d["id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    ids
}

/// Every row of `member` as (valid-from, valid-to, computed-at), oldest first.
async fn rows_of(fx: &Fixture, member: &str) -> Vec<(String, Option<String>, String)> {
    let docs = fx
        .raw_fetch(&format!(
            "match $p isa population, has spec-hash \"{POP}\";
                   $c isa concept, has concept-id \"{member}\";
                   $pm isa population-member, links (owning-population: $p, member: $c);
             fetch {{ \"vf\": $pm.valid-from, \"vt\": $pm.valid-to, \"ca\": $pm.computed-at }};"
        ))
        .await;
    let mut rows: Vec<_> = docs
        .iter()
        .map(|d| {
            (
                d["vf"].as_str().unwrap().to_string(),
                d["vt"].as_str().map(str::to_string),
                d["ca"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    rows.sort();
    rows
}

fn ids(xs: &[&str]) -> Vec<ConceptId> {
    xs.iter().map(|x| ConceptId::new(*x)).collect()
}

#[tokio::test]
async fn populations_writer_maintains_windows_across_passes() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    seed_concepts(&fx, &["asset/a", "asset/b", "asset/c"]).await;

    assert!(
        w.define_population(POP, "typeql-3", "match $c isa concept;")
            .await
            .unwrap()
    );
    assert!(
        !w.define_population(POP, "typeql-3", "match $c isa concept;")
            .await
            .unwrap(),
        "redefining with the same spec is a no-op"
    );

    let t0 = dt("2026-01-01T00:00:00Z");
    let t1 = dt("2026-02-01T00:00:00Z");
    let t2 = dt("2026-03-01T00:00:00Z");
    let t3 = dt("2026-04-01T00:00:00Z");

    // t0: {a, b}.
    let r = w
        .materialize_population(POP, &ids(&["asset/a", "asset/b"]), t0)
        .await
        .unwrap();
    assert_eq!(
        (r.opens_attempted, r.closes_attempted, r.refreshes_attempted),
        (2, 0, 0)
    );

    // t1: unchanged — valid-from must not move; computed-at advances.
    let r = w
        .materialize_population(POP, &ids(&["asset/a", "asset/b"]), t1)
        .await
        .unwrap();
    assert_eq!(
        (r.opens_attempted, r.closes_attempted, r.refreshes_attempted),
        (0, 0, 2)
    );
    assert_eq!(
        rows_of(&fx, "asset/a").await,
        vec![(
            "2026-01-01T00:00:00.000000000".into(),
            None,
            "2026-02-01T00:00:00.000000000".into()
        )]
    );

    // t2: a leaves, c joins.
    let r = w
        .materialize_population(POP, &ids(&["asset/b", "asset/c"]), t2)
        .await
        .unwrap();
    assert_eq!(
        (r.opens_attempted, r.closes_attempted, r.refreshes_attempted),
        (1, 1, 1)
    );
    assert_eq!(
        rows_of(&fx, "asset/a").await,
        vec![(
            "2026-01-01T00:00:00.000000000".into(),
            Some("2026-03-01T00:00:00.000000000".into()),
            "2026-03-01T00:00:00.000000000".into()
        )],
        "a departed member's window closes AND its computed-at advances"
    );

    // t3: a rejoins — a second epoch row, the closed one stays.
    let r = w
        .materialize_population(POP, &ids(&["asset/a", "asset/b", "asset/c"]), t3)
        .await
        .unwrap();
    assert_eq!(
        (r.opens_attempted, r.closes_attempted, r.refreshes_attempted),
        (1, 0, 2)
    );
    assert_eq!(rows_of(&fx, "asset/a").await.len(), 2);

    // As-of reads follow the epochs; the present-state view excludes closed rows.
    assert_eq!(
        members_at(&fx, "2026-01-15T00:00:00").await,
        vec!["asset/a", "asset/b"]
    );
    assert_eq!(
        members_at(&fx, "2026-03-15T00:00:00").await,
        vec!["asset/b", "asset/c"]
    );
    let mut now = w.population_members(POP).await.unwrap();
    now.sort();
    assert_eq!(now, ids(&["asset/a", "asset/b", "asset/c"]));

    fx.drop_db().await;
}

#[tokio::test]
async fn populations_writer_closes_one_membership() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    seed_concepts(&fx, &["asset/a", "asset/b"]).await;
    w.define_population(POP, "typeql-3", "match $c isa concept;")
        .await
        .unwrap();
    w.materialize_population(
        POP,
        &ids(&["asset/a", "asset/b"]),
        dt("2026-01-01T00:00:00Z"),
    )
    .await
    .unwrap();

    let t1 = dt("2026-02-01T00:00:00Z");
    assert!(
        w.close_population_membership(POP, &ConceptId::new("asset/a"), t1)
            .await
            .unwrap()
    );
    assert!(
        !w.close_population_membership(POP, &ConceptId::new("asset/a"), t1)
            .await
            .unwrap(),
        "an already-closed membership reports nothing closed"
    );
    assert_eq!(w.population_members(POP).await.unwrap(), ids(&["asset/b"]));
    // b's row is untouched by the single close.
    assert_eq!(
        rows_of(&fx, "asset/b").await,
        vec![(
            "2026-01-01T00:00:00.000000000".into(),
            None,
            "2026-01-01T00:00:00.000000000".into()
        )]
    );

    fx.drop_db().await;
}

#[tokio::test]
async fn populations_writer_rejects_bad_input_and_writes_nothing() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    seed_concepts(&fx, &["asset/a"]).await;
    let t0 = dt("2026-01-01T00:00:00Z");

    // Unknown population.
    let err = w
        .materialize_population("pop/missing", &ids(&["asset/a"]), t0)
        .await
        .unwrap_err();
    assert!(matches!(err, EpistemicError::InvalidInput(_)), "{err:?}");

    w.define_population(POP, "typeql-3", "match $c isa concept;")
        .await
        .unwrap();

    // Same hash, different spec.
    let err = w
        .define_population(POP, "typeql-3", "match $c isa concept, has tag \"x\";")
        .await
        .unwrap_err();
    assert!(matches!(err, EpistemicError::InvalidInput(_)), "{err:?}");

    // A member that is not a stored concept fails the whole pass.
    let err = w
        .materialize_population(POP, &ids(&["asset/a", "asset/ghost"]), t0)
        .await
        .unwrap_err();
    assert!(matches!(err, EpistemicError::InvalidInput(_)), "{err:?}");
    assert!(w.population_members(POP).await.unwrap().is_empty());

    // A pass older than the population's latest recorded time.
    w.materialize_population(POP, &ids(&["asset/a"]), dt("2026-02-01T00:00:00Z"))
        .await
        .unwrap();
    let err = w.materialize_population(POP, &[], t0).await.unwrap_err();
    assert!(matches!(err, EpistemicError::InvalidInput(_)), "{err:?}");
    assert_eq!(w.population_members(POP).await.unwrap(), ids(&["asset/a"]));

    // A pass stamped ahead of the host clock.
    let ahead = chrono::Utc::now() + chrono::TimeDelta::hours(1);
    let err = w
        .materialize_population(POP, &ids(&["asset/a"]), ahead)
        .await
        .unwrap_err();
    assert!(matches!(err, EpistemicError::InvalidInput(_)), "{err:?}");
    let err = w
        .close_population_membership(POP, &ConceptId::new("asset/a"), ahead)
        .await
        .unwrap_err();
    assert!(matches!(err, EpistemicError::InvalidInput(_)), "{err:?}");

    fx.drop_db().await;
}

#[tokio::test]
async fn blast_radius_at_follows_the_materialized_window() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    seed_concepts(&fx, &["asset/a"]).await;
    fx.raw_seed(&format!(
        r#"insert
             $p isa population, has spec-hash "{POP}", has spec-dialect "typeql-3",
                 has spec-text "match $c isa concept;";
             $b isa belief, has concept-id "mem/bel/cohort", has belief-state "asserted";
             scoped (scoped-belief: $b, population: $p);"#
    ))
    .await;

    let a = ConceptId::new("asset/a");
    w.materialize_population(POP, std::slice::from_ref(&a), dt("2026-01-01T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(
        present_blast_radius(&fx, "asset/a").await,
        ["mem/bel/cohort"],
        "a current member sweeps its cohort through present-state blast-radius"
    );
    w.materialize_population(POP, &[], dt("2026-03-01T00:00:00Z"))
        .await
        .unwrap();
    assert!(
        present_blast_radius(&fx, "asset/a").await.is_empty(),
        "a departed member no longer sweeps its former cohort"
    );

    assert_eq!(
        w.blast_radius_at(&a, dt("2026-02-01T00:00:00Z"))
            .await
            .unwrap(),
        vec![MemId::new("mem/bel/cohort")],
        "while a was a member, retracting it sweeps the cohort belief"
    );
    assert!(
        w.blast_radius_at(&a, dt("2026-04-01T00:00:00Z"))
            .await
            .unwrap()
            .is_empty(),
        "after a left, retracting it sweeps nothing"
    );

    fx.drop_db().await;
}

/// Present-state `blast-radius($r)` of concept `member`, read directly (the
/// `EpistemicReader` entry point takes a memory item).
async fn present_blast_radius(fx: &Fixture, member: &str) -> Vec<String> {
    let docs = fx
        .raw_fetch(&format!(
            "match $r isa concept, has concept-id \"{member}\";
                   let $m in blast-radius($r);
                   $m has concept-id $mid;
             fetch {{ \"id\": $mid }};"
        ))
        .await;
    let mut out: Vec<String> = docs
        .iter()
        .map(|d| d["id"].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Two writers (separate per-population locks) race overlapping passes; the write
/// guards alone must keep one open row per member and `computed-at` from moving
/// backwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn populations_writer_overlapping_passes_keep_one_open_row() {
    let fx = Fixture::new().await;
    let w1 = fx.writer(None, None);
    let w2 = fx.writer(None, None);
    seed_concepts(&fx, &["asset/a", "asset/b", "asset/c"]).await;
    let (d1, d2) = tokio::join!(
        w1.define_population(POP, "typeql-3", "match $c isa concept;"),
        w2.define_population(POP, "typeql-3", "match $c isa concept;"),
    );
    assert_eq!(
        [d1.unwrap(), d2.unwrap()].iter().filter(|c| **c).count(),
        1,
        "exactly one writer creates the population"
    );

    let rounds: [(&[&str], &[&str]); 4] = [
        (&["asset/a", "asset/b"], &["asset/a", "asset/b"]),
        (&["asset/a", "asset/c"], &["asset/b", "asset/c"]),
        (&["asset/a"], &["asset/a", "asset/b", "asset/c"]),
        (&["asset/b", "asset/c"], &["asset/c"]),
    ];
    let mut latest = String::new();
    for (i, (m1, m2)) in rounds.iter().enumerate() {
        let t1 = dt(&format!("2026-0{}-01T00:00:00Z", i + 1));
        let t2 = dt(&format!("2026-0{}-15T00:00:00Z", i + 1));
        // The later pass may finish first; the earlier one is then refused as
        // stale by the planner or made a no-op by the write guards.
        let (m1, m2) = (ids(m1), ids(m2));
        let (r1, r2) = tokio::join!(
            w1.materialize_population(POP, &m1, t1),
            w2.materialize_population(POP, &m2, t2),
        );
        for r in [r1, r2] {
            if let Err(e) = r {
                assert!(matches!(e, EpistemicError::InvalidInput(_)), "{e:?}");
            }
        }
        for member in ["asset/a", "asset/b", "asset/c"] {
            let rows = rows_of(&fx, member).await;
            assert!(
                rows.iter().filter(|(_, vt, _)| vt.is_none()).count() <= 1,
                "round {i}: {member} has more than one open row: {rows:?}"
            );
            for (vf, vt, _) in &rows {
                if let Some(vt) = vt {
                    assert!(
                        vf <= vt,
                        "round {i}: {member} window closes before it opens"
                    );
                }
            }
        }
        let now_latest = latest_computed_at(&fx).await;
        assert!(
            now_latest >= latest,
            "round {i}: computed-at moved backwards"
        );
        latest = now_latest;
    }
    fx.drop_db().await;
}

async fn latest_computed_at(fx: &Fixture) -> String {
    let docs = fx
        .raw_fetch(&format!(
            "match $p isa population, has spec-hash \"{POP}\";
                   $pm isa population-member, links (owning-population: $p),
                       has computed-at $ca;
             fetch {{ \"ca\": $ca }};"
        ))
        .await;
    docs.iter()
        .filter_map(|d| d["ca"].as_str().map(str::to_string))
        .max()
        .unwrap_or_default()
}
