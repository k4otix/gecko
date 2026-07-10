//! Sandbox execution pool.
//!
//! Holds the single WebAssembly executor (QuickJS guest), lazily initialized on
//! first use to avoid paying the WASM compilation cost until a program actually
//! runs. Execution is WASM-only, so there is no cross-engine dispatch — MCP/API/
//! DB/TypeQL are scope-gated host capabilities, not separate engines.

use std::sync::OnceLock;

use tracing::info;
use uuid::Uuid;

use super::engine::{ExecutionResult, ExtensionCallback, HostImports};
use super::wasm_executor::WasmExecutor;
use crate::okf::types::ScriptEngine;

/// Pool wrapping the lazily-initialized [`WasmExecutor`].
pub struct SandboxPool {
    wasm: OnceLock<Result<WasmExecutor, String>>,
}

impl SandboxPool {
    pub fn new() -> Self {
        Self {
            wasm: OnceLock::new(),
        }
    }

    /// Returns the WasmExecutor, initializing (and compiling the module) on first call.
    fn wasm_executor(&self) -> Result<&WasmExecutor, String> {
        self.wasm
            .get_or_init(|| {
                info!("Initializing WebAssembly sandbox engine");
                WasmExecutor::new().map_err(|e| format!("Failed to initialize Wasm engine: {e}"))
            })
            .as_ref()
            .map_err(std::clone::Clone::clone)
    }

    /// Execute a program in the WASM sandbox. `scopes` are the concept's granted
    /// capability scopes, enforced by the host bridge (S3).
    pub async fn execute(
        &self,
        code: &str,
        handle_id: Uuid,
        host_imports: &HostImports,
        scopes: &[String],
        timeout_ms: Option<u64>,
        extension_callback: Option<ExtensionCallback>,
    ) -> ExecutionResult {
        match self.wasm_executor() {
            Ok(wasm) => {
                wasm.evaluate(
                    code,
                    handle_id,
                    host_imports,
                    scopes,
                    timeout_ms,
                    extension_callback,
                )
                .await
            }
            Err(e) => ExecutionResult {
                output: serde_json::Value::Null,
                duration_ms: 0,
                engine: ScriptEngine::QuickJs,
                success: false,
                error: Some(e),
            },
        }
    }
}

impl Default for SandboxPool {
    fn default() -> Self {
        Self::new()
    }
}
