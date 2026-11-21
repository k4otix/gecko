//! Live TypeDB acceptance tests for two suites:
//!
//! - **Write-count enforcement.** These drive [`RouterGraphStore::write`] directly to
//!   prove that a [`GraphWrite`] carrying an expected row count commits only when its
//!   `match` bound exactly that many rows, and otherwise rolls the whole transaction
//!   back.
//! - **The belief-edge seam.** These drive a bound `BeliefEdgeWriter` over a probe
//!   extension fragment to prove that a belief and its declared edges commit together,
//!   that a missing, ambiguous or ill-typed player commits nothing, and that a
//!   declaration the live schema does not match is refused before any write.
//!
//! Each test provisions its own throwaway database on `localhost:1729`
//! (admin/password), applies core + mem schema, and drops it. The real `gecko`
//! database is never touched. Requires a running TypeDB service (CI provides one).

use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::Mutex;

use gecko_engine::db::RouterGraphStore;
use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_extension_api::{
    ActorId, AttrDecl, AttrValueType, BeliefDraft, BeliefEdge, BeliefEdgeDecl, DerivationMethod,
    EpistemicError, EpistemicWriter, GeckoExtension, GraphStore, GraphValue, GraphWrite, MemId,
    ProvenanceSource, RoleDecl, RowCountMismatch, RunContext, Visibility,
};
use mem_gecko::{BeliefEdgeRegistry, ExtensionFragment, MemWriter};
use ulid::Ulid;

mod common;

const CORE_SCHEMA: &str = include_str!("../../gecko-engine/schema/core_schema.tql");

/// A probe extension's schema: one relation linking a belief, a concept and a
/// `probe-target` found by a unique non-key attribute.
const PROBE_SCHEMA: &str = r#"define
  attribute probe-ref value string;
  attribute probe-tag value string;
  attribute probe-verdict value string @values("yes", "no");
  entity probe-target sub concept @doc("Test player looked up by a unique non-key attribute."),
      owns probe-ref @unique,
      owns probe-tag;
  relation probe-link @doc("Test belief edge."),
      owns probe-verdict @card(1),
      relates probe-subject @doc("Subject."),
      relates probe-object @doc("Object."),
      relates probe-claim @doc("The belief.");
  concept plays probe-link:probe-subject;
  probe-target plays probe-link:probe-object;
  belief plays probe-link:probe-claim;
"#;

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
        router
            .apply_schema(PROBE_SCHEMA)
            .await
            .expect("probe schema applies");
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
    assert_eq!(
        RowCountMismatch::from_error(&err),
        Some(RowCountMismatch {
            op: 1,
            expected: 1,
            got: 0
        }),
        "the error names the second op: {err:?}"
    );

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

const VERDICT: &[AttrDecl] = &[AttrDecl {
    attr: "probe-verdict",
    value: AttrValueType::String,
    required: true,
}];

const ROLES_BY_REF: &[RoleDecl] = &[
    RoleDecl {
        role: "probe-subject",
        player: "concept",
        key: "concept-id",
    },
    RoleDecl {
        role: "probe-object",
        player: "probe-target",
        key: "probe-ref",
    },
];

const ROLES_BY_TAG: &[RoleDecl] = &[
    RoleDecl {
        role: "probe-subject",
        player: "concept",
        key: "concept-id",
    },
    RoleDecl {
        role: "probe-object",
        player: "probe-target",
        key: "probe-tag",
    },
];

const ROLES_MISSING: &[RoleDecl] = &[RoleDecl {
    role: "probe-missing",
    player: "concept",
    key: "concept-id",
}];

static DECL_BY_REF: [BeliefEdgeDecl; 1] = [BeliefEdgeDecl {
    relation: "probe-link",
    belief_role: "probe-claim",
    roles: ROLES_BY_REF,
    owned_attrs: VERDICT,
}];

static DECL_BY_TAG: [BeliefEdgeDecl; 1] = [BeliefEdgeDecl {
    relation: "probe-link",
    belief_role: "probe-claim",
    roles: ROLES_BY_TAG,
    owned_attrs: VERDICT,
}];

static DECL_MISSING: [BeliefEdgeDecl; 1] = [BeliefEdgeDecl {
    relation: "probe-link",
    belief_role: "probe-claim",
    roles: ROLES_MISSING,
    owned_attrs: &[],
}];

/// A fragment the registry accepts that gives `probe-link` a role the applied
/// schema never defines.
const GHOST_SCHEMA: &str = r#"define
  relation probe-link @doc("Test belief edge."),
      relates probe-missing @doc("Absent role."),
      relates probe-claim @doc("The belief.");
  concept plays probe-link:probe-missing;
  belief plays probe-link:probe-claim;
"#;

fn registry(schema: &'static str, decls: &'static [BeliefEdgeDecl]) -> BeliefEdgeRegistry {
    BeliefEdgeRegistry::build(
        CORE_SCHEMA,
        mem_gecko::MemGecko::new().schema(),
        &[ExtensionFragment {
            name: "probe-ext",
            schema,
            migrations: &[],
            decls,
        }],
    )
    .expect("registry builds")
}

