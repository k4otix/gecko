//! Sandbox execution pool.
//!
//! Dispatches script execution to the appropriate engine:
//! - `Rhai`: Native embedded Rhai engine with step-capped evaluation.
//! - `QuickJs`: JavaScript execution via a WebAssembly-sandboxed JS runtime.

use std::sync::OnceLock;

use tracing::info;
use uuid::Uuid;

use super::engine::{ExecutionResult, HostImports, ScriptExecutor};
use super::rhai_executor::RhaiExecutor;
use super::wasm_executor::WasmExecutor;
use crate::okf::types::ScriptEngine;

/// Pool of sandbox instances.
///
/// Holds a Rhai executor (always available) and a lazily-initialized
/// WasmExecutor for QuickJS (initialized on first JS execution to avoid
/// paying the Wasm compilation cost when only Rhai scripts are used).
pub struct SandboxPool {
    rhai: RhaiExecutor,
    wasm: OnceLock<Result<WasmExecutor, String>>,
}

impl SandboxPool {
    pub fn new() -> Self {
        Self {
            rhai: RhaiExecutor::new(),
            wasm: OnceLock::new(),
        }
    }

    /// Returns a reference to the WasmExecutor, initializing it on first call.
    fn wasm_executor(&self) -> Result<&WasmExecutor, String> {
        self.wasm
            .get_or_init(|| {
                info!("Initializing WebAssembly sandbox engine");
                WasmExecutor::new().map_err(|e| format!("Failed to initialize Wasm engine: {e}"))
            })
            .as_ref()
            .map_err(std::clone::Clone::clone)
    }

    /// Dispatch execution to the appropriate engine based on the script type.
    pub fn execute(
        &self,
        engine: &ScriptEngine,
        code: &str,
        handle_id: Uuid,
        host_imports: &HostImports,
        timeout_ms: Option<u64>,
        extension_callback: Option<crate::sandbox::engine::ExtensionCallback>,
    ) -> ExecutionResult {
        match engine {
            ScriptEngine::Rhai => self.rhai.evaluate(
                code,
                handle_id,
                host_imports,
                timeout_ms,
                extension_callback,
            ),
            ScriptEngine::QuickJs => match self.wasm_executor() {
                Ok(wasm) => {
                    wasm.evaluate(code, handle_id, host_imports, timeout_ms, extension_callback)
                }
                Err(e) => ExecutionResult {
                    output: serde_json::Value::Null,
                    duration_ms: 0,
                    engine: ScriptEngine::QuickJs,
                    success: false,
                    error: Some(e),
                },
            },
        }
    }
}

impl Default for SandboxPool {
    fn default() -> Self {
        Self::new()
    }
}

