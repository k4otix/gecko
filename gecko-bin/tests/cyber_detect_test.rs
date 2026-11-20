// End-to-end proof of the cyber-gecko detect host bridge: claim writes a real
// detection belief + technique-detection edge through the belief-edge seam,
// coverage_gaps/blinded run the persisted TypeDB functions, and
// disposition/precision compute over real dispositions. Exercises the sync->async
// bridge over a read-only graph handle and an edge writer bound to cyber-gecko.
//
// Gated on `cyber`; needs the multi-thread runtime the bridge's block_in_place
// requires.
#![cfg(feature = "cyber")]

use std::sync::Arc;

use cyber_gecko::CyberGecko;
use cyber_gecko::detect::cyber_extension_callback;
use cyber_gecko::stix::{cyber_post_sync, to_okf};
use gecko_engine::db::CORE_SCHEMA;
use gecko_engine::db::graph_store::RouterGraphStore;
use gecko_engine::extension::GeckoExtension;
use gecko_engine::sandbox::engine::ExtensionCallback;
use gecko_engine::syncer::bundle::sync_bundle;
use gecko_extension_api::{
    ActorId, BeliefDraft, BeliefEdge, BeliefEdgeWriter, ConceptId, DerivationMethod, GraphReader,
    ProvenanceSource, RunContext, Visibility,
};
use mem_gecko::{BeliefEdgeRegistry, MemGecko, MemWriter};
use serde_json::{Value, json};
use tokio::sync::Mutex;

mod common;
use common::TestDb;

// A minimal ATT&CK bundle: one technique with an attack-id, plus a data
// source/component wired by `detects` (so the technique has required evidence but
// no covering sensor -> it will show up as blinded).
const BUNDLE: &str = r#"{
  "type": "bundle",
  "objects": [
    {
      "type": "attack-pattern",
      "id": "attack-pattern--00000000-0000-0000-0000-0000000000a1",
      "name": "Command and Scripting Interpreter",
      "external_references": [{ "source_name": "mitre-attack", "external_id": "T1059" }]
    },
    {
      "type": "x-mitre-data-source",
      "id": "x-mitre-data-source--00000000-0000-0000-0000-0000000000e5",
      "name": "Command",
      "external_references": [{ "source_name": "mitre-attack", "external_id": "DS0017" }]
    },
    {
      "type": "x-mitre-data-component",
      "id": "x-mitre-data-component--00000000-0000-0000-0000-0000000000f6",
      "name": "Command Execution",
      "x_mitre_data_source_ref": "x-mitre-data-source--00000000-0000-0000-0000-0000000000e5"
    },
    {
      "type": "relationship",
      "id": "relationship--00000000-0000-0000-0000-000000000103",
      "relationship_type": "detects",
      "source_ref": "x-mitre-data-component--00000000-0000-0000-0000-0000000000f6",
      "target_ref": "attack-pattern--00000000-0000-0000-0000-0000000000a1"
    }
  ]
}"#;

const LOGIC_ID: &str = "playbooks/detect-t1059";

fn call(cb: &ExtensionCallback, func: &str, args: Value) -> Result<Value, String> {
    cb("cyber-gecko", func, args)
}

/// Applies the core, mem and cyber schemas to the test database.
async fn apply_schemas(test_db: &TestDb) {
    let mut db = test_db.router();
    db.apply_schema(CORE_SCHEMA).await.expect("core schema");
    for ext in [
        Box::new(MemGecko::new()) as Box<dyn GeckoExtension>,
        Box::new(CyberGecko::new()),
    ] {
        db.apply_schema(ext.schema())
            .await
            .unwrap_or_else(|e| panic!("{} schema: {e}", ext.name()));
    }
}

/// Applies the schemas, syncs the ATT&CK backbone and seeds the detection-logic
/// concept the claims reference.
async fn seed_detection_graph(test_db: &TestDb) {
    apply_schemas(test_db).await;
    let mut db = test_db.router();
    let (manifest, typed_rels) = to_okf(BUNDLE).expect("to_okf");
    sync_bundle(&mut db, &manifest, &cyber_post_sync(&typed_rels))
        .await
        .expect("sync_bundle");

    // A plain concept the claim references.
    let tx = db.begin_write().await.expect("write");
    tx.query(&format!(
        "insert $c isa concept, has concept-id == '{LOGIC_ID}';"
    ))
    .await
    .expect("insert logic");
    tx.commit().await.expect("commit logic");
}

/// The read-only graph handle and the cyber-bound belief-edge writer the host
/// gives the detect bridge.
fn detect_handles(test_db: &TestDb) -> (Arc<dyn GraphReader>, Arc<dyn BeliefEdgeWriter>) {
    let store = Arc::new(RouterGraphStore::new(Arc::new(Mutex::new(
        test_db.router(),
    ))));
    let registry = BeliefEdgeRegistry::from_extensions(
        CORE_SCHEMA,
        &[mem_gecko::extension(), cyber_gecko::extension()],
    )
    .expect("belief-edge registry");
    let mem = Arc::new(MemWriter::without_index(store.clone()).with_belief_edges(registry));
    let edges = mem
        .edge_writer_for("cyber-gecko")
        .expect("cyber-gecko declares belief edges");
    (store as Arc<dyn GraphReader>, edges)
}

fn run_ctx(concept_id: &str) -> RunContext {
    RunContext::new(
        ActorId::new("system"),
        ProvenanceSource::ExecutableDoc {
            concept_id: ConceptId::new(concept_id),
        },
        chrono::Utc::now(),
    )
}

