// End-to-end proof of the cyber-gecko detect host bridge: claim writes a real
// detection belief + technique-detection edge through the belief-edge seam,
// coverage_gaps and blinded run the persisted TypeDB functions, and
// disposition/precision compute over real dispositions. The blinded cases each
// sync a sensor file over the ATT&CK fixture and assert the whole report. Exercises the sync->async
// bridge over a read-only graph handle and an edge writer bound to cyber-gecko.
//
// Gated on `cyber`; needs the multi-thread runtime the bridge's block_in_place
// requires.
#![cfg(feature = "cyber")]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::sync::Arc;

use cyber_gecko::CyberGecko;
use cyber_gecko::detect::cyber_extension_callback;
use cyber_gecko::sensors;
use gecko_engine::db::CORE_SCHEMA;
use gecko_engine::db::graph_store::RouterGraphStore;
use gecko_engine::extension::GeckoExtension;
use gecko_engine::okf::parser::parse_bundle;
use gecko_engine::sandbox::engine::ExtensionCallback;
use gecko_engine::syncer::bundle::sync_bundle;
use gecko_extension_api::{
    ActorId, BeliefDraft, BeliefEdge, BeliefEdgeWriter, ConceptId, DerivationMethod, GraphReader,
    ProvenanceSource, RunContext, Visibility,
};
use gecko_extension_api::{EpistemicWriter, MemId};
use mem_gecko::{BeliefEdgeRegistry, MemGecko, MemWriter};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::Mutex;

#[path = "common/attack.rs"]
mod attack;
mod common;
use common::TestDb;

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

