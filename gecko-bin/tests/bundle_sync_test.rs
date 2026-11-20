// Exercises bundle-sync insert/skip/update/delete-citation coverage against
// core+mem schema only — no cyber-specific schema or types are touched, so this
// test runs on every push, including the candle-free Tier-2 command
// (`cargo test --test '*' --no-default-features`).
use futures_util::StreamExt;
use std::collections::HashSet;
use std::fs;
use tempfile::TempDir;

#[cfg(feature = "cyber")]
use cyber_gecko::CyberGecko;
use gecko_engine::db::router::{DbError, TypeDbRouter};
use gecko_engine::db::run_graph_writes;
use gecko_engine::extension::GeckoExtension;
use gecko_engine::okf::parser::parse_bundle;
use gecko_engine::okf::types::{TypedAttrValue, TypedAttribute};
use gecko_engine::syncer::bundle::sync_bundle;
use gecko_extension_api::GraphWrite;
use mem_gecko::{BELIEF_TIER_ROOT, MemGecko};

mod common;
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

#[tokio::test]
async fn gc_of_a_concept_a_belief_edge_references_is_refused_before_any_delete() {
    let test_db = TestDb::new("gecko_test_sync_gc_ref");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;

    let temp_dir = TempDir::new().unwrap();
    create_test_bundle(temp_dir.path());
    let bundle = parse_bundle(temp_dir.path()).expect("Failed to parse bundle");
    sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT)
        .await
        .expect("Failed to sync bundle");
    let referenced = bundle.concepts[0].concept_id.clone();

    // A belief whose source-link names the concept as its origin.
    let tx = db.begin_write().await.expect("Failed to begin write");
    let setup = [
        GraphWrite::single(
            "given $id: string, $st: string; \
             insert $b isa belief, has concept-id == $id, has belief-state == $st;",
            vec!["id".into(), "st".into()],
            vec!["mem/bel/ref".into(), "asserted".into()],
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
                "mem/bel/ref".into(),
                referenced.clone().into(),
                "concept".into(),
            ],
        )
        .expecting(1),
    ];
    run_graph_writes(&tx, &setup)
        .await
        .expect("belief setup writes");
    tx.commit().await.expect("commit belief setup");
    let concepts_before = count(&mut db, "match $c isa concept;").await;

    let mut dropped = bundle.clone();
    dropped.concepts.retain(|c| c.concept_id != referenced);
    dropped
        .links
        .retain(|l| l.source_id != referenced && l.target_id != referenced);
    dropped
        .hierarchy
        .retain(|h| h.parent_id != referenced && h.child_id != referenced);
    dropped.citations.retain(|c| c.source_id != referenced);

    let result = sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT).await;
    let Err(DbError::BeliefEdgeReferencesRemovedConcept { references }) = result else {
        panic!("expected BeliefEdgeReferencesRemovedConcept, got {result:?}");
    };
    assert_eq!(references, vec![(referenced.clone(), "source-link".into())]);

    assert_eq!(
        count(&mut db, "match $c isa concept, has concept-id $i;").await,
        concepts_before
    );
    let tx = db.begin_read().await.expect("Failed to begin read");
    let rows: Vec<_> = tx
        .query("match $c isa concept, has concept-id $i; select $i;")
        .await
        .expect("concept-id query failed")
        .into_rows()
        .collect::<Vec<_>>()
        .await;
    let ids: Vec<String> = rows
        .into_iter()
        .map(|r| {
            r.expect("concept-id row")
                .get("i")
                .expect("column i")
                .expect("i bound")
                .try_get_string()
                .expect("i is a string")
                .to_string()
        })
        .collect();
    assert_eq!(ids, vec![referenced], "the referenced concept must survive");
}
