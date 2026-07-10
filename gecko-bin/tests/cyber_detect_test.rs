// End-to-end proof of the cyber-gecko detect host bridge: claim writes a real
// detection belief + technique-detection relation, coverage_gaps/blinded run the
// persisted TypeDB functions, and disposition/precision compute over real
// dispositions. Exercises the sync->async bridge and the belief-write path.
//
// Gated on `cyber`; needs the multi-thread runtime the bridge's block_in_place
// requires.
#![cfg(feature = "cyber")]

use std::sync::Arc;

use cyber_gecko::CyberGecko;
use cyber_gecko::detect::cyber_extension_callback;
use cyber_gecko::stix::{cyber_post_sync, to_okf};
use gecko_engine::db::graph_store::RouterGraphStore;
use gecko_engine::extension::GeckoExtension;
use gecko_engine::sandbox::engine::ExtensionCallback;
use gecko_engine::syncer::bundle::sync_bundle;
use gecko_extension_api::{
    ActorId, ConceptId, EpistemicWriter, GraphStore, ProvenanceSource, RunContext,
};
use mem_gecko::{MemGecko, MemWriter};
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detect_bridge_claims_disposes_and_reports() {
    let test_db = TestDb::new("gecko_test_detect");

    // Schema.
    {
        let mut db = test_db.router();
        let core = include_str!("../../core/gecko-engine/schema/core_schema.tql");
        db.apply_schema(core).await.expect("core schema");
        for ext in [
            Box::new(MemGecko::new()) as Box<dyn GeckoExtension>,
            Box::new(CyberGecko::new()),
        ] {
            db.apply_schema(ext.schema())
                .await
                .unwrap_or_else(|e| panic!("{} schema: {e}", ext.name()));
        }

        // Sync the ATT&CK backbone.
        let (manifest, typed_rels) = to_okf(BUNDLE).expect("to_okf");
        sync_bundle(&mut db, &manifest).await.expect("sync_bundle");
        let tx = db.begin_write().await.expect("write");
        cyber_post_sync(&tx, &typed_rels).await.expect("post_sync");
        tx.commit().await.expect("commit");

        // Seed the detection-logic concept (a plain concept the claim references).
        let tx = db.begin_write().await.expect("write");
        tx.query(&format!(
            "insert $c isa concept, has concept-id == '{LOGIC_ID}';"
        ))
        .await
        .expect("insert logic");
        tx.commit().await.expect("commit logic");
    }

    // Build the bridge: a graph store + belief writer + host RunContext.
    let store: Arc<dyn GraphStore> = Arc::new(RouterGraphStore::new(Arc::new(Mutex::new(
        test_db.router(),
    ))));
    let writer: Arc<dyn EpistemicWriter> = Arc::new(MemWriter::without_index(store.clone()));
    let ctx = RunContext::new(
        ActorId::new("system"),
        ProvenanceSource::ExecutableDoc {
            concept_id: ConceptId::new(LOGIC_ID),
        },
        chrono::Utc::now(),
    );
    let base: ExtensionCallback = Arc::new(|_, _, _| Err("base".to_string()));
    let cb = cyber_extension_callback(store, writer, ctx, base);

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
    {
        let mut db = test_db.router();
        let core = include_str!("../../core/gecko-engine/schema/core_schema.tql");
        db.apply_schema(core).await.expect("core schema");
        for ext in [
            Box::new(MemGecko::new()) as Box<dyn GeckoExtension>,
            Box::new(CyberGecko::new()),
        ] {
            db.apply_schema(ext.schema()).await.expect("ext schema");
        }
    }

    let store: Arc<dyn GraphStore> = Arc::new(RouterGraphStore::new(Arc::new(Mutex::new(
        test_db.router(),
    ))));
    let writer: Arc<dyn EpistemicWriter> = Arc::new(MemWriter::without_index(store.clone()));
    let ctx = RunContext::new(
        ActorId::new("system"),
        ProvenanceSource::ExecutableDoc {
            concept_id: ConceptId::new("x"),
        },
        chrono::Utc::now(),
    );
    let base: ExtensionCallback = Arc::new(|_, _, _| Err("base".to_string()));
    let cb = cyber_extension_callback(store, writer, ctx, base);

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
