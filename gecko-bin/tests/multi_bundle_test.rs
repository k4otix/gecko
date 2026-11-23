// Several OKF bundles synced into one database: each sync owns, updates and
// garbage-collects only the concepts its bundle contains, and refuses a concept
// ID another bundle holds. Core + mem schema only, so it runs on every push.
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use futures_util::StreamExt;
use tempfile::TempDir;

use gecko_engine::db::CORE_SCHEMA;
use gecko_engine::db::router::DbError;
use gecko_engine::extension::GeckoExtension;
use gecko_engine::okf::parser::parse_bundle;
use gecko_engine::syncer::bundle::{SyncResult, sync_bundle};
use mem_gecko::MemGecko;

mod common;
use common::TestDb;

/// Bundle `alpha`: a directory concept, its child, and a note linking the child.
const ALPHA: &[(&str, &str, &str)] = &[
    ("tables.md", "Tables", "All tables."),
    ("tables/x.md", "X", "Table x."),
    ("notes/a1.md", "A1", "See [x](/tables/x.md)."),
];

/// Bundle `beta`: a note linking its sibling and alpha's `tables/x`, plus a table.
const BETA: &[(&str, &str, &str)] = &[
    (
        "notes/b1.md",
        "B1",
        "See [b2](/notes/b2.md) and [x](/tables/x.md).",
    ),
    ("notes/b2.md", "B2", "Note b2."),
    ("tables/y.md", "Y", "Table y."),
];

/// An OKF note: `type: Note` frontmatter with `title`, then `body`.
fn note(title: &str, body: &str) -> String {
    format!("---\ntype: Note\ntitle: {title}\n---\n\n{body}\n")
}

/// Writes `bundle.json` named `name` and each `(path, title, body)` as a note.
fn write_bundle(dir: &Path, name: &str, files: &[(&str, &str, &str)]) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("bundle.json"),
        serde_json::json!({ "name": name }).to_string(),
    )
    .unwrap();
    for (path, title, body) in files {
        let file = dir.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, note(title, body)).unwrap();
    }
}

async fn apply_schemas(db: &TestDb) {
    let mut router = db.router();
    router
        .apply_schema(CORE_SCHEMA)
        .await
        .expect("apply core schema");
    router
        .apply_schema(MemGecko::new().schema())
        .await
        .expect("apply mem schema");
}

async fn sync_dir(db: &TestDb, dir: &Path) -> Result<SyncResult, DbError> {
    let bundle = parse_bundle(dir).expect("parse bundle");
    sync_bundle(&mut db.router(), &bundle, &[], mem_gecko::BELIEF_TIER_ROOT).await
}

/// Runs a raw write query and commits it.
async fn raw_write(db: &TestDb, query: &str) {
    let mut router = db.router();
    let tx = router.begin_write().await.expect("begin write");
    tx.query(query).await.expect("raw write");
    tx.commit().await.expect("commit raw write");
}

/// Counts the rows a match query returns.
async fn count(db: &TestDb, query: &str) -> usize {
    let mut router = db.router();
    let tx = router.begin_read().await.expect("begin read");
    tx.query(query)
        .await
        .expect("count query")
        .into_rows()
        .map(|r| {
            r.expect("count row");
        })
        .count()
        .await
}

/// Runs a fetch query and returns each document as JSON.
async fn fetch_docs(db: &TestDb, query: &str) -> Vec<serde_json::Value> {
    let mut router = db.router();
    let tx = router.begin_read().await.expect("begin read");
    let answer = tx.query(query).await.expect("fetch query");
    let mut stream = answer.into_documents();
    let mut out = Vec::new();
    while let Some(doc) = stream.next().await {
        let doc = doc.expect("fetch document");
        out.push(serde_json::to_value(doc.into_json()).expect("document json"));
    }
    out
}

fn str_field(doc: &serde_json::Value, key: &str) -> String {
    doc[key]
        .as_str()
        .unwrap_or_else(|| panic!("document {doc} has no string {key:?}"))
        .to_string()
}

