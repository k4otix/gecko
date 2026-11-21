// End-to-end proof of the cyber-gecko STIX ingest path: a STIX bundle is mapped
// to OKF, synced through the core record path, and its typed SROs are linked.
// Verifies that typed attributes (ioc-value/ioc-type, attack-id) are persisted as
// real `owns` — not folded into metadata-json — and that the ATT&CK backbone
// (`detects`, `x_mitre_data_source_ref`) materializes as typed relations.
//
// Gated on `cyber`: the extension's schema and types only exist when compiled in.
#![cfg(feature = "cyber")]

use cyber_gecko::CyberGecko;
use cyber_gecko::stix::{cyber_post_sync, to_okf, typed_relation_players};
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
      ],
      "x_mitre_platforms": ["Windows"]
    },
    {
      "type": "course-of-action",
      "id": "course-of-action--00000000-0000-0000-0000-0000000000a8",
      "name": "Command and Scripting Interpreter Mitigation",
      "external_references": [
        { "source_name": "mitre-attack", "external_id": "T1059" }
      ],
      "x_mitre_deprecated": true
    },
    {
      "type": "attack-pattern",
      "id": "attack-pattern--00000000-0000-0000-0000-0000000000a9",
      "name": "Revoked Technique",
      "external_references": [
        { "source_name": "mitre-attack", "external_id": "T1059" }
      ],
      "revoked": true
    },
    {
      "type": "x-mitre-analytic",
      "id": "x-mitre-analytic--00000000-0000-0000-0000-0000000000a7",
      "name": "Analytic 0001",
      "x_mitre_platforms": ["Linux"]
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
    let writes = cyber_post_sync(&typed_rels);
    sync_bundle(&mut db, &manifest, &writes, mem_gecko::BELIEF_TIER_ROOT)
        .await
        .expect("sync_bundle");

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
    // A deprecated or revoked object reusing a live ATT&CK id is ingested without it.
    assert_eq!(
        count(&tx, r#"match $c isa concept, has attack-id "T1059";"#).await,
        1,
        "only the live technique may claim T1059"
    );
    assert_eq!(
        count(&tx, "match $c isa course-of-action;").await,
        1,
        "the deprecated mitigation is still ingested"
    );
    assert_eq!(
        count(&tx, "match $a isa attack-pattern, has revoked true;").await,
        1,
        "the revoked technique is still ingested"
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
    assert_eq!(
        count(&tx, r#"match $d isa data-source, has platform "Windows";"#).await,
        1,
        "a data source's x_mitre_platforms must persist"
    );
    // An analytic has no platform-owning subtype, so it is ingested without one.
    assert_eq!(
        count(&tx, r#"match $c isa concept, has platform "Linux";"#).await,
        1,
        "only the attack-pattern carries Linux; the analytic's platforms are not written"
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

    // Re-syncing the same bundle is idempotent: relation counts do not change.
    sync_bundle(&mut db, &manifest, &writes, mem_gecko::BELIEF_TIER_ROOT)
        .await
        .expect("second sync_bundle");
    let tx = db.begin_read().await.expect("begin read after resync");
    for (label, expected) in [
        ("uses", 1),
        ("indicates", 1),
        ("requires-evidence", 1),
        ("evidence-of", 1),
    ] {
        assert_eq!(
            count(&tx, &format!("match $r isa {label};")).await,
            expected,
            "{label} count must be unchanged after a second sync"
        );
    }
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

/// A tool that uses a technique, and a detection strategy that detects one (an
/// ATT&CK shape the cyber schema does not model as a typed relation).
const SHAPES_BUNDLE: &str = r#"{
  "type": "bundle",
  "objects": [
    { "type": "attack-pattern", "id": "attack-pattern--00000000-0000-0000-0000-0000000001a1", "name": "T" },
    { "type": "tool", "id": "tool--00000000-0000-0000-0000-0000000001b2", "name": "Tool" },
    { "type": "x-mitre-detection-strategy", "id": "x-mitre-detection-strategy--00000000-0000-0000-0000-0000000001c3", "name": "Strategy" },
    { "type": "relationship", "id": "relationship--00000000-0000-0000-0000-000000000201",
      "relationship_type": "uses",
      "source_ref": "tool--00000000-0000-0000-0000-0000000001b2",
      "target_ref": "attack-pattern--00000000-0000-0000-0000-0000000001a1" },
    { "type": "relationship", "id": "relationship--00000000-0000-0000-0000-000000000202",
      "relationship_type": "detects",
      "source_ref": "x-mitre-detection-strategy--00000000-0000-0000-0000-0000000001c3",
      "target_ref": "attack-pattern--00000000-0000-0000-0000-0000000001a1" }
  ]
}"#;

async fn apply_cyber_schema(db: &mut gecko_engine::db::router::TypeDbRouter) {
    db.apply_schema(include_str!(
        "../../core/gecko-engine/schema/core_schema.tql"
    ))
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
}

#[tokio::test]
async fn stix_sync_links_tool_uses_and_records_unmodeled_shapes_as_links() {
    let test_db = TestDb::new("gecko_test_stix_shapes");
    let mut db = test_db.router();
    apply_cyber_schema(&mut db).await;

    let (manifest, typed_rels) = to_okf(SHAPES_BUNDLE).expect("to_okf");
    assert_eq!(typed_rels.len(), 1, "only the tool-uses relation is typed");
    assert!(
        manifest.links.iter().any(|l| l.link_text == "detects"),
        "the strategy-detects relation is kept as a concept-link"
    );
    sync_bundle(
        &mut db,
        &manifest,
        &cyber_post_sync(&typed_rels),
        mem_gecko::BELIEF_TIER_ROOT,
    )
    .await
    .expect("sync_bundle");

    let tx = db.begin_read().await.expect("begin read");
    assert_eq!(
        count(&tx, "match $t isa tool; $r isa uses, links (user: $t);").await,
        1
    );
    assert_eq!(count(&tx, "match $r isa requires-evidence;").await, 0);
}

#[tokio::test]
async fn typed_relation_players_match_the_schema() {
    let test_db = TestDb::new("gecko_test_stix_players");
    let mut db = test_db.router();
    apply_cyber_schema(&mut db).await;
    let tx = db.begin_read().await.expect("begin read");

    for (relation, role, players) in typed_relation_players() {
        assert_eq!(
            count(
                &tx,
                &format!("match $t sub cyber-object; $t plays {relation}:{role};")
            )
            .await,
            players.len(),
            "{relation}:{role} players in the schema differ from the adapter's {players:?}"
        );
        for player in players {
            assert_eq!(
                count(
                    &tx,
                    &format!("match $t label {player}; $t plays {relation}:{role};")
                )
                .await,
                1,
                "{player} must play {relation}:{role}"
            );
        }
    }
}

#[tokio::test]
async fn required_attributes_of_the_live_cyber_schema() {
    let test_db = TestDb::new("gecko_test_stix_required");
    let mut db = test_db.router();
    apply_cyber_schema(&mut db).await;

    let schema = db.database_schema().await.expect("database schema");
    let required = gecko_engine::db::required::required_attributes(&schema).expect("parse");
    for (entity, attrs) in [
        ("observable", &["concept-id", "ioc-value", "ioc-type"][..]),
        ("attack-pattern", &["concept-id"][..]),
    ] {
        let got = required
            .get(entity)
            .unwrap_or_else(|| panic!("{entity} missing from {required:?}"));
        for a in attrs {
            assert!(got.contains(*a), "{entity} must require {a}: {got:?}");
        }
    }
}
