//! `gecko run --args` hands a playbook its arguments as `gecko.args`, and refuses
//! a non-object value before contacting TypeDB.

mod common;
use common::TestDb;

use std::path::Path;
use std::process::Output;

/// Writes a minimal external-TypeDB `gecko.toml` with no extensions into `dir`.
fn write_config(dir: &Path) -> std::path::PathBuf {
    let config = dir.join("gecko.toml");
    std::fs::write(
        &config,
        "[typedb]\nendpoint = \"localhost:1729\"\nmode = \"external\"\n",
    )
    .expect("write gecko.toml");
    config
}

fn gecko(dir: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_gecko"));
    cmd.arg("--config")
        .arg(write_config(dir))
        .env("GECKO_CACHE_DIR", dir.join("cache"));
    cmd
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
fn run_output(out: &Output) -> serde_json::Value {
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

const ECHO_PLAYBOOK: &str = r#"---
type: playbook
title: Echo an argument
engine: quickjs
---

# Echo an argument

```javascript
({ echoed: gecko.args.k })
```
"#;

#[tokio::test]
async fn run_args_reach_the_playbook() {
    let db = TestDb::new("run_args_echo");
    let tmp = tempfile::tempdir().expect("tempdir");
    let bundle = tmp.path().join("bundle");
    std::fs::create_dir_all(bundle.join("playbooks")).unwrap();
    std::fs::write(bundle.join("playbooks/echo.md"), ECHO_PLAYBOOK).unwrap();

    let synced = gecko(tmp.path())
        .arg("--database")
        .arg(&db.name)
        .arg("sync")
        .arg(&bundle)
        .output()
        .await
        .expect("spawn gecko sync");
    assert!(synced.status.success(), "sync failed\n{}", text(&synced));
    assert!(
        db.router()
            .list_databases()
            .await
            .expect("list databases")
            .contains(&db.name),
        "sync created the database"
    );

    let out = gecko(tmp.path())
        .arg("--database")
        .arg(&db.name)
        .args(["run", "playbooks/echo", "--args", r#"{"k":"v"}"#])
        .output()
        .await
        .expect("spawn gecko run");
    assert_eq!(run_output(&out), serde_json::json!({"echoed": "v"}));
}

#[tokio::test]
async fn non_object_args_fail_before_connecting() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = gecko(tmp.path())
        .args(["--address", "127.0.0.1:1", "run", "x", "--args", "[]"])
        .output()
        .await
        .expect("spawn gecko run");
    let output = text(&out);
    assert!(
        !out.status.success(),
        "a non-object --args was accepted\n{output}"
    );
    assert!(output.contains("must be a JSON object"), "{output}");
    assert!(
        !output.to_lowercase().contains("connect"),
        "the run reached TypeDB\n{output}"
    );
}
