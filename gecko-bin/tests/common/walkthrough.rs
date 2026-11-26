//! A small STIX bundle holding the real ATT&CK ids the cyber worked example uses
//! (`extensions/cyber-gecko/README.md`), shaped like Enterprise 20.1 where it matters:
//!
//! - the tactics execution (`TA0002`) and persistence (`TA0003`);
//! - `T1059`, with the sub-technique `T1059.001`, in execution;
//! - `T1053`, with the sub-technique `T1053.005`, in execution and persistence;
//! - one detection strategy and one Windows analytic per sub-technique, reading the
//!   log sources 20.1's analytics read. Both read `WinEventLog:Sysmon`; only
//!   `T1053.005`'s also reads `WinEventLog:Security`, which the sample sensor file
//!   leaves out.
//!
//! It is a subset of 20.1, chosen to keep the covered and blind split. 20.1 also puts
//! `T1053` and `T1053.005` in privilege-escalation (`TA0004`), runs `T1059.001` on
//! Linux and macOS too, has the analytics read more data components (Process Metadata;
//! Scheduled Job Modification and Windows Registry Key Modification), and names more
//! PowerShell channels (`EventCode=4103, 4104, 4105, 4106`).

use serde_json::{Value, json};

fn object(ty: &str, n: u32, name: &str, attack_id: &str) -> Value {
    json!({
        "type": ty,
        "id": format!("{ty}--60000000-0000-4000-8000-{n:012}"),
        "name": name,
        "external_references": [{ "source_name": "mitre-attack", "external_id": attack_id }],
        "x_mitre_deprecated": false,
    })
}

fn id(obj: &Value) -> String {
    obj["id"].as_str().expect("an id").to_string()
}

fn tactic(n: u32, attack_id: &str, shortname: &str, name: &str) -> Value {
    let mut t = object("x-mitre-tactic", n, name, attack_id);
    t["x_mitre_shortname"] = json!(shortname);
    t["x_mitre_domains"] = json!(["enterprise-attack"]);
    t
}

fn technique(n: u32, attack_id: &str, name: &str, phases: &[&str]) -> Value {
    let mut t = object("attack-pattern", n, name, attack_id);
    t["x_mitre_platforms"] = json!(["Windows"]);
    t["x_mitre_is_subtechnique"] = json!(attack_id.contains('.'));
    t["kill_chain_phases"] = phases
        .iter()
        .map(|p| json!({ "kill_chain_name": "mitre-attack", "phase_name": p }))
        .collect();
    t
}

fn relationship(n: u32, rel_type: &str, source: &Value, target: &Value) -> Value {
    json!({
        "type": "relationship",
        "id": format!("relationship--6e000000-0000-4000-8000-{n:012}"),
        "relationship_type": rel_type,
        "source_ref": id(source),
        "target_ref": id(target),
    })
}

/// The bundle as STIX JSON text.
pub fn bundle() -> String {
    let execution = tactic(1, "TA0002", "execution", "Execution");
    let persistence = tactic(2, "TA0003", "persistence", "Persistence");
    let t1059 = technique(
        10,
        "T1059",
        "Command and Scripting Interpreter",
        &["execution"],
    );
    let t1059_001 = technique(11, "T1059.001", "PowerShell", &["execution"]);
    let t1053 = technique(
        12,
        "T1053",
        "Scheduled Task/Job",
        &["execution", "persistence"],
    );
    let t1053_005 = technique(
        13,
        "T1053.005",
        "Scheduled Task",
        &["execution", "persistence"],
    );

    let components: Vec<Value> = [
        (20, "Process Creation", "DC0032"),
        (21, "Command Execution", "DC0064"),
        (22, "Module Load", "DC0016"),
        (23, "Scheduled Job Creation", "DC0001"),
        (24, "File Creation", "DC0039"),
    ]
    .iter()
    .map(|(n, name, aid)| object("x-mitre-data-component", *n, name, aid))
    .collect();
    let source = |component: usize, name: &str, channel: &str| {
        json!({
            "x_mitre_data_component_ref": id(&components[component]),
            "name": name,
            "channel": channel,
        })
    };
    let analytic = |n: u32, attack_id: &str, sources: Vec<Value>| {
        let mut a = object("x-mitre-analytic", n, attack_id, attack_id);
        a["x_mitre_platforms"] = json!(["Windows"]);
        a["x_mitre_log_source_references"] = Value::Array(sources);
        a
    };
    let powershell_analytic = analytic(
        30,
        "AN1252",
        vec![
            source(0, "WinEventLog:Sysmon", "EventCode=1"),
            source(1, "WinEventLog:PowerShell", "EventCode=4104"),
            source(2, "WinEventLog:Sysmon", "EventCode=7"),
        ],
    );
    let task_analytic = analytic(
        31,
        "AN1221",
        vec![
            source(3, "WinEventLog:Security", "EventCode=4698"),
            source(0, "WinEventLog:Sysmon", "EventCode=1"),
            source(4, "WinEventLog:Sysmon", "EventCode=11"),
        ],
    );
    let mut powershell_strategy = object("x-mitre-detection-strategy", 40, "PowerShell", "DET0455");
    powershell_strategy["x_mitre_analytic_refs"] = json!([id(&powershell_analytic)]);
    let mut task_strategy = object(
        "x-mitre-detection-strategy",
        41,
        "Scheduled Task",
        "DET0441",
    );
    task_strategy["x_mitre_analytic_refs"] = json!([id(&task_analytic)]);

    let relationships = vec![
        relationship(1, "subtechnique-of", &t1059_001, &t1059),
        relationship(2, "subtechnique-of", &t1053_005, &t1053),
        relationship(3, "detects", &powershell_strategy, &t1059_001),
        relationship(4, "detects", &task_strategy, &t1053_005),
    ];

    let mut objects = vec![
        execution,
        persistence,
        t1059,
        t1059_001,
        t1053,
        t1053_005,
        powershell_analytic,
        task_analytic,
        powershell_strategy,
        task_strategy,
    ];
    objects.extend(components);
    objects.extend(relationships);
    json!({
        "type": "bundle",
        "id": "bundle--6f000000-0000-4000-8000-000000000001",
        "objects": objects,
    })
    .to_string()
}