/// Syncs the ATT&CK fixture, then a playbook bundle holding the detection-logic
/// concept the claims reference.
async fn seed_detection_graph(test_db: &TestDb) {
    attack::sync_fixture(test_db).await;
    let mut db = test_db.router();

    let dir = TempDir::new().expect("tempdir");
    fs::create_dir_all(dir.path().join("playbooks")).expect("playbooks dir");
    fs::write(
        dir.path().join("bundle.json"),
        json!({ "name": "detect-playbooks" }).to_string(),
    )
    .expect("write bundle.json");
    fs::write(
        dir.path().join(format!("{LOGIC_ID}.md")),
        "---\ntype: playbook\ntitle: Detect T1059\n---\n\n# Detect T1059\n",
    )
    .expect("write playbook");
    let playbooks = parse_bundle(dir.path()).expect("parse playbook bundle");
    sync_bundle(&mut db, &playbooks, &[], mem_gecko::BELIEF_TIER_ROOT)
        .await
        .expect("sync playbook bundle");
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
        json!({ "logic_id": LOGIC_ID, "technique_id": attack::TECHNIQUE_ATTACK_ID, "method": "external-tool", "confidence": 0.9 }),
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
async fn precision_ignores_a_superseded_disposition() {
    let test_db = TestDb::new("gecko_test_detect_superseded");
    seed_detection_graph(&test_db).await;

    let (graph, edges) = detect_handles(&test_db);
    let ctx = run_ctx(LOGIC_ID);
    let cb = cyber_extension_callback(graph.clone(), edges, ctx.clone(), base());

    let mut fp_belief = String::new();
    for verdict in ["false-positive", "true-positive"] {
        let disposed = call(
            &cb,
            "disposition",
            json!({ "logic_id": LOGIC_ID, "verdict": verdict }),
        )
        .unwrap_or_else(|e| panic!("disposition {verdict}: {e}"));
        if verdict == "false-positive" {
            fp_belief = disposed["belief_id"].as_str().unwrap().to_string();
        }
    }

    // Superseding the false-positive belief retires it; the successor carries
    // no disposition edge.
    let writer = MemWriter::without_index(Arc::new(RouterGraphStore::new(Arc::new(Mutex::new(
        test_db.router(),
    )))));
    writer
        .supersede(
            &ctx,
            MemId::new(fp_belief),
            BeliefDraft {
                text: "the alert was reviewed again".into(),
                owner: ctx.actor.clone(),
                visibility: Visibility::Private,
                confidence: Some(0.5),
                entrenchment: None,
            },
            "corrected",
        )
        .await
        .expect("supersede the false-positive disposition");

    let prec = call(
        &cb,
        "precision",
        json!({ "logic_id": LOGIC_ID, "window": 3600 }),
    )
    .expect("precision");
    assert_eq!(prec["true_positives"], 1);
    assert_eq!(prec["false_positives"], 0);
    assert_eq!(prec["precision"], 1.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn precision_with_no_asserted_dispositions_is_null() {
    let test_db = TestDb::new("gecko_test_detect_no_dispositions");
    seed_detection_graph(&test_db).await;

    let (graph, edges) = detect_handles(&test_db);
    let ctx = run_ctx(LOGIC_ID);
    let cb = cyber_extension_callback(graph, edges, ctx.clone(), base());

    // One false-positive disposition exists, but its belief is superseded, so no
    // asserted disposition remains: 0/0 is reported as a null precision.
    let disposed = call(
        &cb,
        "disposition",
        json!({ "logic_id": LOGIC_ID, "verdict": "false-positive" }),
    )
    .expect("disposition");
    let writer = MemWriter::without_index(Arc::new(RouterGraphStore::new(Arc::new(Mutex::new(
        test_db.router(),
    )))));
    writer
        .supersede(
            &ctx,
            MemId::new(disposed["belief_id"].as_str().unwrap()),
            BeliefDraft {
                text: "the alert was reviewed again".into(),
                owner: ctx.actor.clone(),
                visibility: Visibility::Private,
                confidence: Some(0.5),
                entrenchment: None,
            },
            "corrected",
        )
        .await
        .expect("supersede the false-positive disposition");

    let prec = call(
        &cb,
        "precision",
        json!({ "logic_id": LOGIC_ID, "window": 3600 }),
    )
    .expect("precision");
    assert_eq!(prec["true_positives"], 0);
    assert_eq!(prec["false_positives"], 0);
    assert!(prec["precision"].is_null(), "{prec}");
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

// ── blinded ─────────────────────────────────────────────────────────────────

/// The technique's concept id, which `blinded` reports even once its attack-id is gone.
const TECHNIQUE_CONCEPT_ID: &str = "stix/attack-pattern/a0000000-0000-4000-8000-000000000001";

/// The Windows analytic's requirements, grouped by log source, with no sensor.
fn windows_missing() -> Value {
    json!([
        { "log_source": "NSM:Flow", "data_components": [attack::NETWORK_CONNECTION_CREATION] },
        { "log_source": "WinEventLog:Security",
          "data_components": [attack::COMMAND_EXECUTION, attack::PROCESS_CREATION] },
        { "log_source": "WinEventLog:Sysmon", "data_components": [attack::PROCESS_CREATION] },
    ])
}

/// The Linux analytic's requirements, grouped by log source, with no sensor.
fn linux_missing() -> Value {
    json!([
        { "log_source": "auditd:SYSCALL", "data_components": [attack::PROCESS_CREATION] },
        { "log_source": "linux:syslog", "data_components": [attack::COMMAND_EXECUTION] },
        { "log_source": "nsm:flow", "data_components": [attack::NETWORK_CONNECTION_CREATION] },
    ])
}

fn blind(platform: &str, analytic: &str, missing: Value) -> Value {
    json!({ "platform": platform, "analytic": analytic, "no_log_sources": false, "missing": missing })
}

fn macos_blind() -> Value {
    json!({ "platform": "macOS", "analytic": attack::MACOS_ANALYTIC, "no_log_sources": true, "missing": [] })
}

/// One `blinded_detections` entry for the claim of `logic` on T1059.
fn entry(logic: &str, blind_platforms: Value, unassessed: &[&str]) -> Value {
    json!({
        "concept_id": logic,
        "technique": attack::TECHNIQUE_ATTACK_ID,
        "technique_concept_id": TECHNIQUE_CONCEPT_ID,
        "blind_platforms": blind_platforms,
        "unassessed_platforms": unassessed,
    })
}

/// The whole `blinded` report for `entries` and no stale components.
fn report(entries: Vec<Value>) -> Value {
    json!({ "blinded_detections": entries, "stale_sensor_components": [] })
}

/// Syncs a playbook bundle holding one detection-logic concept per id in `logic_ids`.
async fn sync_playbooks(test_db: &TestDb, logic_ids: &[&str]) {
    let dir = TempDir::new().expect("tempdir");
    fs::create_dir_all(dir.path().join("playbooks")).expect("playbooks dir");
    fs::write(
        dir.path().join("bundle.json"),
        json!({ "name": "detect-playbooks" }).to_string(),
    )
    .expect("write bundle.json");
    for id in logic_ids {
        fs::write(
            dir.path().join(format!("{id}.md")),
            format!("---\ntype: playbook\ntitle: {id}\n---\n\n# {id}\n"),
        )
        .expect("write playbook");
    }
    let playbooks = parse_bundle(dir.path()).expect("parse playbook bundle");
    let mut db = test_db.router();
    sync_bundle(&mut db, &playbooks, &[], mem_gecko::BELIEF_TIER_ROOT)
        .await
        .expect("sync playbook bundle");
}

/// The ATT&CK fixture as JSON, with `edit` applied to its object list.
fn attack_bundle(edit: impl FnOnce(&mut Vec<Value>)) -> String {
    let mut bundle: Value = serde_json::from_str(&attack::fixture_json()).expect("fixture json");
    edit(bundle["objects"].as_array_mut().expect("objects"));
    bundle.to_string()
}

/// The fixture object `stix_id`.
fn object_mut<'a>(objects: &'a mut [Value], stix_id: &str) -> &'a mut Value {
    objects
        .iter_mut()
        .find(|o| o["id"] == stix_id)
        .unwrap_or_else(|| panic!("no fixture object {stix_id}"))
}

/// Re-syncs the ATT&CK bundle `json` with its post-sync writes.
async fn resync_attack(test_db: &TestDb, json: &str) {
    let (manifest, rels) = cyber_gecko::stix::to_okf(json).expect("bundle converts");
    let mut db = test_db.router();
    sync_bundle(
        &mut db,
        &manifest,
        &cyber_gecko::stix::cyber_post_sync(&rels),
        mem_gecko::BELIEF_TIER_ROOT,
    )
    .await
    .expect("re-sync the ATT&CK bundle");
}

/// Syncs the sensor file `yaml` under the bundle `bundle_name`.
async fn sync_sensor_file(test_db: &TestDb, yaml: &str, bundle_name: &str) {
    let file = sensors::parse(yaml).expect("the sensor file parses");
    let mut db = test_db.router();
    sync_bundle(
        &mut db,
        &sensors::to_okf(&file, bundle_name),
        &[],
        mem_gecko::BELIEF_TIER_ROOT,
    )
    .await
    .expect("sync the sensor file");
}

/// The `blinded` test rig: the ATT&CK fixture (technique, strategy, analytics for
/// Windows, Linux and the zero-requirement macOS, components), the detection
/// logic `logic_ids`, each claiming T1059, and the bridge.
struct Rig {
    test_db: TestDb,
    cb: ExtensionCallback,
}

impl Rig {
    async fn new(name: &str, logic_ids: &[&str]) -> Self {
        let test_db = TestDb::new(name);
        attack::sync_fixture(&test_db).await;
        sync_playbooks(&test_db, logic_ids).await;
        let (graph, edges) = detect_handles(&test_db);
        let cb = cyber_extension_callback(graph, edges, run_ctx(logic_ids[0]), base());
        for logic in logic_ids {
            call(
                &cb,
                "claim",
                json!({ "logic_id": logic, "technique_id": attack::TECHNIQUE_ATTACK_ID,
                        "method": "external-tool", "confidence": 0.9 }),
            )
            .unwrap_or_else(|e| panic!("claim {logic}: {e}"));
        }
        Rig { test_db, cb }
    }

    /// A rig with the single claim of [`LOGIC_ID`] and the sensor file `yaml`.
    async fn with_sensors(name: &str, yaml: &str) -> Self {
        let rig = Rig::new(name, &[LOGIC_ID]).await;
        rig.sensors(yaml).await;
        rig
    }

    async fn sensors(&self, yaml: &str) {
        sync_sensor_file(&self.test_db, yaml, sensors::DEFAULT_BUNDLE_NAME).await;
    }

    fn blinded(&self) -> Value {
        call(&self.cb, "blinded", json!({})).expect("blinded")
    }
}

const WINDOWS_FULL: &str = r#"
platforms: [Windows]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [WinEventLog:Sysmon, WinEventLog:Security, NSM:Flow]
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blind_with_no_sensor_lists_every_requirement_by_log_source() {
    let rig = Rig::with_sensors(
        "gecko_test_blinded_no_sensor",
        "platforms: [Windows]\nsensors: []\n",
    )
    .await;
    assert_eq!(
        rig.blinded(),
        report(vec![entry(
            LOGIC_ID,
            json!([blind(
                "Windows",
                attack::WINDOWS_ANALYTIC,
                windows_missing()
            )]),
            &[],
        )])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn covered_by_log_source_including_a_whitespace_or_case_variant() {
    // ATT&CK spells `NSM:Flow ` with a trailing space and `wineventlog:security`
    // in lower case; clean, differently-cased sensor entries cover both.
    let rig = Rig::with_sensors(
        "gecko_test_blinded_by_log_source",
        r#"
platforms: [Windows]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [WinEventLog:Sysmon, WINEVENTLOG:Security, nsm:FLOW]
"#,
    )
    .await;
    assert_eq!(rig.blinded(), report(vec![]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn covered_by_component_fallback() {
    let rig = Rig::with_sensors(
        "gecko_test_blinded_by_component",
        r#"
platforms: [Linux]
sensors:
  - name: linux-audit
    platforms: [Linux]
    log_sources: [auditd:SYSCALL]
    components: [command execution, Network Connection Creation]
"#,
    )
    .await;
    assert_eq!(rig.blinded(), report(vec![]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_coverage_is_blind_listing_only_gaps() {
    let rig = Rig::with_sensors(
        "gecko_test_blinded_partial",
        r#"
platforms: [Windows]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [WinEventLog:Sysmon, NSM:Flow]
"#,
    )
    .await;
    assert_eq!(
        rig.blinded(),
        report(vec![entry(
            LOGIC_ID,
            json!([blind(
                "Windows",
                attack::WINDOWS_ANALYTIC,
                json!([{ "log_source": "WinEventLog:Security",
                         "data_components": [attack::COMMAND_EXECUTION, attack::PROCESS_CREATION] }]),
            )]),
            &[],
        )])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undeclared_platform_is_never_reported() {
    // Linux and macOS have analytics that cannot run, but only Windows is declared,
    // and a sensor on an undeclared platform does not bring it in.
    let rig = Rig::with_sensors(
        "gecko_test_blinded_undeclared",
        r#"
platforms: [Windows]
sensors:
  - name: linux-audit
    platforms: [Linux]
    log_sources: [auditd:SYSCALL]
"#,
    )
    .await;
    let out = rig.blinded();
    let platforms: Vec<&str> = out["blinded_detections"][0]["blind_platforms"]
        .as_array()
        .expect("blind platforms")
        .iter()
        .map(|p| p["platform"].as_str().expect("platform"))
        .collect();
    assert_eq!(platforms, ["Windows"]);
    assert_eq!(
        out["blinded_detections"][0]["unassessed_platforms"],
        json!([])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declared_platform_without_analytic_is_unassessed() {
    let rig = Rig::with_sensors(
        "gecko_test_blinded_unassessed",
        r#"
platforms: [Windows, Containers]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [WinEventLog:Sysmon, WinEventLog:Security, NSM:Flow]
"#,
    )
    .await;
    assert_eq!(
        rig.blinded(),
        report(vec![entry(LOGIC_ID, json!([]), &["Containers"])])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_requirement_analytic_reports_no_log_sources() {
    let rig = Rig::with_sensors(
        "gecko_test_blinded_zero_requirement",
        r#"
platforms: [macOS]
sensors:
  - name: mac-edr
    platforms: [macOS]
    log_sources: [macos:unifiedlog]
    components: [Process Creation]
"#,
    )
    .await;
    assert_eq!(
        rig.blinded(),
        report(vec![entry(LOGIC_ID, json!([macos_blind()]), &[])])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sensor_on_wrong_platform_does_not_cover() {
    // The Linux sensor collects every Windows log source too, but only Linux's.
    let rig = Rig::with_sensors(
        "gecko_test_blinded_wrong_platform",
        r#"
platforms: [Windows, Linux]
sensors:
  - name: linux-audit
    platforms: [Linux]
    log_sources: [auditd:SYSCALL, linux:syslog, nsm:flow, WinEventLog:Sysmon, WinEventLog:Security]
"#,
    )
    .await;
    assert_eq!(
        rig.blinded(),
        report(vec![entry(
            LOGIC_ID,
            json!([blind(
                "Windows",
                attack::WINDOWS_ANALYTIC,
                windows_missing()
            )]),
            &[],
        )])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_platform_analytic_is_covered_only_by_a_sensor_on_the_assessed_platform() {
    // The Windows analytic also runs on Linux; a Windows sensor collecting every
    // log source it reads covers it on Windows, not on Linux.
    let rig = Rig::new("gecko_test_blinded_multi_platform", &[LOGIC_ID]).await;
    let json = attack_bundle(|objects| {
        object_mut(objects, attack::WINDOWS_ANALYTIC_STIX_ID)["x_mitre_platforms"] =
            json!(["Windows", "Linux"]);
    });
    resync_attack(&rig.test_db, &json).await;
    rig.sensors(
        r#"
platforms: [Linux]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [WinEventLog:Sysmon, WinEventLog:Security, NSM:Flow]
"#,
    )
    .await;
    assert_eq!(
        rig.blinded(),
        report(vec![entry(
            LOGIC_ID,
            json!([
                blind("Linux", attack::WINDOWS_ANALYTIC, windows_missing()),
                blind("Linux", attack::LINUX_ANALYTIC, linux_missing()),
            ]),
            &[],
        )])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removed_sensor_stops_covering() {
    let rig = Rig::with_sensors("gecko_test_blinded_removed_sensor", WINDOWS_FULL).await;
    assert_eq!(rig.blinded(), report(vec![]));

    rig.sensors("platforms: [Windows]\nsensors: []\n").await;
    assert_eq!(
        rig.blinded(),
        report(vec![entry(
            LOGIC_ID,
            json!([blind(
                "Windows",
                attack::WINDOWS_ANALYTIC,
                windows_missing()
            )]),
            &[],
        )])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removed_component_stops_covering() {
    let rig = Rig::with_sensors(
        "gecko_test_blinded_removed_component",
        r#"
platforms: [Linux]
sensors:
  - name: linux-audit
    platforms: [Linux]
    log_sources: [auditd:SYSCALL, nsm:flow]
    components: [Command Execution]
"#,
    )
    .await;
    assert_eq!(rig.blinded(), report(vec![]));

    rig.sensors(
        r#"
platforms: [Linux]
sensors:
  - name: linux-audit
    platforms: [Linux]
    log_sources: [auditd:SYSCALL, nsm:flow]
"#,
    )
    .await;
    assert_eq!(
        rig.blinded(),
        report(vec![entry(
            LOGIC_ID,
            json!([blind(
                "Linux",
                attack::LINUX_ANALYTIC,
                json!([{ "log_source": "linux:syslog", "data_components": [attack::COMMAND_EXECUTION] }]),
            )]),
            &[],
        )])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deprecated_analytic_is_ignored() {
    // The deprecated Windows analytic reads only WinEventLog:Security, which this
    // sensor covers; were it counted, Windows would be covered.
    let rig = Rig::with_sensors(
        "gecko_test_blinded_deprecated_analytic",
        r#"
platforms: [Windows]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [WinEventLog:Security]
"#,
    )
    .await;
    assert_eq!(
        rig.blinded(),
        report(vec![entry(
            LOGIC_ID,
            json!([blind(
                "Windows",
                attack::WINDOWS_ANALYTIC,
                json!([
                    { "log_source": "NSM:Flow", "data_components": [attack::NETWORK_CONNECTION_CREATION] },
                    { "log_source": "WinEventLog:Sysmon", "data_components": [attack::PROCESS_CREATION] },
                ]),
            )]),
            &[],
        )])
    );
}

/// The report for a claim of [`LOGIC_ID`] whose technique is dead, with
/// `technique` as the output carries it.
fn dead_technique_report(technique: Value) -> Value {
    json!({
        "blinded_detections": [{
            "concept_id": LOGIC_ID,
            "technique": technique,
            "technique_concept_id": TECHNIQUE_CONCEPT_ID,
            "blind_platforms": [],
            "unassessed_platforms": ["Linux", "Windows"],
        }],
        "stale_sensor_components": [],
    })
}

const WINDOWS_LINUX_FULL: &str = r#"
platforms: [Windows, Linux]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [WinEventLog:Sysmon, WinEventLog:Security, NSM:Flow]
  - name: linux-audit
    platforms: [Linux]
    log_sources: [auditd:SYSCALL, linux:syslog, nsm:flow]
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claim_on_retired_technique_is_unassessed() {
    let rig = Rig::with_sensors("gecko_test_blinded_retired_technique", WINDOWS_LINUX_FULL).await;
    assert_eq!(rig.blinded(), report(vec![]));

    // The technique leaves ATT&CK, which keeps another technique's strategy; the
    // claim's belief edge retires it, and its attack-id goes with its other
    // attributes.
    let json = attack_bundle(|objects| {
        objects.retain(|o| o["id"] != attack::TECHNIQUE_STIX_ID && o["type"] != "relationship");
        let other = "attack-pattern--a0000000-0000-4000-8000-000000000002";
        let strategy = "x-mitre-detection-strategy--d0000000-0000-4000-8000-000000000002";
        objects.extend([
            json!({ "type": "attack-pattern", "id": other, "name": "Valid Accounts",
                    "external_references": [{ "source_name": "mitre-attack", "external_id": "T1078" }] }),
            json!({ "type": "x-mitre-detection-strategy", "id": strategy, "name": "Detect Valid Accounts",
                    "external_references": [{ "source_name": "mitre-attack", "external_id": "DET0002" }] }),
            json!({ "type": "relationship", "id": "relationship--e0000000-0000-4000-8000-000000000003",
                    "relationship_type": "detects", "source_ref": strategy, "target_ref": other }),
        ]);
    });
    resync_attack(&rig.test_db, &json).await;
    assert_eq!(rig.blinded(), dead_technique_report(Value::Null));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claim_on_deprecated_technique_is_unassessed() {
    let rig = Rig::with_sensors(
        "gecko_test_blinded_deprecated_technique",
        WINDOWS_LINUX_FULL,
    )
    .await;
    let json = attack_bundle(|objects| {
        object_mut(objects, attack::TECHNIQUE_STIX_ID)["x_mitre_deprecated"] = json!(true);
    });
    resync_attack(&rig.test_db, &json).await;
    // A deprecated object does not claim its unique attack-id.
    assert_eq!(rig.blinded(), dead_technique_report(Value::Null));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claim_on_technique_with_only_deprecated_strategy_is_unassessed() {
    let rig = Rig::with_sensors("gecko_test_blinded_deprecated_strategy", WINDOWS_LINUX_FULL).await;
    let json = attack_bundle(|objects| {
        object_mut(objects, attack::STRATEGY_STIX_ID)["x_mitre_deprecated"] = json!(true);
    });
    resync_attack(&rig.test_db, &json).await;
    assert_eq!(
        rig.blinded(),
        dead_technique_report(json!(attack::TECHNIQUE_ATTACK_ID))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_order_is_deterministic() {
    let (a, b) = ("playbooks/detect-a", "playbooks/detect-b");
    // Claimed b first, so the report's order is its own, not the claims'.
    let rig = Rig::new("gecko_test_blinded_order", &[b, a]).await;
    rig.sensors("platforms: [Windows, Linux]\nsensors: []\n")
        .await;
    let platforms = json!([
        blind("Linux", attack::LINUX_ANALYTIC, linux_missing()),
        blind("Windows", attack::WINDOWS_ANALYTIC, windows_missing()),
    ]);
    let expected = report(vec![
        entry(a, platforms.clone(), &[]),
        entry(b, platforms, &[]),
    ]);
    assert_eq!(rig.blinded(), expected);
    assert_eq!(rig.blinded(), expected, "a second call reports the same");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_groups_log_source_spellings_by_key() {
    // ATT&CK names `WinEventLog:Security` for Process Creation and
    // `wineventlog:security` for Command Execution: one entry, the smaller spelling.
    let rig = Rig::with_sensors(
        "gecko_test_blinded_grouping",
        r#"
platforms: [Windows]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [WinEventLog:Sysmon, NSM:Flow]
"#,
    )
    .await;
    let out = rig.blinded();
    assert_eq!(
        out["blinded_detections"][0]["blind_platforms"][0]["missing"],
        json!([{ "log_source": "WinEventLog:Security",
                 "data_components": [attack::COMMAND_EXECUTION, attack::PROCESS_CREATION] }])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_fallback_component_is_reported() {
    let rig = Rig::with_sensors(
        "gecko_test_blinded_stale",
        r#"
platforms: [Linux]
sensors:
  - name: linux-audit
    platforms: [Linux]
    log_sources: [auditd:SYSCALL, nsm:flow]
    components: [Command Execution]
"#,
    )
    .await;
    assert_eq!(rig.blinded(), report(vec![]));

    // Command Execution leaves ATT&CK, along with the requirements naming it.
    let json = attack_bundle(|objects| {
        objects.retain(|o| o["name"] != attack::COMMAND_EXECUTION);
    });
    resync_attack(&rig.test_db, &json).await;
    assert_eq!(
        rig.blinded(),
        json!({
            "blinded_detections": [],
            "stale_sensor_components": [{ "sensor": "linux-audit", "component": attack::COMMAND_EXECUTION }],
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_sensor_files_union_platforms_and_coverage() {
    let rig = Rig::new("gecko_test_blinded_two_files", &[LOGIC_ID]).await;
    sync_sensor_file(
        &rig.test_db,
        r#"
platforms: [Windows]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [WinEventLog:Sysmon, WinEventLog:Security]
"#,
        "sensors-a",
    )
    .await;
    sync_sensor_file(
        &rig.test_db,
        r#"
platforms: [Linux]
sensors:
  - name: network
    platforms: [Windows, Linux]
    log_sources: [NSM:Flow, auditd:SYSCALL]
"#,
        "sensors-b",
    )
    .await;
    // Windows is covered by the two files together; Linux, declared by the
    // second, still lacks linux:syslog.
    assert_eq!(
        rig.blinded(),
        report(vec![entry(
            LOGIC_ID,
            json!([blind(
                "Linux",
                attack::LINUX_ANALYTIC,
                json!([{ "log_source": "linux:syslog", "data_components": [attack::COMMAND_EXECUTION] }]),
            )]),
            &[],
        )])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_sensor_list_is_blind_everywhere_assessed() {
    let rig = Rig::with_sensors(
        "gecko_test_blinded_empty_list",
        "platforms: [Windows, Linux, macOS, Containers]\nsensors: []\n",
    )
    .await;
    assert_eq!(
        rig.blinded(),
        report(vec![entry(
            LOGIC_ID,
            json!([
                blind("Linux", attack::LINUX_ANALYTIC, linux_missing()),
                blind("Windows", attack::WINDOWS_ANALYTIC, windows_missing()),
                macos_blind(),
            ]),
            &["Containers"],
        )])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blinded_errors_without_a_sensor_file() {
    let rig = Rig::new("gecko_test_blinded_no_file", &[LOGIC_ID]).await;
    let err = call(&rig.cb, "blinded", json!({})).expect_err("no sensor file is an error");
    assert!(err.contains("no sensor file synced"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blinded_errors_on_a_pre_current_graph() {
    let (_test_db, cb) = pre_current_bridge("gecko_test_blinded_pre_current").await;
    let err = call(&cb, "blinded", json!({})).expect_err("a pre-current graph is an error");
    assert!(err.contains("no current ATT&CK detection data"), "{err}");
    assert!(err.contains("re-sync the ATT&CK bundle"), "{err}");
}

/// A graph holding a pre-v18 bundle, where a data component detects the technique
/// directly, and a sensor file; and the bridge over it.
async fn pre_current_bridge(name: &str) -> (TestDb, ExtensionCallback) {
    let test_db = TestDb::new(name);
    apply_schemas(&test_db).await;
    let json = json!({
        "type": "bundle",
        "id": "bundle--f0000000-0000-4000-8000-000000000002",
        "objects": [
            { "type": "attack-pattern", "id": attack::TECHNIQUE_STIX_ID, "name": "T",
              "external_references": [{ "source_name": "mitre-attack", "external_id": "T1059" }] },
            { "type": "x-mitre-data-component",
              "id": "x-mitre-data-component--c0000000-0000-4000-8000-000000000001",
              "name": attack::PROCESS_CREATION },
            { "type": "relationship", "id": "relationship--e0000000-0000-4000-8000-000000000002",
              "relationship_type": "detects",
              "source_ref": "x-mitre-data-component--c0000000-0000-4000-8000-000000000001",
              "target_ref": attack::TECHNIQUE_STIX_ID },
        ],
    })
    .to_string();
    resync_attack(&test_db, &json).await;
    sync_sensor_file(&test_db, WINDOWS_FULL, sensors::DEFAULT_BUNDLE_NAME).await;

    let (graph, edges) = detect_handles(&test_db);
    let cb = cyber_extension_callback(graph, edges, run_ctx(LOGIC_ID), base());
    (test_db, cb)
}

// ── recommend_sources ───────────────────────────────────────────────────────

impl Rig {
    fn recommend(&self) -> Value {
        call(&self.cb, "recommend_sources", json!({})).expect("recommend_sources")
    }
}

fn source(log_source: &str, key: &str, channels: &[&str]) -> Value {
    json!({ "log_source": log_source, "key": key, "channels": channels })
}

fn claim_ref(logic: &str) -> Value {
    json!({ "concept_id": logic, "technique": attack::TECHNIQUE_ATTACK_ID,
            "technique_concept_id": TECHNIQUE_CONCEPT_ID })
}

fn unfixable(logic: &str, platform: &str, reason: &str) -> Value {
    json!({ "concept_id": logic, "technique": attack::TECHNIQUE_ATTACK_ID,
            "technique_concept_id": TECHNIQUE_CONCEPT_ID, "platform": platform,
            "reason": reason })
}

fn linux_sources() -> Value {
    json!([
        source("auditd:SYSCALL", "auditd:syscall", &["execve"]),
        source("linux:syslog", "linux:syslog", &[]),
        source("nsm:flow", "nsm:flow", &["connect"]),
    ])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_sensor_recommends_each_platform_in_order() {
    let (a, b) = ("playbooks/detect-a", "playbooks/detect-b");
    let rig = Rig::new("gecko_test_recommend_no_sensor", &[b, a]).await;
    rig.sensors("platforms: [Windows, Linux, macOS, Containers]\nsensors: []\n")
        .await;
    let both = json!([claim_ref(a), claim_ref(b)]);
    assert_eq!(
        rig.recommend(),
        json!({
            "steps": [
                { "platform": "Linux", "add": linux_sources(), "unblinds": both,
                  "cumulative_unblinded": 2 },
                { "platform": "Windows",
                  "add": [
                      source("NSM:Flow", "nsm:flow", &[]),
                      source("WinEventLog:Security", "wineventlog:security",
                             &["EventCode=4103", "EventCode=4688"]),
                      source("WinEventLog:Sysmon", "wineventlog:sysmon",
                             &["EventCode=1", "EventCode=10"]),
                  ],
                  "unblinds": both, "cumulative_unblinded": 4 },
            ],
            "unfixable": [
                unfixable(a, "Containers", "no-analytic"),
                unfixable(a, "macOS", "no-requirements"),
                unfixable(b, "Containers", "no-analytic"),
                unfixable(b, "macOS", "no-requirements"),
            ],
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_sensor_recommends_only_the_remainder() {
    let rig = Rig::with_sensors(
        "gecko_test_recommend_partial",
        r#"
platforms: [Windows, Linux]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [WinEventLog:Sysmon, WinEventLog:Security]
"#,
    )
    .await;
    let one = json!([claim_ref(LOGIC_ID)]);
    assert_eq!(
        rig.recommend(),
        json!({
            "steps": [
                { "platform": "Windows", "add": [source("NSM:Flow", "nsm:flow", &[])],
                  "unblinds": one, "cumulative_unblinded": 1 },
                { "platform": "Linux", "add": linux_sources(), "unblinds": one,
                  "cumulative_unblinded": 2 },
            ],
            "unfixable": [],
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fully_covered_recommends_nothing() {
    let rig = Rig::with_sensors("gecko_test_recommend_covered", WINDOWS_FULL).await;
    assert_eq!(rig.recommend(), json!({ "steps": [], "unfixable": [] }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recommend_sources_errors_without_a_sensor_file() {
    let rig = Rig::new("gecko_test_recommend_no_file", &[LOGIC_ID]).await;
    let err = call(&rig.cb, "recommend_sources", json!({})).expect_err("no sensor file");
    let blinded = call(&rig.cb, "blinded", json!({})).expect_err("no sensor file");
    assert_eq!(err, blinded);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recommend_sources_errors_without_detection_strategies() {
    let (_test_db, cb) = pre_current_bridge("gecko_test_recommend_pre_current").await;
    let err = call(&cb, "recommend_sources", json!({})).expect_err("a pre-current graph");
    let blinded = call(&cb, "blinded", json!({})).expect_err("a pre-current graph");
    assert_eq!(err, blinded);
}

// ── coverage property ───────────────────────────────────────────────────────

/// One generated sensor: its platforms, log sources as written, and fallback
/// components.
#[derive(Debug, Clone)]
struct GenSensor {
    platforms: Vec<&'static str>,
    log_sources: Vec<String>,
    components: Vec<&'static str>,
}

const GEN_PLATFORMS: [&str; 4] = ["Windows", "Linux", "macOS", "Containers"];
const GEN_LOG_SOURCES: [&str; 6] = [
    "WinEventLog:Sysmon",
    "WinEventLog:Security",
    "NSM:Flow",
    "auditd:SYSCALL",
    "linux:syslog",
    "macos:unifiedlog",
];
const GEN_COMPONENTS: [&str; 3] = [
    attack::PROCESS_CREATION,
    attack::COMMAND_EXECUTION,
    attack::NETWORK_CONNECTION_CREATION,
];

/// The fixture's live requirements on each platform with an analytic that names
/// log sources: (data component, log-source key).
fn fixture_requirements(platform: &str) -> Vec<(&'static str, &'static str)> {
    match platform {
        "Windows" => vec![
            (attack::PROCESS_CREATION, "wineventlog:sysmon"),
            (attack::PROCESS_CREATION, "wineventlog:security"),
            (attack::COMMAND_EXECUTION, "wineventlog:security"),
            (attack::NETWORK_CONNECTION_CREATION, "nsm:flow"),
        ],
        "Linux" => vec![
            (attack::PROCESS_CREATION, "auditd:syscall"),
            (attack::COMMAND_EXECUTION, "linux:syslog"),
            (attack::NETWORK_CONNECTION_CREATION, "nsm:flow"),
        ],
        _ => vec![],
    }
}

fn gen_sensor() -> impl proptest::strategy::Strategy<Value = GenSensor> {
    use proptest::prelude::*;
    let spelling =
        (0..GEN_LOG_SOURCES.len(), 0..3usize, any::<bool>()).prop_map(|(i, case, padded)| {
            let name = match case {
                0 => GEN_LOG_SOURCES[i].to_string(),
                1 => GEN_LOG_SOURCES[i].to_lowercase(),
                _ => GEN_LOG_SOURCES[i].to_uppercase(),
            };
            if padded { format!(" {name} ") } else { name }
        });
    (
        proptest::sample::subsequence(GEN_PLATFORMS.to_vec(), 1..=4),
        proptest::collection::vec(spelling, 0..=3),
        proptest::sample::subsequence(GEN_COMPONENTS.to_vec(), 0..=2),
    )
        .prop_filter("a sensor collects something", |(_, l, c)| {
            !l.is_empty() || !c.is_empty()
        })
        .prop_map(|(platforms, log_sources, components)| GenSensor {
            platforms,
            log_sources,
            components,
        })
}

/// The sensor file for `declared` and `sensors`, with every name quoted so its
/// spaces survive YAML.
fn gen_yaml(declared: &[&str], sensors: &[GenSensor]) -> String {
    let quoted = |items: &[String]| {
        items
            .iter()
            .map(|s| format!("{s:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let owned = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut yaml = format!("platforms: [{}]\nsensors:\n", quoted(&owned(declared)));
    if sensors.is_empty() {
        yaml = format!("platforms: [{}]\nsensors: []\n", quoted(&owned(declared)));
    }
    for (i, s) in sensors.iter().enumerate() {
        yaml.push_str(&format!(
            "  - name: s{i}\n    platforms: [{}]\n",
            quoted(&owned(&s.platforms))
        ));
        if !s.log_sources.is_empty() {
            yaml.push_str(&format!("    log_sources: [{}]\n", quoted(&s.log_sources)));
        }
        if !s.components.is_empty() {
            yaml.push_str(&format!(
                "    components: [{}]\n",
                quoted(&owned(&s.components))
            ));
        }
    }
    yaml
}

/// What the coverage rule says of one declared platform: unassessed, blind with
/// each missing key's components (empty for the zero-requirement macOS analytic),
/// or covered.
#[derive(Debug, PartialEq)]
enum Expected {
    Unassessed,
    Blind(BTreeMap<String, BTreeSet<String>>),
    Covered,
}

/// The coverage rule, independently of TypeDB: a requirement is covered by a sensor
/// on the platform that collects its log-source key (trimmed, case-insensitive) or
/// covers its data component; an analytic runs when it has a requirement and all are
/// covered.
fn expected(platform: &str, sensors: &[GenSensor]) -> Expected {
    if platform == "Containers" {
        return Expected::Unassessed;
    }
    let mut missing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (component, key) in fixture_requirements(platform) {
        let covered = sensors.iter().any(|s| {
            s.platforms.contains(&platform)
                && (s.log_sources.iter().any(|l| l.trim().to_lowercase() == key)
                    || s.components.contains(&component))
        });
        if !covered {
            missing
                .entry(key.to_string())
                .or_default()
                .insert(component.to_string());
        }
    }
    if platform == "macOS" || !missing.is_empty() {
        Expected::Blind(missing)
    } else {
        Expected::Covered
    }
}

/// What `blinded` reported for each declared platform of the one claim.
fn observed(report: &Value, declared: &[&str]) -> BTreeMap<String, Expected> {
    let mut out: BTreeMap<String, Expected> = declared
        .iter()
        .map(|p| (p.to_string(), Expected::Covered))
        .collect();
    for d in report["blinded_detections"].as_array().expect("detections") {
        for p in d["unassessed_platforms"].as_array().expect("unassessed") {
            out.insert(p.as_str().expect("platform").into(), Expected::Unassessed);
        }
        for b in d["blind_platforms"].as_array().expect("blind_platforms") {
            let mut missing = BTreeMap::new();
            for m in b["missing"].as_array().expect("missing") {
                let key = m["log_source"].as_str().expect("log_source").to_lowercase();
                let components = m["data_components"]
                    .as_array()
                    .expect("data_components")
                    .iter()
                    .map(|c| c.as_str().expect("component").to_string())
                    .collect();
                missing.insert(key, components);
            }
            out.insert(
                b["platform"].as_str().expect("platform").into(),
                Expected::Blind(missing),
            );
        }
    }
    out
}

/// Random sensor files over the fixture: `blinded` matches the coverage rule on
/// every declared platform, and `recommend_sources` adds exactly the missing keys
/// of each blind platform with requirements and reports the rest as unfixable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coverage_matches_the_rule_for_random_sensor_files() {
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;

    let cases = (
        proptest::sample::subsequence(GEN_PLATFORMS.to_vec(), 1..=4),
        proptest::collection::vec(gen_sensor(), 0..=3),
    );
    let mut runner = TestRunner::deterministic();
    let rig = Rig::new("gecko_test_coverage_property", &[LOGIC_ID]).await;
    let mut seen = BTreeSet::new();
    for case in 0..48 {
        let (declared, sensors) = cases.new_tree(&mut runner).expect("a case").current();
        let yaml = gen_yaml(&declared, &sensors);
        rig.sensors(&yaml).await;

        let want: BTreeMap<String, Expected> = declared
            .iter()
            .map(|p| (p.to_string(), expected(p, &sensors)))
            .collect();
        for e in want.values() {
            seen.insert(match e {
                Expected::Covered => "covered",
                Expected::Unassessed => "unassessed",
                Expected::Blind(m) if m.is_empty() => "zero-requirement blind",
                Expected::Blind(_) => "blind",
            });
        }
        assert_eq!(
            observed(&rig.blinded(), &declared),
            want,
            "case {case}, sensor file:\n{yaml}"
        );

        let recommend = rig.recommend();
        let mut added: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for step in recommend["steps"].as_array().expect("steps") {
            let platform = step["platform"].as_str().expect("platform").to_string();
            for a in step["add"].as_array().expect("add") {
                added
                    .entry(platform.clone())
                    .or_default()
                    .insert(a["key"].as_str().expect("key").to_string());
            }
        }
        let mut unfixable = BTreeMap::new();
        for u in recommend["unfixable"].as_array().expect("unfixable") {
            unfixable.insert(
                u["platform"].as_str().expect("platform").to_string(),
                u["reason"].as_str().expect("reason").to_string(),
            );
        }
        let mut want_added = BTreeMap::new();
        let mut want_unfixable = BTreeMap::new();
        for (platform, e) in &want {
            match e {
                Expected::Unassessed => {
                    want_unfixable.insert(platform.clone(), "no-analytic".to_string());
                }
                Expected::Blind(missing) if missing.is_empty() => {
                    want_unfixable.insert(platform.clone(), "no-requirements".to_string());
                }
                Expected::Blind(missing) => {
                    want_added.insert(
                        platform.clone(),
                        missing.keys().cloned().collect::<BTreeSet<_>>(),
                    );
                }
                Expected::Covered => {}
            }
        }
        assert_eq!(added, want_added, "case {case}, sensor file:\n{yaml}");
        assert_eq!(
            unfixable, want_unfixable,
            "case {case}, sensor file:\n{yaml}"
        );
    }
    assert_eq!(
        seen.len(),
        4,
        "the cases never produced every outcome: only {seen:?}"
    );
}
