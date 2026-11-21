// Exercises bundle-sync insert/skip/update/delete-citation coverage against
// core+mem schema only — no cyber-specific schema or types are touched, so this
// test runs on every push, including the candle-free Tier-2 command
// (`cargo test --test '*' --no-default-features`).
use futures_util::StreamExt;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[cfg(feature = "cyber")]
use cyber_gecko::CyberGecko;
use gecko_engine::db::router::{DbError, TypeDbRouter};
use gecko_engine::db::run_graph_writes;
use gecko_engine::extension::GeckoExtension;
use gecko_engine::okf::parser::parse_bundle;
use gecko_engine::okf::types::{HierarchyEdge, OkfBundle, OkfLink, TypedAttrValue, TypedAttribute};
use gecko_engine::syncer::bundle::sync_bundle;
use gecko_extension_api::GraphWrite;
use mem_gecko::{BELIEF_TIER_ROOT, MemGecko};

#[path = "common/belief.rs"]
mod belief;
mod common;
use belief::claim_with_belief;
use common::TestDb;

fn create_test_bundle(dir: &std::path::Path) {
    let concept_dir = dir.join("playbooks");
    fs::create_dir_all(&concept_dir).unwrap();
    fs::write(
        concept_dir.join("test_concept.md"),
        r#"---
type: Playbook
title: Test Concept
description: A concept to test syncing
resource: db://test
tags: [test, sync]
---

This is a test concept. It links to [another](https://example.com).
"#,
    )
    .unwrap();
}

#[tokio::test]
async fn test_bundle_sync() {
    // Self-cleaning database: dropped from the server when `test_db` goes out of
    // scope (declared first so it drops last, after `db` closes).
    let test_db = TestDb::new("gecko_test_sync");
    let mut db = test_db.router();

    let core_schema = include_str!("../../core/gecko-engine/schema/core_schema.tql");
    db.apply_schema(core_schema)
        .await
        .expect("Failed to apply core schema");

    #[allow(unused_mut)]
    let mut extensions: Vec<Box<dyn GeckoExtension>> = vec![Box::new(MemGecko::new())];
    #[cfg(feature = "cyber")]
    extensions.push(Box::new(CyberGecko::new()));

    for ext in extensions {
        let schema = ext.schema();
        if !schema.trim().is_empty() {
            db.apply_schema(schema)
                .await
                .unwrap_or_else(|e| panic!("Failed to apply {} schema: {}", ext.name(), e));
        }
    }

    let temp_dir = TempDir::new().unwrap();
    create_test_bundle(temp_dir.path());

    let bundle = parse_bundle(temp_dir.path()).expect("Failed to parse bundle");

    let result = sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT)
        .await
        .expect("Failed to sync bundle");
    assert_eq!(result.concepts_inserted, 1);
    assert_eq!(result.citations_attempted, 1);
    // Concept IDs are bundle-relative paths with no namespace prefix.
    assert_eq!(bundle.concepts[0].concept_id, "playbooks/test_concept");

    // Re-syncing an unchanged bundle must be idempotent: the concept's
    // content hash matches, so it is skipped rather than re-inserted.
    let result2 = sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT)
        .await
        .expect("Failed to re-sync bundle");
    assert_eq!(
        result2.concepts_inserted, 0,
        "re-sync should insert nothing"
    );
    assert_eq!(
        result2.concepts_skipped, 1,
        "unchanged concept should be skipped"
    );
    assert_eq!(result2.concepts_deleted, 0, "nothing should be GC'd");

    // Editing the concept changes its content hash, exercising the in-place
    // update path. The new body drops the external link, so its citation should
    // be cleared.
    fs::write(
        temp_dir.path().join("playbooks/test_concept.md"),
        r#"---
type: Playbook
title: Test Concept Revised
description: An edited concept
tags: [test, sync]
---

The body has changed.
"#,
    )
    .unwrap();
    let edited = parse_bundle(temp_dir.path()).expect("Failed to re-parse bundle");
    let result3 = sync_bundle(&mut db, &edited, &[], BELIEF_TIER_ROOT)
        .await
        .expect("Failed to sync edited bundle");
    assert_eq!(result3.concepts_updated, 1, "edited concept should update");
    assert_eq!(result3.concepts_inserted, 0);
    assert_eq!(result3.concepts_skipped, 0);

    // Verify the edit landed and there is still exactly one concept.
    let tx = db.begin_read().await.expect("Failed to begin read");
    let answer = tx
        .query(r#"match $c isa concept, has title "Test Concept Revised";"#)
        .await
        .expect("query failed");
    let rows: Vec<_> = answer.into_rows().collect::<Vec<_>>().await;
    assert_eq!(rows.len(), 1, "Edited concept not found in database");

    // In-place update must not orphan or duplicate relations: the single
    // containment survives, and the now-linkless body clears the citation.
    for (typeql, expected, msg) in [
        (
            "match $c isa concept;",
            1,
            "update must not duplicate the concept",
        ),
        (
            "match $r isa containment;",
            1,
            "containment must survive an update, not orphan or duplicate",
        ),
        (
            "match $r isa citation;",
            0,
            "citation should be cleared when the edited body drops its link",
        ),
    ] {
        let tx = db.begin_read().await.expect("Failed to begin read");
        let n = tx
            .query(typeql)
            .await
            .expect("count query failed")
            .into_rows()
            .collect::<Vec<_>>()
            .await
            .len();
        assert_eq!(n, expected, "{msg}");
    }
}

