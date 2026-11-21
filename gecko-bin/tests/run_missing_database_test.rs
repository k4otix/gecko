//! `gecko run` against a database that does not exist fails without creating it
//! (a read must never create a database), and against an existing database it
//! reports a concept that matches nothing as not found.

mod common;
use common::TestDb;

/// Runs `gecko run <concept_id>` against `db` with a minimal external-TypeDB config
/// and returns the exit status and the combined stdout and stderr.
async fn gecko_run(db: &TestDb, concept_id: &str) -> (bool, String) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = tmp.path().join("gecko.toml");
    std::fs::write(
        &config,
        "[typedb]\nendpoint = \"localhost:1729\"\nmode = \"external\"\n",
    )
    .expect("write gecko.toml");
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_gecko"))
        .arg("--config")
        .arg(&config)
        .arg("--database")
        .arg(&db.name)
        .args(["run", concept_id])
        .env("GECKO_CACHE_DIR", tmp.path().join("cache"))
        .output()
        .await
        .expect("spawn gecko");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[tokio::test]
async fn run_against_a_missing_database_fails_and_does_not_create_it() {
    let db = TestDb::new("run_missing");
    let mut router = db.router();
    assert!(
        !router
            .list_databases()
            .await
            .expect("list databases")
            .contains(&db.name),
        "the unique database name already exists"
    );

    let (ok, output) = gecko_run(&db, "playbooks/anything").await;

    let created = router
        .list_databases()
        .await
        .expect("list databases")
        .contains(&db.name);
    // `TestDb` deletes the database on drop, so a failed assertion leaves nothing behind.
    assert!(!ok, "gecko run succeeded against a missing database");
    assert!(
        output.contains("Failed to read schema"),
        "the failure did not come from the schema read:\n{output}"
    );
    assert!(!created, "gecko run created the missing database");
}

#[tokio::test]
async fn run_of_an_unknown_concept_reports_not_found() {
    let db = TestDb::new("run_unknown");
    let mut router = db.router();
    router
        .apply_schema(include_str!(
            "../../core/gecko-engine/schema/core_schema.tql"
        ))
        .await
        .expect("apply core schema");

    let (ok, output) = gecko_run(&db, "playbooks/no\"such\\concept").await;

    assert!(!ok, "gecko run succeeded for an unknown concept");
    assert!(
        output.contains("Playbook 'playbooks/no\"such\\concept' not found."),
        "unexpected output:\n{output}"
    );
}
