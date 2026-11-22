// End-to-end proof of the sensor file against a live ATT&CK database: the
// pre-sync check refuses a database without ATT&CK and every component that does
// not resolve to exactly one live data component, a sensor file syncs
// idempotently, a re-sync drops a removed component, and the post-sync notices
// name what the file declares that ATT&CK does not use.
//
// Gated on `cyber`: the extension's schema and types only exist when compiled in.
#![cfg(feature = "cyber")]

use std::sync::Arc;

use cyber_gecko::CyberGecko;
use cyber_gecko::sensors::{
    SensorFile, check_against_graph, check_attack, check_bundle_name, notices, parse, to_okf,
};
use gecko_engine::db::CORE_SCHEMA;
use gecko_engine::db::graph_store::RouterGraphStore;
use gecko_engine::extension::GeckoExtension;
use gecko_engine::syncer::bundle::{SyncResult, sync_bundle};
use gecko_extension_api::{GraphReader, GraphValue};
use mem_gecko::MemGecko;
use serde_json::Value;
use tokio::sync::Mutex;

#[path = "common/attack.rs"]
mod attack;
mod common;
use common::TestDb;

/// The read-only graph handle the cyber bridge uses.
fn graph(test_db: &TestDb) -> Arc<dyn GraphReader> {
    Arc::new(RouterGraphStore::new(Arc::new(Mutex::new(
        test_db.router(),
    ))))
}

async fn sync_sensors(test_db: &TestDb, file: &SensorFile) -> SyncResult {
    let mut db = test_db.router();
    sync_bundle(
        &mut db,
        &to_okf(file, cyber_gecko::sensors::DEFAULT_BUNDLE_NAME),
        &[],
        mem_gecko::BELIEF_TIER_ROOT,
    )
    .await
    .expect("sync the sensor file")
}

/// The sensor `sensors/sensor/<name>`'s values of each of `attrs`.
async fn sensor_attrs(
    graph: &Arc<dyn GraphReader>,
    name: &str,
    attrs: &[&str],
) -> Vec<Vec<String>> {
    let rows = graph
        .read(
            "given $id: string; \
             match $s isa sensor, has concept-id $cid; $cid == $id; \
             fetch { \"platform\": [$s.platform], \"log-source-key\": [$s.log-source-key], \
                     \"covered-component-key\": [$s.covered-component-key] };",
            &["id".into()],
            &[GraphValue::from(format!("sensors/sensor/{name}").as_str())],
        )
        .await
        .expect("read the sensor");
    assert_eq!(rows.len(), 1, "one sensor {name}: {rows:?}");
    attrs
        .iter()
        .map(|a| {
            let mut v: Vec<String> = rows[0][*a]
                .as_array()
                .unwrap_or_else(|| panic!("{a} is a list: {rows:?}"))
                .iter()
                .map(|x| x.as_str().expect("a string").to_string())
                .collect();
            v.sort();
            v
        })
        .collect()
}

const WITH_COMPONENT: &str = r#"
platforms: [Windows, Linux]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [WinEventLog:Sysmon, " WinEventLog:Security"]
    components: [" process CREATION"]
"#;

const WITHOUT_COMPONENT: &str = r#"
platforms: [Windows, Linux]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [WinEventLog:Sysmon, " WinEventLog:Security"]
"#;

#[tokio::test]
async fn refuses_a_database_without_attack() {
    let test_db = TestDb::new("gecko_test_sensors_no_attack");
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

    let file = parse(WITHOUT_COMPONENT).expect("parses");
    let err = check_against_graph(&graph(&test_db), &file)
        .await
        .expect_err("no ATT&CK in the database");
    assert!(
        err.contains("sync the ATT&CK bundle into this database first"),
        "{err}"
    );
}

#[tokio::test]
async fn refuses_a_database_without_a_schema() {
    let test_db = TestDb::new("gecko_test_sensors_no_schema");
    test_db
        .router()
        .ensure_database()
        .await
        .expect("create the database");

    // `gecko sync-sensors` checks for ATT&CK before the bundle name, whose query
    // a database without the schema cannot type.
    let graph = graph(&test_db);
    let err = check_attack(&graph)
        .await
        .expect_err("no ATT&CK in the database");
    assert!(
        err.contains("sync the ATT&CK bundle into this database first"),
        "{err}"
    );
    let file = parse(WITHOUT_COMPONENT).expect("parses");
    let err = check_against_graph(&graph, &file)
        .await
        .expect_err("no ATT&CK in the database");
    assert!(
        err.contains("sync the ATT&CK bundle into this database first"),
        "{err}"
    );
}