#[tokio::test]
async fn failing_extension_write_rolls_back_the_whole_sync() {
    let test_db = TestDb::new("gecko_test_sync_rollback");
    let mut db = test_db.router();

    let core_schema = include_str!("../../core/gecko-engine/schema/core_schema.tql");
    db.apply_schema(core_schema)
        .await
        .expect("Failed to apply core schema");
    db.apply_schema(MemGecko::new().schema())
        .await
        .expect("Failed to apply mem schema");

    let temp_dir = TempDir::new().unwrap();
    create_test_bundle(temp_dir.path());
    let bundle = parse_bundle(temp_dir.path()).expect("Failed to parse bundle");

    let bad = [GraphWrite::plain("insert $x isa no-such-type;")];
    let result = sync_bundle(&mut db, &bundle, &bad, BELIEF_TIER_ROOT).await;
    assert!(
        result.is_err(),
        "a failing extension write must fail the sync"
    );

    for typeql in ["match $c isa concept;", "match $b isa bundle;"] {
        let tx = db.begin_read().await.expect("Failed to begin read");
        let n = tx
            .query(typeql)
            .await
            .expect("count query failed")
            .into_rows()
            .collect::<Vec<_>>()
            .await
            .len();
        assert_eq!(n, 0, "rolled-back sync must leave nothing for: {typeql}");
    }
}

/// Applies the core schema and, when `with_mem`, mem's schema.
async fn apply_core(db: &mut TypeDbRouter, with_mem: bool) {
    let core_schema = include_str!("../../core/gecko-engine/schema/core_schema.tql");
    db.apply_schema(core_schema)
        .await
        .expect("Failed to apply core schema");
    if with_mem {
        db.apply_schema(MemGecko::new().schema())
            .await
            .expect("Failed to apply mem schema");
    }
}

/// The number of answers `typeql` matches in a fresh read transaction.
async fn count(db: &mut TypeDbRouter, typeql: &str) -> usize {
    let tx = db.begin_read().await.expect("Failed to begin read");
    tx.query(typeql)
        .await
        .expect("count query failed")
        .into_rows()
        .collect::<Vec<_>>()
        .await
        .len()
}

/// The stored `file-hash` of the database's single concept.
async fn sole_file_hash(db: &mut TypeDbRouter) -> String {
    let tx = db.begin_read().await.expect("Failed to begin read");
    let rows: Vec<_> = tx
        .query("match $c isa concept, has file-hash $h; select $h;")
        .await
        .expect("file-hash query failed")
        .into_rows()
        .collect::<Vec<_>>()
        .await;
    assert_eq!(rows.len(), 1, "expected exactly one stored file-hash");
    let row = rows.into_iter().next().unwrap().expect("file-hash row");
    row.get("h")
        .expect("column h")
        .expect("h bound")
        .try_get_string()
        .expect("h is a string")
        .to_string()
}

