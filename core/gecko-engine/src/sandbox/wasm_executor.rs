//! The WebAssembly sandbox executor.
//!
//! All synced code is untrusted and runs here: a QuickJS (Boa) guest compiled to
//! WASM, hosted by wasmtime with strictly no filesystem or network access. The
//! guest reads its program from stdin and writes a JSON result to stdout.
//!
//! The executor is **async**: instantiation, `_start`, and the
//! `host_call_extension` bridge all run on the async path (`instantiate_async` /
//! `call_async` / `func_wrap_async`). The extension callback is currently
//! synchronous and resolves immediately, but the plumbing is async-ready because
//! real host capabilities (MCP, API, DB, TypeQL) are I/O-bound.
//!
//! Safety limits (S1): a linear-memory cap and single instance/memory/table via
//! `StoreLimits`, plus an epoch-based execution timeout. Host capability calls are
//! default-deny and gated by the concept's granted `scopes` (S3).

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use uuid::Uuid;
use wasmtime::{Caller, Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::{WasiCtxBuilder, p1::WasiP1Ctx};

use crate::okf::types::ScriptEngine;

use super::engine::{ExecutionResult, ExtensionCallback, HostImports};

// Embed the pre-compiled QuickJS (Boa) JavaScript WebAssembly binary.
const QUICKJS_WASM: &[u8] = include_bytes!("quickjs.wasm");

/// Maximum linear memory a guest may allocate (S1). Generous enough for a QuickJS
/// heap while still bounding a runaway allocation.
const MAX_MEMORY_BYTES: usize = 256 * 1024 * 1024;

/// Maximum table elements a guest may allocate (S1).
const MAX_TABLE_ELEMENTS: usize = 100_000;

/// Default execution timeout applied when a concept sets no `timeout-ms` (S1).
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// How often the background ticker advances the epoch. One epoch == one tick, so
/// the timeout in epochs is `timeout_ms / EPOCH_TICK_MS`.
const EPOCH_TICK_MS: u64 = 10;

fn read_string<T>(
    memory: &wasmtime::Memory,
    caller: &Caller<'_, T>,
    ptr: u32,
    len: u32,
) -> Result<String, ()> {
    let mut buf = vec![0u8; len as usize];
    memory
        .read(caller, ptr as usize, &mut buf)
        .map_err(|_| ())?;
    String::from_utf8(buf).map_err(|_| ())
}

/// Store data threaded into each execution: the WASI context, the S1 resource
/// limiter, the concept's granted scopes (S3), and the extension-call bridge.
struct ExecutorCtx {
    wasi: WasiP1Ctx,
    limits: StoreLimits,
    allowed_scopes: Vec<String>,
    extension_callback: Option<ExtensionCallback>,
}

pub struct WasmExecutor {
    engine: Engine,
    module: Module,
    /// Signals the epoch ticker thread to shut down when the executor is dropped.
    shutdown: Arc<AtomicBool>,
}

impl WasmExecutor {
    pub fn new() -> Result<Self> {
        // A compute-only execution environment. Host capabilities (MCP/API/DB) are
        // I/O-bound, so the host bridge and guest calls run on the async path
        // (`instantiate_async`/`call_async`/`func_wrap_async`); wasmtime 46 enables
        // async unconditionally, so no explicit `async_support` toggle is needed.
        let mut config = Config::new();

        // Trim features we do not need; keep bulk memory / multi-value (used by
        // the QuickJS guest and multi-return host imports).
        config.wasm_simd(false);
        config.wasm_relaxed_simd(false);
        config.wasm_bulk_memory(true);
        config.wasm_multi_value(true);

        // Epoch interruption backs the execution timeout.
        config.epoch_interruption(true);

        let engine = Engine::new(&config)?;

        // Background thread advances the epoch every tick; exits on Drop.
        let shutdown = Arc::new(AtomicBool::new(false));
        let ticker_shutdown = Arc::clone(&shutdown);
        let ticker_engine = engine.clone();
        std::thread::spawn(move || {
            while !ticker_shutdown.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(EPOCH_TICK_MS));
                ticker_engine.increment_epoch();
            }
        });

        // Compile the embedded module once; reused across executions.
        let module = Module::from_binary(&engine, QUICKJS_WASM)?;

        Ok(Self {
            engine,
            module,
            shutdown,
        })
    }

    /// Evaluate a JavaScript program in the sandbox.
    ///
    /// - `code`: the program source (fed to the guest via stdin).
    /// - `handle_id`: opaque handle for host state access (reserved for host imports).
    /// - `host_imports`: available host-import descriptors (reserved).
    /// - `scopes`: the concept's granted capability scopes; a `callExtension(ext, func)`
    ///   is permitted only if `"ext:func"` is present (S3, default-deny).
    /// - `timeout_ms`: execution timeout; defaults to [`DEFAULT_TIMEOUT_MS`].
    /// - `extension_callback`: bridge invoked for permitted host-capability calls.
    pub async fn evaluate(
        &self,
        code: &str,
        _handle_id: Uuid,
        _host_imports: &HostImports,
        scopes: &[String],
        timeout_ms: Option<u64>,
        extension_callback: Option<ExtensionCallback>,
    ) -> ExecutionResult {
        let start_time = std::time::Instant::now();
        let fail = |msg: String| ExecutionResult {
            output: serde_json::Value::Null,
            duration_ms: u64::try_from(start_time.elapsed().as_millis()).unwrap_or(0),
            engine: ScriptEngine::QuickJs,
            success: false,
            error: Some(msg),
        };

        // 1. Virtual stdin (program) / stdout (JSON result). No fs or network.
        let stdin = MemoryInputPipe::new(code.to_string());
        let stdout = MemoryOutputPipe::new(10 * 1024 * 1024); // 10 MiB output cap

        let mut wasi_builder = WasiCtxBuilder::new();
        wasi_builder.stdin(stdin);
        wasi_builder.stdout(stdout.clone());
        let wasi_ctx = wasi_builder.build_p1();

        // 2. Store data: WASI, S1 limits, S3 scopes, extension bridge.
        let ctx = ExecutorCtx {
            wasi: wasi_ctx,
            limits: StoreLimitsBuilder::new()
                .memory_size(MAX_MEMORY_BYTES)
                .table_elements(MAX_TABLE_ELEMENTS)
                .instances(1)
                .memories(1)
                .tables(1)
                .trap_on_grow_failure(true)
                .build(),
            allowed_scopes: scopes.to_vec(),
            extension_callback,
        };
        let mut store = Store::new(&self.engine, ctx);
        store.limiter(|ctx| &mut ctx.limits);

        // Execution timeout via epoch deadline (1 epoch == EPOCH_TICK_MS).
        let timeout_epochs = timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS) / EPOCH_TICK_MS;
        store.set_epoch_deadline(timeout_epochs.max(1));

        // 3. Link WASI Preview 1 (async) imports.
        let mut linker: Linker<ExecutorCtx> = Linker::new(&self.engine);
        if let Err(e) = wasmtime_wasi::p1::add_to_linker_async(&mut linker, |ctx| &mut ctx.wasi) {
            return fail(format!("Failed to link WASI: {e}"));
        }

        // 4. Link the async host-capability bridge (S5: propagate link errors).
        if let Err(e) = linker.func_wrap_async(
            "env",
            "host_call_extension",
            |mut caller: Caller<'_, ExecutorCtx>,
             (
                ext_name_ptr,
                ext_name_len,
                func_name_ptr,
                func_name_len,
                args_json_ptr,
                args_json_len,
                out_buf_ptr,
                out_buf_capacity,
            ): (u32, u32, u32, u32, u32, u32, u32, u32)|
             -> Box<dyn Future<Output = i32> + Send + '_> {
                Box::new(async move {
                    let memory = match caller.get_export("memory") {
                        Some(wasmtime::Extern::Memory(m)) => m,
                        _ => return -1,
                    };

                    let ext_name = match read_string(&memory, &caller, ext_name_ptr, ext_name_len) {
                        Ok(s) => s,
                        Err(()) => return -2,
                    };
                    let func_name =
                        match read_string(&memory, &caller, func_name_ptr, func_name_len) {
                            Ok(s) => s,
                            Err(()) => return -3,
                        };
                    let args_json_str =
                        match read_string(&memory, &caller, args_json_ptr, args_json_len) {
                            Ok(s) => s,
                            Err(()) => return -4,
                        };
                    let args_json: serde_json::Value =
                        serde_json::from_str(&args_json_str).unwrap_or(serde_json::Value::Null);

                    // S3: default-deny. Only permit the call if the concept was
                    // granted the matching "ext:func" scope in its frontmatter.
                    let required_scope = format!("{ext_name}:{func_name}");
                    let granted = caller
                        .data()
                        .allowed_scopes
                        .iter()
                        .any(|s| s == &required_scope);

                    let res: Result<String, String> = if !granted {
                        Err(format!(
                            "scope '{required_scope}' not granted to this concept"
                        ))
                    } else {
                        // The callback resolves synchronously today; awaiting here
                        // keeps the bridge ready for I/O-bound capabilities.
                        match caller.data().extension_callback.clone() {
                            Some(callback) => {
                                callback(&ext_name, &func_name, args_json).map(|v| v.to_string())
                            }
                            None => Err("No extensions loaded".to_string()),
                        }
                    };

                    // Write result/error into the guest buffer. Positive length =>
                    // success payload; negative length => error (magnitude = len).
                    match res {
                        Ok(s) => {
                            let bytes = s.as_bytes();
                            if bytes.len() as u32 > out_buf_capacity {
                                return -5;
                            }
                            if memory
                                .write(&mut caller, out_buf_ptr as usize, bytes)
                                .is_err()
                            {
                                return -6;
                            }
                            bytes.len() as i32
                        }
                        Err(e) => {
                            let bytes = e.as_bytes();
                            let len = (bytes.len() as u32).min(out_buf_capacity) as usize;
                            let _ = memory.write(&mut caller, out_buf_ptr as usize, &bytes[..len]);
                            -(len as i32)
                        }
                    }
                })
            },
        ) {
            return fail(format!("Failed to link host_call_extension: {e}"));
        }

        // 5. Instantiate and run `_start` on the async path.
        let instance = match linker.instantiate_async(&mut store, &self.module).await {
            Ok(i) => i,
            Err(e) => return fail(format!("Failed to instantiate Wasm module: {e}")),
        };
        let start_func = match instance.get_typed_func::<(), ()>(&mut store, "_start") {
            Ok(f) => f,
            Err(e) => return fail(format!("Failed to find _start: {e}")),
        };
        if let Err(e) = start_func.call_async(&mut store, ()).await {
            return fail(format!("JS Execution trap: {e}"));
        }

        // 6. Parse the JSON the guest wrote to stdout.
        let output_bytes = stdout.contents();
        let output_str = String::from_utf8_lossy(&output_bytes);
        let output_json = serde_json::from_str(&output_str)
            .unwrap_or_else(|_| serde_json::json!({ "raw_output": output_str.trim() }));

        // The guest signals failure by emitting {"success": false, "error": ...}.
        let mut success = true;
        let mut error = None;
        if let Some(obj) = output_json.as_object() {
            if let Some(succ) = obj.get("success").and_then(serde_json::Value::as_bool) {
                success = succ;
            }
            if let Some(err) = obj.get("error").and_then(serde_json::Value::as_str) {
                error = Some(err.to_string());
            }
        }

        ExecutionResult {
            output: output_json,
            duration_ms: u64::try_from(start_time.elapsed().as_millis()).unwrap_or(0),
            engine: ScriptEngine::QuickJs,
            success,
            error,
        }
    }
}

impl Drop for WasmExecutor {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}
