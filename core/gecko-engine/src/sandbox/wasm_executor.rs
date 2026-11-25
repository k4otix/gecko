//! The WebAssembly sandbox executor.
//!
//! All synced code is untrusted and runs here: a QuickJS (Boa) guest compiled to
//! WASM, hosted by wasmtime with strictly no filesystem or network access. The
//! guest reads its program from stdin and writes a JSON result to stdout.
//!
//! The executor is **async**: instantiation, `_start`, and the
//! `host_call_extension` bridge all run on the async path (`instantiate_async` /
//! `call_async` / `func_wrap_async`). The extension callback is a synchronous `Fn`,
//! but the plumbing is async so I/O-bound host capabilities (MCP, API, DB, TypeQL)
//! can be awaited on the same path.
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

/// Cap on a guest-supplied extension/function name length (bytes). The guest hands
/// the host a `len`; reject an oversized one before allocating so a bogus length
/// cannot force a huge buffer at the untrusted boundary.
const MAX_IMPORT_NAME_LEN: usize = 256;

/// Cap on a guest-supplied args-JSON length (bytes) at the untrusted boundary, for
/// the same reason as [`MAX_IMPORT_NAME_LEN`].
const MAX_ARGS_JSON_LEN: usize = 8 * 1024 * 1024;

/// Cap on a host reply or error message held for the guest to read (bytes).
const MAX_REPLY_LEN: usize = 16 * 1024 * 1024;

/// The message stashed for a reply over [`MAX_REPLY_LEN`].
const REPLY_TOO_LARGE: &str = "host response exceeded 16 MiB";

/// The message stashed for an extension error that carried none.
const EMPTY_ERROR: &str = "the extension call failed with an empty error";

/// Why a guest-supplied string could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadError {
    /// Its length is over the cap.
    TooLong,
    /// `ptr..ptr + len` lies outside guest memory.
    OutOfBounds,
    /// Its bytes are not UTF-8.
    NotUtf8,
}

/// Reads `len` bytes at `ptr` from guest memory `mem` as UTF-8, rejecting a `len`
/// above `max` before allocating (untrusted-boundary DoS guard).
fn read_guest_str(mem: &[u8], ptr: u32, len: u32, max: usize) -> Result<String, ReadError> {
    if len as usize > max {
        return Err(ReadError::TooLong);
    }
    let start = ptr as usize;
    let bytes = start
        .checked_add(len as usize)
        .and_then(|end| mem.get(start..end))
        .ok_or(ReadError::OutOfBounds)?;
    String::from_utf8(bytes.to_vec()).map_err(|_| ReadError::NotUtf8)
}

/// The message for a guest string `what` that failed `e`, `limit` being its cap.
fn read_error(what: &str, limit: &str, e: ReadError) -> String {
    match e {
        ReadError::TooLong => format!("{what} is over {limit}"),
        ReadError::OutOfBounds => format!("{what} lies outside guest memory"),
        ReadError::NotUtf8 => format!("{what} is not UTF-8"),
    }
}

/// Turns a host-call outcome into the bytes the guest will read and the length it
/// is told: `+len` for a reply, `-len` for an error message. A reply over
/// [`MAX_REPLY_LEN`] becomes an error, an error over it is truncated on a UTF-8
/// boundary, and an empty error gets a message, so the length is never 0.
fn stash_outcome(res: Result<String, String>) -> (Vec<u8>, i32) {
    let message = match res {
        Ok(reply) if reply.len() <= MAX_REPLY_LEN => {
            let len = reply.len() as i32;
            return (reply.into_bytes(), len);
        }
        Ok(_) => REPLY_TOO_LARGE.to_string(),
        Err(e) if e.is_empty() => EMPTY_ERROR.to_string(),
        Err(mut e) => {
            if e.len() > MAX_REPLY_LEN {
                let mut cut = MAX_REPLY_LEN;
                while !e.is_char_boundary(cut) {
                    cut -= 1;
                }
                e.truncate(cut);
            }
            e
        }
    };
    let len = message.len() as i32;
    (message.into_bytes(), -len)
}

/// Takes the pending reply for a guest buffer of `out_cap` bytes. The reply is
/// taken before any check, so it can be read at most once: `-1` when none is
/// pending, `-2` when the buffer is too small (the reply is discarded).
fn take_reply(pending: &mut Option<Vec<u8>>, out_cap: u32) -> Result<Vec<u8>, i32> {
    let bytes = pending.take().ok_or(-1)?;
    if (out_cap as usize) < bytes.len() {
        return Err(-2);
    }
    Ok(bytes)
}

