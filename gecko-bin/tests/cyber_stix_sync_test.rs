// End-to-end proof of the cyber-gecko STIX ingest path: a STIX bundle is mapped
// to OKF, synced through the core record path, and its typed SROs are linked.
// Verifies that typed attributes (ioc-value/ioc-type, attack-id) are persisted as
// real `owns` — not folded into metadata-json — and that ATT&CK's detection model
// (strategies, analytics and their log-source requirements) materializes as typed
// relations that a re-import replaces.
//
// Gated on `cyber`: the extension's schema and types only exist when compiled in.
#![cfg(feature = "cyber")]

use cyber_gecko::CyberGecko;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use cyber_gecko::stix::{cyber_post_sync, to_okf, typed_relation_players};
use futures_util::StreamExt;
use gecko_engine::db::CORE_SCHEMA;
use gecko_engine::db::graph_store::RouterGraphStore;
use gecko_engine::db::router::TypeDbRouter;
use gecko_engine::extension::GeckoExtension;
use gecko_engine::syncer::bundle::{SyncResult, sync_bundle};
use mem_gecko::MemGecko;
use typedb_driver::Transaction;
use typedb_driver::answer::ConceptRow;
use typedb_driver::concept::Value;
use typedb_driver::given::{GivenRowEntry, GivenRows};

#[path = "common/attack.rs"]
mod attack;
#[path = "common/belief.rs"]
mod belief;
mod common;
#[path = "common/provenance.rs"]
mod provenance;
#[path = "common/stix_objects.rs"]
mod stix_objects;
#[path = "common/structure.rs"]
mod structure;
use belief::claim_with_belief;
use common::TestDb;

