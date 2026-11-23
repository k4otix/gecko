//! `MemConsolidationDaemon` acceptance tests (live TypeDB).
//!
//! Each test provisions a throwaway database, drives the real daemon over a real
//! [`MemWriter`], and drops the database. The `gecko` database is never touched.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::sync::Mutex;

use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_engine::db::{RouterGraphStore, run_graph_writes};
use gecko_extension_api::GeckoExtension;
use gecko_extension_api::{
    ActorId, ConceptId, DateTime, Embedder, EpisodeDraft, EpistemicError, EpistemicWriter,
    FilterMeta, GraphReader, GraphStore, GraphValue, GraphWrite, MemId, Outcome, ProvenanceSource,
    RunContext, SemanticIndex,
};
use mem_gecko::{ConsolidationDaemon, MemConsolidationDaemon, MemWriter};
use ulid::Ulid;

mod common;
#[path = "common/provenance.rs"]
mod provenance;

const CORE_SCHEMA: &str = include_str!("../../gecko-engine/schema/core_schema.tql");

struct Fixture {
    shared: Arc<Mutex<TypeDbRouter>>,
    name: String,
}

impl Fixture {
    async fn new() -> Self {
        let name = format!("c1test_{}", Ulid::generate().to_string().to_lowercase());
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
    ) -> Arc<MemWriter> {
        let store = Arc::new(RouterGraphStore::new(self.shared.clone()));
        Arc::new(MemWriter::new(store, embedder, index))
    }

    async fn raw_fetch(&self, tql: &str) -> Vec<serde_json::Value> {
        let mut router = self.shared.lock().await;
        let tx = router.begin_read().await.expect("read tx");
        let answer = tx.query(tql).await.expect("read query");
        let mut out = Vec::new();
        if answer.is_document_stream() {
            let mut stream = answer.into_documents();
            while let Some(row) = stream.next().await {
                let doc = row.expect("document row");
                out.push(serde_json::from_str(&doc.into_json().to_string()).unwrap());
            }
        }
        out
    }

