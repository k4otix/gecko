// Applies the `cyber` extension schema alongside core+mem, so it is compiled ONLY
// when the `cyber` feature is on. The canonical candle-free Tier-2 command
// (`cargo test --test '*' --no-default-features`) drops `cyber`, so this binary
// compiles empty and passes there; a `--features cyber` (or default) run runs it.
#![cfg(feature = "cyber")]

use futures_util::StreamExt;
use std::fs;
use tempfile::TempDir;

use cyber_gecko::CyberGecko;
use gecko_engine::extension::GeckoExtension;
use gecko_engine::okf::parser::parse_bundle;
use gecko_engine::syncer::bundle::sync_bundle;
use mem_gecko::MemGecko;

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

    let extensions: Vec<Box<dyn GeckoExtension>> =
        vec![Box::new(CyberGecko::new()), Box::new(MemGecko::new())];

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

    let result = sync_bundle(&mut db, &bundle)
        .await
        .expect("Failed to sync bundle");
    assert_eq!(result.concepts_inserted, 1);
    assert_eq!(result.citations_created, 1);
    // Concept IDs are bundle-relative paths with no namespace prefix.
    assert_eq!(bundle.concepts[0].concept_id, "playbooks/test_concept");

    // Re-syncing an unchanged bundle must be idempotent (C1): the concept's
    // content hash matches, so it is skipped rather than re-inserted.
    let result2 = sync_bundle(&mut db, &bundle)
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
    let result3 = sync_bundle(&mut db, &edited)
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