fn base() -> ExtensionCallback {
    Arc::new(|_, _, _| Err("base".to_string()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detect_bridge_claims_disposes_and_reports() {
    let test_db = TestDb::new("gecko_test_detect");
    seed_detection_graph(&test_db).await;

    let (graph, edges) = detect_handles(&test_db);
    let cb = cyber_extension_callback(graph, edges, run_ctx(LOGIC_ID), base());

    // Before any claim, T1059 is an undetected technique.
    let gaps = call(&cb, "coverage_gaps", json!({})).expect("coverage_gaps");
    let before = gaps["undetected_techniques"].as_array().unwrap().len();
    assert_eq!(before, 1, "T1059 should start uncovered");

    // Claim: logic detects T1059 (external-tool, high confidence).
    let claimed = call(
        &cb,
        "claim",
        json!({ "logic_id": LOGIC_ID, "technique_id": "T1059", "method": "external-tool", "confidence": 0.9 }),
    )
    .expect("claim");
    assert!(
        claimed["belief_id"]
            .as_str()
            .unwrap()
            .starts_with("mem/bel/")
    );

    // Now the gap is closed.
    let gaps = call(&cb, "coverage_gaps", json!({})).expect("coverage_gaps");
    assert_eq!(
        gaps["undetected_techniques"].as_array().unwrap().len(),
        0,
        "claim should remove T1059 from the gap list"
    );

    // The claim's required evidence (the data component) has no covering sensor,
    // so the detecting logic is blinded.
    let blind = call(&cb, "blinded", json!({})).expect("blinded");
    let blinded_ids: Vec<&str> = blind["blinded_detections"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["concept_id"].as_str())
        .collect();
    assert!(
        blinded_ids.contains(&LOGIC_ID),
        "logic should be blinded (no sensor covers its evidence): {blinded_ids:?}"
    );

    // Dispose two alerts (1 TP, 1 FP) -> precision 0.5.
    for verdict in ["true-positive", "false-positive"] {
        call(
            &cb,
            "disposition",
            json!({ "logic_id": LOGIC_ID, "verdict": verdict }),
        )
        .unwrap_or_else(|e| panic!("disposition {verdict}: {e}"));
    }
    let prec = call(
        &cb,
        "precision",
        json!({ "logic_id": LOGIC_ID, "window": 3600 }),
    )
    .expect("precision");
    assert_eq!(prec["true_positives"], 1);
    assert_eq!(prec["false_positives"], 1);
    assert_eq!(prec["precision"], 0.5);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detect_rejects_bad_input_instead_of_fabricating() {
    let test_db = TestDb::new("gecko_test_detect_reject");
    apply_schemas(&test_db).await;

    let (graph, edges) = detect_handles(&test_db);
    let cb = cyber_extension_callback(graph, edges, run_ctx("x"), base());

    // Unknown method, out-of-range confidence, and unknown verdict must all error.
    assert!(
        call(
            &cb,
            "claim",
            json!({ "logic_id": "l", "technique_id": "T1", "method": "bogus", "confidence": 0.5 })
        )
        .is_err()
    );
    assert!(call(&cb, "claim", json!({ "logic_id": "l", "technique_id": "T1", "method": "external-tool", "confidence": 2.0 })).is_err());
    assert!(
        call(
            &cb,
            "disposition",
            json!({ "logic_id": "l", "verdict": "maybe" })
        )
        .is_err()
    );
    // A claim about a technique absent from the graph is refused, not fabricated.
    assert!(call(&cb, "claim", json!({ "logic_id": "nope", "technique_id": "T9999", "method": "external-tool", "confidence": 0.5 })).is_err());
}

/// The number of beliefs in the graph.
async fn belief_count(graph: &Arc<dyn GraphReader>) -> usize {
    graph
        .read(
            "match $b isa belief; fetch { \"id\": $b.concept-id };",
            &[],
            &[],
        )
        .await
        .expect("count beliefs")
        .len()
}

fn edge(technique: &str) -> BeliefEdge {
    BeliefEdge {
        relation: "technique-detection".into(),
        players: vec![
            ("detecting-logic".into(), LOGIC_ID.into()),
            ("detected-technique".into(), technique.into()),
        ],
        attrs: vec![],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edge_to_an_unknown_technique_commits_no_belief() {
    let test_db = TestDb::new("gecko_test_detect_edge");
    seed_detection_graph(&test_db).await;

    let (graph, edges) = detect_handles(&test_db);
    let ctx = run_ctx(LOGIC_ID);
    let draft = BeliefDraft {
        text: format!("detection '{LOGIC_ID}' detects technique 'T1059' (confidence 0.9)"),
        owner: ctx.actor.clone(),
        visibility: Visibility::Private,
        confidence: Some(0.9),
        entrenchment: None,
    };
    let method = DerivationMethod::ExternalTool;

    // Control: an edge to a technique in the graph commits one belief.
    let before = belief_count(&graph).await;
    edges
        .assert_belief_with_edges(&ctx, draft.clone(), &[], method, &[edge("T1059")])
        .await
        .expect("an edge to a known technique commits");
    let after_control = belief_count(&graph).await;
    assert_eq!(after_control, before + 1);

    // An edge whose technique matches nothing fails the whole write.
    let err = edges
        .assert_belief_with_edges(&ctx, draft, &[], method, &[edge("T9999")])
        .await
        .expect_err("an edge to an unknown technique must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("technique-detection") && msg.contains("T9999"),
        "the failure must name the edge write: {msg}"
    );
    assert_eq!(
        belief_count(&graph).await,
        after_control,
        "a failed edge must commit no belief"
    );
}
