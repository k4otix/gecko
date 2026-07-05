//! Wasm pool placeholder for Phase 2.
//!
//! Phase 1: This module provides a passthrough that delegates to native executors.
//! Phase 2: Will use `wasmtime::PoolingAllocator` for pre-warmed Wasm sandbox
//! instances with embedded Rhai/QuickJS modules (design §3.2).

use uuid::Uuid;

use super::engine::{ExecutionResult, HostImports, ScriptExecutor};
use super::rhai_executor::RhaiExecutor;
use crate::okf::types::ScriptEngine;

/// Pool of sandbox instances.
///
/// Phase 1: Delegates directly to native executors.
/// Phase 2: Will manage pre-allocated Wasm memory blocks via wasmtime PoolingAllocator.
pub struct SandboxPool {
    rhai: RhaiExecutor,
    // Phase 2: quickjs: QuickJsExecutor,
    // Phase 2: wasm_engine: wasmtime::Engine,
    // Phase 2: instance_pool: wasmtime::PoolingAllocator,
}

impl SandboxPool {
    pub fn new() -> Self {
        Self {
            rhai: RhaiExecutor::new(),
        }
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
            ScriptEngine::QuickJs => {
                // Phase 1: QuickJS not yet implemented
                ExecutionResult {
                    output: serde_json::Value::Null,
                    duration_ms: 0,
                    engine: ScriptEngine::QuickJs,
                    success: false,
                    error: Some("QuickJS executor not yet implemented (Phase 2)".to_string()),
                }
            }
        }
    }
}

impl Default for SandboxPool {
    fn default() -> Self {
        Self::new()
    }
}
