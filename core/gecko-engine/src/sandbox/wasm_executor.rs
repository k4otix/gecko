use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use uuid::Uuid;
use wasmtime::{Config, Engine, Linker, Module, Store};
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::{WasiCtxBuilder, p1::WasiP1Ctx};

use crate::okf::types::ScriptEngine;

use super::engine::{ExecutionResult, HostImports, ScriptExecutor};

// Embed the pre-compiled Boa JavaScript WebAssembly binary into the executable.
const QUICKJS_WASM: &[u8] = include_bytes!("quickjs.wasm");

fn read_string<T>(
    memory: &wasmtime::Memory,
    caller: &wasmtime::Caller<'_, T>,
    ptr: u32,
    len: u32,
) -> Result<String, ()> {
    let mut buf = vec![0u8; len as usize];
    memory
        .read(caller, ptr as usize, &mut buf)
        .map_err(|_| ())?;
    String::from_utf8(buf).map_err(|_| ())
}

pub struct WasmExecutor {
    engine: Engine,
    module: Module,
    /// Signals the epoch ticker thread to shut down when dropped.
    shutdown: Arc<AtomicBool>,
}

struct ExecutorCtx {
    wasi: WasiP1Ctx,
    extension_callback: Option<crate::sandbox::engine::ExtensionCallback>,
}

impl WasmExecutor {
    pub fn new() -> Result<Self> {
        // Configure a highly secure, compute-only execution environment
        let mut config = Config::new();

        // Disable all caching and unneeded features
        config.wasm_simd(false);
        config.wasm_relaxed_simd(false);
        config.wasm_bulk_memory(true);
        config.wasm_multi_value(true);

        // Enable Epoch Interruption for timeout mechanisms
        config.epoch_interruption(true);

        let engine = Engine::new(&config)?;

        // Spawn a background thread to tick the epoch every 10ms.
        // The thread exits when the shutdown flag is set (via Drop).
        let shutdown = Arc::new(AtomicBool::new(false));
        let ticker_shutdown = Arc::clone(&shutdown);
        let ticker_engine = engine.clone();
        std::thread::spawn(move || {
            while !ticker_shutdown.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(10));
                ticker_engine.increment_epoch();
            }
        });

        // Compile the embedded WebAssembly module once
        let module = Module::from_binary(&engine, QUICKJS_WASM)?;

        Ok(Self {
            engine,
            module,
            shutdown,
        })
    }
}

impl Drop for WasmExecutor {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

impl ScriptExecutor for WasmExecutor {
    fn evaluate(
        &self,
        code: &str,
        _handle_id: Uuid,
        _host_imports: &HostImports,
        timeout_ms: Option<u64>,
        extension_callback: Option<crate::sandbox::engine::ExtensionCallback>,
    ) -> ExecutionResult {
        let start_time = std::time::Instant::now();

        // 1. Create Virtual Pipes for Stdin/Stdout
        let stdin = MemoryInputPipe::new(code.to_string());
        let stdout = MemoryOutputPipe::new(10 * 1024 * 1024); // 10MB limit

        // 2. Build the WASI context with strictly NO filesystem/network access
        let mut wasi_builder = WasiCtxBuilder::new();
        wasi_builder.stdin(stdin);
        wasi_builder.stdout(stdout.clone());
        let wasi_ctx = wasi_builder.build_p1();

        // 3. Create a new store for this specific execution
        let ctx = ExecutorCtx {
            wasi: wasi_ctx,
            extension_callback,
        };
        let mut store = Store::new(&self.engine, ctx);

        // Set the timeout epoch (1 epoch = 10ms)
        let timeout_epochs = timeout_ms.unwrap_or(30000) / 10;
        store.set_epoch_deadline(timeout_epochs);

        // 4. Link WASI Preview 1 imports
        let mut linker: Linker<ExecutorCtx> = Linker::new(&self.engine);
        if let Err(e) = wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |ctx| &mut ctx.wasi) {
            return ExecutionResult {
                output: serde_json::Value::Null,
                duration_ms: start_time.elapsed().as_millis() as u64,
                engine: ScriptEngine::QuickJs,
                success: false,
                error: Some(format!("Failed to link WASI: {e}")),
            };
        }

