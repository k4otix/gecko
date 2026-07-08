//! Pipeline integration tests (C4/WS4).
//!
//! These exercise `execute_playbook` end-to-end through the real WASM sandbox.
//! The pipeline's record hook is log-only for now, so it never opens a TypeDB
//! transaction — the router is constructed but never connected, letting these run
//! without a live database.

use std::sync::Arc;

use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_engine::pipeline::{PlaybookRun, execute_playbook};
use gecko_engine::sandbox::engine::{ExtensionCallback, HostImports};
use gecko_engine::sandbox::wasm_pool::SandboxPool;
use gecko_engine::state::registry::StateRegistry;

/// A router that is never connected — the pipeline's record hook does not touch it.
fn offline_router() -> TypeDbRouter {
    TypeDbRouter::new(DbConfig {
        address: "localhost:1729".to_string(),
        database: "unused".to_string(),
        username: "admin".to_string(),
        password: "password".to_string(),
        tls: TlsMode::Disabled,
    })
}

#[tokio::test]
async fn test_pipeline_executes_program() {
    let mut db = offline_router();
    let registry = StateRegistry::new();
    let pool = SandboxPool::new();

    let run = PlaybookRun {
        concept_id: "playbooks/calc",
        program: "({ answer: 40 + 2 })",
        scopes: &[],
        timeout_ms: None,
    };

    let result = execute_playbook(
        &run,
        &mut db,
        &registry,
        &pool,
        &HostImports::default(),
        None,
    )
    .await
    .expect("pipeline should not error");

    assert!(
        result.execution.success,
        "error: {:?}",
        result.execution.error
    );
    assert_eq!(result.execution.output, serde_json::json!({ "answer": 42 }));
    assert!(result.committed, "a successful run should be recorded");
}

#[tokio::test]
async fn test_pipeline_enforces_scopes() {
    let mut db = offline_router();
    let registry = StateRegistry::new();
    let pool = SandboxPool::new();

    // Default-deny (S3): the run grants no scopes, so the host call must be blocked
    // and the callback must never fire.
    let cb: ExtensionCallback = Arc::new(|_ext, _func, _args| {
        panic!("callback must not run without a granted scope");
    });

    let run = PlaybookRun {
        concept_id: "playbooks/denied",
        program: r#"gecko.callExtension("cyber-gecko", "mde_isolate", { machine_id: "h1" });"#,
        scopes: &[],
        timeout_ms: None,
    };

    let result = execute_playbook(
        &run,
        &mut db,
        &registry,
        &pool,
        &HostImports::default(),
        Some(cb),
    )
    .await
    .expect("pipeline returns Ok even when the script fails");

    assert!(!result.execution.success, "ungranted host call must fail");
    assert!(!result.committed, "a failed run must not be recorded");
}
