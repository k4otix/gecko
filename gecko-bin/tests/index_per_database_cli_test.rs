//! One semantic index file per database, driven through the `gecko` binary against
//! live TypeDB (`localhost:1729`): `drop` removes only that database's file, a
//! database recreated under a dropped one's name reconciles away the stale vectors,
//! the reserved database's file is never touched, and a stale `[semantic_index]
//! path` key fails `run` but not `databases` or `drop`. Each test uses throwaway
//! databases; the real `gecko` database is never reached.

use std::path::{Path, PathBuf};

mod common;
use common::TestDb;

/// A playbook that derives one belief with `text` and outputs its id.
fn derive_playbook(text: &str) -> String {
    format!(
        "---\ntype: playbook\ntitle: Rig derive\nengine: quickjs\nscopes: [mem:derive]\n---\n\n\
         # Rig derive\n\n```javascript\nconst b = mem.derive({}, []);\n({{ id: b.id }})\n```\n",
        serde_json::to_string(text).expect("a string serializes")
    )
}

/// A temp working directory: a config, a cache, the index directory and an OKF
/// bundle with two derive playbooks.
struct Rig {
    _tmp: tempfile::TempDir,
    config: PathBuf,
    cache: PathBuf,
    index_dir: PathBuf,
    okf_dir: PathBuf,
}

impl Rig {
    /// A rig whose `[semantic_index]` table carries `index_line` and whose TypeDB
    /// endpoint is `endpoint`.
    fn with(endpoint: &str, index_line: impl FnOnce(&Path) -> String) -> Rig {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let root = tmp.path();
        let index_dir = root.join("idx");
        let config = root.join("gecko.toml");
        std::fs::write(
            &config,
            format!(
                "[typedb]\nendpoint = \"{endpoint}\"\nmode = \"external\"\n\n\
                 [extensions]\nenabled = []\n\n\
                 [semantic_index]\nenabled = true\n{}\nembedder = \"stub\"\nbackend = \"hnsw\"\n",
                index_line(&index_dir)
            ),
        )
        .expect("write gecko.toml");
        let cache = root.join("cache");
        std::fs::create_dir_all(&cache).expect("cache dir");
        let okf_dir = root.join("okf");
        std::fs::create_dir_all(okf_dir.join("playbooks")).expect("okf dir");
        std::fs::write(
            okf_dir.join("bundle.json"),
            serde_json::json!({ "name": "index-rig" }).to_string(),
        )
        .expect("write bundle.json");
        for (name, text) in [("first", "first claim"), ("second", "second claim")] {
            std::fs::write(
                okf_dir.join(format!("playbooks/{name}.md")),
                derive_playbook(text),
            )
            .expect("write playbook");
        }
        Rig {
            _tmp: tmp,
            config,
            cache,
            index_dir,
            okf_dir,
        }
    }

    /// A rig on the live server whose index directory is set with `dir`.
    fn new() -> Rig {
        Rig::with("localhost:1729", |dir| {
            format!("dir = {}", toml_str(dir.to_str().expect("utf-8 temp path")))
        })
    }

    fn index_file(&self, db: &str) -> PathBuf {
        self.index_dir.join(format!("{db}.hnsw"))
    }
}

