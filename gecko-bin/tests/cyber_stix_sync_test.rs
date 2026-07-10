// End-to-end proof of the cyber-gecko STIX ingest path: a STIX bundle is mapped
// to OKF, synced through the core record path, and its typed SROs are linked.
// Verifies that typed attributes (ioc-value/ioc-type, attack-id) are persisted as
// real `owns` — not folded into metadata-json — and that the ATT&CK backbone
// (`detects`, `x_mitre_data_source_ref`) materializes as typed relations.
//
// Gated on `cyber`: the extension's schema and types only exist when compiled in.
#![cfg(feature = "cyber")]

use cyber_gecko::CyberGecko;
use cyber_gecko::stix::{cyber_post_sync, to_okf};
use futures_util::StreamExt;
use gecko_engine::extension::GeckoExtension;
use gecko_engine::syncer::bundle::sync_bundle;
use mem_gecko::MemGecko;
use typedb_driver::Transaction;

mod common;
use common::TestDb;

/// A representative STIX 2.1 / ATT&CK bundle exercising SDOs, an SCO observable
/// (defanged IP), the embedded data-source ref, and three SRO shapes.
const BUNDLE: &str = r#"{
  "type": "bundle",
  "objects": [
    {
      "type": "attack-pattern",
      "id": "attack-pattern--00000000-0000-0000-0000-0000000000a1",
      "name": "Command and Scripting Interpreter",
      "external_references": [
        { "source_name": "mitre-attack", "external_id": "T1059" }
      ],
      "kill_chain_phases": [
        { "kill_chain_name": "mitre-attack", "phase_name": "execution" }
      ],
      "x_mitre_platforms": ["Windows", "Linux"]
    },
    {
      "type": "malware",
      "id": "malware--00000000-0000-0000-0000-0000000000b2",
      "name": "ExampleRAT"
    },
    {
      "type": "indicator",
      "id": "indicator--00000000-0000-0000-0000-0000000000c3",
      "name": "Bad IP",
      "pattern": "[ipv4-addr:value = '1.1.1.1']",
      "valid_until": "2027-01-01T00:00:00Z"
    },
    {
      "type": "ipv4-addr",
      "id": "ipv4-addr--00000000-0000-0000-0000-0000000000d4",
      "value": "1.1.1[.]1"
    },
    {
      "type": "x-mitre-data-source",
      "id": "x-mitre-data-source--00000000-0000-0000-0000-0000000000e5",
      "name": "Command",
      "external_references": [
        { "source_name": "mitre-attack", "external_id": "DS0017" }
      ]
    },
    {
      "type": "x-mitre-data-component",
      "id": "x-mitre-data-component--00000000-0000-0000-0000-0000000000f6",
      "name": "Command Execution",
      "x_mitre_data_source_ref": "x-mitre-data-source--00000000-0000-0000-0000-0000000000e5"
    },
    {
      "type": "relationship",
      "id": "relationship--00000000-0000-0000-0000-000000000101",
      "relationship_type": "indicates",
      "source_ref": "indicator--00000000-0000-0000-0000-0000000000c3",
      "target_ref": "malware--00000000-0000-0000-0000-0000000000b2"
    },
    {
      "type": "relationship",
      "id": "relationship--00000000-0000-0000-0000-000000000102",
      "relationship_type": "uses",
      "source_ref": "malware--00000000-0000-0000-0000-0000000000b2",
      "target_ref": "attack-pattern--00000000-0000-0000-0000-0000000000a1"
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

/// Counts the rows a match query returns.
async fn count(tx: &Transaction, query: &str) -> usize {
    let answer = tx.query(query).await.expect("query failed");
    answer.into_rows().collect::<Vec<_>>().await.len()
}

#[tokio::test]
async fn stix_sync_persists_typed_attributes_and_backbone() {
    let test_db = TestDb::new("gecko_test_stix");
    let mut db = test_db.router();

    // Apply core + mem + cyber schema.
    let core_schema = include_str!("../../core/gecko-engine/schema/core_schema.tql");
    db.apply_schema(core_schema)
        .await
        .expect("apply core schema");
    for ext in [
        Box::new(MemGecko::new()) as Box<dyn GeckoExtension>,
        Box::new(CyberGecko::new()),
    ] {
        db.apply_schema(ext.schema())
            .await
            .unwrap_or_else(|e| panic!("apply {} schema: {e}", ext.name()));
    }

    // Map the STIX bundle and sync it through the record path.
    let (manifest, typed_rels) = to_okf(BUNDLE).expect("to_okf");
    sync_bundle(&mut db, &manifest).await.expect("sync_bundle");

    let tx = db.begin_write().await.expect("begin write");
    cyber_post_sync(&tx, &typed_rels)
        .await
        .expect("cyber_post_sync");
    tx.commit().await.expect("commit post-sync");

    let tx = db.begin_read().await.expect("begin read");

    // Observable IOC persisted as typed owns, defanged + normalized (1.1.1[.]1).
    assert_eq!(
        count(
            &tx,
            r#"match $o isa observable, has ioc-value "1.1.1.1", has ioc-type "ipv4";"#
        )
        .await,
        1,
        "observable must carry normalized ioc-value/ioc-type as typed attributes"
    );

    // attack-id persisted as a typed, unique key (not buried in metadata-json).
    assert_eq!(
        count(
            &tx,
            r#"match $a isa attack-pattern, has attack-id "T1059";"#
        )
        .await,
        1,
        "attack-pattern must carry attack-id"
    );

    // kill-chain-phase and both platforms landed as multi-valued typed attributes.
    assert_eq!(
        count(
            &tx,
            r#"match $a isa attack-pattern, has kill-chain-phase "execution";"#
        )
        .await,
        1
    );
    assert_eq!(
        count(&tx, r#"match $a isa attack-pattern, has platform $p;"#).await,
        2,
        "both x_mitre_platforms values must persist"
    );

    // indicator's valid-until parsed to a real datetime typed attribute.
    assert_eq!(
        count(&tx, r#"match $i isa indicator, has valid-until $v;"#).await,
        1
    );

    // SROs: indicates + uses linked, detects -> requires-evidence, and the
    // embedded data-source ref synthesized -> evidence-of.
    assert_eq!(count(&tx, "match $r isa indicates;").await, 1);
    assert_eq!(count(&tx, "match $r isa uses;").await, 1);
    assert_eq!(
        count(&tx, "match $r isa requires-evidence;").await,
        1,
        "detects SRO must materialize as a requires-evidence relation"
    );
    assert_eq!(
        count(&tx, "match $r isa evidence-of;").await,
        1,
        "x_mitre_data_source_ref must synthesize an evidence-of relation"
    );
}

#[tokio::test]
async fn to_okf_is_content_hash_stable() {
    let (a, _) = to_okf(BUNDLE).expect("to_okf a");
    let (b, _) = to_okf(BUNDLE).expect("to_okf b");
    for (ca, cb) in a.concepts.iter().zip(b.concepts.iter()) {
        assert_eq!(ca.file_hash, cb.file_hash, "hash must be deterministic");
    }
}

#[test]
fn to_okf_rejects_non_bundle() {
    let err = to_okf(r#"{"type":"indicator","objects":[]}"#).unwrap_err();
    assert!(err.contains("bundle"), "got: {err}");
}
