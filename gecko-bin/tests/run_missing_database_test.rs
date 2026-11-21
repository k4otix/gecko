//! `gecko run` against a database that does not exist fails without creating it:
//! a read must never create a database.

mod common;
use common::TestDb;

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
        .args(["run", "playbooks/anything"])
        .env("GECKO_CACHE_DIR", tmp.path().join("cache"))
        .output()
        .await
        .expect("spawn gecko");

    let created = router
        .list_databases()
        .await
        .expect("list databases")
        .contains(&db.name);
    // `TestDb` deletes the database on drop, so a failed assertion leaves nothing behind.
    assert!(
        !out.status.success(),
        "gecko run succeeded against a missing database"
    );
    assert!(!created, "gecko run created the missing database");
}
