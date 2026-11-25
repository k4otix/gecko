//! An ATT&CK fixture of technique structure: tactics, sub-techniques and
//! revocations, beside the one detection strategy that `claim` and `blinded`
//! read.
//!
//! - Two enterprise tactics, `execution` ([`EXECUTION_ATTACK_ID`]) and
//!   `persistence` ([`PERSISTENCE_ATTACK_ID`]).
//! - A live parent technique [`PARENT_ATTACK_ID`] in both tactics, with live
//!   sub-techniques [`SUB1_ATTACK_ID`] (execution) and [`SUB2_ATTACK_ID`]
//!   (persistence), and a deprecated sub-technique [`RETIRED_SUB_ATTACK_ID`]
//!   (execution).
//! - A revocation chain: [`REVOKED_FIRST_ATTACK_ID`] is revoked by
//!   [`REVOKED_SECOND_ATTACK_ID`], itself revoked by [`SUB1_ATTACK_ID`].
//! - A deprecated technique [`DEPRECATED_ATTACK_ID`] with no replacement.
//! - A revoked malware [`MALWARE_ATTACK_ID`] replaced by the live tool
//!   [`TOOL_ATTACK_ID`].
//! - The detection strategy [`STRATEGY_ATTACK_ID`] detects the parent and both
//!   live sub-techniques through one Windows analytic reading [`COMPONENT`] from
//!   [`LOG_SOURCE`].
//!
//! Every object carries its own `mitre-attack` id. Included with
//! `#[path = "common/structure.rs"] mod structure;` beside `mod common;` and
//! `mod stix_objects;`.

use serde_json::{Value, json};

use super::common::TestDb;
use super::stix_objects::{analytic, object, source, sync_stix_fixture};

/// The `execution` tactic's ATT&CK id.
pub const EXECUTION_ATTACK_ID: &str = "TA0002";
/// The `execution` tactic's STIX id.
pub const EXECUTION_STIX_ID: &str = "x-mitre-tactic--50000000-0000-4000-8000-000000000002";
/// The `persistence` tactic's ATT&CK id.
pub const PERSISTENCE_ATTACK_ID: &str = "TA0003";
/// The `persistence` tactic's STIX id.
pub const PERSISTENCE_STIX_ID: &str = "x-mitre-tactic--50000000-0000-4000-8000-000000000003";
/// The parent technique's ATT&CK id.
pub const PARENT_ATTACK_ID: &str = "T1500";
/// The parent technique's STIX id.
pub const PARENT_STIX_ID: &str = "attack-pattern--51000000-0000-4000-8000-000000000001";
/// The first live sub-technique's ATT&CK id.
pub const SUB1_ATTACK_ID: &str = "T1500.001";
/// The first live sub-technique's STIX id.
pub const SUB1_STIX_ID: &str = "attack-pattern--51000000-0000-4000-8000-000000000002";
/// The second live sub-technique's ATT&CK id.
pub const SUB2_ATTACK_ID: &str = "T1500.002";
/// The second live sub-technique's STIX id.
pub const SUB2_STIX_ID: &str = "attack-pattern--51000000-0000-4000-8000-000000000003";
/// The deprecated sub-technique's ATT&CK id.
pub const RETIRED_SUB_ATTACK_ID: &str = "T1500.003";
/// The deprecated sub-technique's STIX id.
pub const RETIRED_SUB_STIX_ID: &str = "attack-pattern--51000000-0000-4000-8000-000000000004";
/// The ATT&CK id of the technique at the start of the revocation chain.
pub const REVOKED_FIRST_ATTACK_ID: &str = "T1501";
/// The STIX id of the technique at the start of the revocation chain.
pub const REVOKED_FIRST_STIX_ID: &str = "attack-pattern--51000000-0000-4000-8000-000000000005";
/// The ATT&CK id of the revoked technique that revokes the first.
pub const REVOKED_SECOND_ATTACK_ID: &str = "T1502";
/// The STIX id of the revoked technique that revokes the first.
pub const REVOKED_SECOND_STIX_ID: &str = "attack-pattern--51000000-0000-4000-8000-000000000006";
/// The deprecated technique's ATT&CK id.
pub const DEPRECATED_ATTACK_ID: &str = "T1503";
/// The deprecated technique's STIX id.
pub const DEPRECATED_STIX_ID: &str = "attack-pattern--51000000-0000-4000-8000-000000000007";
/// The revoked malware's ATT&CK id.
pub const MALWARE_ATTACK_ID: &str = "S0500";
/// The revoked malware's STIX id.
pub const MALWARE_STIX_ID: &str = "malware--52000000-0000-4000-8000-000000000001";
/// The ATT&CK id of the tool that replaces the malware.
pub const TOOL_ATTACK_ID: &str = "S0501";
/// The STIX id of the tool that replaces the malware.
pub const TOOL_STIX_ID: &str = "tool--52000000-0000-4000-8000-000000000002";
/// The detection strategy's ATT&CK id.
pub const STRATEGY_ATTACK_ID: &str = "DET0500";
/// The detection strategy's STIX id.
pub const STRATEGY_STIX_ID: &str =
    "x-mitre-detection-strategy--53000000-0000-4000-8000-000000000001";
