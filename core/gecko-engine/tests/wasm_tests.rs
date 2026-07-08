use gecko_engine::sandbox::engine::{ExtensionCallback, HostImports};
use gecko_engine::sandbox::wasm_executor::WasmExecutor;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn test_wasm_simple_execution() {
    let executor = WasmExecutor::new().unwrap();
    let code = "({ result: 40 + 2, status: 'success' })";

    let result = executor
        .evaluate(
            code,
            Uuid::new_v4(),
            &HostImports::default(),
            &[],
            None,
            None,
        )
        .await;

    assert!(result.success);
    assert_eq!(
        result.output,
        serde_json::json!({"result": 42, "status": "success"})
    );
}

#[tokio::test]
async fn test_wasm_host_bindings() {
    let executor = WasmExecutor::new().unwrap();
    let code = r#"
        const res = gecko.callExtension("test-ext", "echo", { msg: "hello" });
        ({ result: res.msg })
    "#;

    let cb: ExtensionCallback = Arc::new(|ext, func, args| {
        assert_eq!(ext, "test-ext");
        assert_eq!(func, "echo");
        Ok(args) // just echo back
    });

    // The concept must be granted the "test-ext:echo" scope for the call (S3).
    let result = executor
        .evaluate(
            code,
            Uuid::new_v4(),
            &HostImports::default(),
            &["test-ext:echo".to_string()],
            None,
            Some(cb),
        )
        .await;

    assert!(result.success);
    assert_eq!(result.output, serde_json::json!({"result": "hello"}));
}

#[tokio::test]
async fn test_wasm_scope_denied() {
    // Default-deny (S3): a host call the concept was not scoped for must fail,
    // and the callback must never be invoked.
    let executor = WasmExecutor::new().unwrap();
    let code = r#"gecko.callExtension("test-ext", "echo", { msg: "hello" });"#;

    let cb: ExtensionCallback = Arc::new(|_ext, _func, _args| {
        panic!("callback must not run when the scope is not granted");
    });

    let result = executor
        .evaluate(
            code,
            Uuid::new_v4(),
            &HostImports::default(),
            &[], // no scopes granted
            None,
            Some(cb),
        )
        .await;

    assert!(!result.success, "call without a granted scope must fail");
    let err = result.error.unwrap_or_default();
    assert!(
        err.contains("not granted"),
        "error should explain the scope denial, got: {err}"
    );
}

#[tokio::test]
async fn test_wasm_epoch_timeout() {
    let executor = WasmExecutor::new().unwrap();
    let code = "while(true) {}";

    // Set a very short timeout (50ms).
    let result = executor
        .evaluate(
            code,
            Uuid::new_v4(),
            &HostImports::default(),
            &[],
            Some(50),
            None,
        )
        .await;

    assert!(
        !result.success,
        "Infinite loop should have timed out and failed"
    );
}