    async fn raw_write(&self, tql: &str) {
        let mut router = self.shared.lock().await;
        let tx = router.begin_write().await.expect("write tx");
        tx.query(tql).await.expect("write query");
        tx.commit().await.expect("commit");
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

    /// The ids of the consolidation steps recorded for run `run`.
    async fn steps_for_run(&self, run: &str) -> Vec<String> {
        self.raw_fetch(&format!(
            r#"match
                 $r isa doc-run, has run-id "{run}";
                 $l isa episode, has concept-id $lid;
                 $s isa consolidation-step, links (affected: $l, run: $r);
               fetch {{ "id": $lid }};"#
        ))
        .await
        .iter()
        .map(|d| d["id"].as_str().unwrap().to_string())
        .collect()
    }

    async fn count(&self, ty: &str) -> usize {
        self.raw_fetch(&format!(r#"match $x isa {ty}; fetch {{ "k": "x" }};"#))
            .await
            .len()
    }

    async fn is_tombstoned(&self, id: &str) -> bool {
        !self
            .raw_fetch(&format!(
                r#"match $e isa episode, has concept-id "{id}", has consolidation-state "tombstoned";
                   fetch {{ "k": "x" }};"#
            ))
            .await
            .is_empty()
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

fn ctx(actor: &str) -> RunContext {
    RunContext::new(
        ActorId::new(actor),
        ProvenanceSource::Manual,
        dt("2026-01-01T00:00:00Z"),
    )
}

fn draft(text: &str, event: &str, ingest: &str) -> EpisodeDraft {
    EpisodeDraft {
        text: text.to_string(),
        event_time: dt(event),
        ingest_time: dt(ingest),
    }
}

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

/// An index whose removals always fail.
struct FailingRemoveIndex;
impl SemanticIndex for FailingRemoveIndex {
    fn upsert(&self, _id: ConceptId, _v: &[f32], _m: FilterMeta) -> Result<(), EpistemicError> {
        Ok(())
    }
    fn remove(&self, _id: ConceptId) -> Result<(), EpistemicError> {
        Err(EpistemicError::Storage("index offline".into()))
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

/// Seeds an episode of `kind` straight into the graph with the given owner set.
async fn seed_episode(
    fx: &Fixture,
    kind: &str,
    id: &str,
    title: &str,
    event: &str,
    ingest: &str,
    owners: &[&str],
) {
    let mut owner_match = String::new();
    let mut owner_links = String::new();
    for (i, o) in owners.iter().enumerate() {
        fx.raw_write(&format!(r#"put $a isa agent, has agent-id "{o}";"#))
            .await;
        owner_match.push_str(&format!(r#"$a{i} isa agent, has agent-id "{o}"; "#));
        owner_links.push_str(&format!(", owner: $a{i}"));
    }
    fx.raw_seed(&format!(
        r#"match {owner_match} insert $e isa {kind}, has concept-id "{id}", has title "{title}",
             has event-time {event}, has ingest-time {ingest}; ownership (owned: $e{owner_links});"#
    ))
    .await;
}

/// Three same-owner, same-text, same-time episodes with distinct ingest times;
/// returns them earliest-first.
async fn observe_cohort(w: &MemWriter, text: &str) -> Vec<MemId> {
    let c = ctx("agent-1");
    let mut ids = Vec::new();
    for ingest in [
        "2026-01-01T00:00:00Z",
        "2026-01-01T00:00:01Z",
        "2026-01-01T00:00:02Z",
    ] {
        ids.push(
            w.observe(&c, draft(text, "2026-01-01T00:00:00Z", ingest))
                .await
                .unwrap(),
        );
    }
    ids
}

#[tokio::test]
async fn tick_dedups_a_cohort_and_reports_its_run() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ids = observe_cohort(&w, "the beacon fired").await;
    let daemon = MemConsolidationDaemon::new(w.clone());

    let report = daemon.tick(dt("2026-02-01T00:00:00Z")).await.unwrap();
    assert_eq!(report.episodes_deduped, 2);
    let run = report.run_id.expect("a tick that wrote names its run");

    assert!(!fx.is_tombstoned(&ids[0].0).await, "the earliest survives");
    assert!(fx.is_tombstoned(&ids[1].0).await);
    assert!(fx.is_tombstoned(&ids[2].0).await);
    let mut steps = fx.steps_for_run(&run.to_string()).await;
    steps.sort();
    let mut want = vec![ids[1].0.clone(), ids[2].0.clone()];
    want.sort();
    assert_eq!(steps, want, "one step per loser, all under the tick's run");
    let agent = fx
        .raw_fetch(
            r#"match $a isa agent, has agent-id "consolidation-daemon";
               $s isa consolidation-step, links (performer: $a);
               fetch { "k": "x" };"#,
        )
        .await;
    assert_eq!(agent.len(), 2, "both steps name the daemon agent");
    fx.drop_db().await;
}

#[tokio::test]
async fn tick_leaves_non_duplicates_alone() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let (t0, t1) = ("2026-01-01T00:00:00", "2026-01-01T00:00:01");
    // Different owner sets.
    seed_episode(&fx, "episode", "own-a", "same", t0, t0, &["x"]).await;
    seed_episode(&fx, "episode", "own-b", "same", t0, t1, &["y"]).await;
    // Different event times.
    seed_episode(&fx, "episode", "time-a", "same-t", t0, t0, &["x"]).await;
    seed_episode(&fx, "episode", "time-b", "same-t", t1, t1, &["x"]).await;
    // Two retrieval events with identical shape.
    seed_episode(&fx, "retrieval-event", "re-a", "recall", t0, t0, &["x"]).await;
    seed_episode(&fx, "retrieval-event", "re-b", "recall", t0, t1, &["x"]).await;
    // Two same-outcome prediction resolutions.
    let c = ctx("agent-1");
    for text in ["p1", "p2"] {
        let b = w
            .assert_belief(
                &c,
                gecko_extension_api::BeliefDraft {
                    text: text.into(),
                    owner: ActorId::new("agent-1"),
                    visibility: gecko_extension_api::Visibility::Private,
                    confidence: Some(0.5),
                    entrenchment: None,
                },
                &[],
                gecko_extension_api::DerivationMethod::LlmSynthesis,
            )
            .await
            .unwrap();
        w.record_prediction(&c, b.clone(), 0.6).await.unwrap();
        w.resolve_prediction(&c, b, Outcome::Confirmed)
            .await
            .unwrap();
    }

    let report = MemConsolidationDaemon::new(w)
        .tick(dt("2026-02-01T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(report.episodes_deduped, 0);
    assert_eq!(fx.count("consolidation-step").await, 0);
    fx.drop_db().await;
}

#[tokio::test]
async fn a_second_tick_does_nothing() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    observe_cohort(&w, "again").await;
    let daemon = MemConsolidationDaemon::new(w);
    assert_eq!(
        daemon
            .tick(dt("2026-02-01T00:00:00Z"))
            .await
            .unwrap()
            .episodes_deduped,
        2
    );
    let (runs, steps, agents) = (
        fx.count("doc-run").await,
        fx.count("consolidation-step").await,
        fx.count("agent").await,
    );

    let second = daemon.tick(dt("2026-02-02T00:00:00Z")).await.unwrap();
    assert_eq!(second.episodes_deduped, 0);
    assert_eq!(second.vectors_removed, 0);
    assert_eq!(second.run_id, None);
    assert_eq!(fx.count("doc-run").await, runs);
    assert_eq!(fx.count("consolidation-step").await, steps);
    assert_eq!(fx.count("agent").await, agents);
    fx.drop_db().await;
}

#[tokio::test]
async fn a_tick_with_no_duplicates_writes_nothing() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    seed_episode(
        &fx,
        "episode",
        "solo",
        "one",
        "2026-01-01T00:00:00",
        "2026-01-01T00:00:00",
        &["x"],
    )
    .await;
    let (runs, agents) = (fx.count("doc-run").await, fx.count("agent").await);

    let report = MemConsolidationDaemon::new(w)
        .tick(dt("2026-02-01T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(report.episodes_deduped, 0);
    assert_eq!(report.run_id, None);
    assert_eq!(fx.count("doc-run").await, runs);
    assert_eq!(fx.count("agent").await, agents);
    assert_eq!(fx.count("consolidation-step").await, 0);
    fx.drop_db().await;
}

#[tokio::test]
async fn ingest_time_ties_keep_the_lowest_id() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let t = "2026-01-01T00:00:00";
    for id in ["tie-c", "tie-a", "tie-b"] {
        seed_episode(&fx, "episode", id, "tied", t, t, &["x"]).await;
    }
    let report = MemConsolidationDaemon::new(w)
        .tick(dt("2026-02-01T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(report.episodes_deduped, 2);
    assert!(
        !fx.is_tombstoned("tie-a").await,
        "the lowest id is the keeper"
    );
    assert!(fx.is_tombstoned("tie-b").await);
    assert!(fx.is_tombstoned("tie-c").await);
    fx.drop_db().await;
}

#[tokio::test]
async fn mixed_owner_cohort_dedups_only_identical_owner_sets() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let (t0, t1) = ("2026-01-01T00:00:00", "2026-01-01T00:00:01");
    seed_episode(&fx, "episode", "x-1", "mix", t0, t0, &["x"]).await;
    seed_episode(&fx, "episode", "x-2", "mix", t0, t1, &["x"]).await;
    seed_episode(&fx, "episode", "xy-1", "mix", t0, t0, &["x", "y"]).await;
    seed_episode(&fx, "episode", "xy-2", "mix", t0, t1, &["y", "x"]).await;

    let report = MemConsolidationDaemon::new(w)
        .tick(dt("2026-02-01T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(report.episodes_deduped, 2);
    assert!(!fx.is_tombstoned("x-1").await && !fx.is_tombstoned("xy-1").await);
    assert!(fx.is_tombstoned("x-2").await && fx.is_tombstoned("xy-2").await);
    fx.drop_db().await;
}

#[tokio::test]
async fn tick_reports_index_outcomes() {
    let fx = Fixture::new().await;
    let index = Arc::new(RecordingIndex::default());
    let w = fx.writer(Some(Arc::new(StubEmbedder)), Some(index.clone()));
    let ids = observe_cohort(&w, "indexed").await;
    let report = MemConsolidationDaemon::new(w)
        .tick(dt("2026-02-01T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(report.vectors_removed, 2);
    assert_eq!(report.vectors_marked_dirty, 0);
    let removed = index.removed.lock().unwrap().clone();
    assert!(!removed.contains(&ConceptId(ids[0].0.clone())));
    fx.drop_db().await;

    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    observe_cohort(&w, "unindexed").await;
    let report = MemConsolidationDaemon::new(w)
        .tick(dt("2026-02-01T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(report.episodes_deduped, 2);
    assert_eq!(report.vectors_removed, 0);
    assert_eq!(report.vectors_marked_dirty, 0);
    fx.drop_db().await;

    let fx = Fixture::new().await;
    let w = fx.writer(
        Some(Arc::new(StubEmbedder)),
        Some(Arc::new(FailingRemoveIndex)),
    );
    observe_cohort(&w, "index offline").await;
    let report = MemConsolidationDaemon::new(w)
        .tick(dt("2026-02-01T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(
        report.episodes_deduped, 2,
        "an index failure never fails the write"
    );
    assert_eq!(report.vectors_removed, 0);
    assert_eq!(report.vectors_marked_dirty, 2);
    fx.drop_db().await;
}

/// A graph whose reads return a fixed duplicate pair and whose writes always fail,
/// recording the ops each write attempt carried.
struct FailingStore {
    attempts: std::sync::Mutex<Vec<String>>,
}

#[async_trait]
impl GraphReader for FailingStore {
    async fn read(
        &self,
        _query: &str,
        _vars: &[String],
        _row: &[GraphValue],
    ) -> Result<Vec<serde_json::Value>, EpistemicError> {
        Ok(["dup-a", "dup-b"]
            .iter()
            .enumerate()
            .map(|(i, id)| {
                serde_json::json!({
                    "id": id,
                    "title": "t",
                    "et": "2026-01-01T00:00:00",
                    "it": format!("2026-01-01T00:00:0{i}"),
                    "owner": "x",
                })
            })
            .collect())
    }
}

#[async_trait]
impl GraphStore for FailingStore {
    async fn write(&self, ops: &[GraphWrite]) -> Result<(), EpistemicError> {
        self.attempts.lock().unwrap().push(format!("{ops:?}"));
        Err(EpistemicError::Storage("disk on fire".into()))
    }
}

#[tokio::test]
async fn a_failing_tick_names_its_run() {
    let store = Arc::new(FailingStore {
        attempts: Default::default(),
    });
    let daemon = MemConsolidationDaemon::new(Arc::new(MemWriter::without_index(store.clone())));
    let err = daemon
        .tick(dt("2026-02-01T00:00:00Z"))
        .await
        .expect_err("the write fails");
    let EpistemicError::Storage(msg) = err else {
        panic!("expected Storage, got {err:?}");
    };
    assert!(msg.contains("disk on fire"), "{msg}");
    let attempts = store.attempts.lock().unwrap().clone();
    assert_eq!(attempts.len(), 1);
    let run = msg
        .split_whitespace()
        .find_map(|w| Ulid::from_string(w.trim_end_matches(':')).ok())
        .unwrap_or_else(|| panic!("no run id in: {msg}"));
    assert!(
        attempts[0].contains(&run.to_string()),
        "the named run is the one the write carried"
    );
}

/// A graph whose cohort read fails.
struct UnreadableStore;

#[async_trait]
impl GraphReader for UnreadableStore {
    async fn read(
        &self,
        _query: &str,
        _vars: &[String],
        _row: &[GraphValue],
    ) -> Result<Vec<serde_json::Value>, EpistemicError> {
        Err(EpistemicError::Storage("read refused".into()))
    }
}

#[async_trait]
impl GraphStore for UnreadableStore {
    async fn write(&self, _ops: &[GraphWrite]) -> Result<(), EpistemicError> {
        panic!("a failed cohort read must not reach a write");
    }
}

#[tokio::test]
async fn a_failing_cohort_read_does_not_name_a_run() {
    let daemon = MemConsolidationDaemon::new(Arc::new(MemWriter::without_index(Arc::new(
        UnreadableStore,
    ))));
    let err = daemon
        .tick(dt("2026-02-01T00:00:00Z"))
        .await
        .expect_err("the read fails");
    let EpistemicError::Storage(msg) = err else {
        panic!("expected Storage, got {err:?}");
    };
    assert!(msg.contains("read refused"), "{msg}");
    assert!(
        !msg.contains("consolidation run"),
        "no run was written: {msg}"
    );
}

#[tokio::test]
async fn a_referenced_loser_is_tombstoned_and_keeps_its_derivation() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ids = observe_cohort(&w, "evidence").await;
    let c = ctx("agent-1");
    let belief = w
        .assert_belief(
            &c,
            gecko_extension_api::BeliefDraft {
                text: "derived from the loser".into(),
                owner: ActorId::new("agent-1"),
                visibility: gecko_extension_api::Visibility::Private,
                confidence: Some(0.7),
                entrenchment: None,
            },
            &[ids[2].clone()],
            gecko_extension_api::DerivationMethod::LlmSynthesis,
        )
        .await
        .unwrap();

    let report = MemConsolidationDaemon::new(w.clone())
        .tick(dt("2026-02-01T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(report.episodes_deduped, 2);
    assert!(fx.is_tombstoned(&ids[2].0).await);
    let chain = gecko_extension_api::EpistemicReader::derivation_chain(&*w, belief)
        .await
        .unwrap();
    assert!(
        chain.contains(&ids[2]),
        "the derivation still reaches the tombstoned loser: {chain:?}"
    );
    fx.drop_db().await;
}

#[tokio::test]
async fn daemon_tick_writes_share_one_run_context() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    observe_cohort(&w, "one run").await;
    let (t0, t1) = ("2026-01-01T00:00:00", "2026-01-01T00:00:01");
    seed_episode(&fx, "episode", "second-a", "other", t0, t0, &["x"]).await;
    seed_episode(&fx, "episode", "second-b", "other", t0, t1, &["x"]).await;

    let report = MemConsolidationDaemon::new(w)
        .tick(dt("2026-02-01T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(report.episodes_deduped, 3, "two cohorts, three losers");
    let run = report.run_id.unwrap().to_string();
    assert_eq!(fx.steps_for_run(&run).await.len(), 3);
    assert_eq!(
        fx.count("consolidation-step").await,
        3,
        "no step escapes the run"
    );
    let runs = fx
        .raw_fetch(
            r#"match $r isa doc-run, has run-id $id; $s isa consolidation-step, links (run: $r);
               fetch { "id": $id };"#,
        )
        .await;
    assert!(runs.iter().all(|d| d["id"] == run.as_str()));
    fx.drop_db().await;
}

/// A graph that tombstones `target` right after the cohort read, so the tick's
/// guarded write for that loser is refused.
struct TombstoneAfterReadStore {
    inner: RouterGraphStore,
    shared: Arc<Mutex<TypeDbRouter>>,
    target: String,
}

#[async_trait]
impl GraphReader for TombstoneAfterReadStore {
    async fn read(
        &self,
        query: &str,
        vars: &[String],
        row: &[GraphValue],
    ) -> Result<Vec<serde_json::Value>, EpistemicError> {
        let docs = self.inner.read(query, vars, row).await?;
        let mut router = self.shared.lock().await;
        let tx = router.begin_write().await.expect("write tx");
        tx.query(&format!(
            r#"match $e isa episode, has concept-id "{}";
               update $e has consolidation-state "tombstoned";"#,
            self.target
        ))
        .await
        .expect("tombstone");
        tx.commit().await.expect("commit");
        Ok(docs)
    }
}

#[async_trait]
impl GraphStore for TombstoneAfterReadStore {
    async fn write(&self, ops: &[GraphWrite]) -> Result<(), EpistemicError> {
        self.inner.write(ops).await
    }
}

#[tokio::test]
async fn a_refused_pair_is_skipped_and_counted_without_aborting_the_tick() {
    let fx = Fixture::new().await;
    let w = fx.writer(None, None);
    let ids = observe_cohort(&w, "raced").await;
    let (t0, t1) = ("2026-01-01T00:00:00", "2026-01-01T00:00:01");
    seed_episode(&fx, "episode", "other-a", "other", t0, t0, &["x"]).await;
    seed_episode(&fx, "episode", "other-b", "other", t0, t1, &["x"]).await;

    let store = Arc::new(TombstoneAfterReadStore {
        inner: RouterGraphStore::new(fx.shared.clone()),
        shared: fx.shared.clone(),
        target: ids[1].0.clone(),
    });
    let daemon = MemConsolidationDaemon::new(Arc::new(MemWriter::without_index(store)));
    let report = daemon.tick(dt("2026-02-01T00:00:00Z")).await.unwrap();

    assert_eq!(report.pairs_refused, 1, "the raced loser is refused");
    assert_eq!(report.episodes_deduped, 2, "the other pairs still dedup");
    assert!(fx.is_tombstoned(&ids[2].0).await);
    assert!(fx.is_tombstoned("other-b").await);
    assert!(!fx.is_tombstoned(&ids[0].0).await);
    fx.drop_db().await;
}
