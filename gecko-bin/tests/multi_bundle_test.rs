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
    sync_bundle(&mut db.router(), &bundle, &[]).await
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
    let rows = tx
        .query(query)
        .await
        .expect("count query")
        .into_rows()
        .collect::<Vec<_>>()
        .await;
    rows.into_iter()
        .map(|r| r.expect("count row"))
        .collect::<Vec<_>>()
        .len()
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
}
