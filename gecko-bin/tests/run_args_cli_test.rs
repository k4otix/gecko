//! `gecko run` hands a playbook its run inputs: `--args` as `gecko.args`, refused
//! before contacting TypeDB when it is not an object, and the playbook's stored
//! `timeout-ms` as its execution timeout.

mod common;
use common::TestDb;

use gecko_engine::db::{CORE_SCHEMA, run_graph_writes};
use gecko_extension_api::{GraphValue, GraphWrite};

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

/// The JSON output of a successful `gecko run`: it exited 0 and printed
/// "Execution successful!".
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

/// Syncs a one-playbook bundle holding `playbook` as `playbooks/<name>.md`.
async fn sync_playbook(db: &TestDb, dir: &Path, name: &str, playbook: &str) {
    let bundle = dir.join("bundle");
    std::fs::create_dir_all(bundle.join("playbooks")).unwrap();
    std::fs::write(bundle.join(format!("playbooks/{name}.md")), playbook).unwrap();
    let synced = gecko(dir)
        .arg("--database")
        .arg(&db.name)
        .arg("sync")
        .arg(&bundle)
        .output()
        .await
        .expect("spawn gecko sync");
    assert!(synced.status.success(), "sync failed\n{}", text(&synced));
}

async fn run(db: &TestDb, dir: &Path, concept_id: &str) -> Output {
    gecko(dir)
        .arg("--database")
        .arg(&db.name)
        .args(["run", concept_id])
        .output()
        .await
        .expect("spawn gecko run")
}

const LOOPING_PLAYBOOK: &str = r#"---
type: playbook
title: Loop forever
engine: quickjs
timeout-ms: 500
---

# Loop forever

```javascript
while (true) {}
```
"#;

const THROWING_PLAYBOOK: &str = r#"---
type: playbook
title: Throw
engine: quickjs
---

# Throw

```javascript
throw new Error("boom");
```
"#;

#[tokio::test]
async fn a_failed_playbook_exits_non_zero() {
    let db = TestDb::new("run_failed_exit");
    let tmp = tempfile::tempdir().expect("tempdir");
    sync_playbook(&db, tmp.path(), "throw", THROWING_PLAYBOOK).await;

    let out = run(&db, tmp.path(), "playbooks/throw").await;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "a failed run exited 0\n{}",
        text(&out)
    );
    assert!(
        stdout.contains("Execution failed!")
            && stdout.contains("Error: JS Execution Error:")
            && stdout.contains("boom"),
        "the failure was not printed\n{}",
        text(&out)
    );
    assert!(
        !stderr.contains("ERROR") && !stderr.contains("the playbook failed"),
        "a failed playbook was logged as a host error\n{}",
        text(&out)
    );
}

#[tokio::test]
async fn a_playbook_times_out_at_its_timeout_ms() {
    let db = TestDb::new("run_timeout");
    let tmp = tempfile::tempdir().expect("tempdir");
    sync_playbook(&db, tmp.path(), "loop", LOOPING_PLAYBOOK).await;

    let started = std::time::Instant::now();
    let out = run(&db, tmp.path(), "playbooks/loop").await;
    let elapsed = started.elapsed();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Execution failed!") && stdout.contains("execution timed out after 500 ms"),
        "the run did not time out at 500 ms\n{}",
        text(&out)
    );
    assert!(
        !out.status.success(),
        "a timed-out run exited 0\n{}",
        text(&out)
    );
    assert!(
        elapsed < std::time::Duration::from_secs(25),
        "the run took {elapsed:?}, so the 30 s default applied"
    );
}

#[tokio::test]
async fn a_stored_timeout_out_of_range_fails_the_run() {
    let db = TestDb::new("run_timeout_stored");
    let tmp = tempfile::tempdir().expect("tempdir");
    sync_playbook(&db, tmp.path(), "echo", ECHO_PLAYBOOK).await;
    let mut router = db.router();
    let tx = router.begin_write().await.expect("write tx");
    run_graph_writes(
        &tx,
        &[GraphWrite::single(
            "given $id: string, $t: integer; \
             match $c isa concept, has concept-id $cid; $cid == $id; \
             insert $c has timeout-ms == $t;",
            vec!["id".into(), "t".into()],
            vec!["playbooks/echo".into(), GraphValue::Long(700_000)],
        )
        .expecting(1)],
    )
    .await
    .expect("store an over-ceiling timeout-ms");
    tx.commit().await.expect("commit");

    let out = run(&db, tmp.path(), "playbooks/echo").await;
    let output = text(&out);
    assert!(
        !out.status.success(),
        "the run accepted a bad stored timeout\n{output}"
    );
    assert!(
        output.contains("timeout-ms")
            && output.contains("playbooks/echo")
            && output.contains("edit the playbook"),
        "{output}"
    );
}

