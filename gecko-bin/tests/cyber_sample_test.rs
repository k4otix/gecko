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
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Output;

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

/// The JSON output of a successful `gecko run`. A failed playbook still exits 0,
/// so success is read from the printed result, not the exit status.
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
    let mut router = db.router();
    let tx = router.begin_read().await.expect("read tx");
    let answer = tx
        .query(format!(
            r#"match $b isa belief, has concept-id "{id}", has belief-state $s; fetch {{ "s": $s }};"#
        ))
        .await
        .expect("read query");
    let mut out = Vec::new();
    let mut stream = answer.into_documents();
    while let Some(doc) = stream.next().await {
        let doc = doc.expect("a document");
        out.push(serde_json::from_str(&doc.into_json().to_string()).unwrap());
    }
    out
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
    assert!(
        ids(&report, "recommended").contains(&"WinEventLog:Security".to_string()),
        "{report}"
    );

    let verdict = cli
        .run(&commands[3].concept_id, commands[3].args.as_deref())
        .await;
    assert_eq!(verdict["logic_id"], "detections/encoded_powershell");

    let precision = cli
        .run(&commands[4].concept_id, commands[4].args.as_deref())
        .await;
    assert_eq!(precision["precision"].as_f64(), Some(1.0), "{precision}");
    assert_eq!(precision["true_positives"], 1);

    let withdraw_args = commands[5]
        .args
        .as_deref()
        .expect("withdraw_claim takes args")
        .replace("<belief_id>", &task_belief);
    let withdrawn = cli.run(&commands[5].concept_id, Some(&withdraw_args)).await;
    assert_eq!(withdrawn["id"], json!(task_belief));

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
}