async fn strings(db: &TestDb, query: &str) -> BTreeSet<String> {
    fetch_docs(db, query)
        .await
        .iter()
        .map(|d| str_field(d, "a"))
        .collect()
}

async fn pairs(db: &TestDb, query: &str) -> BTreeSet<(String, String)> {
    fetch_docs(db, query)
        .await
        .iter()
        .map(|d| (str_field(d, "a"), str_field(d, "b")))
        .collect()
}

async fn concept_ids(db: &TestDb) -> BTreeSet<String> {
    strings(
        db,
        r#"match $c isa concept, has concept-id $id; fetch { "a": $id };"#,
    )
    .await
}

/// `(concept-id, bundle-name)` for every containment.
async fn membership(db: &TestDb) -> BTreeSet<(String, String)> {
    pairs(
        db,
        r#"match $ct isa containment, links (container: $b, member: $c);
                 $b has bundle-name $bn; $c has concept-id $id;
           fetch { "a": $id, "b": $bn };"#,
    )
    .await
}

async fn bundle_names(db: &TestDb) -> BTreeSet<String> {
    strings(
        db,
        r#"match $b isa bundle, has bundle-name $bn; fetch { "a": $bn };"#,
    )
    .await
}

/// `(source, target)` concept IDs for every concept-link.
async fn links(db: &TestDb) -> BTreeSet<(String, String)> {
    pairs(
        db,
        r#"match $l isa concept-link, links (source: $s, target: $t);
                 $s has concept-id $sid; $t has concept-id $tid;
           fetch { "a": $sid, "b": $tid };"#,
    )
    .await
}

/// `(parent, child)` concept IDs for every hierarchy edge.
async fn hierarchy(db: &TestDb) -> BTreeSet<(String, String)> {
    pairs(
        db,
        r#"match $h isa hierarchy, links (parent: $p, child: $c);
                 $p has concept-id $pid; $c has concept-id $cid;
           fetch { "a": $pid, "b": $cid };"#,
    )
    .await
}

