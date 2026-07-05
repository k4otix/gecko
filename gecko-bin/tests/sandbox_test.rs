use futures_util::StreamExt;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use uuid::Uuid;

use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_engine::okf::parser::parse_bundle;
use gecko_engine::okf::types::ScriptEngine;
use gecko_engine::sandbox::engine::{HostImports, ScriptExecutor};
use gecko_engine::sandbox::rhai_executor::RhaiExecutor;
use gecko_engine::syncer::bundle::sync_bundle;

fn unique_db_name() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("gecko_test_sandbox_{}", ts)
}

#[tokio::test]
async fn test_sandbox_execution() {
    // 1. Setup bundle directory
    let temp_dir = TempDir::new().unwrap();
    let bundle_path = temp_dir.path();

    // Create a playbook concept in the bundle
    let playbook_content = r#"---
type: playbook
title: Test Playbook
---

# Test Playbook

This is a test playbook.

```rhai
let x = 40;
let y = 2;
x + y
```
"#;

    let playbooks_dir = bundle_path.join("playbooks");
    fs::create_dir_all(&playbooks_dir).unwrap();
    fs::write(playbooks_dir.join("test_playbook.md"), playbook_content).unwrap();

    // Create a dummy bundle manifest
    let bundle_json = serde_json::json!({
        "name": "test-bundle",
        "description": "Test Bundle"
    });
    fs::write(bundle_path.join("bundle.json"), bundle_json.to_string()).unwrap();

    // 2. Parse the bundle
    let manifest = parse_bundle(bundle_path).expect("Failed to parse bundle");
    let bundle_name = bundle_path.file_name().unwrap().to_str().unwrap();
    let expected_id = format!("{}:playbooks/test_playbook", bundle_name);

    assert_eq!(manifest.concepts.len(), 1);
    let concept = &manifest.concepts[0];
    assert_eq!(concept.concept_id, expected_id);
    assert_eq!(concept.concept_type, "playbook");
    assert_eq!(concept.code_blocks.len(), 1);

    // 3. Setup TypeDB
    let config = DbConfig {
        address: "localhost:1729".to_string(),
        database: unique_db_name(),
        username: "admin".to_string(),
        password: "password".to_string(),
        tls: TlsMode::Disabled,
    };

    let mut db = TypeDbRouter::new(config.clone());

    // Apply core schema
    let core_schema = include_str!("../../core/gecko-engine/schema/core_schema.tql");
    db.apply_schema(core_schema)
        .await
        .expect("Failed to apply core schema");

    // 4. Sync the bundle
    let _sync_result = sync_bundle(&mut db, &manifest)
        .await
        .expect("Failed to sync bundle");

    // 5. Fetch the code-block from TypeDB
    let tx = db
        .begin_read()
        .await
        .expect("Failed to begin read transaction");
    let query = format!(
        r#"
        match 
            $c isa concept, 
                has concept-id "{}",
                has code-block $cb; 
        fetch {{"code": $cb}};
    "#,
        expected_id
    );

    let answer = tx.query(query).await.expect("Failed to execute query");
    let mut code_block = String::new();

    if answer.is_document_stream() {
        let mut stream = answer.into_documents();
        if let Some(Ok(doc)) = stream.next().await {
            let json_str = doc.into_json().to_string();
            let json: serde_json::Value = serde_json::from_str(&json_str).unwrap();
            if let Some(code) = json
                .as_object()
                .and_then(|m| m.get("code"))
                .and_then(|v| v.as_str())
            {
                code_block = code.to_string();
            }
        }
    }

    assert!(
        !code_block.is_empty(),
        "Failed to fetch code block from database"
    );

    // 6. Execute in Sandbox
    let executor = RhaiExecutor::new();
    let result = executor.evaluate(
        &code_block,
        Uuid::new_v4(),
        &HostImports::default(),
        None,
        None,
    );

    assert!(
        result.success,
        "Script execution failed: {:?}",
        result.error
    );
    assert_eq!(result.output, serde_json::json!(42));
    assert_eq!(result.engine, ScriptEngine::Rhai);
}