fn toml_str(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// Runs the binary with the rig's config and cache, optionally against `db`, and
/// returns whether it succeeded plus its stdout followed by its stderr.
async fn gecko(rig: &Rig, db: Option<&str>, args: &[&str]) -> (bool, String) {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_gecko"));
    cmd.arg("--config").arg(&rig.config);
    if let Some(db) = db {
        cmd.arg("--database").arg(db);
    }
    let out = cmd
        .args(args)
        .env("GECKO_CACHE_DIR", &rig.cache)
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

/// Runs the binary and asserts it succeeded, returning its output.
async fn gecko_ok(rig: &Rig, db: Option<&str>, args: &[&str]) -> String {
    let (ok, text) = gecko(rig, db, args).await;
    assert!(ok, "gecko {args:?} failed:\n{text}");
    text
}

/// Initializes `db`'s schema and syncs the rig's bundle into it.
async fn setup(rig: &Rig, db: &str) {
    gecko_ok(rig, Some(db), &["schema-init"]).await;
    gecko_ok(
        rig,
        Some(db),
        &["sync", rig.okf_dir.to_str().expect("utf-8 temp path")],
    )
    .await;
}

/// Runs `concept` and returns the belief id its output carries.
async fn run_derive(rig: &Rig, db: &str, concept: &str) -> String {
    let text = gecko_ok(rig, Some(db), &["run", concept]).await;
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix("Output: "))
        .unwrap_or_else(|| panic!("run {concept} printed no output line:\n{text}"));
    let v: serde_json::Value = serde_json::from_str(line).expect("output is JSON");
    v["id"].as_str().expect("output carries an id").to_string()
}

/// The concept ids a persisted index snapshot holds.
fn snapshot_ids(path: &Path) -> Vec<String> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let snap: serde_json::Value = serde_json::from_slice(&bytes).expect("snapshot JSON");
    snap["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|e| e["concept_id"].as_str().expect("concept_id").to_string())
        .collect()
}

#[tokio::test]
async fn drop_removes_only_that_databases_index_file() {
    let rig = Rig::new();
    let db = TestDb::new("idxdrop");
    setup(&rig, &db.name).await;
    run_derive(&rig, &db.name, "playbooks/first").await;
    assert!(
        rig.index_file(&db.name).exists(),
        "the run wrote its index file"
    );
    let sibling = rig.index_file("other");
    std::fs::write(&sibling, b"{}").expect("write sibling");

    gecko_ok(&rig, None, &["drop", &db.name]).await;
    let remaining = db.router().list_databases().await.expect("list databases");
    assert!(!remaining.contains(&db.name), "the database was dropped");
    assert!(!rig.index_file(&db.name).exists(), "drop removed the file");
    assert!(sibling.exists(), "another database's file is untouched");
}

#[tokio::test]
async fn a_recreated_database_does_not_keep_the_dropped_ones_vectors() {
    let rig = Rig::new();
    let db = TestDb::new("idxrecreate");
    setup(&rig, &db.name).await;
    let first = run_derive(&rig, &db.name, "playbooks/first").await;
    let file = rig.index_file(&db.name);
    let stale = rig.index_dir.join("stale.copy");
    std::fs::copy(&file, &stale).expect("keep the first run's index");

    gecko_ok(&rig, None, &["drop", &db.name]).await;
    let listed = gecko_ok(&rig, None, &["databases"]).await;
    assert!(
        !listed.lines().any(|l| l.trim() == db.name),
        "the database is gone before it is recreated:\n{listed}"
    );
    // Put the dropped database's index back, so the next run has to reconcile it.
    std::fs::copy(&stale, &file).expect("restore the stale index");
    setup(&rig, &db.name).await;
    let second = run_derive(&rig, &db.name, "playbooks/second").await;

    let ids = snapshot_ids(&file);
    assert!(ids.contains(&second), "the new belief is indexed: {ids:?}");
    assert!(
        !ids.contains(&first),
        "the dropped database's vector is gone: {ids:?}"
    );
}

#[tokio::test]
async fn drop_of_the_reserved_database_leaves_its_index_file() {
    // No server listens on port 1, so nothing is reachable even if the guard regressed.
    let rig = Rig::with("localhost:1", |dir| {
        format!("dir = {}", toml_str(dir.to_str().expect("utf-8 temp path")))
    });
    std::fs::create_dir_all(&rig.index_dir).expect("index dir");
    let file = rig.index_file("gecko");
    std::fs::write(&file, b"{}").expect("write reserved index");

    let (ok, text) = gecko(&rig, None, &["drop", "gecko"]).await;
    assert!(!ok, "dropping the reserved database is refused:\n{text}");
    assert!(file.exists(), "its index file is untouched");
}

#[tokio::test]
async fn a_stale_path_key_fails_run_but_not_databases_or_drop() {
    let rig = Rig::with("localhost:1729", |_| "path = \"x.hnsw\"".to_string());
    let db = TestDb::new("idxstale");
    setup(&rig, &db.name).await;

    gecko_ok(&rig, None, &["databases"]).await;
    let (ok, text) = gecko(&rig, Some(&db.name), &["run", "playbooks/first"]).await;
    assert!(!ok, "run refuses a stale path key:\n{text}");
    assert!(text.contains("[semantic_index] dir"), "{text}");

    let text = gecko_ok(&rig, None, &["drop", &db.name]).await;
    assert!(
        text.contains("warning"),
        "drop warns about the index:\n{text}"
    );
}