#[tokio::test]
async fn refuses_every_unmatched_or_ambiguous_component_at_once() {
    let test_db = TestDb::new("gecko_test_sensors_components");
    attack::sync_fixture(&test_db).await;

    // A second live `Command Execution` makes that name ambiguous; a deprecated
    // `Process Creation` does not compete with the live one.
    let mut db = test_db.router();
    let tx = db.begin_write().await.expect("write tx");
    tx.query(
        r#"insert
             $a isa data-component, has concept-id "extra/command-execution",
               has component-key "command execution";
             $b isa data-component, has concept-id "extra/process-creation",
               has component-key "process creation", has deprecated true;"#,
    )
    .await
    .expect("seed extra components");
    tx.commit().await.expect("commit");

    let file = parse(&format!(
        "platforms: [Windows]\nsensors:\n  - name: edr\n    platforms: [Windows]\n    \
         components: [{}, No Such Component, {}]\n",
        attack::PROCESS_CREATION,
        attack::COMMAND_EXECUTION
    ))
    .expect("parses");
    let graph = graph(&test_db);
    let err = check_against_graph(&graph, &file)
        .await
        .expect_err("two components do not resolve");
    assert!(err.contains("No Such Component"), "{err}");
    assert!(err.contains(attack::COMMAND_EXECUTION), "{err}");
    assert!(!err.contains(attack::PROCESS_CREATION), "{err}");

    let ok = parse(WITH_COMPONENT).expect("parses");
    check_against_graph(&graph, &ok)
        .await
        .expect("Process Creation resolves to one live component");
}

#[tokio::test]
async fn refuses_a_platform_in_the_wrong_case() {
    let test_db = TestDb::new("gecko_test_sensors_platform_case");
    attack::sync_fixture(&test_db).await;
    let file = parse(
        "platforms: [windows, Linux, AIX]\nsensors:\n  - name: edr\n    \
         platforms: [MACOS, Linux]\n    components: [No Such Component]\n",
    )
    .expect("parses");
    let graph = graph(&test_db);
    let err = check_against_graph(&graph, &file)
        .await
        .expect_err("two platforms are in the wrong case");
    for needle in [
        "platform 'windows' should be spelled 'Windows'",
        "sensor 'edr': platform 'MACOS' should be spelled 'macOS'",
        "No Such Component",
    ] {
        assert!(err.contains(needle), "{needle} missing from {err}");
    }
    assert!(!err.contains("'Linux'"), "{err}");
    assert!(!err.contains("AIX"), "{err}");

    let ok = parse(
        "platforms: [Windows, Linux, AIX]\nsensors:\n  - name: edr\n    \
         platforms: [macOS]\n    log_sources: [a]\n",
    )
    .expect("parses");
    check_against_graph(&graph, &ok)
        .await
        .expect("every platform is spelled as ATT&CK spells it");
}

#[tokio::test]
async fn refuses_a_bundle_name_that_holds_other_records() {
    let test_db = TestDb::new("gecko_test_sensors_bundle_name");
    attack::sync_fixture(&test_db).await;
    let graph = graph(&test_db);

    let err = check_bundle_name(&graph, "stix-sync")
        .await
        .expect_err("stix-sync holds the ATT&CK import");
    assert!(err.contains("'stix-sync'"), "{err}");
    assert!(err.contains("not sensors"), "{err}");

    check_bundle_name(&graph, cyber_gecko::sensors::DEFAULT_BUNDLE_NAME)
        .await
        .expect("no bundle of that name yet");
    sync_sensors(&test_db, &parse(WITH_COMPONENT).expect("parses")).await;
    check_bundle_name(&graph, cyber_gecko::sensors::DEFAULT_BUNDLE_NAME)
        .await
        .expect("the bundle holds only the sensor file's records");
}

#[tokio::test]
async fn sync_twice_changes_nothing() {
    let test_db = TestDb::new("gecko_test_sensors_twice");
    attack::sync_fixture(&test_db).await;
    let file = parse(WITH_COMPONENT).expect("parses");
    let graph = graph(&test_db);
    check_against_graph(&graph, &file)
        .await
        .expect("the file resolves");

    let first = sync_sensors(&test_db, &file).await;
    assert_eq!(first.concepts_inserted, 2, "{first:?}");
    let attrs = &["platform", "log-source-key", "covered-component-key"];
    let before = sensor_attrs(&graph, "windows-edr", attrs).await;
    assert_eq!(
        before,
        vec![
            vec!["Windows".to_string()],
            vec![
                "wineventlog:security".to_string(),
                "wineventlog:sysmon".to_string()
            ],
            vec!["process creation".to_string()],
        ]
    );

    let second = sync_sensors(&test_db, &file).await;
    assert_eq!(
        (
            second.concepts_inserted,
            second.concepts_updated,
            second.concepts_deleted,
            second.concepts_skipped
        ),
        (0, 0, 0, 2),
        "{second:?}"
    );
    assert_eq!(sensor_attrs(&graph, "windows-edr", attrs).await, before);

    let env = graph
        .read(
            "match $e isa monitored-environment, has concept-id $cid; \
             fetch { \"id\": $cid, \"platform\": [$e.platform] };",
            &[],
            &[],
        )
        .await
        .expect("read the environment");
    assert_eq!(env.len(), 1, "{env:?}");
    assert_eq!(env[0]["id"], Value::from("sensors/environment"));
    let mut platforms: Vec<&str> = env[0]["platform"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|p| p.as_str().expect("a string"))
        .collect();
    platforms.sort();
    assert_eq!(platforms, ["Linux", "Windows"]);
}