fn set<const N: usize>(items: [&str; N]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn pair_set<const N: usize>(items: [(&str, &str); N]) -> BTreeSet<(String, String)> {
    items
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

/// A fresh, schema-applied database plus directories holding `ALPHA` and `BETA`.
async fn setup(prefix: &str) -> (TestDb, TempDir, TempDir) {
    let db = TestDb::new(prefix);
    apply_schemas(&db).await;
    let alpha = TempDir::new().unwrap();
    write_bundle(alpha.path(), "alpha", ALPHA);
    let beta = TempDir::new().unwrap();
    write_bundle(beta.path(), "beta", BETA);
    (db, alpha, beta)
}

fn expect_collision(
    result: Result<SyncResult, DbError>,
) -> (String, Vec<(String, Option<String>)>, String) {
    match result {
        Err(e @ DbError::ConceptIdCollision { .. }) => {
            let msg = e.to_string();
            let DbError::ConceptIdCollision { bundle, ids } = e else {
                unreachable!()
            };
            (bundle, ids, msg)
        }
        other => panic!("expected ConceptIdCollision, got {other:?}"),
    }
}

#[tokio::test]
async fn two_bundles_share_a_database() {
    let (db, alpha, beta) = setup("gecko_test_mb_share").await;

    let a = sync_dir(&db, alpha.path()).await.expect("sync alpha");
    assert_eq!(a.concepts_inserted, 3);
    let b = sync_dir(&db, beta.path()).await.expect("sync beta");
    assert_eq!(b.concepts_inserted, 3);
    assert_eq!(b.concepts_deleted, 0, "beta must not GC alpha's concepts");

    let expected = pair_set([
        ("tables", "alpha"),
        ("tables/x", "alpha"),
        ("notes/a1", "alpha"),
        ("notes/b1", "beta"),
        ("notes/b2", "beta"),
        ("tables/y", "beta"),
    ]);
    assert_eq!(membership(&db).await, expected);

    let links_before = links(&db).await;
    let hierarchy_before = hierarchy(&db).await;
    let hierarchy_count = count(&db, "match $h isa hierarchy;").await;

    let again = sync_dir(&db, alpha.path()).await.expect("re-sync alpha");
    assert_eq!(again.concepts_skipped, 3);
    assert_eq!(again.concepts_deleted, 0);
    assert_eq!(membership(&db).await, expected);
    assert_eq!(links(&db).await, links_before);
    assert_eq!(hierarchy(&db).await, hierarchy_before);

    assert_eq!(
        count(&db, "match $r isa containment;").await,
        6,
        "one containment per concept"
    );
    assert_eq!(count(&db, "match $h isa hierarchy;").await, hierarchy_count);
}

#[tokio::test]
async fn resync_garbage_collects_only_its_own_bundle() {
    let (db, alpha, beta) = setup("gecko_test_mb_gc").await;
    sync_dir(&db, alpha.path()).await.expect("sync alpha");
    sync_dir(&db, beta.path()).await.expect("sync beta");
    let beta_rows: BTreeSet<_> = membership(&db)
        .await
        .into_iter()
        .filter(|(_, b)| b == "beta")
        .collect();

    fs::remove_file(alpha.path().join("notes/a1.md")).unwrap();
    let r = sync_dir(&db, alpha.path()).await.expect("re-sync alpha");
    assert_eq!(r.concepts_deleted, 1);

    assert_eq!(
        concept_ids(&db).await,
        set(["tables", "tables/x", "notes/b1", "notes/b2", "tables/y"])
    );
    let beta_after: BTreeSet<_> = membership(&db)
        .await
        .into_iter()
        .filter(|(_, b)| b == "beta")
        .collect();
    assert_eq!(beta_after, beta_rows);
}

#[tokio::test]
async fn colliding_concept_id_is_refused() {
    let (db, alpha, _) = setup("gecko_test_mb_collide").await;
    sync_dir(&db, alpha.path()).await.expect("sync alpha");

    let ids_before = concept_ids(&db).await;
    let membership_before = membership(&db).await;
    let links_before = links(&db).await;
    let hierarchy_before = hierarchy(&db).await;
    let bundles_before = bundle_names(&db).await;

    let beta = TempDir::new().unwrap();
    let mut files = BETA.to_vec();
    files.push(("tables/x.md", "X", "Beta's own x."));
    write_bundle(beta.path(), "beta", &files);

    let (bundle, ids, _) = expect_collision(sync_dir(&db, beta.path()).await);
    assert_eq!(bundle, "beta");
    assert_eq!(ids, vec![("tables/x".into(), Some("alpha".into()))]);

    assert_eq!(concept_ids(&db).await, ids_before);
    assert_eq!(membership(&db).await, membership_before);
    assert_eq!(links(&db).await, links_before);
    assert_eq!(hierarchy(&db).await, hierarchy_before);
    assert_eq!(bundle_names(&db).await, bundles_before);
    assert_eq!(bundle_names(&db).await, set(["alpha"]));
}

#[tokio::test]
async fn uncontained_concept_counts_as_a_collision() {
    let (db, _, beta) = setup("gecko_test_mb_uncontained").await;
    raw_write(&db, r#"insert $c isa concept, has concept-id "notes/b1";"#).await;

    let (_, ids, _) = expect_collision(sync_dir(&db, beta.path()).await);
    assert_eq!(ids, vec![("notes/b1".into(), None)]);
    assert_eq!(concept_ids(&db).await, set(["notes/b1"]));
}

#[tokio::test]
async fn renamed_bundle_is_refused_with_guidance() {
    let (db, alpha, _) = setup("gecko_test_mb_rename").await;
    sync_dir(&db, alpha.path()).await.expect("sync alpha");

    fs::write(
        alpha.path().join("bundle.json"),
        serde_json::json!({ "name": "atlas" }).to_string(),
    )
    .unwrap();
    let (bundle, ids, msg) = expect_collision(sync_dir(&db, alpha.path()).await);
    assert_eq!(bundle, "atlas");
    assert_eq!(ids.len(), 3);
    assert!(
        ids.iter()
            .all(|(_, holder)| holder.as_deref() == Some("alpha")),
        "{ids:?}"
    );
    assert!(msg.contains("alpha"), "{msg}");
    assert!(msg.contains("fresh database"), "{msg}");
    assert_eq!(bundle_names(&db).await, set(["alpha"]));
}

#[tokio::test]
async fn empty_bundle_deletes_nothing_from_others() {
    let (db, alpha, beta) = setup("gecko_test_mb_empty").await;
    sync_dir(&db, alpha.path()).await.expect("sync alpha");
    sync_dir(&db, beta.path()).await.expect("sync beta");
    let ids_before = concept_ids(&db).await;

    let gamma = TempDir::new().unwrap();
    write_bundle(gamma.path(), "gamma", &[]);
    let r = sync_dir(&db, gamma.path()).await.expect("sync gamma");
    assert_eq!(r.concepts_inserted, 0);
    assert_eq!(r.concepts_deleted, 0);
    assert_eq!(concept_ids(&db).await, ids_before);
    assert_eq!(bundle_names(&db).await, set(["alpha", "beta", "gamma"]));
}

#[tokio::test]
async fn shared_legacy_concept_is_released_not_deleted() {
    let (db, alpha, _) = setup("gecko_test_mb_legacy").await;
    raw_write(
        &db,
        r#"insert
             $a isa bundle, has bundle-name "alpha";
             $b isa bundle, has bundle-name "beta";
             $s isa concept, has concept-id "notes/shared";
             $o isa concept, has concept-id "notes/beta-only";
             $sa isa containment, links (container: $a, member: $s);
             $sb isa containment, links (container: $b, member: $s);
             $ob isa containment, links (container: $b, member: $o);"#,
    )
    .await;

    let r = sync_dir(&db, alpha.path()).await.expect("sync alpha");
    assert_eq!(r.concepts_deleted, 0);
    assert_eq!(r.concepts_released, 1);

    let ids = concept_ids(&db).await;
    assert!(ids.contains("notes/shared"), "{ids:?}");
    assert!(ids.contains("notes/beta-only"), "{ids:?}");
    let members = membership(&db).await;
    assert!(members.contains(&("notes/shared".into(), "beta".into())));
    assert!(!members.contains(&("notes/shared".into(), "alpha".into())));

    let beta = TempDir::new().unwrap();
    write_bundle(beta.path(), "beta", &[]);
    let r = sync_dir(&db, beta.path()).await.expect("sync empty beta");
    assert_eq!(r.concepts_deleted, 2, "beta owns both alone now");
    assert_eq!(
        concept_ids(&db).await,
        set(["tables", "tables/x", "notes/a1"])
    );
}

fn expected_links() -> BTreeSet<(String, String)> {
    pair_set([("notes/a1", "tables/x"), ("notes/b1", "notes/b2")])
}

#[tokio::test]
async fn link_does_not_cross_bundles() {
    let (db, alpha, beta) = setup("gecko_test_mb_link").await;
    sync_dir(&db, alpha.path()).await.expect("sync alpha");
    sync_dir(&db, beta.path()).await.expect("sync beta");

    assert_eq!(links(&db).await, expected_links());
    assert_eq!(hierarchy(&db).await, pair_set([("tables", "tables/x")]));
}

#[tokio::test]
async fn link_does_not_cross_bundles_in_either_order() {
    let (db, alpha, beta) = setup("gecko_test_mb_link_rev").await;
    sync_dir(&db, beta.path()).await.expect("sync beta");
    sync_dir(&db, alpha.path()).await.expect("sync alpha");
    sync_dir(&db, beta.path()).await.expect("re-sync beta");

    assert_eq!(links(&db).await, expected_links());
    assert_eq!(hierarchy(&db).await, pair_set([("tables", "tables/x")]));
}

#[tokio::test]
async fn updated_concept_relinks_within_its_bundle() {
    let (db, alpha, beta) = setup("gecko_test_mb_relink").await;
    sync_dir(&db, alpha.path()).await.expect("sync alpha");
    sync_dir(&db, beta.path()).await.expect("sync beta");

    fs::write(
        alpha.path().join("notes/a1.md"),
        note("A1 renamed", "See [x](/tables/x.md)."),
    )
    .unwrap();
    let r = sync_dir(&db, alpha.path()).await.expect("re-sync alpha");
    assert_eq!(r.concepts_updated, 1);

    assert_eq!(links(&db).await, expected_links());
    assert_eq!(count(&db, "match $l isa concept-link;").await, 2);
    assert_eq!(hierarchy(&db).await, pair_set([("tables", "tables/x")]));
}

/// `gecko sync-stix` driven through the binary, one STIX file per bundle name.
#[cfg(feature = "cyber")]
mod cli {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    const MITRE_IDENTITY: &str = r#"{
      "type": "identity",
      "id": "identity--c78cb6e5-0c4b-4611-8297-d1b8b55e40b5",
      "name": "The MITRE Corporation"
    }"#;

    /// A STIX bundle JSON document holding `objects` (comma-joined JSON objects).
    fn stix(objects: &[&str]) -> String {
        format!(r#"{{"type":"bundle","objects":[{}]}}"#, objects.join(","))
    }

    const T1059: &str = r#"{
      "type": "attack-pattern",
      "id": "attack-pattern--00000000-0000-0000-0000-0000000000a1",
      "name": "Command and Scripting Interpreter",
      "external_references": [{ "source_name": "mitre-attack", "external_id": "T1059" }]
    }"#;
    const DATA_SOURCE: &str = r#"{
      "type": "x-mitre-data-source",
      "id": "x-mitre-data-source--00000000-0000-0000-0000-0000000000e5",
      "name": "Command",
      "external_references": [{ "source_name": "mitre-attack", "external_id": "DS0017" }]
    }"#;
    const DATA_COMPONENT: &str = r#"{
      "type": "x-mitre-data-component",
      "id": "x-mitre-data-component--00000000-0000-0000-0000-0000000000f6",
      "name": "Command Execution",
      "x_mitre_data_source_ref": "x-mitre-data-source--00000000-0000-0000-0000-0000000000e5"
    }"#;
    const DETECTS: &str = r#"{
      "type": "relationship",
      "id": "relationship--00000000-0000-0000-0000-000000000103",
      "relationship_type": "detects",
      "source_ref": "x-mitre-data-component--00000000-0000-0000-0000-0000000000f6",
      "target_ref": "attack-pattern--00000000-0000-0000-0000-0000000000a1"
    }"#;
    const T1404: &str = r#"{
      "type": "attack-pattern",
      "id": "attack-pattern--00000000-0000-0000-0000-0000000000b1",
      "name": "Exploit OS Vulnerability",
      "external_references": [{ "source_name": "mitre-attack", "external_id": "T1404" }]
    }"#;
    const T1003: &str = r#"{
      "type": "attack-pattern",
      "id": "attack-pattern--00000000-0000-0000-0000-0000000000c1",
      "name": "OS Credential Dumping",
      "external_references": [{ "source_name": "mitre-attack", "external_id": "T1003" }]
    }"#;
    const MALWARE: &str = r#"{
      "type": "malware",
      "id": "malware--00000000-0000-0000-0000-0000000000c2",
      "name": "Dumper",
      "is_family": true
    }"#;

    const DATA_SOURCE_ID: &str = "stix/x-mitre-data-source/00000000-0000-0000-0000-0000000000e5";
    const IDENTITY_ID: &str = "stix/identity/c78cb6e5-0c4b-4611-8297-d1b8b55e40b5";

    fn enterprise() -> String {
        stix(&[MITRE_IDENTITY, T1059, DATA_SOURCE, DATA_COMPONENT, DETECTS])
    }

    /// `ENTERPRISE` without its data source; the data component still references it.
    fn enterprise_next() -> String {
        stix(&[MITRE_IDENTITY, T1059, DATA_COMPONENT, DETECTS])
    }

    fn mobile() -> String {
        stix(&[MITRE_IDENTITY, T1404])
    }

    fn other() -> String {
        stix(&[T1003, MALWARE])
    }

    /// Writes a `gecko.toml` that targets the local TypeDB with the cyber extension
    /// enabled and no semantic index.
    fn write_config(dir: &Path) -> PathBuf {
        let path = dir.join("gecko.toml");
        fs::write(
            &path,
            "[typedb]\nendpoint = \"localhost:1729\"\nmode = \"external\"\n\n[extensions]\nenabled = [\"cyber\"]\n",
        )
        .unwrap();
        path
    }

    fn gecko(config: &Path) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_gecko"));
        cmd.arg("--config").arg(config);
        cmd
    }

    fn run_ok(cmd: &mut Command) -> String {
        let out = cmd.output().expect("spawn gecko");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "expected success\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        stdout
    }

    /// Asserts failure and returns stderr, where the tracing `error!` line is written.
    fn run_err(cmd: &mut Command) -> String {
        let out = cmd.output().expect("spawn gecko");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            !out.status.success(),
            "expected failure\n--- stdout ---\n{}\n--- stderr ---\n{stderr}",
            String::from_utf8_lossy(&out.stdout)
        );
        stderr
    }

    fn write_stix(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    /// Concept IDs contained by the bundle named `bundle`.
    fn members_of(rows: &BTreeSet<(String, String)>, bundle: &str) -> BTreeSet<String> {
        rows.iter()
            .filter(|(_, b)| b == bundle)
            .map(|(id, _)| id.clone())
            .collect()
    }

    #[tokio::test]
    async fn sync_stix_bundle_name_override() {
        let db = TestDb::new("gecko_test_stix_override");
        let dir = TempDir::new().unwrap();
        let config = write_config(dir.path());
        let enterprise_file = write_stix(dir.path(), "enterprise.json", &enterprise());
        let next_file = write_stix(dir.path(), "enterprise-next.json", &enterprise_next());
        let other_file = write_stix(dir.path(), "other.json", &other());

        run_ok(
            gecko(&config)
                .args(["--database", &db.name, "sync-stix"])
                .arg(&enterprise_file),
        );
        assert_eq!(
            bundle_names(&db).await,
            BTreeSet::from(["stix-sync".to_string()])
        );
        let enterprise_ids = concept_ids(&db).await;

        run_ok(
            gecko(&config)
                .args(["--database", &db.name, "sync-stix"])
                .arg(&other_file)
                .args(["--bundle-name", "other-intel"]),
        );
        assert_eq!(
            bundle_names(&db).await,
            BTreeSet::from(["stix-sync".to_string(), "other-intel".to_string()])
        );
        let rows = membership(&db).await;
        let other_ids = BTreeSet::from([
            "stix/attack-pattern/00000000-0000-0000-0000-0000000000c1".to_string(),
            "stix/malware/00000000-0000-0000-0000-0000000000c2".to_string(),
        ]);
        assert_eq!(members_of(&rows, "other-intel"), other_ids);
        assert_eq!(members_of(&rows, "stix-sync"), enterprise_ids);

        let paths = strings(
            &db,
            r#"match $b isa bundle, has bundle-name "other-intel", has bundle-path $p;
               fetch { "a": $p };"#,
        )
        .await;
        let expected = fs::canonicalize(&other_file)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(paths, BTreeSet::from([expected]));

        run_ok(
            gecko(&config)
                .args(["--database", &db.name, "sync-stix"])
                .arg(&next_file),
        );
        let after = membership(&db).await;
        assert!(!after.iter().any(|(id, _)| id == DATA_SOURCE_ID));
        let kept = members_of(&after, "stix-sync");
        assert_eq!(
            kept,
            enterprise_ids
                .iter()
                .filter(|id| id.as_str() != DATA_SOURCE_ID)
                .cloned()
                .collect::<BTreeSet<_>>()
        );
        assert_eq!(members_of(&after, "other-intel"), other_ids);
    }

    #[tokio::test]
    async fn second_attack_domain_is_refused() {
        let db = TestDb::new("gecko_test_stix_refuse");
        let dir = TempDir::new().unwrap();
        let config = write_config(dir.path());
        let enterprise_file = write_stix(dir.path(), "enterprise.json", &enterprise());
        let mobile_file = write_stix(dir.path(), "mobile.json", &mobile());

        run_ok(
            gecko(&config)
                .args(["--database", &db.name, "sync-stix"])
                .arg(&enterprise_file)
                .args(["--bundle-name", "attack-enterprise"]),
        );
        let before = concept_ids(&db).await;

        let output = run_err(
            gecko(&config)
                .args(["--database", &db.name, "sync-stix"])
                .arg(&mobile_file)
                .args(["--bundle-name", "attack-mobile"]),
        );
        assert!(output.contains(IDENTITY_ID), "{output}");
        assert!(output.contains("attack-enterprise"), "{output}");
        assert_eq!(
            bundle_names(&db).await,
            BTreeSet::from(["attack-enterprise".to_string()])
        );
        assert_eq!(concept_ids(&db).await, before);
    }

    #[tokio::test]
    async fn bundle_name_override_selects_the_default_database() {
        let target = TestDb::new("gecko_test_stix_name");
        let dir = TempDir::new().unwrap();
        let config = write_config(dir.path());
        let other_file = write_stix(dir.path(), "other.json", &other());

        run_ok(
            gecko(&config)
                .arg("sync-stix")
                .arg(&other_file)
                .args(["--bundle-name", &target.name]),
        );
        assert_eq!(
            bundle_names(&target).await,
            BTreeSet::from([target.name.clone()])
        );
    }

    /// A playbook that records a detection claim for T1059 through the cyber bridge.
    const DETECT_PLAYBOOK: &str = r#"---
type: playbook
title: Detect T1059
engine: quickjs
scopes: [cyber-gecko:claim]
---

# Detect T1059

```js
gecko.callExtension("cyber-gecko", "claim", { logic_id: "playbooks/detect-t1059", technique_id: "T1059", method: "external-tool", confidence: 0.9 });
```
"#;

    #[tokio::test]
    async fn playbooks_and_attack_share_a_database() {
        let db = TestDb::new("gecko_test_playbooks_attack");
        let dir = TempDir::new().unwrap();
        let config = write_config(dir.path());
        let playbooks = dir.path().join("playbooks-bundle");
        fs::create_dir_all(playbooks.join("playbooks")).unwrap();
        fs::write(
            playbooks.join("bundle.json"),
            serde_json::json!({ "name": "detect-playbooks" }).to_string(),
        )
        .unwrap();
        fs::write(playbooks.join("playbooks/detect-t1059.md"), DETECT_PLAYBOOK).unwrap();
        let enterprise_file = write_stix(dir.path(), "enterprise.json", &enterprise());

        // The playbooks sync first, so the STIX sync must leave them alone.
        run_ok(
            gecko(&config)
                .args(["--database", &db.name, "sync"])
                .arg(&playbooks),
        );
        run_ok(
            gecko(&config)
                .args(["--database", &db.name, "sync-stix"])
                .arg(&enterprise_file),
        );
        let stdout =
            run_ok(gecko(&config).args(["--database", &db.name, "run", "playbooks/detect-t1059"]));
        assert!(stdout.contains("Execution successful!"), "{stdout}");

        assert_eq!(count(&db, "match $d isa technique-detection;").await, 1);
        assert_eq!(
            bundle_names(&db).await,
            BTreeSet::from(["detect-playbooks".to_string(), "stix-sync".to_string()])
        );
        let rows = membership(&db).await;
        for pair in [
            ("playbooks/detect-t1059", "detect-playbooks"),
            (
                "stix/attack-pattern/00000000-0000-0000-0000-0000000000a1",
                "stix-sync",
            ),
        ] {
            assert!(
                rows.contains(&(pair.0.to_string(), pair.1.to_string())),
                "missing {pair:?} in {rows:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_changed_type_hint_on_a_shared_concept_is_refused() {
    let db = TestDb::new("gecko_test_mb_retype_shared");
    apply_schemas(&db).await;
    db.router()
        .apply_schema(
            "define entity retype-probe sub concept @doc(\"A test-only concept subtype.\");",
        )
        .await
        .expect("apply the retype-probe schema");
    let alpha = TempDir::new().unwrap();
    write_bundle(alpha.path(), "alpha", &[("a.md", "A", "Note a.")]);
    let beta = TempDir::new().unwrap();
    write_bundle(beta.path(), "beta", &[("a.md", "A", "Note a.")]);
    sync_dir(&db, alpha.path()).await.expect("sync alpha");
    raw_write(
        &db,
        r#"match $c isa concept, has concept-id "a";
           insert $b isa bundle, has bundle-name "beta";
                  $ct isa containment, links (container: $b, member: $c);"#,
    )
    .await;
    sync_dir(&db, beta.path()).await.expect("sync beta");
    let shared = pair_set([("a", "alpha"), ("a", "beta")]);
    assert_eq!(membership(&db).await, shared);

    let mut retyped = parse_bundle(alpha.path()).expect("parse alpha");
    retyped.concepts[0].type_hint = Some("retype-probe".into());
    let result = sync_bundle(&mut db.router(), &retyped, &[], mem_gecko::BELIEF_TIER_ROOT).await;
    let Err(e @ DbError::TypeChangeShared { .. }) = result else {
        panic!("expected TypeChangeShared, got {result:?}");
    };
    let message = e.to_string();
    assert!(message.contains("`a`"), "{message}");
    let DbError::TypeChangeShared { ids } = e else {
        unreachable!()
    };
    assert_eq!(
        ids,
        vec![(
            "a".to_string(),
            "concept".to_string(),
            "retype-probe".to_string()
        )]
    );

    assert_eq!(
        count(&db, r#"match $c isa! concept, has concept-id "a";"#).await,
        1,
        "`a` keeps its type"
    );
    assert_eq!(membership(&db).await, shared);
    sync_dir(&db, beta.path())
        .await
        .expect("beta's next sync still succeeds");
}

#[tokio::test]
async fn garbage_collecting_a_cross_bundle_link_target_commits() {
    let (db, alpha, beta) = setup("gecko_test_mb_gc_cross_link").await;
    sync_dir(&db, alpha.path()).await.expect("sync alpha");
    sync_dir(&db, beta.path()).await.expect("sync beta");
    // Relations from beta's concepts to an alpha concept, as an extension or an
    // earlier sync may have written them.
    raw_write(
        &db,
        r#"match $a isa concept, has concept-id "notes/a1";
                 $b1 isa concept, has concept-id "notes/b1";
                 $b2 isa concept, has concept-id "notes/b2";
           insert concept-link (source: $b1, target: $a);
                  hierarchy (parent: $b2, child: $a);"#,
    )
    .await;

    fs::remove_file(alpha.path().join("notes/a1.md")).unwrap();
    let r = sync_dir(&db, alpha.path())
        .await
        .expect("garbage-collecting a cross-bundle link target commits");
    assert_eq!(r.concepts_deleted, 1);
    assert_eq!(links(&db).await, pair_set([("notes/b1", "notes/b2")]));
    assert_eq!(hierarchy(&db).await, pair_set([("tables", "tables/x")]));
    assert!(concept_ids(&db).await.contains("notes/b1"));
}
