//! The cyber worked example in `extensions/cyber-gecko/README.md` runs as written.
//!
//! The test reads every `gecko run` command from the README's `## Worked example`
//! section and runs it through the `gecko` binary, against a small STIX fixture
//! holding the walkthrough's real ATT&CK ids, the sample sensor file, and the sample
//! bundle. A README edit that breaks a command breaks this test.

#![cfg(feature = "cyber")]

mod common;
#[path = "common/walkthrough.rs"]
mod walkthrough;

use common::TestDb;
use gecko_engine::db::graph_store::RouterGraphStore;
use gecko_extension_api::{GraphReader, GraphValue};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;
use tokio::sync::Mutex;

const REPO_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
const SENSORS: &str = "extensions/cyber-gecko/sample-sensors.yaml";
const BUNDLE: &str = "extensions/cyber-gecko/sample-bundle";

/// One `gecko run` line from the README.
struct RunCommand {
    concept_id: String,
    args: Option<String>,
}

/// The `## Worked example` section of the cyber README.
fn walkthrough_section() -> String {
    let readme =
        std::fs::read_to_string(Path::new(REPO_ROOT).join("extensions/cyber-gecko/README.md"))
            .expect("read the cyber README");
    let start = readme
        .find("## Worked example")
        .expect("the README has a worked example");
    let rest = &readme[start..];
    let end = rest[3..].find("\n## ").map_or(rest.len(), |i| i + 3);
    rest[..end].to_string()
}

/// Every `gecko run <concept-id> --database attack [--args '<json>']` line, in order.
/// A line with any other shape (an extra flag, another database, a split line) fails.
fn run_commands(section: &str) -> Vec<RunCommand> {
    section
        .lines()
        .filter_map(|line| line.trim().strip_prefix("gecko run "))
        .map(|rest| {
            let (concept_id, tail) = rest
                .split_once(' ')
                .unwrap_or_else(|| panic!("a walkthrough command names no database: {rest}"));
            let args = match tail.strip_prefix("--database attack") {
                Some("") => None,
                Some(more) => Some(
                    more.strip_prefix(" --args '")
                        .and_then(|a| a.strip_suffix('\''))
                        .unwrap_or_else(|| {
                            panic!(
                                "expected `--database attack` then an optional \
                                 single-quoted --args: {rest}"
                            )
                        })
                        .to_string(),
                ),
                None => panic!("a walkthrough command must be `--database attack`: {rest}"),
            };
            RunCommand {
                concept_id: concept_id.to_string(),
                args,
            }
        })
        .collect()
}

fn text(out: &Output) -> String {
    format!(
        "--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The JSON output of a successful `gecko run`: it exited 0 and printed
/// "Execution successful!".
fn run_output(out: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("Execution successful!"),
        "the run did not succeed\n{}",
        text(out)
    );
    let line = stdout
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("Output: "))
        .unwrap_or_else(|| panic!("no Output line\n{}", text(out)));
    serde_json::from_str(line).unwrap_or_else(|e| panic!("output is not JSON ({e}): {line}"))
}

struct Cli {
    config: PathBuf,
    cache: PathBuf,
    database: String,
}

impl Cli {
    fn new(dir: &Path, database: &str) -> Self {
        let config = dir.join("gecko.toml");
        std::fs::write(
            &config,
            "[typedb]\nendpoint = \"localhost:1729\"\nmode = \"external\"\n\n\
             [extensions]\nenabled = [\"cyber\"]\n\n\
             [semantic_index]\nenabled = true\nembedder = \"stub\"\n",
        )
        .expect("write gecko.toml");
        Self {
            config,
            cache: dir.join("cache"),
            database: database.to_string(),
        }
    }

    async fn gecko(&self, args: &[&str]) -> Output {
        tokio::process::Command::new(env!("CARGO_BIN_EXE_gecko"))
            .current_dir(REPO_ROOT)
            .arg("--config")
            .arg(&self.config)
            .args(args)
            .args(["--database", &self.database])
            .env("GECKO_CACHE_DIR", &self.cache)
            .output()
            .await
            .expect("spawn gecko")
    }

    async fn ok(&self, args: &[&str]) {
        let out = self.gecko(args).await;
        assert!(
            out.status.success(),
            "gecko {args:?} failed\n{}",
            text(&out)
        );
    }

    async fn run(&self, concept_id: &str, run_args: Option<&str>) -> Value {
        let mut args = vec!["run", concept_id];
        if let Some(run_args) = run_args {
            args.extend(["--args", run_args]);
        }
        run_output(&self.gecko(&args).await)
    }
}