#[tokio::test]
async fn a_playbook_runs_on_a_schema_without_timeout_ms() {
    let db = TestDb::new("run_no_timeout_attr");
    let tmp = tempfile::tempdir().expect("tempdir");
    let legacy_schema = CORE_SCHEMA
        .replace("    owns timeout-ms @card(0..1),\n", "")
        .lines()
        .filter(|l| !l.trim_start().starts_with("attribute timeout-ms"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !legacy_schema.contains("timeout-ms"),
        "the legacy schema still names timeout-ms"
    );
    db.router()
        .apply_schema(&legacy_schema)
        .await
        .expect("apply the schema without timeout-ms");
    db.router()
        .apply_schema(mem_gecko::extension().schema())
        .await
        .expect("apply the mem schema");
    let mut router = db.router();
    let tx = router.begin_write().await.expect("write tx");
    run_graph_writes(
        &tx,
        &[GraphWrite::single(
            "given $id: string, $code: string; \
             insert $c isa concept, has concept-id == $id, has code-block == $code, \
                    has engine \"javascript\";",
            vec!["id".into(), "code".into()],
            vec!["playbooks/legacy".into(), "({ ok: true })".into()],
        )
        .expecting(1)],
    )
    .await
    .expect("insert the playbook concept");
    tx.commit().await.expect("commit");

    let out = run(&db, tmp.path(), "playbooks/legacy").await;
    assert_eq!(run_output(&out), serde_json::json!({ "ok": true }));
}

#[tokio::test]
async fn a_bundle_with_an_unsupported_engine_fails_to_sync() {
    let db = TestDb::new("run_unsupported_engine");
    let tmp = tempfile::tempdir().expect("tempdir");
    let bundle = tmp.path().join("bundle");
    std::fs::create_dir_all(bundle.join("playbooks")).unwrap();
    std::fs::write(bundle.join("playbooks/hello.md"), ECHO_PLAYBOOK).unwrap();
    std::fs::write(
        bundle.join("playbooks/typo.md"),
        "---\ntype: playbook\ntitle: Typo\nengine: javscript\n---\n\n```js\n1\n```\n",
    )
    .unwrap();
    let out = gecko(tmp.path())
        .arg("--database")
        .arg(&db.name)
        .arg("sync")
        .arg(&bundle)
        .output()
        .await
        .expect("spawn gecko sync");
    let output = text(&out);
    assert!(!out.status.success(), "the sync succeeded\n{output}");
    assert!(
        output.contains("typo.md") && output.contains("javscript"),
        "the failure does not name the file and value\n{output}"
    );
    let dbs = db.router().list_databases().await.expect("list databases");
    assert!(
        !dbs.contains(&db.name),
        "a refused parse created the database\n{output}"
    );
}

#[tokio::test]
async fn sync_prints_provenance_warnings() {
    let db = TestDb::new("sync_provenance_warnings");
    let tmp = tempfile::tempdir().expect("tempdir");
    let bundle = tmp.path().join("bundle");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(
        bundle.join("page.md"),
        "---\ntype: guide\ntitle: Page\nstale_after: soon\n---\n\nBody.\n",
    )
    .unwrap();
    let out = gecko(tmp.path())
        .arg("--database")
        .arg(&db.name)
        .arg("sync")
        .arg(&bundle)
        .output()
        .await
        .expect("spawn gecko sync");
    let output = text(&out);
    assert!(out.status.success(), "the sync failed\n{output}");
    assert!(
        output.contains("warning:") && output.contains("page.md") && output.contains("soon"),
        "the warning is not printed\n{output}"
    );
}