/// A representative STIX 2.1 / ATT&CK bundle exercising SDOs, an SCO observable
/// (defanged IP), a detection strategy with one analytic, and four SRO shapes, one
/// of them the pre-current data-component `detects`.
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
      "type": "x-mitre-detection-strategy",
      "id": "x-mitre-detection-strategy--00000000-0000-0000-0000-0000000000a6",
      "name": "Detect Command and Scripting Interpreter",
      "x_mitre_analytic_refs": ["x-mitre-analytic--00000000-0000-0000-0000-0000000000a7"]
    },
    {
      "type": "x-mitre-analytic",
      "id": "x-mitre-analytic--00000000-0000-0000-0000-0000000000a7",
      "name": "Analytic 0001",
      "x_mitre_platforms": ["Linux"],
      "x_mitre_log_source_references": [
        {
          "x_mitre_data_component_ref": "x-mitre-data-component--00000000-0000-0000-0000-0000000000f6",
          "name": "auditd:SYSCALL",
          "channel": "execve"
        }
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
      "id": "relationship--00000000-0000-0000-0000-000000000104",
      "relationship_type": "detects",
      "source_ref": "x-mitre-detection-strategy--00000000-0000-0000-0000-0000000000a6",
      "target_ref": "attack-pattern--00000000-0000-0000-0000-0000000000a1"
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
    let (manifest, rels) = to_okf(BUNDLE).expect("to_okf");
    let writes = cyber_post_sync(&rels);
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
    assert_eq!(
        count(&tx, r#"match $a isa analytic, has platform "Linux";"#).await,
        1,
        "an analytic's x_mitre_platforms must persist"
    );

    // indicator's valid-until parsed to a real datetime typed attribute.
    assert_eq!(
        count(&tx, r#"match $i isa indicator, has valid-until $v;"#).await,
        1
    );

    // SROs: indicates + uses linked, a strategy's detects -> strategy-detects, and
    // the embedded analytic and log-source refs synthesized.
    assert_eq!(count(&tx, "match $r isa indicates;").await, 1);
    assert_eq!(count(&tx, "match $r isa uses;").await, 1);
    assert_eq!(
        count(&tx, "match $r isa strategy-detects;").await,
        1,
        "a strategy's detects SRO must materialize as strategy-detects"
    );
    assert_eq!(
        count(&tx, "match $r isa strategy-analytic;").await,
        1,
        "x_mitre_analytic_refs must synthesize strategy-analytic"
    );
    assert_eq!(
        count(&tx, "match $r isa analytic-requirement;").await,
        1,
        "x_mitre_log_source_references must synthesize analytic-requirement"
    );

    // Re-syncing the same bundle is idempotent: relation counts do not change.
    sync_bundle(&mut db, &manifest, &writes, mem_gecko::BELIEF_TIER_ROOT)
        .await
        .expect("second sync_bundle");
    let tx = db.begin_read().await.expect("begin read after resync");
    for (label, expected) in [
        ("uses", 1),
        ("indicates", 1),
        ("strategy-detects", 1),
        ("strategy-analytic", 1),
        ("analytic-requirement", 1),
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

/// A tool that uses a technique, and a data component that detects one (the
/// pre-current ATT&CK shape the cyber schema does not model as a typed relation).
const SHAPES_BUNDLE: &str = r#"{
  "type": "bundle",
  "objects": [
    { "type": "attack-pattern", "id": "attack-pattern--00000000-0000-0000-0000-0000000001a1", "name": "T" },
    { "type": "tool", "id": "tool--00000000-0000-0000-0000-0000000001b2", "name": "Tool" },
    { "type": "x-mitre-data-component", "id": "x-mitre-data-component--00000000-0000-0000-0000-0000000001c3", "name": "Component" },
    { "type": "relationship", "id": "relationship--00000000-0000-0000-0000-000000000201",
      "relationship_type": "uses",
      "source_ref": "tool--00000000-0000-0000-0000-0000000001b2",
      "target_ref": "attack-pattern--00000000-0000-0000-0000-0000000001a1" },
    { "type": "relationship", "id": "relationship--00000000-0000-0000-0000-000000000202",
      "relationship_type": "detects",
      "source_ref": "x-mitre-data-component--00000000-0000-0000-0000-0000000001c3",
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

    let (manifest, rels) = to_okf(SHAPES_BUNDLE).expect("to_okf");
    assert_eq!(rels.typed.len(), 1, "only the tool-uses relation is typed");
    assert!(
        manifest.links.iter().any(|l| l.link_text == "detects"),
        "the data-component detects relation is kept as a concept-link"
    );
    sync_bundle(
        &mut db,
        &manifest,
        &cyber_post_sync(&rels),
        mem_gecko::BELIEF_TIER_ROOT,
    )
    .await
    .expect("sync_bundle");

    let tx = db.begin_read().await.expect("begin read");
    assert_eq!(
        count(&tx, "match $t isa tool; $r isa uses, links (user: $t);").await,
        1
    );
    assert_eq!(count(&tx, "match $r isa strategy-detects;").await, 0);
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
        ("tactic", &["concept-id", "tactic-shortname"][..]),
    ] {
        let got = required
            .get(entity)
            .unwrap_or_else(|| panic!("{entity} missing from {required:?}"));
        for a in attrs {
            assert!(got.contains(*a), "{entity} must require {a}: {got:?}");
        }
    }
}

const T1059_STIX_ID: &str = "attack-pattern--00000000-0000-0000-0000-0000000000a1";
const IP_OBSERVABLE_STIX_ID: &str = "ipv4-addr--00000000-0000-0000-0000-0000000000d4";

/// `BUNDLE` without the object whose STIX id is `stix_id`; relationships that
/// reference it stay in the bundle.
fn bundle_without(stix_id: &str) -> String {
    let mut bundle: serde_json::Value = serde_json::from_str(BUNDLE).expect("BUNDLE is JSON");
    let objects = bundle["objects"].as_array_mut().expect("objects array");
    let before = objects.len();
    objects.retain(|o| o["id"] != stix_id);
    assert_eq!(objects.len(), before - 1, "{stix_id} is not in BUNDLE");
    bundle.to_string()
}

/// Syncs a STIX bundle through the record path.
async fn sync_stix(db: &mut TypeDbRouter, stix: &str) -> SyncResult {
    let (manifest, rels) = to_okf(stix).expect("to_okf");
    sync_bundle(
        db,
        &manifest,
        &cyber_post_sync(&rels),
        mem_gecko::BELIEF_TIER_ROOT,
    )
    .await
    .expect("sync_bundle")
}

/// The concept-id of the one concept `selector` matches as `$c`.
async fn concept_id_of(db: &mut TypeDbRouter, selector: &str) -> String {
    let tx = db.begin_read().await.expect("begin read");
    let rows: Vec<_> = tx
        .query(format!("match {selector} $c has concept-id $i; select $i;"))
        .await
        .expect("concept-id query")
        .into_rows()
        .collect::<Vec<_>>()
        .await;
    assert_eq!(rows.len(), 1, "{selector} must match exactly one concept");
    rows.into_iter()
        .next()
        .expect("one row")
        .expect("row")
        .get("i")
        .expect("column i")
        .expect("i bound")
        .try_get_string()
        .expect("concept-id string")
        .to_string()
}

/// Runs `query` (a `given $id: string; ...` TypeQL query) with `id` bound, and
/// returns its rows.
async fn rows_for_id(tx: &Transaction, query: &str, id: &str) -> Vec<ConceptRow> {
    let mut given = GivenRows::new(vec!["id".to_string()], 1);
    given
        .push_row(vec![GivenRowEntry::Value(Value::String(id.to_string()))])
        .expect("bind id");
    let answer = tx
        .query_with_rows(query, given)
        .await
        .expect("query failed");
    answer
        .into_rows()
        .map(|r| r.expect("row"))
        .collect::<Vec<_>>()
        .await
}

/// The labels of the relations the concept `id` plays a role in, sorted.
async fn relations_played(db: &mut TypeDbRouter, id: &str) -> Vec<String> {
    let tx = db.begin_read().await.expect("begin read");
    let rows = rows_for_id(
        &tx,
        "given $id: string; \
         match $c isa concept, has concept-id $cid; $cid == $id; \
               $r links ($c); $r isa! $t; select $t;",
        id,
    )
    .await;
    let mut labels: Vec<String> = rows
        .into_iter()
        .map(|r| {
            r.get("t")
                .expect("column t")
                .expect("t bound")
                .try_get_label()
                .expect("type label")
                .to_string()
        })
        .collect();
    labels.sort();
    labels
}

#[tokio::test]
async fn dropping_a_claimed_technique_retires_it_and_its_typed_relations() {
    let test_db = TestDb::new("gecko_test_stix_retire_technique");
    let mut db = test_db.router();
    apply_cyber_schema(&mut db).await;
    sync_stix(&mut db, BUNDLE).await;

    let id = concept_id_of(&mut db, r#"$c isa attack-pattern, has attack-id "T1059";"#).await;
    claim_with_belief(&mut db, "mem/bel/claim", &id).await;
    let before = relations_played(&mut db, &id).await;
    assert!(
        before
            .iter()
            .any(|l| l != "containment" && l != "source-link"),
        "the technique plays no typed relation before retirement: {before:?}"
    );

    let result = sync_stix(&mut db, &bundle_without(T1059_STIX_ID)).await;
    assert_eq!(result.concepts_retired, 1);

    let tx = db.begin_read().await.expect("begin read");
    assert_eq!(
        rows_for_id(
            &tx,
            "given $id: string; match $c isa concept, has concept-id $cid; $cid == $id; \
             $c has retired-at $t;",
            &id
        )
        .await
        .len(),
        1,
        "the dropped technique is not stamped retired-at"
    );
    assert_eq!(
        rows_for_id(
            &tx,
            "given $id: string; match $c isa concept, has concept-id $cid; $cid == $id; \
             $c has attack-id $a;",
            &id
        )
        .await
        .len(),
        0,
        "the retired technique kept its attack-id"
    );
    drop(tx);
    assert_eq!(
        relations_played(&mut db, &id).await,
        vec!["containment".to_string(), "source-link".to_string()],
        "the retired technique plays more than its containment and the belief's source-link"
    );
}

#[tokio::test]
async fn retiring_a_claimed_observable_keeps_its_required_attributes() {
    let test_db = TestDb::new("gecko_test_stix_retire_observable");
    let mut db = test_db.router();
    apply_cyber_schema(&mut db).await;
    sync_stix(&mut db, BUNDLE).await;

    let id = concept_id_of(&mut db, r#"$c isa observable, has ioc-value "1.1.1.1";"#).await;
    claim_with_belief(&mut db, "mem/bel/claim", &id).await;

    let result = sync_stix(&mut db, &bundle_without(IP_OBSERVABLE_STIX_ID)).await;
    assert_eq!(result.concepts_retired, 1);

    let tx = db.begin_read().await.expect("begin read");
    assert_eq!(
        rows_for_id(
            &tx,
            "given $id: string; match $c isa observable, has concept-id $cid; $cid == $id; \
             $c has ioc-value \"1.1.1.1\", has ioc-type \"ipv4\", has retired-at $t;",
            &id
        )
        .await
        .len(),
        1,
        "the retired observable lost a required attribute or retired-at"
    );
}

/// The fixture's analytic requirements, keyed by (analytic concept-id, component
/// key, log-source name) to (log-source key, sorted channels).
type Requirements = BTreeMap<(String, String, String), (String, Vec<String>)>;

fn string_column(row: &ConceptRow, column: &str) -> String {
    let concept = row
        .get(column)
        .unwrap_or_else(|e| panic!("column {column}: {e}"))
        .unwrap_or_else(|| panic!("{column} unbound"));
    concept
        .try_get_string()
        .unwrap_or_else(|| panic!("{column} is not a string"))
        .to_string()
}

async fn select_rows(tx: &Transaction, query: &str) -> Vec<ConceptRow> {
    tx.query(query)
        .await
        .expect("query failed")
        .into_rows()
        .map(|r| r.expect("row"))
        .collect::<Vec<_>>()
        .await
}

async fn requirements(db: &mut TypeDbRouter) -> Requirements {
    let tx = db.begin_read().await.expect("begin read");
    let mut out = Requirements::new();
    for row in select_rows(
        &tx,
        "match $r isa analytic-requirement, links (analytic: $a, component: $c), \
               has log-source-name $n, has log-source-key $k; \
               $a has concept-id $aid; $c has component-key $ck; \
         select $aid, $ck, $n, $k;",
    )
    .await
    {
        let key = (
            string_column(&row, "aid"),
            string_column(&row, "ck"),
            string_column(&row, "n"),
        );
        let previous = out.insert(key.clone(), (string_column(&row, "k"), Vec::new()));
        assert!(previous.is_none(), "two requirements for {key:?}");
    }
    for row in select_rows(
        &tx,
        "match $r isa analytic-requirement, links (analytic: $a, component: $c), \
               has log-source-name $n, has log-source-channel $ch; \
               $a has concept-id $aid; $c has component-key $ck; \
         select $aid, $ck, $n, $ch;",
    )
    .await
    {
        let key = (
            string_column(&row, "aid"),
            string_column(&row, "ck"),
            string_column(&row, "n"),
        );
        out.get_mut(&key)
            .unwrap_or_else(|| panic!("a channel on an unlisted requirement {key:?}"))
            .1
            .push(string_column(&row, "ch"));
    }
    for (_, channels) in out.values_mut() {
        channels.sort();
    }
    out
}

/// The concept-id the adapter derives for an ATT&CK STIX id.
fn concept_id(stix_id: &str) -> String {
    let (ty, uuid) = stix_id.split_once("--").expect("a STIX id");
    format!("stix/{ty}/{uuid}")
}

fn requirement(
    analytic_stix_id: &str,
    component: &str,
    name: &str,
    key: &str,
    channels: &[&str],
) -> ((String, String, String), (String, Vec<String>)) {
    (
        (
            concept_id(analytic_stix_id),
            component.to_lowercase(),
            name.to_string(),
        ),
        (
            key.to_string(),
            channels.iter().map(|c| c.to_string()).collect(),
        ),
    )
}

/// The requirements [`attack::fixture_json`] syncs to.
fn fixture_requirements() -> Requirements {
    use attack::*;
    [
        requirement(
            WINDOWS_ANALYTIC_STIX_ID,
            PROCESS_CREATION,
            "WinEventLog:Sysmon",
            "wineventlog:sysmon",
            &["EventCode=1", "EventCode=10"],
        ),
        requirement(
            WINDOWS_ANALYTIC_STIX_ID,
            PROCESS_CREATION,
            "WinEventLog:Security",
            "wineventlog:security",
            &["EventCode=4688"],
        ),
        requirement(
            WINDOWS_ANALYTIC_STIX_ID,
            COMMAND_EXECUTION,
            "wineventlog:security",
            "wineventlog:security",
            &["EventCode=4103"],
        ),
        requirement(
            WINDOWS_ANALYTIC_STIX_ID,
            NETWORK_CONNECTION_CREATION,
            "NSM:Flow",
            "nsm:flow",
            &[],
        ),
        requirement(
            LINUX_ANALYTIC_STIX_ID,
            PROCESS_CREATION,
            "auditd:SYSCALL",
            "auditd:syscall",
            &["execve"],
        ),
        requirement(
            LINUX_ANALYTIC_STIX_ID,
            COMMAND_EXECUTION,
            "linux:syslog",
            "linux:syslog",
            &[],
        ),
        requirement(
            LINUX_ANALYTIC_STIX_ID,
            NETWORK_CONNECTION_CREATION,
            "nsm:flow",
            "nsm:flow",
            &["connect"],
        ),
        requirement(
            DEPRECATED_ANALYTIC_STIX_ID,
            PROCESS_CREATION,
            "WinEventLog:Security",
            "wineventlog:security",
            &["EventCode=4688"],
        ),
    ]
    .into_iter()
    .collect()
}

/// The fixture with `edit` applied to its parsed objects.
fn edited_fixture(edit: impl FnOnce(&mut Vec<serde_json::Value>)) -> String {
    let mut bundle: serde_json::Value =
        serde_json::from_str(&attack::fixture_json()).expect("fixture is JSON");
    edit(bundle["objects"].as_array_mut().expect("objects array"));
    bundle.to_string()
}

fn object_mut<'a>(
    objects: &'a mut [serde_json::Value],
    stix_id: &str,
) -> &'a mut serde_json::Value {
    objects
        .iter_mut()
        .find(|o| o["id"] == stix_id)
        .unwrap_or_else(|| panic!("{stix_id} is not in the fixture"))
}

/// Counts of every detection-model relation and of each typed record.
async fn detection_counts(db: &mut TypeDbRouter) -> Vec<(&'static str, usize)> {
    let tx = db.begin_read().await.expect("begin read");
    let mut out = Vec::new();
    for (label, query) in [
        ("strategy-detects", "match $r isa strategy-detects;"),
        ("strategy-analytic", "match $r isa strategy-analytic;"),
        ("analytic-requirement", "match $r isa analytic-requirement;"),
        (
            "log-source-channel",
            "match $r isa analytic-requirement, has log-source-channel $c;",
        ),
        ("detection-strategy", "match $s isa detection-strategy;"),
        ("analytic", "match $a isa analytic;"),
        (
            "component-key",
            "match $c isa data-component, has component-key $k;",
        ),
    ] {
        out.push((label, count(&tx, query).await));
    }
    out
}

#[tokio::test]
async fn stix_sync_models_strategies_analytics_and_requirements() {
    let test_db = TestDb::new("gecko_test_stix_detection_model");
    attack::sync_fixture(&test_db).await;
    let mut db = test_db.router();

    assert_eq!(
        detection_counts(&mut db).await,
        vec![
            ("strategy-detects", 1),
            ("strategy-analytic", 4),
            ("analytic-requirement", 8),
            ("log-source-channel", 7),
            ("detection-strategy", 1),
            ("analytic", 4),
            ("component-key", 3),
        ]
    );
    assert_eq!(requirements(&mut db).await, fixture_requirements());

    let tx = db.begin_read().await.expect("begin read");
    assert_eq!(
        count(
            &tx,
            r#"match $s isa detection-strategy, has attack-id "DET0001";
               $t isa attack-pattern, has attack-id "T1059";
               $r isa strategy-detects, links (strategy: $s, technique: $t);"#
        )
        .await,
        1
    );
    for (analytic, platform) in [
        (attack::WINDOWS_ANALYTIC, "Windows"),
        (attack::LINUX_ANALYTIC, "Linux"),
        (attack::MACOS_ANALYTIC, "macOS"),
    ] {
        assert_eq!(
            count(
                &tx,
                &format!(
                    r#"match $a isa analytic, has attack-id "{analytic}", has platform $p;
                       $p == "{platform}";
                       $s isa detection-strategy, has attack-id "DET0001";
                       $r isa strategy-analytic, links (strategy: $s, analytic: $a);"#
                )
            )
            .await,
            1,
            "{analytic} on {platform}"
        );
    }
    assert_eq!(
        count(&tx, "match $a isa analytic, has deprecated true;").await,
        1
    );
    assert_eq!(
        count(
            &tx,
            "match $a isa analytic, has deprecated true, has attack-id $i;"
        )
        .await,
        0,
        "the deprecated analytic claims no attack-id"
    );
    assert_eq!(
        count(&tx, "match $c isa cyber-object, has deprecated false;").await,
        8
    );
}

#[tokio::test]
async fn a_reimport_replaces_requirements() {
    let test_db = TestDb::new("gecko_test_stix_requirement_replace");
    attack::sync_fixture(&test_db).await;
    let mut db = test_db.router();

    let changed = edited_fixture(|objects| {
        let windows = object_mut(objects, attack::WINDOWS_ANALYTIC_STIX_ID);
        let sources = windows["x_mitre_log_source_references"]
            .as_array_mut()
            .expect("log-source references");
        let before = sources.len();
        sources.retain(|r| r["name"] != "WinEventLog:Security");
        assert_eq!(sources.len(), before - 1);
    });
    sync_stix(&mut db, &changed).await;

    let mut expected = fixture_requirements();
    let (removed, _) = requirement(
        attack::WINDOWS_ANALYTIC_STIX_ID,
        attack::PROCESS_CREATION,
        "WinEventLog:Security",
        "",
        &[],
    );
    assert!(expected.remove(&removed).is_some());
    assert_eq!(requirements(&mut db).await, expected);
}

#[tokio::test]
async fn a_reimport_without_an_analytic_leaves_no_requirement() {
    let test_db = TestDb::new("gecko_test_stix_analytic_removed");
    attack::sync_fixture(&test_db).await;
    let mut db = test_db.router();

    let changed = edited_fixture(|objects| {
        objects.retain(|o| o["id"] != attack::LINUX_ANALYTIC_STIX_ID);
        let strategy = object_mut(objects, attack::STRATEGY_STIX_ID);
        strategy["x_mitre_analytic_refs"]
            .as_array_mut()
            .expect("analytic refs")
            .retain(|r| r != attack::LINUX_ANALYTIC_STIX_ID);
    });
    let result = sync_stix(&mut db, &changed).await;
    assert_eq!(result.concepts_deleted, 1, "the Linux analytic is deleted");

    let tx = db.begin_read().await.expect("begin read");
    assert_eq!(
        count(
            &tx,
            "match $r isa analytic-requirement; not { $r links (analytic: $a); };"
        )
        .await,
        0,
        "an analytic-requirement outlived its analytic"
    );
    assert_eq!(
        count(
            &tx,
            "match $r isa strategy-analytic; not { $r links (analytic: $a); };"
        )
        .await,
        0,
        "a strategy-analytic outlived its analytic"
    );
    assert_eq!(count(&tx, "match $r isa analytic-requirement;").await, 5);
    assert_eq!(count(&tx, "match $r isa strategy-analytic;").await, 3);
}

#[tokio::test]
async fn a_reimport_without_a_detects_relationship_leaves_no_strategy_detects() {
    let test_db = TestDb::new("gecko_test_stix_detects_removed");
    attack::sync_fixture(&test_db).await;
    let mut db = test_db.router();

    let changed = edited_fixture(|objects| {
        let before = objects.len();
        objects.retain(|o| o["relationship_type"] != "detects");
        assert_eq!(objects.len(), before - 1);
    });
    sync_stix(&mut db, &changed).await;

    let tx = db.begin_read().await.expect("begin read");
    assert_eq!(
        count(
            &tx,
            &format!(
                r#"match $s isa detection-strategy, has attack-id "{}";
                   $r isa strategy-detects, links (strategy: $s);"#,
                attack::STRATEGY_ATTACK_ID
            )
        )
        .await,
        0,
        "a strategy-detects outlived its detects relationship"
    );
    assert_eq!(count(&tx, "match $r isa strategy-analytic;").await, 4);
}

#[tokio::test]
async fn stix_resync_is_stable() {
    let test_db = TestDb::new("gecko_test_stix_detection_resync");
    attack::sync_fixture(&test_db).await;
    let mut db = test_db.router();
    let counts = detection_counts(&mut db).await;
    let reqs = requirements(&mut db).await;

    sync_stix(&mut db, &attack::fixture_json()).await;
    assert_eq!(detection_counts(&mut db).await, counts);
    assert_eq!(requirements(&mut db).await, reqs);
}

#[tokio::test]
async fn a_database_synced_by_the_previous_adapter_is_retyped() {
    let test_db = TestDb::new("gecko_test_stix_previous_adapter");
    let mut db = test_db.router();
    apply_cyber_schema(&mut db).await;

    // The previous adapter stored strategies and analytics as bare cyber-objects
    // without platforms, and kept a strategy's detects SRO as a concept-link.
    let fixture = attack::fixture_json();
    let (mut previous, _) = to_okf(&fixture).expect("to_okf");
    for c in &mut previous.concepts {
        if matches!(
            c.type_hint.as_deref(),
            Some("detection-strategy" | "analytic")
        ) {
            c.type_hint = Some("cyber-object".to_string());
            c.typed_attributes.retain(|a| a.name != "platform");
            c.file_hash = format!("previous-adapter-{}", c.concept_id);
        }
    }
    previous.links.push(gecko_extension_api::okf::OkfLink {
        source_id: concept_id(attack::STRATEGY_STIX_ID),
        target_id: concept_id(attack::TECHNIQUE_STIX_ID),
        link_text: "detects".to_string(),
    });
    sync_bundle(&mut db, &previous, &[], mem_gecko::BELIEF_TIER_ROOT)
        .await
        .expect("seed with the previous adapter's shape");
    {
        let tx = db.begin_read().await.expect("begin read");
        assert_eq!(count(&tx, "match $c isa! cyber-object;").await, 5);
    }

    sync_stix(&mut db, &fixture).await;

    let tx = db.begin_read().await.expect("begin read");
    assert_eq!(count(&tx, "match $c isa! cyber-object;").await, 0);
    drop(tx);
    assert_eq!(
        detection_counts(&mut db).await,
        vec![
            ("strategy-detects", 1),
            ("strategy-analytic", 4),
            ("analytic-requirement", 8),
            ("log-source-channel", 7),
            ("detection-strategy", 1),
            ("analytic", 4),
            ("component-key", 3),
        ]
    );
    assert_eq!(requirements(&mut db).await, fixture_requirements());
}

/// The (first-role, second-role) concept-id pairs of every `relation`.
async fn relation_pairs(
    db: &mut TypeDbRouter,
    relation: &str,
    [a, b]: [&str; 2],
) -> BTreeSet<(String, String)> {
    let tx = db.begin_read().await.expect("begin read");
    select_rows(
        &tx,
        &format!(
            "match $r isa {relation}, links ({a}: $x, {b}: $y); \
                   $x has concept-id $xi; $y has concept-id $yi; \
             select $xi, $yi;"
        ),
    )
    .await
    .iter()
    .map(|row| (string_column(row, "xi"), string_column(row, "yi")))
    .collect()
}

/// The structure relations with their roles, in (source, target) order.
const STRUCTURE_RELATIONS: [(&str, [&str; 2]); 3] = [
    ("subtechnique-of", ["subtechnique", "parent"]),
    ("revoked-by", ["revoked", "replacement"]),
    ("technique-tactic", ["technique", "tactic"]),
];

/// The concept-id pairs `relation` links in the structure fixture.
fn structure_pairs(relation: &str) -> BTreeSet<(String, String)> {
    use structure::*;
    let pairs: &[(&str, &str)] = match relation {
        "subtechnique-of" => &[
            (SUB1_STIX_ID, PARENT_STIX_ID),
            (SUB2_STIX_ID, PARENT_STIX_ID),
            (RETIRED_SUB_STIX_ID, PARENT_STIX_ID),
        ],
        "revoked-by" => &[
            (REVOKED_FIRST_STIX_ID, REVOKED_SECOND_STIX_ID),
            (REVOKED_SECOND_STIX_ID, SUB1_STIX_ID),
            (MALWARE_STIX_ID, TOOL_STIX_ID),
        ],
        "technique-tactic" => &[
            (PARENT_STIX_ID, EXECUTION_STIX_ID),
            (PARENT_STIX_ID, PERSISTENCE_STIX_ID),
            (SUB1_STIX_ID, EXECUTION_STIX_ID),
            (SUB2_STIX_ID, PERSISTENCE_STIX_ID),
        ],
        other => panic!("{other} is not a structure relation"),
    };
    pairs
        .iter()
        .map(|(a, b)| (concept_id(a), concept_id(b)))
        .collect()
}

/// Asserts that every structure relation links exactly the fixture's pairs.
async fn assert_structure_relations(db: &mut TypeDbRouter) {
    for (relation, roles) in STRUCTURE_RELATIONS {
        assert_eq!(
            relation_pairs(db, relation, roles).await,
            structure_pairs(relation),
            "{relation}"
        );
    }
}

#[tokio::test]
async fn stix_sync_writes_subtechnique_revocation_and_tactic_relations() {
    let test_db = TestDb::new("gecko_test_stix_structure");
    structure::sync_fixture(&test_db).await;
    let mut db = test_db.router();
    assert_structure_relations(&mut db).await;

    {
        let tx = db.begin_read().await.expect("begin read");
        assert_eq!(
            count(&tx, "match $t isa tactic, has tactic-shortname $s;").await,
            2
        );
        assert_eq!(
            count(
                &tx,
                &format!(
                    r#"match $t isa attack-pattern, has retired-attack-id "{}";
                       not {{ $t has attack-id $i; }};"#,
                    structure::REVOKED_FIRST_ATTACK_ID
                )
            )
            .await,
            1,
            "a revoked technique keeps its ATT&CK id only as retired-attack-id"
        );
    }

    sync_stix(&mut db, &structure::fixture_json()).await;
    assert_structure_relations(&mut db).await;
}

/// The structure fixture with `edit` applied to its parsed objects.
fn edited_structure(edit: impl FnOnce(&mut Vec<serde_json::Value>)) -> String {
    let mut bundle: serde_json::Value =
        serde_json::from_str(&structure::fixture_json()).expect("fixture is JSON");
    edit(bundle["objects"].as_array_mut().expect("objects array"));
    bundle.to_string()
}

#[tokio::test]
async fn a_reimport_replaces_a_changed_parent_and_a_lifted_revocation() {
    use structure::*;
    let test_db = TestDb::new("gecko_test_stix_structure_replace");
    structure::sync_fixture(&test_db).await;
    let mut db = test_db.router();

    let changed = edited_structure(|objects| {
        let parent_link = objects
            .iter_mut()
            .find(|o| {
                o["relationship_type"] == "subtechnique-of" && o["source_ref"] == SUB2_STIX_ID
            })
            .expect("S2's subtechnique-of");
        parent_link["target_ref"] = SUB1_STIX_ID.into();
        let before = objects.len();
        objects.retain(|o| {
            !(o["relationship_type"] == "revoked-by" && o["source_ref"] == MALWARE_STIX_ID)
        });
        assert_eq!(objects.len(), before - 1);
        object_mut(objects, MALWARE_STIX_ID)["revoked"] = false.into();
        let phases = object_mut(objects, PARENT_STIX_ID)["kill_chain_phases"]
            .as_array_mut()
            .expect("P's phases");
        phases.retain(|p| p["phase_name"] != "persistence");
        assert_eq!(phases.len(), 1);
    });
    sync_stix(&mut db, &changed).await;

    let (s1, s2, p, m) = (
        concept_id(SUB1_STIX_ID),
        concept_id(SUB2_STIX_ID),
        concept_id(PARENT_STIX_ID),
        concept_id(MALWARE_STIX_ID),
    );
    let parents: Vec<String> =
        relation_pairs(&mut db, "subtechnique-of", ["subtechnique", "parent"])
            .await
            .into_iter()
            .filter(|(sub, _)| *sub == s2)
            .map(|(_, parent)| parent)
            .collect();
    assert_eq!(parents, vec![s1], "S2 has exactly one parent, S1");
    assert!(
        !relation_pairs(&mut db, "revoked-by", ["revoked", "replacement"])
            .await
            .iter()
            .any(|(revoked, _)| *revoked == m),
        "the malware's lifted revocation outlived the re-import"
    );
    let parent_tactics = relation_pairs(&mut db, "technique-tactic", ["technique", "tactic"])
        .await
        .into_iter()
        .filter(|(technique, _)| *technique == p)
        .count();
    assert_eq!(parent_tactics, 1, "P lost the phase it no longer names");
}

#[tokio::test]
async fn upgrading_from_mapping_2_leaves_no_untyped_subtechnique_or_revocation_links() {
    let test_db = TestDb::new("gecko_test_stix_structure_upgrade");
    let mut db = test_db.router();
    apply_cyber_schema(&mut db).await;

    // Mapping 2 stored tactics as bare cyber-objects without a shortname, and kept
    // the subtechnique-of and revoked-by SROs as concept-links.
    let fixture = structure::fixture_json();
    let (mut previous, rels) = to_okf(&fixture).expect("to_okf");
    for c in &mut previous.concepts {
        if c.type_hint.as_deref() == Some("tactic") {
            c.type_hint = Some("cyber-object".to_string());
            c.typed_attributes.retain(|a| a.name != "tactic-shortname");
        }
        c.file_hash = format!("mapping-2-{}", c.concept_id);
    }
    for r in rels
        .typed
        .iter()
        .filter(|r| matches!(r.rel_type.as_str(), "subtechnique-of" | "revoked-by"))
    {
        previous.links.push(gecko_extension_api::okf::OkfLink {
            source_id: r.source.clone(),
            target_id: r.target.clone(),
            link_text: r.rel_type.clone(),
        });
    }
    sync_bundle(&mut db, &previous, &[], mem_gecko::BELIEF_TIER_ROOT)
        .await
        .expect("seed with mapping 2's shape");
    let untyped = r#"match $l isa concept-link, has link-text $t;
                     { $t == "subtechnique-of"; } or { $t == "revoked-by"; };"#;
    {
        let tx = db.begin_read().await.expect("begin read");
        assert_eq!(count(&tx, untyped).await, 6);
        assert_eq!(count(&tx, "match $c isa tactic;").await, 0);
    }

    sync_stix(&mut db, &fixture).await;

    {
        let tx = db.begin_read().await.expect("begin read");
        assert_eq!(
            count(&tx, untyped).await,
            0,
            "an untyped structure link survived"
        );
        assert_eq!(count(&tx, "match $c isa tactic;").await, 2);
        assert_eq!(count(&tx, "match $c isa! cyber-object;").await, 0);
    }
    assert_structure_relations(&mut db).await;
}

#[tokio::test]
async fn a_reimport_where_an_analytic_loses_every_log_source_leaves_no_requirement() {
    let test_db = TestDb::new("gecko_test_stix_analytic_unreferenced");
    attack::sync_fixture(&test_db).await;
    let mut db = test_db.router();

    // The Windows analytic stays in the bundle, but no strategy references it and
    // it names no log source.
    let changed = edited_fixture(|objects| {
        let windows = object_mut(objects, attack::WINDOWS_ANALYTIC_STIX_ID);
        windows
            .as_object_mut()
            .expect("analytic object")
            .remove("x_mitre_log_source_references")
            .expect("log-source references");
        let strategy = object_mut(objects, attack::STRATEGY_STIX_ID);
        strategy["x_mitre_analytic_refs"]
            .as_array_mut()
            .expect("analytic refs")
            .retain(|r| r != attack::WINDOWS_ANALYTIC_STIX_ID);
    });
    sync_stix(&mut db, &changed).await;

    let windows = concept_id(attack::WINDOWS_ANALYTIC_STIX_ID);
    let expected: Requirements = fixture_requirements()
        .into_iter()
        .filter(|((analytic, _, _), _)| *analytic != windows)
        .collect();
    assert_eq!(requirements(&mut db).await, expected);
    let tx = db.begin_read().await.expect("begin read");
    assert_eq!(count(&tx, "match $r isa strategy-analytic;").await, 3);
}

/// The sensor file the full-bundle check syncs: three platforms and a few common
/// log sources.
const FULL_BUNDLE_SENSORS: &str = r#"
platforms: [Windows, Linux, macOS]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [WinEventLog:Sysmon, WinEventLog:Security]
  - name: linux-auditd
    platforms: [Linux]
    log_sources: [auditd:SYSCALL]
  - name: mac-logging
    platforms: [macOS]
    log_sources: [macos:unifiedlog]
"#;

/// The concept count and the count of every typed relation the adapter writes.
async fn graph_counts(db: &mut TypeDbRouter) -> BTreeMap<String, usize> {
    let tx = db.begin_read().await.expect("begin read");
    let mut counts = BTreeMap::new();
    counts.insert(
        "concept".to_string(),
        count(&tx, "match $c isa concept;").await,
    );
    counts.insert(
        "tactic".to_string(),
        count(&tx, "match $t isa tactic;").await,
    );
    for (relation, _, _) in typed_relation_players() {
        if !counts.contains_key(relation) {
            counts.insert(
                relation.to_string(),
                count(&tx, &format!("match $r isa {relation};")).await,
            );
        }
    }
    counts
}

/// Imports a real ATT&CK bundle twice into one database and asserts the second
/// import changes no count, then times `blinded` on the full graph. Opt-in: set
/// `GECKO_ATTACK_BUNDLE` to the path of an ATT&CK STIX bundle.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stix_full_bundle_resync_is_stable() {
    let Ok(path) = std::env::var("GECKO_ATTACK_BUNDLE") else {
        eprintln!("skipping: GECKO_ATTACK_BUNDLE is not set");
        return;
    };
    let stix = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));

    let test_db = TestDb::new("gecko_test_stix_full");
    let mut db = test_db.router();
    apply_cyber_schema(&mut db).await;

    let start = Instant::now();
    sync_stix(&mut db, &stix).await;
    let first_sync = start.elapsed();
    let first = graph_counts(&mut db).await;

    let start = Instant::now();
    sync_stix(&mut db, &stix).await;
    let resync = start.elapsed();
    let second = graph_counts(&mut db).await;

    eprintln!("first sync: {first_sync:?}, re-sync: {resync:?}");
    eprintln!("counts after the first sync: {first:?}");
    eprintln!("counts after the re-sync: {second:?}");
    assert_eq!(first, second, "a re-sync must change no count");
    for relation in ["strategy-analytic", "analytic-requirement"] {
        assert!(first[relation] > 0, "no {relation} in the full bundle");
    }

    // Sensors over the full graph, detection claims, and `blinded` timed twice.
    let sensor_file = cyber_gecko::sensors::parse(FULL_BUNDLE_SENSORS).expect("sensor file");
    sync_bundle(
        &mut db,
        &cyber_gecko::sensors::to_okf(&sensor_file, cyber_gecko::sensors::DEFAULT_BUNDLE_NAME),
        &[],
        mem_gecko::BELIEF_TIER_ROOT,
    )
    .await
    .expect("sync the sensor file");

    let logic_id = "playbooks/detect-full";
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("playbooks")).expect("playbooks dir");
    std::fs::write(
        dir.path().join("bundle.json"),
        r#"{ "name": "detect-playbooks" }"#,
    )
    .expect("write bundle.json");
    std::fs::write(
        dir.path().join(format!("{logic_id}.md")),
        "---\ntype: playbook\ntitle: Detect\n---\n\n# Detect\n",
    )
    .expect("write playbook");
    let playbooks = gecko_engine::okf::parser::parse_bundle(dir.path()).expect("parse playbooks");
    sync_bundle(&mut db, &playbooks, &[], mem_gecko::BELIEF_TIER_ROOT)
        .await
        .expect("sync playbooks");

    let store = Arc::new(RouterGraphStore::new(Arc::new(tokio::sync::Mutex::new(
        test_db.router(),
    ))));
    let registry = mem_gecko::BeliefEdgeRegistry::from_extensions(
        CORE_SCHEMA,
        &[mem_gecko::extension(), cyber_gecko::extension()],
    )
    .expect("belief-edge registry");
    let mem =
        Arc::new(mem_gecko::MemWriter::without_index(store.clone()).with_belief_edges(registry));
    let edges = mem
        .edge_writer_for("cyber-gecko")
        .expect("cyber-gecko declares belief edges");
    let ctx = gecko_extension_api::RunContext::new(
        gecko_extension_api::ActorId::new("system"),
        gecko_extension_api::ProvenanceSource::ExecutableDoc {
            concept_id: gecko_extension_api::ConceptId::new(logic_id),
        },
        chrono::Utc::now(),
    );
    let base: gecko_engine::sandbox::engine::ExtensionCallback =
        Arc::new(|_, _, _| Err("base".to_string()));
    let cb = cyber_gecko::detect::cyber_extension_callback(
        store as Arc<dyn gecko_extension_api::GraphReader>,
        edges,
        ctx,
        base,
    );
    let call = |func: &str, args: serde_json::Value| {
        let cb = cb.clone();
        let func = func.to_string();
        move || cb("cyber-gecko", &func, args)
    };
    for technique in FULL_BUNDLE_CLAIMS {
        let claim = call(
            "claim",
            serde_json::json!({ "logic_id": logic_id, "technique_id": technique,
                                "method": "external-tool", "confidence": 0.9 }),
        );
        tokio::task::spawn_blocking(claim)
            .await
            .expect("claim task")
            .unwrap_or_else(|e| panic!("claim {technique}: {e}"));
    }

    let start = Instant::now();
    let first_blinded = tokio::task::spawn_blocking(call("blinded", serde_json::json!({})))
        .await
        .expect("blinded task")
        .expect("blinded");
    let first_blinded_time = start.elapsed();
    let start = Instant::now();
    let second_blinded = tokio::task::spawn_blocking(call("blinded", serde_json::json!({})))
        .await
        .expect("blinded task")
        .expect("blinded");
    let second_blinded_time = start.elapsed();

    let entries = first_blinded["blinded_detections"]
        .as_array()
        .expect("blinded_detections is a list")
        .len();
    eprintln!(
        "blinded: first {first_blinded_time:?}, second {second_blinded_time:?}, \
         {entries} blinded detections"
    );
    assert!(entries > 0, "blinded reported nothing on the full graph");
    assert_eq!(first_blinded, second_blinded, "blinded is not stable");

    // `recommend_sources` on the same graph, timed twice and checked against `blinded`.
    let start = Instant::now();
    let first_recommend =
        tokio::task::spawn_blocking(call("recommend_sources", serde_json::json!({})))
            .await
            .expect("recommend_sources task")
            .expect("recommend_sources");
    let first_recommend_time = start.elapsed();
    let start = Instant::now();
    let second_recommend =
        tokio::task::spawn_blocking(call("recommend_sources", serde_json::json!({})))
            .await
            .expect("recommend_sources task")
            .expect("recommend_sources");
    let second_recommend_time = start.elapsed();
    assert_eq!(
        first_recommend, second_recommend,
        "recommend_sources is not stable"
    );
    eprintln!(
        "recommend_sources: first {first_recommend_time:?}, second {second_recommend_time:?}"
    );
    check_recommendation(&first_blinded, &first_recommend);
}

/// The techniques the full-bundle check claims: commonly detected ATT&CK
/// techniques across execution, persistence, credential access, discovery, lateral
/// movement and impact.
const FULL_BUNDLE_CLAIMS: [&str; 25] = [
    "T1003", "T1018", "T1021", "T1027", "T1036", "T1053", "T1055", "T1057", "T1059", "T1069",
    "T1070", "T1071", "T1078", "T1082", "T1083", "T1087", "T1105", "T1112", "T1218", "T1486",
    "T1490", "T1543", "T1547", "T1566", "T1569",
];

/// Checks `recommend_sources` against `blinded` on one graph, and prints the steps.
/// Every blind (claim, platform) is fixed by exactly one step or reported as
/// `no-requirements`; every unassessed one is `no-analytic`; no step re-adds a
/// source; and the running count matches.
fn check_recommendation(blinded: &serde_json::Value, recommend: &serde_json::Value) {
    use std::collections::BTreeSet;
    let claim = |v: &serde_json::Value| {
        (
            v["concept_id"].as_str().expect("concept_id").to_string(),
            v["technique_concept_id"]
                .as_str()
                .expect("technique_concept_id")
                .to_string(),
        )
    };

    let mut blind = BTreeSet::new();
    let mut unassessed = BTreeSet::new();
    for d in blinded["blinded_detections"]
        .as_array()
        .expect("detections")
    {
        for b in d["blind_platforms"].as_array().expect("blind_platforms") {
            let platform = b["platform"].as_str().expect("platform").to_string();
            blind.insert((claim(d), platform));
        }
        for p in d["unassessed_platforms"].as_array().expect("unassessed") {
            unassessed.insert((claim(d), p.as_str().expect("platform").to_string()));
        }
    }

    let mut fixed = BTreeSet::new();
    let mut added = BTreeSet::new();
    let mut cumulative = 0;
    for (n, step) in recommend["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .enumerate()
    {
        let platform = step["platform"].as_str().expect("platform").to_string();
        let add = step["add"].as_array().expect("add");
        let unblinds = step["unblinds"].as_array().expect("unblinds");
        assert!(!add.is_empty() && !unblinds.is_empty(), "step {n} is empty");
        let mut names = Vec::new();
        for a in add {
            let key = a["key"].as_str().expect("key").to_string();
            assert!(
                added.insert((platform.clone(), key)),
                "step {n} re-adds a source"
            );
            names.push(a["log_source"].as_str().expect("log_source").to_string());
        }
        for u in unblinds {
            assert!(
                fixed.insert((claim(u), platform.clone())),
                "step {n} lists a pair twice"
            );
        }
        cumulative += unblinds.len();
        assert_eq!(
            step["cumulative_unblinded"], cumulative,
            "step {n}'s running count"
        );
        eprintln!(
            "step {:>2}: {platform:<8} +{} unblinds {:>2} (total {cumulative:>3})  {}",
            n + 1,
            add.len(),
            unblinds.len(),
            names.join(", ")
        );
    }

    let mut no_requirements = BTreeSet::new();
    let mut no_analytic = BTreeSet::new();
    for u in recommend["unfixable"].as_array().expect("unfixable") {
        let pair = (
            claim(u),
            u["platform"].as_str().expect("platform").to_string(),
        );
        match u["reason"].as_str().expect("reason") {
            "no-requirements" => assert!(no_requirements.insert(pair)),
            "no-analytic" => assert!(no_analytic.insert(pair)),
            other => panic!("unknown unfixable reason {other:?}"),
        }
    }
    eprintln!(
        "recommend_sources: {} steps, {} sources, {} pairs unblinded, {} no-requirements, \
         {} no-analytic",
        recommend["steps"].as_array().map_or(0, Vec::len),
        added.len(),
        fixed.len(),
        no_requirements.len(),
        no_analytic.len()
    );

    assert!(
        !fixed.is_empty(),
        "recommend_sources fixed nothing on the full graph"
    );
    assert!(fixed.is_disjoint(&no_requirements));
    let fixable: BTreeSet<_> = fixed.union(&no_requirements).cloned().collect();
    assert_eq!(
        fixable, blind,
        "every blind pair is fixed or has no requirements"
    );
    assert_eq!(
        no_analytic, unassessed,
        "every unassessed pair is no-analytic"
    );
}

/// End-to-end check that repeated SROs land as one typed relation. TypeDB 3.15
/// already collapses identical rows of one `given` batch, so this test passes with
/// or without host-side deduplication; `stix::tests::repeated_sros_become_one_row`
/// is the test that discriminates.
#[tokio::test]
async fn repeated_sros_become_one_typed_relation() {
    let test_db = TestDb::new("gecko_test_stix_dup_sro");
    let mut db = test_db.router();
    db.apply_schema(CORE_SCHEMA).await.expect("core schema");
    for ext in [
        Box::new(MemGecko::new()) as Box<dyn GeckoExtension>,
        Box::new(CyberGecko::new()),
    ] {
        db.apply_schema(ext.schema())
            .await
            .unwrap_or_else(|e| panic!("apply {} schema: {e}", ext.name()));
    }

    // The same `uses` SRO appears twice under different relationship ids.
    let mut bundle: serde_json::Value = serde_json::from_str(BUNDLE).expect("bundle json");
    let objects = bundle["objects"].as_array_mut().expect("objects");
    let uses = objects
        .iter()
        .find(|o| o["relationship_type"] == "uses")
        .expect("uses SRO")
        .clone();
    let mut repeat = uses;
    repeat["id"] = "relationship--00000000-0000-0000-0000-000000000199".into();
    objects.push(repeat);

    let (manifest, rels) = to_okf(&bundle.to_string()).expect("to_okf");
    let writes = cyber_post_sync(&rels);
    sync_bundle(&mut db, &manifest, &writes, mem_gecko::BELIEF_TIER_ROOT)
        .await
        .expect("sync_bundle");
    let tx = db.begin_read().await.expect("begin read");
    assert_eq!(
        count(&tx, "match $r isa uses;").await,
        1,
        "a repeated SRO must be one relation"
    );
}

const MALWARE_STIX_ID: &str = "malware--00000000-0000-0000-0000-0000000000b2";

/// Every cyber relation whose roles the schema requires, with those roles.
const REQUIRED_ROLES: &[(&str, &[&str])] = &[
    ("indicates", &["indicator-source", "indicated"]),
    ("uses", &["user", "used"]),
    ("attributed-to", &["attributee", "actor"]),
    ("targets", &["threat-source", "target"]),
    ("mitigates", &["mitigation", "mitigated"]),
    ("observed-as", &["pattern-indicator", "observable-value"]),
    ("strategy-detects", &["strategy", "technique"]),
    ("strategy-analytic", &["strategy", "analytic"]),
    ("analytic-requirement", &["analytic", "component"]),
    ("subtechnique-of", &["subtechnique", "parent"]),
    ("revoked-by", &["revoked", "replacement"]),
    ("technique-tactic", &["technique", "tactic"]),
    (
        "technique-detection",
        &["detecting-logic", "detected-technique", "detection-claim"],
    ),
    (
        "alert-disposition",
        &["dispositioned-logic", "disposition-belief"],
    ),
];

/// The number of `relation` rows missing a player for any of `roles`.
async fn one_player_rows(tx: &Transaction, relation: &str, roles: &[&str]) -> usize {
    let missing = roles
        .iter()
        .enumerate()
        .map(|(i, role)| format!("{{ not {{ $r links ({role}: $p{i}); }}; }}"))
        .collect::<Vec<_>>()
        .join(" or ");
    count(tx, &format!("match $r isa {relation}; {missing};")).await
}

#[tokio::test]
async fn a_reimport_that_drops_a_malware_keeps_no_one_player_uses() {
    let test_db = TestDb::new("gecko_test_stix_drop_malware");
    let mut db = test_db.router();
    apply_cyber_schema(&mut db).await;
    sync_stix(&mut db, BUNDLE).await;

    let changed = bundle_without(MALWARE_STIX_ID);
    let (manifest, rels) = to_okf(&changed).expect("to_okf");
    let ids: std::collections::HashSet<&str> = manifest
        .concepts
        .iter()
        .map(|c| c.concept_id.as_str())
        .collect();
    let expected = rels
        .typed
        .iter()
        .filter(|r| r.rel_type == "uses")
        .filter(|r| ids.contains(r.source.as_str()) && ids.contains(r.target.as_str()))
        .count();
    sync_stix(&mut db, &changed).await;

    let tx = db.begin_read().await.expect("begin read");
    assert_eq!(count(&tx, "match $r isa uses;").await, expected);
    for (relation, roles) in REQUIRED_ROLES {
        assert_eq!(
            one_player_rows(&tx, relation, roles).await,
            0,
            "a {relation} lost a player and survived the re-import"
        );
    }
}

#[tokio::test]
async fn a_uses_missing_its_user_is_refused_at_commit() {
    let test_db = TestDb::new("gecko_test_stix_uses_refused");
    let mut db = test_db.router();
    apply_cyber_schema(&mut db).await;

    let tx = db.begin_write().await.expect("begin write");
    tx.query(
        "insert $t isa attack-pattern, has concept-id \"raw/t\"; \
                $r isa uses, links (used: $t);",
    )
    .await
    .expect("the insert itself is accepted");
    let err = tx
        .commit()
        .await
        .expect_err("a uses without its user commits");
    assert!(err.to_string().contains("'uses:user'"), "{err}");
}

const PRE_CORE: &str = include_str!("fixtures/schemas_pre_tightening/core_schema.tql");
const PRE_MEM_TYPES: &str = include_str!("fixtures/schemas_pre_tightening/mem_types.tql");
const PRE_MEM_FUNCTIONS: &str = include_str!("fixtures/schemas_pre_tightening/mem_functions.tql");
const PRE_CYBER: &str = include_str!("fixtures/schemas_pre_tightening/cyber_schema.tql");
const PRE_CYBER_FUNCTIONS: &str =
    include_str!("fixtures/schemas_pre_tightening/cyber_functions.tql");

/// The `relation` declaration in a schema export, up to the `;` that ends its
/// last line.
fn relation_block<'a>(export: &'a str, relation: &str) -> &'a str {
    let head = format!("relation {relation}");
    let start = export
        .match_indices(&head)
        .map(|(i, _)| i)
        .find(|&i| {
            export[i + head.len()..]
                .chars()
                .next()
                .is_some_and(|c| c == ' ' || c == ',' || c == '\n')
        })
        .unwrap_or_else(|| panic!("the export declares no relation {relation}"));
    let end = export[start..].find(";\n").expect("the declaration ends") + start;
    &export[start..end]
}

