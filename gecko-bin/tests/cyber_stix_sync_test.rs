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
use std::collections::BTreeMap;
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
    for technique in ["T1059", "T1003", "T1078"] {
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
}

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