        // 5. Link custom host imports
        let _ = linker.func_wrap(
            "env",
            "host_call_extension",
            |mut caller: wasmtime::Caller<'_, ExecutorCtx>,
             ext_name_ptr: u32,
             ext_name_len: u32,
             func_name_ptr: u32,
             func_name_len: u32,
             args_json_ptr: u32,
             args_json_len: u32,
             out_buf_ptr: u32,
             out_buf_capacity: u32|
             -> i32 {
                let memory = match caller.get_export("memory") {
                    Some(wasmtime::Extern::Memory(m)) => m,
                    _ => return -1, // Error
                };

                // Read strings from memory
                let ext_name = match read_string(&memory, &caller, ext_name_ptr, ext_name_len) {
                    Ok(s) => s,
                    Err(()) => return -2,
                };
                let func_name = match read_string(&memory, &caller, func_name_ptr, func_name_len) {
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

                // Execute callback if it exists
                let cb = caller.data().extension_callback.clone();
                let res = match cb {
                    Some(callback) => callback(&ext_name, &func_name, args_json),
                    None => Err("No extensions loaded".to_string()),
                };

                let res_str = match res {
                    Ok(v) => v.to_string(),
                    Err(e) => {
                        // Error string
                        let bytes = e.as_bytes();
                        let len = bytes.len() as u32;
                        if len > out_buf_capacity {
                            return -5;
                        }
                        if memory
                            .write(&mut caller, out_buf_ptr as usize, bytes)
                            .is_err()
                        {
                            return -6;
                        }
                        return -(len as i32); // Negative length indicates error
                    }
                };

                let bytes = res_str.as_bytes();
                let len = bytes.len() as u32;
                if len > out_buf_capacity {
                    return -7;
                }

                if memory
                    .write(&mut caller, out_buf_ptr as usize, bytes)
                    .is_err()
                {
                    return -8;
                }

                len as i32
            },
        );

        // 6. Instantiate the module
        match linker.instantiate(&mut store, &self.module) {
            Ok(instance) => {
                // Find and execute the WASI _start function
                match instance.get_typed_func::<(), ()>(&mut store, "_start") {
                    Ok(start) => {
                        if let Err(e) = start.call(&mut store, ()) {
                            return ExecutionResult {
                                output: serde_json::Value::Null,
                                duration_ms: start_time.elapsed().as_millis() as u64,
                                engine: ScriptEngine::QuickJs,
                                success: false,
                                error: Some(format!("JS Execution trap: {e}")),
                            };
                        }
                    }
                    Err(e) => {
                        return ExecutionResult {
                            output: serde_json::Value::Null,
                            duration_ms: start_time.elapsed().as_millis() as u64,
                            engine: ScriptEngine::QuickJs,
                            success: false,
                            error: Some(format!("Failed to find _start: {e}")),
                        };
                    }
                }

                // 6. Read the JSON output from the virtual stdout pipe
                let output_bytes = stdout.contents();
                let output_str = String::from_utf8_lossy(&output_bytes);

                // Try to parse the JS engine's output as JSON
                let output_json = serde_json::from_str(&output_str).unwrap_or_else(|_| {
                    serde_json::json!({
                        "raw_output": output_str.trim()
                    })
                });

                // Extract success and error fields if present in the JSON payload
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
            Err(e) => ExecutionResult {
                output: serde_json::Value::Null,
                duration_ms: start_time.elapsed().as_millis() as u64,
                engine: ScriptEngine::QuickJs,
                success: false,
                error: Some(format!("Failed to instantiate Wasm module: {e}")),
            },
        }
    }
}
