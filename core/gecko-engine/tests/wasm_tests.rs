use gecko_engine::sandbox::engine::{ExtensionCallback, HostImports, ScriptExecutor};
use gecko_engine::sandbox::wasm_executor::WasmExecutor;
use std::sync::Arc;
use uuid::Uuid;

#[test]
fn test_wasm_simple_execution() {
    let executor = WasmExecutor::new().unwrap();
    let code = "({ result: 40 + 2, status: 'success' })";

    let result = executor.evaluate(code, Uuid::new_v4(), &HostImports::default(), None, None);

    assert!(result.success);
    assert_eq!(
        result.output,
        serde_json::json!({"result": 42, "status": "success"})
    );
}

#[test]
fn test_wasm_host_bindings() {
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

    let result = executor.evaluate(
        code,
        Uuid::new_v4(),
        &HostImports::default(),
        None,
        Some(cb),
    );

    assert!(result.success);
    assert_eq!(result.output, serde_json::json!({"result": "hello"}));
}

#[test]
fn test_wasm_epoch_timeout() {
    let executor = WasmExecutor::new().unwrap();
    let code = "while(true) {}";

    // Set a very short timeout (50ms)
    let result = executor.evaluate(
        code,
        Uuid::new_v4(),
        &HostImports::default(),
        Some(50),
        None,
    );

    assert!(
        !result.success,
        "Infinite loop should have timed out and failed"
    );
}