/// Commits one write query.
async fn write_query(db: &mut TypeDbRouter, query: &str) {
    let tx = db.begin_write().await.expect("begin write");
    tx.query(query).await.expect("write query");
    tx.commit().await.expect("commit");
}

/// The belief edges cyber declares, which have no upgrade cleanup.
const BELIEF_EDGES: [&str; 2] = ["technique-detection", "alert-disposition"];

/// The relations declared with their roles required from the start, which have no
/// pre-tightening form and so no upgrade cleanup.
const REQUIRED_FROM_THE_START: [&str; 3] = ["subtechnique-of", "revoked-by", "technique-tactic"];

/// Whether `relation` has no upgrade cleanup.
fn has_no_cleanup(relation: &str) -> bool {
    BELIEF_EDGES.contains(&relation) || REQUIRED_FROM_THE_START.contains(&relation)
}

/// Applies the frozen pre-tightening core, mem and cyber schemas.
async fn apply_pre_tightening_schemas(db: &mut TypeDbRouter) {
    db.apply_schema(PRE_CORE)
        .await
        .expect("pre-tightening core");
    let functions = PRE_MEM_FUNCTIONS
        .trim_start()
        .strip_prefix("define")
        .expect("the functions fragment starts with define");
    db.apply_schema(&format!("{}\n{functions}", PRE_MEM_TYPES.trim_end()))
        .await
        .expect("pre-tightening mem");
    db.apply_schema(&format!("{PRE_CYBER}\n{PRE_CYBER_FUNCTIONS}"))
        .await
        .expect("pre-tightening cyber");
}