/// Takes the pending reply and copies it into guest memory `mem` at `out_ptr`,
/// returning its length, or `-1` none pending, `-2` buffer too small, `-3` buffer
/// outside guest memory. The reply is cleared in every case.
fn copy_reply(pending: &mut Option<Vec<u8>>, mem: &mut [u8], out_ptr: u32, out_cap: u32) -> i32 {
    let bytes = match take_reply(pending, out_cap) {
        Ok(bytes) => bytes,
        Err(code) => return code,
    };
    let start = out_ptr as usize;
    match start
        .checked_add(bytes.len())
        .and_then(|end| mem.get_mut(start..end))
    {
        Some(dest) => {
            dest.copy_from_slice(&bytes);
            bytes.len() as i32
        }
        None => -3,
    }
}

/// The guest's exported linear memory, or a trap: neither import can answer a guest
/// without one.
fn guest_memory<T>(caller: &mut Caller<'_, T>) -> wasmtime::Result<wasmtime::Memory> {
    match caller.get_export("memory") {
        Some(wasmtime::Extern::Memory(m)) => Ok(m),
        _ => Err(wasmtime::Error::msg("guest exports no linear memory")),
    }
}

/// Store data threaded into each execution: the WASI context, the S1 resource
/// limiter, the concept's granted scopes (S3), and the extension-call bridge.
struct ExecutorCtx {
    wasi: WasiP1Ctx,
    limits: StoreLimits,
    allowed_scopes: Vec<String>,
    extension_callback: Option<ExtensionCallback>,
    /// The outcome of the last `host_call_extension`, until `host_read_reply` takes it.
    pending_reply: Option<Vec<u8>>,
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
        // (`instantiate_async`/`call_async`/`func_wrap_async`). Async support is
        // always on, so no explicit `async_support` toggle is needed.
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
            pending_reply: None,
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