fn writer_with(
    fx: &Fixture,
    registry: BeliefEdgeRegistry,
) -> Arc<dyn gecko_extension_api::BeliefEdgeWriter> {
    let arc = Arc::new(MemWriter::without_index(Arc::new(fx.store())).with_belief_edges(registry));
    arc.edge_writer_for("probe-ext")
        .expect("probe-ext declares belief edges")
}

fn run_ctx() -> RunContext {
    RunContext::new(
        ActorId::new("agent-1"),
        ProvenanceSource::Manual,
        chrono::Utc::now(),
    )
}

fn draft(text: &str) -> BeliefDraft {
    BeliefDraft {
        text: text.to_string(),
        owner: ActorId::new("agent-1"),
        visibility: Visibility::Private,
        confidence: Some(0.8),
        entrenchment: None,
    }
}

fn edge(object_key: &str, object: &str, verdict: &str) -> BeliefEdge {
    BeliefEdge {
        relation: "probe-link".to_string(),
        players: vec![
            ("probe-subject".to_string(), GraphValue::from("s1")),
            (object_key.to_string(), GraphValue::from(object)),
        ],
        attrs: vec![("probe-verdict".to_string(), GraphValue::from(verdict))],
    }
}

async fn seed_probe(fx: &Fixture) {
    fx.raw_write(
        r#"insert
             $s isa concept, has concept-id "s1";
             $t1 isa probe-target, has concept-id "t1", has probe-ref "t1", has probe-tag "shared";
             $t2 isa probe-target, has concept-id "t2", has probe-ref "t2", has probe-tag "shared";"#,
    )
    .await;
}

/// Beliefs whose title is `text`.
async fn beliefs_titled(fx: &Fixture, text: &str) -> usize {
    fx.raw_fetch(&format!(
        r#"match $b isa belief, has title "{text}", has concept-id $id;
           fetch {{ "id": $id }};"#
    ))
    .await
    .len()
}