/// Applies the shipped core and mem with their migrations, then returns the result
/// of applying the shipped cyber with its migrations.
async fn apply_shipped_schemas(db: &mut TypeDbRouter) -> Result<(), String> {
    let mem = MemGecko::new();
    let cyber = CyberGecko::new();
    db.apply_schema_with_migrations(CORE_SCHEMA, gecko_engine::db::CORE_MIGRATIONS)
        .await
        .expect("the shipped core");
    db.apply_schema_with_migrations(mem.schema(), mem.schema_migrations())
        .await
        .expect("the shipped mem");
    db.apply_schema_with_migrations(cyber.schema(), cyber.schema_migrations())
        .await
        .map_err(|e| e.to_string())
}

#[tokio::test]
async fn a_database_holding_a_one_player_uses_upgrades() {
    let test_db = TestDb::new("gecko_test_stix_one_player_upgrade");
    let mut db = test_db.router();
    apply_pre_tightening_schemas(&mut db).await;

    // One fully played row and one row per missing role, for every relation that
    // is not a belief edge.
    write_query(
        &mut db,
        "insert $is isa intrusion-set, has concept-id \"up/is\"; \
                $ta isa threat-actor, has concept-id \"up/ta\"; \
                $mw isa malware, has concept-id \"up/mw\"; \
                $ap isa attack-pattern, has concept-id \"up/ap\"; \
                $in isa indicator, has concept-id \"up/in\"; \
                $ob isa observable, has concept-id \"up/ob\", has ioc-value \"1.1.1.1\", \
                    has ioc-type \"ipv4\"; \
                $ha isa host-asset, has concept-id \"up/ha\"; \
                $co isa course-of-action, has concept-id \"up/co\"; \
                $ds isa detection-strategy, has concept-id \"up/ds\"; \
                $an isa analytic, has concept-id \"up/an\"; \
                $dc isa data-component, has concept-id \"up/dc\"; \
                indicates (indicator-source: $in, indicated: $mw); \
                indicates (indicator-source: $in); \
                indicates (indicated: $mw); \
                uses (user: $is, used: $ap); \
                uses (user: $is); \
                uses (used: $ap); \
                attributed-to (attributee: $is, actor: $ta); \
                attributed-to (attributee: $is); \
                attributed-to (actor: $ta); \
                targets (threat-source: $is, target: $ha); \
                targets (threat-source: $is); \
                targets (target: $ha); \
                mitigates (mitigation: $co, mitigated: $ap); \
                mitigates (mitigation: $co); \
                mitigates (mitigated: $ap); \
                observed-as (pattern-indicator: $in, observable-value: $ob); \
                observed-as (pattern-indicator: $in); \
                observed-as (observable-value: $ob); \
                strategy-detects (strategy: $ds, technique: $ap); \
                strategy-detects (strategy: $ds); \
                strategy-detects (technique: $ap); \
                strategy-analytic (strategy: $ds, analytic: $an); \
                strategy-analytic (strategy: $ds); \
                strategy-analytic (analytic: $an); \
                analytic-requirement (analytic: $an, component: $dc), \
                    has log-source-name \"n\", has log-source-key \"n\"; \
                analytic-requirement (analytic: $an), \
                    has log-source-name \"n\", has log-source-key \"n\"; \
                analytic-requirement (component: $dc), \
                    has log-source-name \"n\", has log-source-key \"n\";",
    )
    .await;
    // Each cleanup's trigger is in the pre-tightening export exactly once.
    let cyber = CyberGecko::new();
    let cleanups: Vec<_> = cyber
        .schema_migrations()
        .iter()
        .filter(|m| m.id.ends_with("-required-roles"))
        .collect();
    assert_eq!(
        cleanups.len(),
        REQUIRED_ROLES.len() - BELIEF_EDGES.len() - REQUIRED_FROM_THE_START.len(),
        "one cleanup per cyber relation that is neither a belief edge nor required from the start"
    );
    let before = db.database_schema().await.expect("schema");
    for m in &cleanups {
        assert_eq!(before.matches(m.applies_when).count(), 1, "{}", m.id);
    }

    for _ in 0..2 {
        apply_shipped_schemas(&mut db)
            .await
            .expect("cyber cleanups, then the shipped cyber");
    }

    let after = db.database_schema().await.expect("schema");
    for m in cyber.schema_migrations() {
        assert!(!after.contains(m.applies_when), "{} still pending", m.id);
    }
    let tx = db.begin_read().await.expect("begin read");
    for (relation, roles) in REQUIRED_ROLES {
        let block = relation_block(&after, relation);
        for role in *roles {
            assert!(
                block.contains(&format!("relates {role} @card(1..1)")),
                "{relation}:{role} is not required: {block}"
            );
        }
        if has_no_cleanup(relation) {
            continue;
        }
        let full = roles
            .iter()
            .enumerate()
            .map(|(i, role)| format!("{role}: $p{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(
            count(&tx, &format!("match $r isa {relation};")).await,
            1,
            "only the fully played {relation} survives"
        );
        assert_eq!(
            count(&tx, &format!("match $r isa {relation}, links ({full});")).await,
            1,
            "the surviving {relation} is the fully played one"
        );
    }
}

#[tokio::test]
async fn a_database_holding_a_one_player_technique_detection_fails_schema_init_naming_the_relation()
{
    let test_db = TestDb::new("gecko_test_stix_one_player_detection");
    let mut db = test_db.router();
    apply_pre_tightening_schemas(&mut db).await;
    write_query(
        &mut db,
        "insert $ap isa attack-pattern, has concept-id \"up/ap\"; \
                $pb isa concept, has concept-id \"up/pb\"; \
                uses (used: $ap); \
                technique-detection (detecting-logic: $pb, detected-technique: $ap);",
    )
    .await;

    let err = apply_shipped_schemas(&mut db)
        .await
        .expect_err("a technique-detection without its claim passes the required roles");
    assert!(err.contains("'technique-detection"), "{err}");

    // The refusing cyber fragment is one schema transaction, so its own cleanups are
    // rolled back (core and mem commit first, in their own transactions).
    let tx = db.begin_read().await.expect("begin read");
    assert_eq!(count(&tx, "match $r isa technique-detection;").await, 1);
    assert_eq!(count(&tx, "match $r isa uses;").await, 1);
}