#[tokio::test]
async fn extension_write_into_the_belief_tier_is_refused() {
    let test_db = TestDb::new("gecko_test_sync_belief_write");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;

    let temp_dir = TempDir::new().unwrap();
    create_test_bundle(temp_dir.path());
    let bundle = parse_bundle(temp_dir.path()).expect("Failed to parse bundle");

    let raw = GraphWrite::single(
        "given $id: string, $st: string; \
         insert $b isa belief, has concept-id == $id, has belief-state == $st;",
        vec!["id".into(), "st".into()],
        vec!["mem/bel/raw".into(), "asserted".into()],
    )
    .expecting(1);
    let result = sync_bundle(&mut db, &bundle, &[raw], BELIEF_TIER_ROOT).await;
    let Err(DbError::BeliefTierWrite { changes }) = result else {
        panic!("expected BeliefTierWrite, got {result:?}");
    };
    let labels: HashSet<&str> = changes.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, HashSet::from(["memory-item", "belief"]));

    assert_eq!(
        count(&mut db, "match $c isa concept;").await,
        0,
        "the refused sync must roll back entirely"
    );
}

#[tokio::test]
async fn sync_without_the_belief_tier_root_is_refused() {
    let test_db = TestDb::new("gecko_test_sync_no_root");
    let mut db = test_db.router();
    apply_core(&mut db, false).await;

    let temp_dir = TempDir::new().unwrap();
    create_test_bundle(temp_dir.path());
    let bundle = parse_bundle(temp_dir.path()).expect("Failed to parse bundle");

    let result = sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT).await;
    let Err(DbError::Schema(m)) = result else {
        panic!("expected a schema error, got {result:?}");
    };
    assert!(m.contains("memory-item"), "{m}");
    assert_eq!(count(&mut db, "match $c isa concept;").await, 0);
    assert_eq!(count(&mut db, "match $b isa bundle;").await, 0);
}

#[tokio::test]
async fn unknown_type_hint_is_refused_as_a_schema_error() {
    // Insert path: a new concept with an unknown type_hint.
    {
        let test_db = TestDb::new("gecko_test_sync_hint_insert");
        let mut db = test_db.router();
        apply_core(&mut db, true).await;

        let temp_dir = TempDir::new().unwrap();
        create_test_bundle(temp_dir.path());
        let mut bundle = parse_bundle(temp_dir.path()).expect("Failed to parse bundle");
        bundle.concepts[0].type_hint = Some("no-such-type".into());

        let result = sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT).await;
        let Err(DbError::Schema(m)) = result else {
            panic!("expected a schema error, got {result:?}");
        };
        assert!(m.contains("no-such-type"), "{m}");
        assert_eq!(count(&mut db, "match $c isa concept;").await, 0);
    }

    // Update path: a changed concept whose typed attribute names an unknown type.
    let test_db = TestDb::new("gecko_test_sync_hint_update");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;

    let temp_dir = TempDir::new().unwrap();
    create_test_bundle(temp_dir.path());
    let bundle = parse_bundle(temp_dir.path()).expect("Failed to parse bundle");
    sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT)
        .await
        .expect("Failed to sync bundle");
    let stored_hash = sole_file_hash(&mut db).await;

    let mut changed = bundle.clone();
    changed.concepts[0].file_hash = format!("{stored_hash}-changed");
    changed.concepts[0].type_hint = Some("no-such-type".into());
    changed.concepts[0].typed_attributes = vec![TypedAttribute {
        name: "title".into(),
        value: TypedAttrValue::String("t".into()),
    }];
    let result = sync_bundle(&mut db, &changed, &[], BELIEF_TIER_ROOT).await;
    let Err(DbError::Schema(m)) = result else {
        panic!("expected a schema error, got {result:?}");
    };
    assert!(m.contains("no-such-type"), "{m}");
    assert_eq!(sole_file_hash(&mut db).await, stored_hash);

    // Update path: a changed concept with no typed attribute whose type_hint is
    // unknown, or names a type outside `concept`.
    for hint in ["no-such-type", "bundle"] {
        let mut changed = bundle.clone();
        changed.concepts[0].file_hash = format!("{stored_hash}-changed");
        changed.concepts[0].type_hint = Some(hint.into());
        assert!(changed.concepts[0].typed_attributes.is_empty());
        let result = sync_bundle(&mut db, &changed, &[], BELIEF_TIER_ROOT).await;
        let Err(DbError::Schema(m)) = result else {
            panic!("expected a schema error for type_hint {hint:?}, got {result:?}");
        };
        assert!(
            m.contains(&format!("type_hint {hint:?} is not a subtype of `concept`")),
            "{m}"
        );
        assert_eq!(sole_file_hash(&mut db).await, stored_hash);
    }
}

/// The concept the two-concept bundle of [`synced_with_referenced`] drops.
const REFERENCED: &str = "playbooks/test_concept";