        // 4. Link the host-call bridge (S5: propagate link errors). A call runs the
        //    extension callback at most once and stashes its outcome; the guest
        //    sizes a buffer from the returned length and reads it with
        //    `host_read_reply`.
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
            ): (u32, u32, u32, u32, u32, u32)|
             -> Box<dyn Future<Output = wasmtime::Result<i32>> + Send + '_> {
                Box::new(async move {
                    // A reply is read at most once, and never by a later call.
                    caller.data_mut().pending_reply = None;
                    let memory = guest_memory(&mut caller)?;

                    let res: Result<String, String> = (|| {
                        let mem = memory.data(&caller);
                        let ext_name =
                            read_guest_str(mem, ext_name_ptr, ext_name_len, MAX_IMPORT_NAME_LEN)
                                .map_err(|e| read_error("the extension name", "256 bytes", e))?;
                        let func_name =
                            read_guest_str(mem, func_name_ptr, func_name_len, MAX_IMPORT_NAME_LEN)
                                .map_err(|e| read_error("the function name", "256 bytes", e))?;

                        // S3: default-deny, checked before the args are read, so an
                        // ungranted call learns nothing about its payload.
                        let required_scope = format!("{ext_name}:{func_name}");
                        if !caller
                            .data()
                            .allowed_scopes
                            .iter()
                            .any(|s| s == &required_scope)
                        {
                            return Err(format!(
                                "scope '{required_scope}' not granted to this concept"
                            ));
                        }

                        let args_json_str = read_guest_str(
                            memory.data(&caller),
                            args_json_ptr,
                            args_json_len,
                            MAX_ARGS_JSON_LEN,
                        )
                        .map_err(|e| read_error("the args JSON", "8 MiB", e))?;
                        let args = serde_json::from_str::<serde_json::Value>(&args_json_str)
                            .map_err(|e| format!("invalid args JSON: {e}"))?;
                        let callback = caller
                            .data()
                            .extension_callback
                            .clone()
                            .ok_or_else(|| "No extensions loaded".to_string())?;
                        callback(&ext_name, &func_name, args).map(|v| v.to_string())
                    })();

                    let (bytes, ret) = stash_outcome(res);
                    caller.data_mut().pending_reply = Some(bytes);
                    Ok(ret)
                })
            },
        ) {
            return fail(format!("Failed to link host_call_extension: {e}"));
        }

        // The guest reads the stashed outcome into a buffer it sized from the
        // returned length: `len` on success, `-1` none pending, `-2` buffer too
        // small, `-3` out-of-bounds write. The reply is cleared either way.
        if let Err(e) = linker.func_wrap(
            "env",
            "host_read_reply",
            |mut caller: Caller<'_, ExecutorCtx>,
             out_ptr: u32,
             out_cap: u32|
             -> wasmtime::Result<i32> {
                let memory = guest_memory(&mut caller)?;
                let (mem, ctx) = memory.data_and_store_mut(&mut caller);
                Ok(copy_reply(&mut ctx.pending_reply, mem, out_ptr, out_cap))
            },
        ) {
            return fail(format!("Failed to link host_read_reply: {e}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stash_outcome_caps_an_oversized_reply() {
        let (bytes, ret) = stash_outcome(Ok("x".repeat(MAX_REPLY_LEN + 1)));
        assert_eq!(bytes, b"host response exceeded 16 MiB");
        assert_eq!(ret, -29);
    }

    #[test]
    fn stash_outcome_returns_a_reply_with_its_length() {
        let (bytes, ret) = stash_outcome(Ok("{}".to_string()));
        assert_eq!(bytes, b"{}");
        assert_eq!(ret, 2);
    }

    #[test]
    fn stash_outcome_gives_an_empty_error_a_message() {
        let (bytes, ret) = stash_outcome(Err(String::new()));
        assert!(ret < 0);
        assert_eq!(bytes.len(), ret.unsigned_abs() as usize);
    }

    #[test]
    fn stash_outcome_truncates_an_oversized_error_on_a_char_boundary() {
        let (bytes, ret) = stash_outcome(Err("é".repeat(MAX_REPLY_LEN)));
        assert!(bytes.len() <= MAX_REPLY_LEN);
        assert!(String::from_utf8(bytes.clone()).is_ok(), "valid UTF-8");
        assert_eq!(ret, -(bytes.len() as i32));
    }

    #[test]
    fn take_reply_reads_once() {
        let mut pending = Some(b"{}".to_vec());
        assert_eq!(take_reply(&mut pending, 2), Ok(b"{}".to_vec()));
        assert_eq!(take_reply(&mut pending, 2), Err(-1));
    }

    #[test]
    fn read_guest_str_names_each_failure() {
        let mem = b"ok\xff\xfe".to_vec();
        assert_eq!(read_guest_str(&mem, 0, 2, 8), Ok("ok".to_string()));
        assert_eq!(read_guest_str(&mem, 0, 9, 8), Err(ReadError::TooLong));
        assert_eq!(read_guest_str(&mem, 3, 2, 8), Err(ReadError::OutOfBounds));
        assert_eq!(
            read_guest_str(&mem, u32::MAX, 2, 8),
            Err(ReadError::OutOfBounds)
        );
        assert_eq!(read_guest_str(&mem, 2, 2, 8), Err(ReadError::NotUtf8));
    }

    #[test]
    fn boundary_messages_name_the_failed_check() {
        assert_eq!(
            read_error("the args JSON", "8 MiB", ReadError::TooLong),
            "the args JSON is over 8 MiB"
        );
        assert_eq!(
            read_error("the extension name", "256 bytes", ReadError::OutOfBounds),
            "the extension name lies outside guest memory"
        );
        assert_eq!(
            read_error("the function name", "256 bytes", ReadError::NotUtf8),
            "the function name is not UTF-8"
        );
    }

    #[test]
    fn copy_reply_refuses_an_out_of_bounds_buffer_and_clears() {
        let mut mem = vec![0u8; 16];
        let mut pending = Some(b"{}".to_vec());
        assert_eq!(copy_reply(&mut pending, &mut mem, 15, 2), -3);
        assert_eq!(
            copy_reply(&mut pending, &mut mem, 0, 2),
            -1,
            "the reply was cleared"
        );

        let mut pending = Some(b"{}".to_vec());
        assert_eq!(copy_reply(&mut pending, &mut mem, 4, 2), 2);
        assert_eq!(&mem[4..6], b"{}");
    }

    #[test]
    fn take_reply_clears_on_a_short_buffer() {
        let mut pending = Some(b"{}".to_vec());
        assert_eq!(take_reply(&mut pending, 1), Err(-2));
        assert_eq!(take_reply(&mut pending, 2), Err(-1));
    }
}
