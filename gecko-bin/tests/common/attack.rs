//! A current-shape ATT&CK fixture (Enterprise v18+): a technique detected by one
//! detection strategy, whose analytics name the data components they read through
//! log-source references.
//!
//! - **Windows** analytic [`WINDOWS_ANALYTIC`]: two channels on one log source, two
//!   spellings of `WinEventLog:Security`, a `"None"` channel, and `NSM:Flow ` with a
//!   trailing space.
//! - **Linux** analytic [`LINUX_ANALYTIC`]: `nsm:flow` in a different case, and a
//!   reference with no channel.
//! - **macOS** analytic [`MACOS_ANALYTIC`]: no log-source references.
//! - A deprecated Windows analytic ([`DEPRECATED_ANALYTIC_STIX_ID`]), which carries
//!   no attack-id once synced.
//!
//! Included with `#[path = "common/attack.rs"] mod attack;` beside `mod common;`,
//! so only the tests that seed from it compile it.

use cyber_gecko::CyberGecko;
use cyber_gecko::stix::{cyber_post_sync, to_okf};
use gecko_engine::db::CORE_SCHEMA;
use gecko_engine::extension::GeckoExtension;
use gecko_engine::syncer::bundle::sync_bundle;
use mem_gecko::MemGecko;
use serde_json::{Value, json};

use super::common::TestDb;

/// The technique's ATT&CK id.
pub const TECHNIQUE_ATTACK_ID: &str = "T1059";
/// The technique's STIX id.
pub const TECHNIQUE_STIX_ID: &str = "attack-pattern--a0000000-0000-4000-8000-000000000001";
/// The detection strategy's ATT&CK id.
pub const STRATEGY_ATTACK_ID: &str = "DET0001";
/// The detection strategy's STIX id.
pub const STRATEGY_STIX_ID: &str =
    "x-mitre-detection-strategy--d0000000-0000-4000-8000-000000000001";
/// The Windows analytic's ATT&CK id.
pub const WINDOWS_ANALYTIC: &str = "AN0001";
/// The Windows analytic's STIX id.
pub const WINDOWS_ANALYTIC_STIX_ID: &str = "x-mitre-analytic--a1000000-0000-4000-8000-000000000001";
/// The Linux analytic's ATT&CK id.
pub const LINUX_ANALYTIC: &str = "AN0002";
/// The Linux analytic's STIX id.
pub const LINUX_ANALYTIC_STIX_ID: &str = "x-mitre-analytic--a1000000-0000-4000-8000-000000000002";
/// The macOS analytic's ATT&CK id; it has no log-source references.
pub const MACOS_ANALYTIC: &str = "AN0003";
/// The macOS analytic's STIX id.
pub const MACOS_ANALYTIC_STIX_ID: &str = "x-mitre-analytic--a1000000-0000-4000-8000-000000000003";
/// The deprecated Windows analytic's STIX id.
pub const DEPRECATED_ANALYTIC_STIX_ID: &str =
    "x-mitre-analytic--a1000000-0000-4000-8000-000000000009";
/// The `Process Creation` data component's name.
pub const PROCESS_CREATION: &str = "Process Creation";
/// The `Command Execution` data component's name.
pub const COMMAND_EXECUTION: &str = "Command Execution";
/// The `Network Connection Creation` data component's name.
pub const NETWORK_CONNECTION_CREATION: &str = "Network Connection Creation";

const PROCESS_CREATION_STIX_ID: &str =
    "x-mitre-data-component--c0000000-0000-4000-8000-000000000001";
const COMMAND_EXECUTION_STIX_ID: &str =
    "x-mitre-data-component--c0000000-0000-4000-8000-000000000002";
const NETWORK_CONNECTION_CREATION_STIX_ID: &str =
    "x-mitre-data-component--c0000000-0000-4000-8000-000000000003";

/// A log-source reference to `component`.
fn source(component: &str, name: &str, channel: Option<&str>) -> Value {
    let mut r = json!({ "x_mitre_data_component_ref": component, "name": name });
    if let Some(channel) = channel {
        r["channel"] = json!(channel);
    }
    r
}

/// The ATT&CK object `stix_id` with an `external_id` of `attack_id`.
fn object(ty: &str, stix_id: &str, name: &str, attack_id: &str) -> Value {
    json!({
        "type": ty,
        "id": stix_id,
        "name": name,
        "external_references": [{ "source_name": "mitre-attack", "external_id": attack_id }],
        "x_mitre_deprecated": false,
    })
}