/// Applies the core and mem schemas, syncs a two-concept bundle whose second
/// concept links to (and is the hierarchy parent of) the first, and gives the
/// first a belief with a `source-link` to it. Returns the full bundle, the
/// referenced concept's ID, and the bundle without that concept and its edges.
async fn synced_with_referenced(
    db: &mut TypeDbRouter,
    dir: &Path,
) -> (OkfBundle, String, OkfBundle) {
    apply_core(db, true).await;
    create_test_bundle(dir);
    fs::write(
        dir.join("playbooks/second.md"),
        "---\ntype: Playbook\ntitle: Second\n---\n\nSee [the first](test_concept.md).\n",
    )
    .unwrap();
    let mut full = parse_bundle(dir).expect("Failed to parse bundle");
    let referenced = REFERENCED.to_string();
    let survivor = full
        .concepts
        .iter()
        .find(|c| c.concept_id != referenced)
        .expect("a second concept")
        .concept_id
        .clone();
    full.hierarchy.push(HierarchyEdge {
        parent_id: survivor.clone(),
        child_id: referenced.clone(),
    });
    let r = sync_bundle(db, &full, &[], BELIEF_TIER_ROOT)
        .await
        .expect("Failed to sync bundle");
    assert_eq!(r.concepts_inserted, 2);
    assert_eq!(
        count(
            db,
            &format!(
                r#"match $s isa concept, has concept-id "{survivor}";
                         $t isa concept, has concept-id "{referenced}";
                         $l isa concept-link, links (source: $s, target: $t);"#
            )
        )
        .await,
        1,
        "the second concept links to the first"
    );
    assert_eq!(
        plays(db, &referenced, "hierarchy").await,
        1,
        "the second concept is the first's hierarchy parent"
    );
    assert_eq!(
        plays(db, &referenced, "citation").await,
        1,
        "the first concept cites an external resource"
    );

    // A belief whose source-link names the concept as its origin.
    claim_with_belief(db, "mem/bel/ref", &referenced).await;

    let mut dropped = full.clone();
    dropped.concepts.retain(|c| c.concept_id != referenced);
    dropped
        .links
        .retain(|l| l.source_id != referenced && l.target_id != referenced);
    dropped
        .hierarchy
        .retain(|h| h.parent_id != referenced && h.child_id != referenced);
    dropped.citations.retain(|c| c.source_id != referenced);
    assert_eq!(dropped.concepts.len(), 1);
    (full, referenced, dropped)
}

/// The `retired-at` values of the concept `id`, rendered for comparison.
async fn retired_at(db: &mut TypeDbRouter, id: &str) -> Vec<String> {
    let tx = db.begin_read().await.expect("Failed to begin read");
    let rows: Vec<_> = tx
        .query(format!(
            r#"match $c isa concept, has concept-id "{id}", has retired-at $t; select $t;"#
        ))
        .await
        .expect("retired-at query failed")
        .into_rows()
        .collect::<Vec<_>>()
        .await;
    rows.into_iter()
        .map(|r| {
            let t = r
                .expect("retired-at row")
                .get("t")
                .expect("column t")
                .expect("t bound")
                .try_get_datetime()
                .expect("t is a datetime");
            t.to_string()
        })
        .collect()
}

/// How many relations of type `rel` the concept `id` plays a role in.
async fn plays(db: &mut TypeDbRouter, id: &str, rel: &str) -> usize {
    count(
        db,
        &format!(r#"match $c isa concept, has concept-id "{id}"; $r isa {rel}, links ($c);"#),
    )
    .await
}

#[tokio::test]
async fn gc_of_a_concept_a_belief_edge_references_retires_it() {
    let test_db = TestDb::new("gecko_test_sync_gc_retire");
    let mut db = test_db.router();
    let temp_dir = TempDir::new().unwrap();
    let (_full, referenced, dropped) = synced_with_referenced(&mut db, temp_dir.path()).await;

    let r = sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT)
        .await
        .expect("dropping a referenced concept retires it");
    assert_eq!(r.concepts_retired, 1);
    assert_eq!(r.concepts_deleted, 0);

    let first = retired_at(&mut db, &referenced).await;
    assert_eq!(first.len(), 1, "exactly one retired-at: {first:?}");
    assert_eq!(
        count(
            &mut db,
            &format!(
                r#"match $c isa concept, has concept-id "{referenced}"; $c has $a;
                         not {{ $a isa concept-id; }}; not {{ $a isa retired-at; }};"#
            )
        )
        .await,
        0,
        "a retired concept keeps only its concept-id and retired-at"
    );
    assert_eq!(plays(&mut db, &referenced, "containment").await, 1);
    assert_eq!(
        count(
            &mut db,
            &format!(
                r#"match $c isa concept, has concept-id "{referenced}";
                         $b isa bundle, has bundle-name "{}";
                         $r isa containment, links (container: $b, member: $c);"#,
                dropped.bundle_name
            )
        )
        .await,
        1,
        "the retired concept stays contained by its bundle"
    );
    for rel in ["hierarchy", "concept-link", "citation"] {
        assert_eq!(plays(&mut db, &referenced, rel).await, 0, "{rel}");
    }
    assert_eq!(count(&mut db, "match $l isa source-link;").await, 1);
    assert_eq!(
        count(
            &mut db,
            &format!(
                r#"match $c isa concept, has concept-id "{referenced}";
                         $l isa source-link, links (origin: $c);"#
            )
        )
        .await,
        1,
        "the belief edge still names the retired concept"
    );

    let again = sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT)
        .await
        .expect("re-sync with the concept still retired");
    assert_eq!(again.concepts_retired, 0);
    assert_eq!(again.concepts_deleted, 0);
    assert_eq!(retired_at(&mut db, &referenced).await, first);
}