/// The Windows analytic's ATT&CK id.
pub const ANALYTIC_ATTACK_ID: &str = "AN0500";
/// The Windows analytic's STIX id.
pub const ANALYTIC_STIX_ID: &str = "x-mitre-analytic--53000000-0000-4000-8000-000000000002";
/// The data component's ATT&CK id.
pub const COMPONENT_ATTACK_ID: &str = "DC0500";
/// The data component's STIX id.
pub const COMPONENT_STIX_ID: &str = "x-mitre-data-component--53000000-0000-4000-8000-000000000003";
/// The data component's name.
pub const COMPONENT: &str = "Module Load";
/// The log source the analytic reads the component from.
pub const LOG_SOURCE: &str = "WinEventLog:Sysmon";

fn tactic(stix_id: &str, attack_id: &str, shortname: &str, name: &str) -> Value {
    let mut t = object("x-mitre-tactic", stix_id, name, attack_id);
    t["x_mitre_shortname"] = json!(shortname);
    t["x_mitre_domains"] = json!(["enterprise-attack"]);
    t
}

/// A Windows technique in the enterprise tactics `phases`.
fn technique(stix_id: &str, attack_id: &str, phases: &[&str]) -> Value {
    let mut t = object("attack-pattern", stix_id, attack_id, attack_id);
    t["x_mitre_platforms"] = json!(["Windows"]);
    t["x_mitre_is_subtechnique"] = json!(attack_id.contains('.'));
    t["kill_chain_phases"] = phases
        .iter()
        .map(|p| json!({ "kill_chain_name": "mitre-attack", "phase_name": p }))
        .collect();
    t
}

fn relationship(n: u32, rel_type: &str, source: &str, target: &str) -> Value {
    json!({
        "type": "relationship",
        "id": format!("relationship--5e000000-0000-4000-8000-{n:012}"),
        "relationship_type": rel_type,
        "source_ref": source,
        "target_ref": target,
    })
}

/// The fixture as a STIX bundle.
pub fn fixture_json() -> String {
    let mut retired_sub = technique(RETIRED_SUB_STIX_ID, RETIRED_SUB_ATTACK_ID, &["execution"]);
    retired_sub["x_mitre_deprecated"] = json!(true);
    let mut revoked_first = technique(REVOKED_FIRST_STIX_ID, REVOKED_FIRST_ATTACK_ID, &[]);
    revoked_first["revoked"] = json!(true);
    let mut revoked_second = technique(REVOKED_SECOND_STIX_ID, REVOKED_SECOND_ATTACK_ID, &[]);
    revoked_second["revoked"] = json!(true);
    let mut deprecated = technique(DEPRECATED_STIX_ID, DEPRECATED_ATTACK_ID, &[]);
    deprecated["x_mitre_deprecated"] = json!(true);
    let mut malware = object(
        "malware",
        MALWARE_STIX_ID,
        "Retired Malware",
        MALWARE_ATTACK_ID,
    );
    malware["revoked"] = json!(true);

    let mut strategy = object(
        "x-mitre-detection-strategy",
        STRATEGY_STIX_ID,
        "Detect T1500",
        STRATEGY_ATTACK_ID,
    );
    strategy["x_mitre_analytic_refs"] = json!([ANALYTIC_STIX_ID]);

    json!({
        "type": "bundle",
        "id": "bundle--5f000000-0000-4000-8000-000000000001",
        "objects": [
            tactic(EXECUTION_STIX_ID, EXECUTION_ATTACK_ID, "execution", "Execution"),
            tactic(PERSISTENCE_STIX_ID, PERSISTENCE_ATTACK_ID, "persistence", "Persistence"),
            technique(PARENT_STIX_ID, PARENT_ATTACK_ID, &["execution", "persistence"]),
            technique(SUB1_STIX_ID, SUB1_ATTACK_ID, &["execution"]),
            technique(SUB2_STIX_ID, SUB2_ATTACK_ID, &["persistence"]),
            retired_sub,
            revoked_first,
            revoked_second,
            deprecated,
            malware,
            object("tool", TOOL_STIX_ID, "Replacement Tool", TOOL_ATTACK_ID),
            strategy,
            analytic(
                ANALYTIC_STIX_ID,
                ANALYTIC_ATTACK_ID,
                "Windows",
                vec![source(COMPONENT_STIX_ID, LOG_SOURCE, Some("EventCode=7"))],
            ),
            object("x-mitre-data-component", COMPONENT_STIX_ID, COMPONENT, COMPONENT_ATTACK_ID),
            relationship(1, "subtechnique-of", SUB1_STIX_ID, PARENT_STIX_ID),
            relationship(2, "subtechnique-of", SUB2_STIX_ID, PARENT_STIX_ID),
            relationship(3, "subtechnique-of", RETIRED_SUB_STIX_ID, PARENT_STIX_ID),
            relationship(4, "revoked-by", REVOKED_FIRST_STIX_ID, REVOKED_SECOND_STIX_ID),
            relationship(5, "revoked-by", REVOKED_SECOND_STIX_ID, SUB1_STIX_ID),
            relationship(6, "revoked-by", MALWARE_STIX_ID, TOOL_STIX_ID),
            relationship(7, "detects", STRATEGY_STIX_ID, PARENT_STIX_ID),
            relationship(8, "detects", STRATEGY_STIX_ID, SUB1_STIX_ID),
            relationship(9, "detects", STRATEGY_STIX_ID, SUB2_STIX_ID),
        ],
    })
    .to_string()
}

/// Applies the core, mem and cyber schemas to `db`, then syncs [`fixture_json`]
/// with its post-sync writes.
pub async fn sync_fixture(db: &TestDb) {
    sync_stix_fixture(db, &fixture_json(), "the structure fixture").await;
}
