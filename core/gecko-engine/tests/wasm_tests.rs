use gecko_engine::sandbox::engine::{ExecutionResult, ExtensionCallback, HostImports};
use gecko_engine::sandbox::wasm_executor::{
    MAX_RUN_ARGS_LEN, MAX_TIMEOUT_MS, SandboxRun, WasmExecutor,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

#[tokio::test]
async fn test_wasm_simple_execution() {
    let executor = WasmExecutor::new().unwrap();
    let code = "({ result: 40 + 2, status: 'success' })";

    let result = executor
        .evaluate(
            SandboxRun {
                code,
                scopes: &[],
                timeout_ms: None,
                args: None,
            },
            Uuid::new_v4(),
            &HostImports::default(),
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
            SandboxRun {
                code,
                scopes: &["test-ext:echo".to_string()],
                timeout_ms: None,
                args: None,
            },
            Uuid::new_v4(),
            &HostImports::default(),
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
            SandboxRun {
                code,
                scopes: &[], // no scopes granted
                timeout_ms: None,
                args: None,
            },
            Uuid::new_v4(),
            &HostImports::default(),
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
            SandboxRun {
                code,
                scopes: &[],
                timeout_ms: Some(50),
                args: None,
            },
            Uuid::new_v4(),
            &HostImports::default(),
            None,
        )
        .await;

    assert!(
        !result.success,
        "Infinite loop should have timed out and failed"
    );
    assert_eq!(
        result.error.as_deref(),
        Some("execution timed out after 50 ms")
    );
}

/// Evaluates `code` with the timeout `timeout_ms`, recording whether a granted host
/// call ran.
async fn eval_with_timeout(code: &str, timeout_ms: Option<u64>) -> (ExecutionResult, bool) {
    let ran = Arc::new(AtomicBool::new(false));
    let flag = ran.clone();
    let cb: ExtensionCallback = Arc::new(move |_, _, _| {
        flag.store(true, Ordering::SeqCst);
        Ok(serde_json::Value::Null)
    });
    let result = WasmExecutor::new()
        .unwrap()
        .evaluate(
            SandboxRun {
                code,
                scopes: &["t:hit".to_string()],
                timeout_ms,
                args: None,
            },
            Uuid::new_v4(),
            &HostImports::default(),
            Some(cb),
        )
        .await;
    (result, ran.load(Ordering::SeqCst))
}

#[tokio::test]
async fn a_zero_timeout_is_refused() {
    let (result, ran) =
        eval_with_timeout(r#"gecko.callExtension("t", "hit", {}); 1"#, Some(0)).await;
    assert!(!result.success);
    let err = result.error.expect("an error");
    assert!(err.contains("timeout") && err.contains("600000"), "{err}");
    assert!(!ran, "the guest must never run");
}

#[tokio::test]
async fn a_timeout_over_the_ceiling_is_refused() {
    let (result, ran) = eval_with_timeout(
        r#"gecko.callExtension("t", "hit", {}); 1"#,
        Some(MAX_TIMEOUT_MS + 1),
    )
    .await;
    assert!(!result.success);
    let err = result.error.expect("an error");
    assert!(err.contains("timeout") && err.contains("600000"), "{err}");
    assert!(!ran, "the guest must never run");
}

#[tokio::test]
async fn the_ceiling_itself_is_accepted() {
    let (result, _) = eval_with_timeout("1 + 1", Some(MAX_TIMEOUT_MS)).await;
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.output, serde_json::json!(2));
}

// ── The host-call boundary ───────────────────────────────────────────────────

use serde_json::{Value, json};
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;

/// The `(ext, func, args)` of every call a recording callback received.
type Calls = Arc<Mutex<Vec<(String, String, Value)>>>;

/// A callback that records each call and answers with `reply(func)`.
fn recording(reply: fn(&str) -> Result<Value, String>) -> (ExtensionCallback, Calls) {
    let calls: Calls = Arc::default();
    let seen = calls.clone();
    let cb: ExtensionCallback = Arc::new(move |ext, func, args| {
        seen.lock()
            .unwrap()
            .push((ext.to_string(), func.to_string(), args));
        reply(func)
    });
    (cb, calls)
}

/// A callback that counts its runs and answers with `reply`.
fn counting(reply: Result<Value, String>) -> (ExtensionCallback, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let runs = count.clone();
    let cb: ExtensionCallback = Arc::new(move |_ext, _func, _args| {
        runs.fetch_add(1, Ordering::SeqCst);
        reply.clone()
    });
    (cb, count)
}

async fn run(code: &str, scopes: &[&str], cb: ExtensionCallback) -> (bool, Value, String) {
    let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
    let result = WasmExecutor::new()
        .unwrap()
        .evaluate(
            SandboxRun {
                code,
                scopes: &scopes,
                timeout_ms: Some(30_000),
                args: None,
            },
            Uuid::new_v4(),
            &HostImports::default(),
            Some(cb),
        )
        .await;
    (
        result.success,
        result.output,
        result.error.unwrap_or_default(),
    )
}

const MEM_SCOPES: [&str; 6] = [
    "mem:recall",
    "mem:remember",
    "mem:derive",
    "mem:supersede",
    "mem:contest",
    "mem:retract",
];

/// A mem callback's canned reply: `{ chunks: [] }` for recall, `{ id }` otherwise.
fn mem_reply(func: &str) -> Result<Value, String> {
    Ok(if func == "recall" {
        json!({ "chunks": [] })
    } else {
        json!({ "id": "x" })
    })
}

/// Asserts each program fails with a `TypeError` mentioning `needle`, and that no
/// host call was made.
async fn assert_type_error(programs: &[&str], needle: &str) {
    for program in programs {
        let (cb, calls) = recording(mem_reply);
        let (ok, _, err) = run(program, &MEM_SCOPES, cb).await;
        assert!(!ok, "{program} must fail");
        assert!(err.contains("TypeError"), "{program}: {err}");
        assert!(err.contains(needle), "{program}: {err} names {needle}");
        assert!(calls.lock().unwrap().is_empty(), "{program}: no host call");
    }
}

#[tokio::test]
async fn a_reply_over_one_mib_arrives_intact_and_runs_once() {
    let blob = "abcdefgh".repeat((2 << 20) / 8);
    let want: u64 = blob.bytes().step_by(4096).map(u64::from).sum();
    let (cb, count) = counting(Ok(json!({ "blob": blob })));
    let code = r#"
        const b = gecko.callExtension("e", "f", {}).blob;
        let sum = 0;
        for (let i = 0; i < b.length; i += 4096) sum += b.charCodeAt(i);
        ({ len: b.length, sum })
    "#;
    let (ok, out, err) = run(code, &["e:f"], cb).await;
    assert!(ok, "{err}");
    assert_eq!(out, json!({ "len": 2 << 20, "sum": want }));
    assert_eq!(count.load(Ordering::SeqCst), 1, "the callback ran once");
}

#[tokio::test]
async fn a_reply_over_the_cap_is_refused_after_one_run() {
    let (cb, count) = counting(Ok(Value::String("x".repeat(17 << 20))));
    let (ok, _, err) = run(r#"gecko.callExtension("e", "f", {})"#, &["e:f"], cb).await;
    assert!(!ok);
    assert!(err.contains("host response exceeded 16 MiB"), "{err}");
    assert_eq!(count.load(Ordering::SeqCst), 1, "the callback ran once");
}

#[tokio::test]
async fn a_callback_error_reaches_the_guest() {
    let (cb, _) = counting(Err("boom".to_string()));
    let (ok, _, err) = run(r#"gecko.callExtension("e", "f", {})"#, &["e:f"], cb).await;
    assert!(!ok);
    assert!(err.contains("Host call failed: boom"), "{err}");
}

#[tokio::test]
async fn an_empty_callback_error_arrives_with_a_message() {
    let (cb, _) = counting(Err(String::new()));
    let (ok, _, err) = run(r#"gecko.callExtension("e", "f", {})"#, &["e:f"], cb).await;
    assert!(!ok);
    assert!(err.contains("Host call failed: "), "{err}");
    assert!(err.contains("empty error"), "{err}");
}

#[tokio::test]
async fn ungranted_call_with_oversized_args_reports_not_granted() {
    let (cb, count) = counting(Ok(json!({})));
    let code = r#"gecko.callExtension("e", "f", { s: "x".repeat(9 << 20) })"#;
    let (ok, _, err) = run(code, &[], cb).await;
    assert!(!ok);
    assert!(err.contains("not granted"), "{err}");
    assert_eq!(count.load(Ordering::SeqCst), 0, "the callback never ran");
}

#[tokio::test]
async fn call_extension_requires_string_names() {
    for code in [
        r#"gecko.callExtension(undefined, "f", {})"#,
        r#"gecko.callExtension("e", 5, {})"#,
    ] {
        let (cb, count) = counting(Ok(json!({})));
        let (ok, _, err) = run(code, &["e:f", ":f", "e:"], cb).await;
        assert!(!ok, "{code}");
        assert!(err.contains("TypeError"), "{code}: {err}");
        assert_eq!(count.load(Ordering::SeqCst), 0, "{code}: no host call");
    }
}

#[tokio::test]
async fn call_extension_refuses_unconvertible_args() {
    for code in [
        r#"const o = {}; o.o = o; gecko.callExtension("e", "f", o)"#,
        r#"gecko.callExtension("e", "f", () => 1)"#,
        r#"gecko.callExtension("e", "f", NaN)"#,
    ] {
        let (cb, count) = counting(Ok(json!({})));
        let (ok, _, err) = run(code, &["e:f"], cb).await;
        assert!(!ok, "{code}");
        assert!(err.contains("TypeError"), "{code}: {err}");
        assert_eq!(count.load(Ordering::SeqCst), 0, "{code}: no host call");
    }
}

#[tokio::test]
async fn call_extension_sends_absent_args_as_an_empty_object() {
    let (cb, calls) = recording(|_| Ok(json!({})));
    let (ok, _, err) = run(r#"gecko.callExtension("e", "f")"#, &["e:f"], cb).await;
    assert!(ok, "{err}");
    assert_eq!(calls.lock().unwrap()[0].2, json!({}));
}

#[tokio::test]
async fn mem_bags_must_be_objects() {
    assert_type_error(&[r#"mem.recall("q", 5)"#], "budget").await;
    assert_type_error(&[r#"mem.derive("t", [], "opts")"#], "opts").await;
    assert_type_error(&[r#"mem.remember("t", [1])"#], "opts").await;
}

#[tokio::test]
async fn mem_bags_refuse_unknown_keys() {
    assert_type_error(
        &[
            r#"mem.recall("q", { maxChunk: 3 })"#,
            r#"mem.recall("q", { maxChunk: undefined })"#,
        ],
        "maxChunk",
    )
    .await;
    assert_type_error(&[r#"mem.recall("q", { maxTokens: 5 })"#], "maxTokens").await;
    assert_type_error(&[r#"mem.recall("q", { 0: 5 })"#], "0").await;
}

#[tokio::test]
async fn mem_values_must_be_json() {
    assert_type_error(
        &[
            r#"mem.recall("q", { maxChunks: NaN })"#,
            r#"mem.recall("q", { maxChunks: () => 5 })"#,
        ],
        "maxChunks",
    )
    .await;
    assert_type_error(
        &[r#"mem.derive("t", [], { confidence: Symbol() })"#],
        "confidence",
    )
    .await;
    assert_type_error(&[r#"mem.derive("t", NaN)"#], "evidence").await;
    assert_type_error(&[r#"mem.supersede("o", "t", Symbol())"#], "reason").await;
}

#[tokio::test]
async fn mem_undefined_option_means_absent() {
    let (cb, calls) = recording(mem_reply);
    let code = r#"mem.recall("q", { asOf: undefined, maxChunks: 3 })"#;
    let (ok, _, err) = run(code, &MEM_SCOPES, cb).await;
    assert!(ok, "{err}");
    assert_eq!(
        calls.lock().unwrap()[0].2,
        json!({ "text": "q", "max_chunks": 3 })
    );
}

#[tokio::test]
async fn mem_values_are_forwarded_unconverted() {
    let (cb, calls) = recording(mem_reply);
    let code = r#"mem.recall("q", { maxChunks: 2.5, asOf: 7 }); mem.supersede("o", "t", 5)"#;
    let (ok, _, err) = run(code, &MEM_SCOPES, cb).await;
    assert!(ok, "{err}");
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls[0].2,
        json!({ "text": "q", "max_chunks": 2.5, "as_of": 7 })
    );
    assert_eq!(calls[1].2, json!({ "old": "o", "text": "t", "reason": 5 }));
}

#[tokio::test]
async fn mem_recall_requires_a_chunks_array() {
    for reply in [
        (|_: &str| Ok(json!({}))) as fn(&str) -> Result<Value, String>,
        |_: &str| Ok(json!({ "chunks": "bad" })),
    ] {
        let (cb, _) = recording(reply);
        let (ok, _, err) = run(r#"mem.recall("q")"#, &MEM_SCOPES, cb).await;
        assert!(!ok, "a reply without a chunks array throws");
        assert!(err.contains("chunks"), "{err}");
    }
}

#[tokio::test]
async fn mem_contest_forwards_missing_claims_for_the_host_to_refuse() {
    let (cb, calls) = recording(mem_reply);
    let (ok, _, err) = run("mem.contest(); mem.contest(null)", &MEM_SCOPES, cb).await;
    assert!(ok, "{err}");
    let calls = calls.lock().unwrap();
    assert_eq!(calls[0].2, json!({}));
    assert_eq!(calls[1].2, json!({ "claims": null }));
}

#[tokio::test]
async fn mem_retract_forwards_id_and_reason() {
    let (cb, calls) = recording(mem_reply);
    let (ok, _, err) = run(
        r#"mem.retract("mem/bel/x", "r"); mem.retract("mem/bel/x")"#,
        &MEM_SCOPES,
        cb,
    )
    .await;
    assert!(ok, "{err}");
    let calls = calls.lock().unwrap();
    assert_eq!(calls[0].0, "mem");
    assert_eq!(calls[0].1, "retract");
    assert_eq!(calls[0].2, json!({ "id": "mem/bel/x", "reason": "r" }));
    assert_eq!(calls[1].2, json!({ "id": "mem/bel/x" }));
}

#[tokio::test]
async fn mem_retract_forwards_a_non_string_reason_unconverted() {
    let (cb, calls) = recording(mem_reply);
    let (ok, _, err) = run(r#"mem.retract("mem/bel/x", 5)"#, &MEM_SCOPES, cb).await;
    assert!(ok, "{err}");
    assert_eq!(
        calls.lock().unwrap()[0].2,
        json!({ "id": "mem/bel/x", "reason": 5 })
    );
}

#[tokio::test]
async fn mem_retract_without_an_id_throws_type_error() {
    assert_type_error(&["mem.retract()", "mem.retract(5)"], "id").await;
}

fn retract_reply(_: &str) -> Result<Value, String> {
    Ok(json!({ "id": "mem/bel/x", "blast_radius": ["mem/bel/y"] }))
}

#[tokio::test]
async fn mem_retract_returns_the_reply() {
    let (cb, _) = recording(retract_reply);
    let code = r#"const r = mem.retract("mem/bel/x"); ({ id: r.id, first: r.blast_radius[0] })"#;
    let (ok, out, err) = run(code, &MEM_SCOPES, cb).await;
    assert!(ok, "{err}");
    assert_eq!(out, json!({ "id": "mem/bel/x", "first": "mem/bel/y" }));
}

#[tokio::test]
async fn mem_retract_needs_its_scope() {
    let (cb, calls) = recording(mem_reply);
    let scopes: Vec<&str> = MEM_SCOPES
        .iter()
        .copied()
        .filter(|s| *s != "mem:retract")
        .collect();
    let (ok, _, err) = run(r#"mem.retract("mem/bel/x")"#, &scopes, cb).await;
    assert!(!ok);
    assert!(err.contains("not granted"), "{err}");
    assert!(calls.lock().unwrap().is_empty(), "the callback never ran");
}

#[tokio::test]
async fn an_error_over_the_cap_reaches_the_guest_truncated() {
    let (cb, _) = counting(Err("x".repeat(17 << 20)));
    let code = r#"
        let len = -1;
        try { gecko.callExtension("e", "f", {}); } catch (e) { len = String(e.message).length; }
        ({ len })
    "#;
    let (ok, out, err) = run(code, &["e:f"], cb).await;
    assert!(ok, "{err}");
    let len = out["len"].as_u64().unwrap();
    let cap = 16u64 << 20;
    let prefix = "Host call failed: ".len() as u64;
    assert!(len > cap - 4 && len <= cap + prefix, "len {len}");
}

#[tokio::test]
async fn mem_recall_does_not_echo_a_bad_reply() {
    let (cb, _) = recording(|_| Ok(json!({ "chunks": "x".repeat(1 << 20) })));
    let (ok, _, err) = run(r#"mem.recall("q")"#, &MEM_SCOPES, cb).await;
    assert!(!ok);
    assert!(err.contains("chunks"), "{err}");
    assert!(err.len() < 300, "a {}-byte error", err.len());
}

/// A writer the contest refusal must never reach.
struct UnreachableWriter;

#[async_trait::async_trait]
impl gecko_extension_api::EpistemicWriter for UnreachableWriter {
    async fn observe(
        &self,
        _: &gecko_extension_api::RunContext,
        _: gecko_extension_api::EpisodeDraft,
    ) -> Result<gecko_extension_api::MemId, gecko_extension_api::EpistemicError> {
        unreachable!("observe")
    }
    async fn assert_belief(
        &self,
        _: &gecko_extension_api::RunContext,
        _: gecko_extension_api::BeliefDraft,
        _: &[gecko_extension_api::MemId],
        _: gecko_extension_api::DerivationMethod,
    ) -> Result<gecko_extension_api::MemId, gecko_extension_api::EpistemicError> {
        unreachable!("assert_belief")
    }
    async fn supersede(
        &self,
        _: &gecko_extension_api::RunContext,
        _: gecko_extension_api::MemId,
        _: gecko_extension_api::BeliefDraft,
        _: &str,
    ) -> Result<gecko_extension_api::MemId, gecko_extension_api::EpistemicError> {
        unreachable!("supersede")
    }
    async fn contest(
        &self,
        _: &gecko_extension_api::RunContext,
        _: &[gecko_extension_api::MemId],
    ) -> Result<gecko_extension_api::AnomalyId, gecko_extension_api::EpistemicError> {
        unreachable!("contest")
    }
    async fn retract(
        &self,
        _: &gecko_extension_api::RunContext,
        _: gecko_extension_api::MemId,
        _: Option<&str>,
    ) -> Result<Vec<gecko_extension_api::MemId>, gecko_extension_api::EpistemicError> {
        unreachable!("retract")
    }
    async fn rests_on(
        &self,
        _: &gecko_extension_api::RunContext,
        _: gecko_extension_api::MemId,
        _: &[gecko_extension_api::MemId],
    ) -> Result<(), gecko_extension_api::EpistemicError> {
        unreachable!("rests_on")
    }
    async fn record_prediction(
        &self,
        _: &gecko_extension_api::RunContext,
        _: gecko_extension_api::MemId,
        _: f64,
    ) -> Result<(), gecko_extension_api::EpistemicError> {
        unreachable!("record_prediction")
    }
    async fn resolve_prediction(
        &self,
        _: &gecko_extension_api::RunContext,
        _: gecko_extension_api::MemId,
        _: gecko_extension_api::Outcome,
    ) -> Result<(), gecko_extension_api::EpistemicError> {
        unreachable!("resolve_prediction")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mem_contest_without_claims_throws_end_to_end() {
    use gecko_engine::sandbox::mem_host::epistemic_extension_callback;
    use gecko_extension_api::{ActorId, ProvenanceSource, RunContext};
    for code in ["mem.contest()", "mem.contest(null)"] {
        let ctx = RunContext::new(
            ActorId::new("agent-1"),
            ProvenanceSource::Manual,
            chrono::Utc::now(),
        );
        let base: ExtensionCallback = Arc::new(|_, _, _| Err("base".to_string()));
        let cb = epistemic_extension_callback(ctx, Arc::new(UnreachableWriter), None, base);
        let (ok, _, err) = run(code, &MEM_SCOPES, cb).await;
        assert!(!ok, "{code} must throw");
        assert!(err.contains("claims"), "{code}: {err}");
    }
}

/// Evaluates `code` with the run arguments `args` and no host calls.
async fn eval_with_args(code: &str, args: Option<&serde_json::Value>) -> ExecutionResult {
    WasmExecutor::new()
        .unwrap()
        .evaluate(
            SandboxRun {
                code,
                scopes: &[],
                timeout_ms: None,
                args: args.map(|a| a.as_object().expect("an object")),
            },
            Uuid::new_v4(),
            &HostImports::default(),
            None,
        )
        .await
}

fn sample_args() -> serde_json::Value {
    serde_json::json!({"logic_id": "detections/x", "n": 3, "a": {"b": 1}, "list": [1, 2]})
}

#[tokio::test]
async fn run_args_reach_the_guest() {
    let result = eval_with_args(
        "({ id: gecko.args.logic_id, n: gecko.args.n, b: gecko.args.a.b, len: gecko.args.list.length })",
        Some(&sample_args()),
    )
    .await;
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        result.output,
        serde_json::json!({"id": "detections/x", "n": 3, "b": 1, "len": 2})
    );
}

#[tokio::test]
async fn absent_run_args_are_an_empty_object() {
    let result = eval_with_args(
        "(() => { const { x } = gecko.args; return { keys: Object.keys(gecko.args).length, x: x === undefined }; })()",
        None,
    )
    .await;
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.output, serde_json::json!({"keys": 0, "x": true}));
}

#[tokio::test]
async fn run_args_are_deep_frozen_in_sloppy_mode() {
    let code = r#"
        gecko.args = 1;
        gecko.args.n = 9;
        gecko.args.a.b = 9;
        gecko.args.extra = 1;
        try { gecko.args.list.push(3); } catch (e) {}
        delete gecko.args;
        ({ n: gecko.args.n, b: gecko.args.a.b, len: gecko.args.list.length,
           extra: "extra" in gecko.args, same: typeof gecko.args })
    "#;
    let result = eval_with_args(code, Some(&sample_args())).await;
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        result.output,
        serde_json::json!({"n": 3, "b": 1, "len": 2, "extra": false, "same": "object"})
    );
}

#[tokio::test]
async fn run_args_are_deep_frozen_in_strict_mode() {
    let code = r#"
        "use strict";
        const throws = (f) => { try { f(); return false; } catch (e) { return e instanceof TypeError; } };
        [
            throws(() => { gecko.args.n = 9; }),
            throws(() => { gecko.args.extra = 1; }),
            throws(() => { gecko.args.a.b = 9; }),
            throws(() => { gecko.args.list.push(3); }),
            throws(() => { gecko.args = 1; }),
            throws(() => { delete gecko.args; }),
        ]
    "#;
    let result = eval_with_args(code, Some(&sample_args())).await;
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        result.output,
        serde_json::json!([true, true, true, true, true, true])
    );
}

#[tokio::test]
async fn oversized_run_args_are_refused() {
    let args = serde_json::json!({ "s": "x".repeat(MAX_RUN_ARGS_LEN) });
    let ran = Arc::new(AtomicBool::new(false));
    let flag = ran.clone();
    let cb: ExtensionCallback = Arc::new(move |_, _, _| {
        flag.store(true, Ordering::SeqCst);
        Ok(serde_json::Value::Null)
    });
    let result = WasmExecutor::new()
        .unwrap()
        .evaluate(
            SandboxRun {
                code: r#"gecko.callExtension("t", "hit", {}); 1"#,
                scopes: &["t:hit".to_string()],
                timeout_ms: None,
                args: args.as_object(),
            },
            Uuid::new_v4(),
            &HostImports::default(),
            Some(cb),
        )
        .await;
    assert!(!result.success);
    assert_eq!(result.output, serde_json::Value::Null);
    let err = result.error.expect("an error");
    assert!(
        err.contains("GECKO_ARGS") && err.contains("1048576"),
        "{err}"
    );
    assert!(!ran.load(Ordering::SeqCst), "the guest must never run");
}

#[tokio::test]
async fn run_args_just_under_the_cap_reach_the_guest() {
    // `{"s":"…"}` adds 8 bytes around the string.
    let len = MAX_RUN_ARGS_LEN - 16 - 8;
    let args = serde_json::json!({ "s": "x".repeat(len) });
    assert_eq!(
        serde_json::to_string(&args).unwrap().len(),
        MAX_RUN_ARGS_LEN - 16
    );
    let result = eval_with_args("gecko.args.s.length", Some(&args)).await;
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.output, serde_json::json!(len));
}
