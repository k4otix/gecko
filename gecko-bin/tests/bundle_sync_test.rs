use futures_util::StreamExt;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

use cyber_gecko::CyberGecko;
use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_engine::extension::GeckoExtension;
use gecko_engine::okf::parser::parse_bundle;
use gecko_engine::syncer::bundle::sync_bundle;
use mem_gecko::MemGecko;

/// Generates a unique database name to avoid collisions in concurrent tests
fn unique_db_name() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("gecko_test_sync_{}", ts)
}

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
    let db_name = unique_db_name();
    let config = DbConfig {
        address: "localhost:1729".to_string(),
        database: db_name.clone(),
        username: "admin".to_string(),
        password: "password".to_string(),
        tls: TlsMode::Disabled,
    };

    let mut db = TypeDbRouter::new(config.clone());

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

    // Verify in db
    let tx = db.begin_read().await.expect("Failed to begin read");
    let answer = tx
        .query(r#"match $c isa concept, has title "Test Concept";"#)
        .await
        .expect("query failed");
    let rows: Vec<_> = answer.into_rows().collect::<Vec<_>>().await;
    assert_eq!(rows.len(), 1, "Concept not found in database");
}
