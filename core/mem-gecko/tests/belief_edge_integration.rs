//! Write-count enforcement acceptance tests (live TypeDB 3.12).
//!
//! These drive [`RouterGraphStore::write`] directly against a live server
//! (`localhost:1729`, admin/password) to prove that a [`GraphWrite`] carrying an
//! expected row count commits only when its `match` bound exactly that many rows,
//! and otherwise rolls the whole transaction back. Each test provisions its own
//! throwaway database, applies core + mem schema, and drops it. The real `gecko`
//! database is never touched.
//!
//! Requires a running TypeDB 3.12 service (CI provides one).

use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::Mutex;

use gecko_engine::db::RouterGraphStore;
use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_extension_api::{
    ActorId, BeliefDraft, DerivationMethod, EpistemicError, EpistemicWriter, GeckoExtension,
    GraphStore, GraphValue, GraphWrite, ProvenanceSource, RunContext, Visibility,
};
use mem_gecko::MemWriter;
use ulid::Ulid;

mod common;

const CORE_SCHEMA: &str = include_str!("../../gecko-engine/schema/core_schema.tql");

/// A test harness: a shared router with core + mem schema applied.
struct Fixture {
    shared: Arc<Mutex<TypeDbRouter>>,
    name: String,
}

impl Fixture {
    async fn new() -> Self {
        let name = format!("betest_{}", Ulid::generate().to_string().to_lowercase());
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

    fn store(&self) -> RouterGraphStore {
        RouterGraphStore::new(self.shared.clone())
    }

    /// Low-level fetch (bypasses the store) for verifying stored graph state.
    async fn raw_fetch(&self, tql: &str) -> Vec<serde_json::Value> {
        let mut router = self.shared.lock().await;
        let tx = router.begin_read().await.expect("read tx");
        let answer = tx.query(tql).await.expect("read query");
        let mut out = Vec::new();
        if answer.is_document_stream() {
            let mut stream = answer.into_documents();
            while let Some(item) = stream.next().await {
                let doc = item.expect("document stream item");
                out.push(serde_json::from_str(&doc.into_json().to_string()).unwrap());
            }
        }
        out
    }

    /// Low-level write (bypasses the store) for seeding fixtures.
    async fn raw_write(&self, tql: &str) {
        let mut router = self.shared.lock().await;
        let tx = router.begin_write().await.expect("write tx");
        tx.query(tql).await.expect("write query");
        tx.commit().await.expect("commit");
    }

    async fn drop_db(&self) {
        let mut router = self.shared.lock().await;
        common::drop_db_with_retry(&mut router, &self.name).await;
    }
}

/// Seeds concepts `c/a` and `c/b`, both tagged `"shared"`.
async fn seed_two_concepts(fx: &Fixture) {
    fx.raw_write(
        r#"insert
             $a isa concept, has concept-id "c/a", has tag "shared";
             $b isa concept, has concept-id "c/b", has tag "shared";"#,
    )
    .await;
}

/// A `hierarchy` insert matching a parent and a child by `concept-id`.
fn hierarchy_op(parent: &str, child: &str) -> GraphWrite {
    GraphWrite::single(
        "given $p: string, $c: string;
         match
           $a isa concept, has concept-id $pa; $pa == $p;
           $b isa concept, has concept-id $cb; $cb == $c;
         insert (parent: $a, child: $b) isa hierarchy;",
        vec!["p".into(), "c".into()],
        vec![GraphValue::from(parent), GraphValue::from(child)],
    )
}

async fn hierarchy_count(fx: &Fixture) -> usize {
    fx.raw_fetch(
        r#"match
             (parent: $a, child: $b) isa hierarchy;
             $a has concept-id $pa; $b has concept-id $cb;
           fetch { "p": $pa, "c": $cb };"#,
    )
    .await
    .len()
}

fn storage_msg(e: EpistemicError) -> String {
    match e {
        EpistemicError::Storage(m) => m,
        other => panic!("expected a Storage error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn expect_rows_accepts_an_exact_insert_match() {
    let fx = Fixture::new().await;
    seed_two_concepts(&fx).await;

    fx.store()
        .write(&[hierarchy_op("c/a", "c/b").expecting(1)])
        .await
        .expect("an exact single match commits");

    let rows = fx
        .raw_fetch(
            r#"match
                 (parent: $a, child: $b) isa hierarchy;
                 $a has concept-id $pa; $b has concept-id $cb;
               fetch { "p": $pa, "c": $cb };"#,
        )
        .await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["p"], "c/a");
    assert_eq!(rows[0]["c"], "c/b");

    fx.drop_db().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn expect_rows_rolls_back_a_zero_match() {
    let fx = Fixture::new().await;
    seed_two_concepts(&fx).await;

    let err = fx
        .store()
        .write(&[
            GraphWrite::plain(r#"insert $c isa concept, has concept-id "c/third";"#),
            hierarchy_op("c/a", "c/missing").expecting(1),
        ])
        .await
        .expect_err("a zero-match insert must not commit");
    let msg = storage_msg(err);
    assert!(msg.contains("expected 1"), "unexpected message: {msg}");
    assert!(msg.contains("matched 0"), "unexpected message: {msg}");

    let third = fx
        .raw_fetch(
            r#"match $c isa concept, has concept-id "c/third";
               fetch { "id": $c.concept-id };"#,
        )
        .await;
    assert!(third.is_empty(), "the earlier op in the batch rolled back");
    assert_eq!(hierarchy_count(&fx).await, 0);

    fx.drop_db().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn expect_rows_rolls_back_a_double_match() {
    let fx = Fixture::new().await;
    seed_two_concepts(&fx).await;
    fx.raw_write(r#"insert $p isa concept, has concept-id "c/parent";"#)
        .await;

    let op = GraphWrite::single(
        "given $p: string, $t: string;
         match
           $a isa concept, has concept-id $pa; $pa == $p;
           $c isa concept, has tag $ct; $ct == $t;
         insert (parent: $a, child: $c) isa hierarchy;",
        vec!["p".into(), "t".into()],
        vec![GraphValue::from("c/parent"), GraphValue::from("shared")],
    )
    .expecting(1);

    let err = fx
        .store()
        .write(&[op])
        .await
        .expect_err("a double-match insert must not commit");
    let msg = storage_msg(err);
    assert!(msg.contains("expected 1"), "unexpected message: {msg}");
    assert!(msg.contains("matched 2"), "unexpected message: {msg}");
    assert_eq!(hierarchy_count(&fx).await, 0);

    fx.drop_db().await;
}

/// A `match … update` answer is a row stream with one row per updated match, so
/// an expected count applies to it exactly as it does to an insert.
#[tokio::test(flavor = "multi_thread")]
async fn expect_rows_counts_an_update() {
    let fx = Fixture::new().await;
    seed_two_concepts(&fx).await;

    let store = Arc::new(fx.store());
    let writer = MemWriter::new(store.clone(), None, None);
    let ctx = RunContext::new(
        ActorId::new("agent-1"),
        ProvenanceSource::Manual,
        chrono::Utc::now(),
    );
    let id = writer
        .assert_belief(
            &ctx,
            BeliefDraft {
                text: "the host is compromised".to_string(),
                owner: ActorId::new("agent-1"),
                visibility: Visibility::Private,
                confidence: Some(0.8),
                entrenchment: None,
            },
            &[],
            DerivationMethod::LlmSynthesis,
        )
        .await
        .expect("belief commits");

    let op = GraphWrite::single(
        "given $id: string;
         match $b isa belief, has concept-id $bc; $bc == $id;
         update $b has belief-state \"contested\";",
        vec!["id".into()],
        vec![GraphValue::String(id.0.clone())],
    )
    .expecting(1);
    store
        .write(&[op])
        .await
        .expect("an exact single-match update commits");

    let state = fx
        .raw_fetch(&format!(
            r#"match $b isa belief, has concept-id "{}", has belief-state $s;
               fetch {{ "s": $s }};"#,
            id.0
        ))
        .await;
    assert_eq!(state.len(), 1);
    assert_eq!(state[0]["s"], "contested");

    fx.drop_db().await;
}