fn analytic(stix_id: &str, attack_id: &str, platform: &str, sources: Vec<Value>) -> Value {
    let mut a = object("x-mitre-analytic", stix_id, attack_id, attack_id);
    a["x_mitre_platforms"] = json!([platform]);
    if !sources.is_empty() {
        a["x_mitre_log_source_references"] = Value::Array(sources);
    }
    a
}

/// The fixture as a STIX bundle.
pub fn fixture_json() -> String {
    let mut technique = object(
        "attack-pattern",
        TECHNIQUE_STIX_ID,
        "Command and Scripting Interpreter",
        TECHNIQUE_ATTACK_ID,
    );
    technique["x_mitre_platforms"] = json!(["Windows", "Linux", "macOS"]);

    let mut strategy = object(
        "x-mitre-detection-strategy",
        STRATEGY_STIX_ID,
        "Detect Command and Scripting Interpreter",
        STRATEGY_ATTACK_ID,
    );
    strategy["x_mitre_analytic_refs"] = json!([
        WINDOWS_ANALYTIC_STIX_ID,
        LINUX_ANALYTIC_STIX_ID,
        MACOS_ANALYTIC_STIX_ID,
        DEPRECATED_ANALYTIC_STIX_ID,
    ]);

    let (pc, ce, ncc) = (
        PROCESS_CREATION_STIX_ID,
        COMMAND_EXECUTION_STIX_ID,
        NETWORK_CONNECTION_CREATION_STIX_ID,
    );
    let windows = analytic(
        WINDOWS_ANALYTIC_STIX_ID,
        WINDOWS_ANALYTIC,
        "Windows",
        vec![
            source(pc, "WinEventLog:Sysmon", Some("EventCode=1")),
            source(pc, "WinEventLog:Sysmon", Some("EventCode=10")),
            source(pc, "WinEventLog:Security", Some("EventCode=4688")),
            source(ce, "wineventlog:security", Some("EventCode=4103")),
            source(ncc, "NSM:Flow ", Some("None")),
        ],
    );
    let linux = analytic(
        LINUX_ANALYTIC_STIX_ID,
        LINUX_ANALYTIC,
        "Linux",
        vec![
            source(pc, "auditd:SYSCALL", Some("execve")),
            source(ce, "linux:syslog", None),
            source(ncc, "nsm:flow", Some("connect")),
        ],
    );
    let macos = analytic(MACOS_ANALYTIC_STIX_ID, MACOS_ANALYTIC, "macOS", vec![]);
    let mut deprecated = analytic(
        DEPRECATED_ANALYTIC_STIX_ID,
        "AN9001",
        "Windows",
        vec![source(pc, "WinEventLog:Security", Some("EventCode=4688"))],
    );
    deprecated["x_mitre_deprecated"] = json!(true);

    json!({
        "type": "bundle",
        "id": "bundle--f0000000-0000-4000-8000-000000000001",
        "objects": [
            technique,
            strategy,
            windows,
            linux,
            macos,
            deprecated,
            object("x-mitre-data-component", pc, PROCESS_CREATION, "DC0032"),
            object("x-mitre-data-component", ce, COMMAND_EXECUTION, "DC0064"),
            object("x-mitre-data-component", ncc, NETWORK_CONNECTION_CREATION, "DC0082"),
            {
                "type": "relationship",
                "id": "relationship--e0000000-0000-4000-8000-000000000001",
                "relationship_type": "detects",
                "source_ref": STRATEGY_STIX_ID,
                "target_ref": TECHNIQUE_STIX_ID,
            },
        ],
    })
    .to_string()
}

/// Applies the core, mem and cyber schemas to `db`, then syncs [`fixture_json`]
/// with its post-sync writes.
pub async fn sync_fixture(db: &TestDb) {
    let mut router = db.router();
    router.apply_schema(CORE_SCHEMA).await.expect("core schema");
    for ext in [
        Box::new(MemGecko::new()) as Box<dyn GeckoExtension>,
        Box::new(CyberGecko::new()),
    ] {
        router
            .apply_schema(ext.schema())
            .await
            .unwrap_or_else(|e| panic!("{} schema: {e}", ext.name()));
    }
    let (manifest, rels) = to_okf(&fixture_json()).expect("fixture converts");
    sync_bundle(
        &mut router,
        &manifest,
        &cyber_post_sync(&rels),
        mem_gecko::BELIEF_TIER_ROOT,
    )
    .await
    .expect("sync the ATT&CK fixture");
}
