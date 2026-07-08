use futures_util::StreamExt;
use std::fs;
use tempfile::TempDir;
use uuid::Uuid;

use gecko_engine::okf::parser::parse_bundle;
use gecko_engine::okf::types::ScriptEngine;
use gecko_engine::sandbox::engine::HostImports;
use gecko_engine::sandbox::wasm_executor::WasmExecutor;
use gecko_engine::syncer::bundle::sync_bundle;

mod common;
use common::TestDb;

#[tokio::test]
async fn test_sandbox_execution() {
    // 1. Setup bundle directory
    let temp_dir = TempDir::new().unwrap();
    let bundle_path = temp_dir.path();

    // A QuickJS playbook: all synced code runs in the one WASM sandbox boundary.
    let playbook_content = r#"---
type: playbook
title: Test Playbook
engine: quickjs
---

# Test Playbook

This is a test playbook.

```js
const x = 40;
const y = 2;
({ answer: x + y })
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
    // Single-bundle-scoped: concept IDs are bundle-relative paths with no prefix.
    let expected_id = "playbooks/test_playbook";

    assert_eq!(manifest.concepts.len(), 1);
    // bundle.json declares the bundle name (metadata only, not a concept-id prefix).
    assert_eq!(manifest.bundle_name, "test-bundle");
    let concept = &manifest.concepts[0];
    assert_eq!(concept.concept_id, expected_id);
    assert_eq!(concept.concept_type, "playbook");
    // One program per concept: the engine-matched js fence is the program.
    assert_eq!(concept.engine, Some(ScriptEngine::QuickJs));
    let program = concept
        .program
        .as_deref()
        .expect("expected an executable program");
    assert!(program.contains("x + y"));

    // 3. Setup TypeDB (self-cleaning database, dropped when `test_db` drops)
    let test_db = TestDb::new("gecko_test_sandbox");
    let mut db = test_db.router();

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

    // 6. Execute in the WASM sandbox (async). No host calls, so no scopes needed.
    let executor = WasmExecutor::new().expect("Failed to initialize Wasm engine");
    let result = executor
        .evaluate(
            &code_block,
            Uuid::new_v4(),
            &HostImports::default(),
            &[],
            None,
            None,
        )
        .await;

    assert!(
        result.success,
        "Script execution failed: {:?}",
        result.error
    );
    assert_eq!(result.output, serde_json::json!({ "answer": 42 }));
    assert_eq!(result.engine, ScriptEngine::QuickJs);
}