#[tokio::test]
async fn restoring_a_retired_concepts_file_unretires_it() {
    let test_db = TestDb::new("gecko_test_sync_unretire");
    let mut db = test_db.router();
    let temp_dir = TempDir::new().unwrap();
    let (full, referenced, dropped) = synced_with_referenced(&mut db, temp_dir.path()).await;
    sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT)
        .await
        .expect("retire");

    let r = sync_bundle(&mut db, &full, &[], BELIEF_TIER_ROOT)
        .await
        .expect("restoring the file");
    assert_eq!(r.concepts_updated, 1);
    assert!(retired_at(&mut db, &referenced).await.is_empty());

    let expected_hash = &full
        .concepts
        .iter()
        .find(|c| c.concept_id == referenced)
        .unwrap()
        .file_hash;
    assert_eq!(
        count(
            &mut db,
            &format!(
                r#"match $c isa concept, has concept-id "{referenced}", has file-hash "{expected_hash}";"#
            )
        )
        .await,
        1,
        "the restored concept carries its file's hash"
    );
    let survivor = &dropped.concepts[0].concept_id;
    assert_eq!(
        count(
            &mut db,
            &format!(
                r#"match $s isa concept, has concept-id "{survivor}";
                         $t isa concept, has concept-id "{referenced}";
                         $l isa concept-link, links (source: $s, target: $t);"#
            )
        )
        .await,
        1,
        "the surviving concept's link to the restored one is back"
    );
}

#[tokio::test]
async fn a_retired_concept_no_longer_referenced_is_deleted() {
    let test_db = TestDb::new("gecko_test_sync_retired_gc");
    let mut db = test_db.router();
    let temp_dir = TempDir::new().unwrap();
    let (_full, referenced, dropped) = synced_with_referenced(&mut db, temp_dir.path()).await;
    sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT)
        .await
        .expect("retire");

    let tx = db.begin_write().await.expect("Failed to begin write");
    for q in [
        "match $l isa source-link; delete $l;",
        r#"match $b isa belief, has concept-id "mem/bel/ref"; delete $b;"#,
    ] {
        tx.query(q).await.expect("raw delete");
    }
    tx.commit().await.expect("commit raw delete");

    let r = sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT)
        .await
        .expect("GC of an unreferenced retired concept");
    assert_eq!(r.concepts_deleted, 1);
    assert_eq!(r.concepts_retired, 0);
    assert_eq!(
        count(
            &mut db,
            &format!(r#"match $c isa concept, has concept-id "{referenced}";"#)
        )
        .await,
        0
    );
}

#[tokio::test]
async fn retired_concept_gets_no_new_links_from_live_files() {
    let test_db = TestDb::new("gecko_test_sync_retired_links");
    let mut db = test_db.router();
    let temp_dir = TempDir::new().unwrap();
    let (_full, referenced, dropped) = synced_with_referenced(&mut db, temp_dir.path()).await;
    sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT)
        .await
        .expect("retire");

    let survivor = dropped.concepts[0].concept_id.clone();
    let mut linking = dropped.clone();
    linking.links.push(OkfLink {
        source_id: survivor.clone(),
        target_id: referenced.clone(),
        link_text: "retired".into(),
    });
    linking.hierarchy.push(HierarchyEdge {
        parent_id: survivor,
        child_id: referenced.clone(),
    });
    sync_bundle(&mut db, &linking, &[], BELIEF_TIER_ROOT)
        .await
        .expect("sync with edges to the retired concept");
    assert_eq!(plays(&mut db, &referenced, "concept-link").await, 0);
    assert_eq!(plays(&mut db, &referenced, "hierarchy").await, 0);
}