/// The belief-state of the belief `id`, read straight from the graph.
async fn belief_state(db: &TestDb, id: &str) -> Vec<Value> {
    RouterGraphStore::new(Arc::new(Mutex::new(db.router())))
        .read(
            "given $id: string; \
             match $b isa belief, has concept-id $cid, has belief-state $s; $cid == $id; \
             fetch { \"s\": $s };",
            &["id".to_string()],
            &[GraphValue::from(id)],
        )
        .await
        .expect("read the belief state")
}

/// The sorted key names of the reply `value`.
fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap_or_else(|| panic!("an object: {value}"))
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

fn ids(report: &Value, field: &str) -> Vec<String> {
    report[field]
        .as_array()
        .unwrap_or_else(|| panic!("{field} is an array: {report}"))
        .iter()
        .map(|v| v.as_str().expect("an id").to_string())
        .collect()
}

fn tactic<'a>(report: &'a Value, attack_id: &str) -> &'a Value {
    report["by_tactic"]
        .as_array()
        .expect("by_tactic")
        .iter()
        .find(|t| t["attack_id"] == attack_id)
        .unwrap_or_else(|| panic!("no tactic {attack_id}: {report}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_readme_walkthrough_runs() {
    let db = TestDb::new("gecko_test_cyber_walkthrough");
    let section = walkthrough_section();
    assert!(
        section.contains("gecko sync-stix enterprise-attack.json --database attack"),
        "the README imports ATT&CK into the attack database"
    );
    assert!(
        section.contains(&format!("gecko sync-sensors {SENSORS} --database attack")),
        "the README syncs {SENSORS}"
    );
    assert!(
        section.contains(&format!("gecko sync {BUNDLE} --database attack")),
        "the README syncs {BUNDLE}"
    );
    let commands = run_commands(&section);
    let names: Vec<&str> = commands.iter().map(|c| c.concept_id.as_str()).collect();
    assert_eq!(
        names,
        [
            "playbooks/claim_detection",
            "playbooks/claim_detection",
            "playbooks/coverage_report",
            "playbooks/record_verdict",
            "playbooks/detection_precision",
            "playbooks/withdraw_claim",
            "playbooks/withdraw_claim",
        ],
        "the walkthrough's gecko run commands"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let stix = tmp.path().join("attack.json");
    std::fs::write(&stix, walkthrough::bundle()).expect("write the fixture");
    let cli = Cli::new(tmp.path(), &db.name);
    cli.ok(&["sync-stix", stix.to_str().unwrap()]).await;
    cli.ok(&["sync-sensors", SENSORS]).await;
    cli.ok(&["sync", BUNDLE]).await;

    let report = cli.run("playbooks/coverage_report", None).await;
    assert_eq!(
        ids(&report, "undetected"),
        ["T1053", "T1053.005", "T1059", "T1059.001"],
        "the baseline"
    );

    let powershell = cli
        .run(&commands[0].concept_id, commands[0].args.as_deref())
        .await;
    assert!(powershell["belief_id"].is_string(), "{powershell}");
    assert_eq!(keys(&powershell), ["belief_id", "logic_id", "technique_id"]);
    let powershell_belief = powershell["belief_id"].as_str().unwrap().to_string();
    let task = cli
        .run(&commands[1].concept_id, commands[1].args.as_deref())
        .await;
    let task_belief = task["belief_id"].as_str().expect("a belief id").to_string();

    let report = cli
        .run(&commands[2].concept_id, commands[2].args.as_deref())
        .await;
    assert_eq!(ids(&report, "undetected"), ["T1053", "T1059"], "{report}");
    assert_eq!(tactic(&report, "TA0002")["live"], 4);
    assert_eq!(tactic(&report, "TA0002")["undetected"], 2);
    assert_eq!(tactic(&report, "TA0003")["live"], 2);
    assert_eq!(tactic(&report, "TA0003")["undetected"], 1);
    assert_eq!(
        ids(&report, "blind"),
        ["detections/scheduled_task_persistence"]
    );
    assert_eq!(ids(&report, "recommended"), ["WinEventLog:Security"]);
    assert_eq!(
        keys(&report),
        ["blind", "by_tactic", "recommended", "undetected"]
    );

    let verdict_args = commands[3]
        .args
        .as_deref()
        .expect("record_verdict takes args")
        .replace("<powershell_belief_id>", &powershell_belief);
    let verdict = cli.run(&commands[3].concept_id, Some(&verdict_args)).await;
    assert_eq!(verdict["logic_id"], "detections/encoded_powershell");
    assert_eq!(verdict["verdict"], "true-positive");
    assert_eq!(verdict["assumes"], json!([powershell_belief]));
    assert!(verdict["belief_id"].is_string(), "{verdict}");
    assert_eq!(
        keys(&verdict),
        ["assumes", "belief_id", "logic_id", "verdict"]
    );

    let precision = cli
        .run(&commands[4].concept_id, commands[4].args.as_deref())
        .await;
    assert_eq!(precision["precision"].as_f64(), Some(1.0), "{precision}");
    assert_eq!(precision["true_positives"], 1);
    assert_eq!(precision["false_positives"], 0);
    assert_eq!(
        keys(&precision),
        [
            "false_positives",
            "logic_id",
            "precision",
            "true_positives",
            "window_seconds"
        ]
    );

    let withdraw_args = commands[5]
        .args
        .as_deref()
        .expect("withdraw_claim takes args")
        .replace("<scheduled_task_belief_id>", &task_belief);
    let withdrawn = cli.run(&commands[5].concept_id, Some(&withdraw_args)).await;
    assert_eq!(withdrawn["id"], json!(task_belief));
    assert_eq!(keys(&withdrawn), ["blast_radius", "id"]);

    let report = cli.run("playbooks/coverage_report", None).await;
    assert_eq!(
        ids(&report, "undetected"),
        ["T1053", "T1053.005", "T1059"],
        "the withdrawn claim's technique is a gap again"
    );
    assert!(ids(&report, "blind").is_empty(), "{report}");
    assert_eq!(
        belief_state(&db, &task_belief).await,
        [json!({ "s": "retracted" })]
    );

    // Withdrawing the PowerShell claim reports the verdict that rested on it.
    let withdraw_args = commands[6]
        .args
        .as_deref()
        .expect("withdraw_claim takes args")
        .replace("<powershell_belief_id>", &powershell_belief);
    let withdrawn = cli.run(&commands[6].concept_id, Some(&withdraw_args)).await;
    assert_eq!(withdrawn["id"], json!(powershell_belief));
    assert_eq!(withdrawn["blast_radius"], json!([verdict["belief_id"]]));
    let report = cli.run("playbooks/coverage_report", None).await;
    assert_eq!(
        ids(&report, "undetected"),
        ["T1053", "T1053.005", "T1059", "T1059.001"],
        "both claims are withdrawn"
    );
}

/// A synced walkthrough database with one claim on the PowerShell detection.
async fn claimed(db: &TestDb, tmp: &Path) -> Cli {
    let stix = tmp.join("attack.json");
    std::fs::write(&stix, walkthrough::bundle()).expect("write the fixture");
    let cli = Cli::new(tmp, &db.name);
    cli.ok(&["sync-stix", stix.to_str().unwrap()]).await;
    cli.ok(&["sync-sensors", SENSORS]).await;
    cli.ok(&["sync", BUNDLE]).await;
    cli.run(
        "playbooks/claim_detection",
        Some(
            r#"{"logic_id":"detections/encoded_powershell","technique_id":"T1059.001","method":"human-assertion","confidence":0.8}"#,
        ),
    )
    .await;
    cli
}

/// The true positives `detection_precision` counts on the PowerShell detection.
async fn true_positives(cli: &Cli) -> Value {
    cli.run(
        "playbooks/detection_precision",
        Some(r#"{"logic_id":"detections/encoded_powershell","window":86400}"#),
    )
    .await["true_positives"]
        .clone()
}

/// Runs `record_verdict` with `assumes` and returns its raw output.
async fn record_verdict(cli: &Cli, assumes: &str) -> Output {
    let args = format!(
        r#"{{"logic_id":"detections/encoded_powershell","verdict":"true-positive","assumes":{assumes}}}"#
    );
    cli.gecko(&["run", "playbooks/record_verdict", "--args", &args])
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn record_verdict_refuses_a_bad_assumes_before_recording() {
    let db = TestDb::new("gecko_test_cyber_bad_assumes");
    let tmp = tempfile::tempdir().expect("tempdir");
    let cli = claimed(&db, tmp.path()).await;
    for assumes in [r#"[]"#, r#"["a","a"]"#, r#"[""]"#, r#""x""#, "null", "[1]"] {
        let out = record_verdict(&cli, assumes).await;
        assert!(
            !out.status.success(),
            "assumes {assumes} was accepted\n{}",
            text(&out)
        );
        assert!(
            text(&out).contains("assumes"),
            "assumes {assumes}: the refusal does not name assumes\n{}",
            text(&out)
        );
    }
    assert_eq!(true_positives(&cli).await, 0, "no verdict was recorded");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn record_verdict_with_a_missing_assumption_keeps_the_verdict() {
    let db = TestDb::new("gecko_test_cyber_missing_assumption");
    let tmp = tempfile::tempdir().expect("tempdir");
    let cli = claimed(&db, tmp.path()).await;
    let out = record_verdict(&cli, r#"["mem/bel/does-not-exist"]"#).await;
    assert!(!out.status.success(), "{}", text(&out));
    assert_eq!(
        true_positives(&cli).await,
        1,
        "the verdict is recorded before mem.restsOn refuses the missing id"
    );
}