#[tokio::test]
async fn notices_name_unused_log_sources_and_platforms() {
    let test_db = TestDb::new("gecko_test_sensors_notices");
    attack::sync_fixture(&test_db).await;
    let file = parse(
        r#"
platforms: [Windows, Linux, Solaris]
sensors:
  - name: windows-edr
    platforms: [AIX]
    log_sources: [WinEventLog:Sysmon, Custom:Unknown]
"#,
    )
    .expect("parses");
    let graph = graph(&test_db);
    check_against_graph(&graph, &file)
        .await
        .expect("the file resolves");
    sync_sensors(&test_db, &file).await;

    let notes = notices(&graph, &file).await.expect("notices");
    let all = notes.join("\n");
    for needle in ["'AIX'", "Custom:Unknown", "Solaris"] {
        assert!(all.contains(needle), "{needle} missing from {notes:?}");
    }
    for absent in ["WinEventLog:Sysmon", "'Linux'", "'Windows'"] {
        assert!(!all.contains(absent), "{absent} in {notes:?}");
    }
    assert_eq!(notes.len(), 3, "{notes:?}");
}

#[tokio::test]
async fn a_removed_component_leaves_the_sensor() {
    let test_db = TestDb::new("gecko_test_sensors_removed");
    attack::sync_fixture(&test_db).await;
    let graph = graph(&test_db);

    sync_sensors(&test_db, &parse(WITH_COMPONENT).expect("parses")).await;
    let attrs = &["covered-component-key", "log-source-key"];
    assert_eq!(
        sensor_attrs(&graph, "windows-edr", attrs).await[0],
        vec!["process creation".to_string()]
    );

    let resync = sync_sensors(&test_db, &parse(WITHOUT_COMPONENT).expect("parses")).await;
    assert_eq!(resync.concepts_updated, 1, "{resync:?}");
    let after = sensor_attrs(&graph, "windows-edr", attrs).await;
    assert!(
        after[0].is_empty(),
        "the removed component stays: {after:?}"
    );
    assert_eq!(
        after[1],
        vec![
            "wineventlog:security".to_string(),
            "wineventlog:sysmon".to_string()
        ]
    );
}

/// Re-syncs the ATT&CK fixture with each object `deprecate` names deprecated.
async fn resync_deprecated(test_db: &TestDb, deprecate: &[&str]) {
    let mut bundle: Value = serde_json::from_str(&attack::fixture_json()).expect("fixture json");
    for o in bundle["objects"].as_array_mut().expect("objects") {
        if deprecate.iter().any(|d| o["id"] == *d || o["name"] == *d) {
            o["x_mitre_deprecated"] = serde_json::json!(true);
        }
    }
    let (manifest, rels) = cyber_gecko::stix::to_okf(&bundle.to_string()).expect("converts");
    let mut db = test_db.router();
    sync_bundle(
        &mut db,
        &manifest,
        &cyber_gecko::stix::cyber_post_sync(&rels),
        mem_gecko::BELIEF_TIER_ROOT,
    )
    .await
    .expect("re-sync the ATT&CK fixture");
}

#[tokio::test]
async fn notices_skip_dead_components_and_analytics_of_a_dead_strategy() {
    let test_db = TestDb::new("gecko_test_sensors_notices_liveness");
    attack::sync_fixture(&test_db).await;
    let file = parse(
        r#"
platforms: [Windows]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [WinEventLog:Sysmon]
"#,
    )
    .expect("parses");
    let graph = graph(&test_db);
    assert!(notices(&graph, &file).await.expect("notices").is_empty());

    // WinEventLog:Sysmon serves only Process Creation.
    resync_deprecated(&test_db, &[attack::PROCESS_CREATION]).await;
    let notes = notices(&graph, &file).await.expect("notices");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("WinEventLog:Sysmon"), "{notes:?}");

    // A deprecated strategy leaves no live analytic on Windows.
    resync_deprecated(
        &test_db,
        &[attack::PROCESS_CREATION, attack::STRATEGY_STIX_ID],
    )
    .await;
    let notes = notices(&graph, &file).await.expect("notices");
    assert!(
        notes.iter().any(|n| n.contains("platform 'Windows'")),
        "{notes:?}"
    );
}