#[tokio::test]
async fn a_dropped_concept_another_bundle_contains_is_released_not_retired() {
    let test_db = TestDb::new("gecko_test_sync_release_not_retire");
    let mut db = test_db.router();
    let temp_dir = TempDir::new().unwrap();
    let (_full, referenced, dropped) = synced_with_referenced(&mut db, temp_dir.path()).await;

    let tx = db.begin_write().await.expect("Failed to begin write");
    tx.query(format!(
        r#"match $c isa concept, has concept-id "{referenced}";
           insert $b isa bundle, has bundle-name "beta";
                  $cb isa containment, links (container: $b, member: $c);"#
    ))
    .await
    .expect("raw beta containment");
    tx.commit().await.expect("commit beta containment");

    let r = sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT)
        .await
        .expect("release");
    assert_eq!(r.concepts_released, 1);
    assert_eq!(r.concepts_retired, 0);
    assert!(retired_at(&mut db, &referenced).await.is_empty());
}

#[tokio::test]
async fn one_sync_retires_two_linked_referenced_concepts_and_deletes_an_unreferenced_one() {
    let test_db = TestDb::new("gecko_test_sync_gc_mixed");
    let mut db = test_db.router();
    let temp_dir = TempDir::new().unwrap();
    let (full, referenced, _dropped) = synced_with_referenced(&mut db, temp_dir.path()).await;
    let linker = full
        .concepts
        .iter()
        .find(|c| c.concept_id != referenced)
        .expect("a second concept")
        .concept_id
        .clone();

    // The linking concept gains a belief reference, and a third concept nothing references.
    let tx = db.begin_write().await.expect("Failed to begin write");
    let setup = [
        GraphWrite::single(
            "given $id: string, $st: string; \
             insert $b isa belief, has concept-id == $id, has belief-state == $st;",
            vec!["id".into(), "st".into()],
            vec!["mem/bel/linker".into(), "asserted".into()],
        )
        .expecting(1),
        GraphWrite::single(
            "given $bid: string, $cid: string, $kind: string; \
             match $b isa belief, has concept-id $bi; $bi == $bid; \
                   $c isa concept, has concept-id $ci; $ci == $cid; \
             insert $l isa source-link, links (memory: $b, origin: $c), \
                    has origin-kind == $kind;",
            vec!["bid".into(), "cid".into(), "kind".into()],
            vec![
                "mem/bel/linker".into(),
                linker.clone().into(),
                "concept".into(),
            ],
        )
        .expecting(1),
    ];
    run_graph_writes(&tx, &setup)
        .await
        .expect("belief setup writes");
    tx.commit().await.expect("commit belief setup");

    let unreferenced = "playbooks/unreferenced".to_string();
    let mut with_third = full.clone();
    let mut third = with_third.concepts[0].clone();
    third.concept_id = unreferenced.clone();
    with_third.concepts.push(third);
    let r = sync_bundle(&mut db, &with_third, &[], BELIEF_TIER_ROOT)
        .await
        .expect("sync the third concept");
    assert_eq!(r.concepts_inserted, 1);

    let mut emptied = with_third.clone();
    emptied.concepts.clear();
    emptied.links.clear();
    emptied.hierarchy.clear();
    emptied.citations.clear();
    let r = sync_bundle(&mut db, &emptied, &[], BELIEF_TIER_ROOT)
        .await
        .expect("dropping every concept");
    assert_eq!(r.concepts_retired, 2);
    assert_eq!(r.concepts_deleted, 1);

    for id in [&referenced, &linker] {
        assert_eq!(retired_at(&mut db, id).await.len(), 1, "{id} is retired");
        assert_eq!(plays(&mut db, id, "concept-link").await, 0, "{id}");
    }
    assert_eq!(
        count(
            &mut db,
            &format!(r#"match $c isa concept, has concept-id "{unreferenced}";"#)
        )
        .await,
        0,
        "the unreferenced concept is deleted"
    );
    assert_eq!(count(&mut db, "match $l isa concept-link;").await, 0);
}