async fn assert_no_probe_links(fx: &Fixture) {
    let links = fx
        .raw_fetch(r#"match $l isa probe-link; fetch { "v": $l.probe-verdict };"#)
        .await;
    assert!(links.is_empty(), "a refused write leaves no probe-link");
}

#[tokio::test(flavor = "multi_thread")]
async fn belief_edge_decl_registry_writes_run_stamped() {
    let fx = Fixture::new().await;
    seed_probe(&fx).await;
    let w = writer_with(&fx, registry(PROBE_SCHEMA, &DECL_BY_REF));
    let ctx = run_ctx();

    let id: MemId = w
        .assert_belief_with_edges(
            &ctx,
            draft("probe belief one"),
            &[],
            DerivationMethod::LlmSynthesis,
            &[edge("probe-object", "t1", "yes")],
        )
        .await
        .expect("belief and edge commit");

    let links = fx
        .raw_fetch(
            r#"match
                 $l isa probe-link, links (probe-claim: $c, probe-subject: $s, probe-object: $o);
                 $c has concept-id $cid;
                 $s has concept-id $sid;
                 $o has concept-id $oid;
               fetch { "c": $cid, "s": $sid, "o": $oid, "v": $l.probe-verdict };"#,
        )
        .await;
    assert_eq!(links.len(), 1, "exactly one probe-link");
    assert_eq!(links[0]["c"], id.0);
    assert_eq!(links[0]["s"], "s1");
    assert_eq!(links[0]["o"], "t1");
    assert_eq!(links[0]["v"], "yes");

    let stamp = fx
        .raw_fetch(&format!(
            r#"match
                 $b isa belief, has concept-id "{}";
                 (memory: $b, origin: $run) isa source-link;
                 $run isa doc-run, has run-id $rid;
               fetch {{ "rid": $rid }};"#,
            id.0
        ))
        .await;
    assert_eq!(stamp.len(), 1, "belief carries a source-link to a doc-run");
    assert_eq!(stamp[0]["rid"], ctx.run_id.to_string());

    fx.drop_db().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_player_commits_no_belief() {
    let fx = Fixture::new().await;
    seed_probe(&fx).await;
    let w = writer_with(&fx, registry(PROBE_SCHEMA, &DECL_BY_REF));

    let err = w
        .assert_belief_with_edges(
            &run_ctx(),
            draft("probe missing player"),
            &[],
            DerivationMethod::LlmSynthesis,
            &[edge("probe-object", "nope", "yes")],
        )
        .await
        .expect_err("an unmatched player must not commit");
    let EpistemicError::Storage(msg) = &err else {
        panic!("expected Storage, got {err:?}");
    };
    assert!(msg.contains("probe-link"), "{msg}");
    assert!(msg.contains("probe-object probe-ref=nope"), "{msg}");
    assert!(msg.contains("matched 0 rows"), "{msg}");
    assert_eq!(beliefs_titled(&fx, "probe missing player").await, 0);
    assert_no_probe_links(&fx).await;

    fx.drop_db().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn belief_with_edges_and_a_missing_evidence_id_names_the_evidence() {
    let fx = Fixture::new().await;
    seed_probe(&fx).await;
    let w = writer_with(&fx, registry(PROBE_SCHEMA, &DECL_BY_REF));

    let err = w
        .assert_belief_with_edges(
            &run_ctx(),
            draft("probe missing evidence"),
            &[MemId::new("mem/ep/does-not-exist")],
            DerivationMethod::LlmSynthesis,
            &[edge("probe-object", "t1", "yes")],
        )
        .await
        .expect_err("a missing evidence id must not commit");
    let EpistemicError::Storage(msg) = &err else {
        panic!("expected Storage, got {err:?}");
    };
    assert!(msg.contains("mem/ep/does-not-exist"), "{msg}");
    assert!(msg.contains("no memory item"), "{msg}");
    assert!(!msg.contains("belief edge"), "{msg}");
    assert_eq!(beliefs_titled(&fx, "probe missing evidence").await, 0);
    assert_no_probe_links(&fx).await;

    fx.drop_db().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn value_outside_values_commits_no_belief() {
    let fx = Fixture::new().await;
    seed_probe(&fx).await;
    let w = writer_with(&fx, registry(PROBE_SCHEMA, &DECL_BY_REF));

    w.assert_belief_with_edges(
        &run_ctx(),
        draft("probe bad verdict"),
        &[],
        DerivationMethod::LlmSynthesis,
        &[edge("probe-object", "t1", "maybe")],
    )
    .await
    .expect_err("a value outside @values must not commit");
    assert_eq!(beliefs_titled(&fx, "probe bad verdict").await, 0);
    assert_no_probe_links(&fx).await;

    fx.drop_db().await;
}

/// `probe-tag` is shared by two targets, so a key lookup on it binds two rows. The
/// live schema check does not test `@key`/`@unique` (TypeDB rejects annotation
/// matching), so the commit-time row count refuses the write as `Storage`.
#[tokio::test(flavor = "multi_thread")]
async fn non_unique_key_commits_no_belief() {
    let fx = Fixture::new().await;
    seed_probe(&fx).await;
    let w = writer_with(&fx, registry(PROBE_SCHEMA, &DECL_BY_TAG));

    let err = w
        .assert_belief_with_edges(
            &run_ctx(),
            draft("probe non-unique key"),
            &[],
            DerivationMethod::LlmSynthesis,
            &[edge("probe-object", "shared", "yes")],
        )
        .await
        .expect_err("a non-unique key must not commit");
    let EpistemicError::Storage(msg) = &err else {
        panic!("expected Storage, got {err:?}");
    };
    assert!(msg.contains("probe-object probe-tag=shared"), "{msg}");
    assert!(msg.contains("matched 2 rows"), "{msg}");
    assert_eq!(beliefs_titled(&fx, "probe non-unique key").await, 0);
    assert_no_probe_links(&fx).await;

    fx.drop_db().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn live_schema_mismatch_is_refused_before_writing() {
    let fx = Fixture::new().await;
    seed_probe(&fx).await;
    let w = writer_with(&fx, registry(GHOST_SCHEMA, &DECL_MISSING));

    let err = w
        .assert_belief_with_edges(
            &run_ctx(),
            draft("probe ghost"),
            &[],
            DerivationMethod::LlmSynthesis,
            &[BeliefEdge {
                relation: "probe-link".to_string(),
                players: vec![("probe-missing".to_string(), GraphValue::from("s1"))],
                attrs: vec![],
            }],
        )
        .await
        .expect_err("a declaration absent from the database is refused");
    match err {
        EpistemicError::InvalidInput(m) => {
            assert!(m.contains("probe-missing"), "message names the role: {m}")
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }
    assert_eq!(beliefs_titled(&fx, "probe ghost").await, 0);

    fx.drop_db().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn two_edges_of_one_relation_both_land() {
    let fx = Fixture::new().await;
    seed_probe(&fx).await;
    let w = writer_with(&fx, registry(PROBE_SCHEMA, &DECL_BY_REF));

    let id = w
        .assert_belief_with_edges(
            &run_ctx(),
            draft("probe two edges"),
            &[],
            DerivationMethod::LlmSynthesis,
            &[
                edge("probe-object", "t1", "yes"),
                edge("probe-object", "t2", "no"),
            ],
        )
        .await
        .expect("both edges commit");

    let links = fx
        .raw_fetch(&format!(
            r#"match
                 $b isa belief, has concept-id "{}";
                 $l isa probe-link, links (probe-claim: $b, probe-object: $o);
                 $o has concept-id $oid;
               fetch {{ "o": $oid }};"#,
            id.0
        ))
        .await;
    let mut objects: Vec<String> = links
        .iter()
        .map(|l| l["o"].as_str().unwrap().to_string())
        .collect();
    objects.sort();
    assert_eq!(objects, ["t1", "t2"]);

    fx.drop_db().await;
}
