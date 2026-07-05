//! Script executor trait and common types.
//!
//! All sandbox implementations (Rhai, QuickJS, future Wasm-isolated variants)
//! implement the `ScriptExecutor` trait for uniform dispatch.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::okf::types::ScriptEngine;

/// Host-imported function definition exposed to sandboxed scripts.
#[derive(Debug, Clone)]
pub struct HostImportDef {
    /// Function name callable from the script (e.g., "mde_isolate").
    pub name: String,
    /// Human-readable description.
    pub description: String,
}

/// Collection of host-imported functions available to a sandbox.
#[derive(Debug, Clone, Default)]
pub struct HostImports {
    pub definitions: Vec<HostImportDef>,
}

/// Callback for executing extension host imports.
pub type ExtensionCallback = std::sync::Arc<
    dyn Fn(&str, &str, serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync,
>;

/// Result of a sandboxed script execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Structured output from the script.
    pub output: serde_json::Value,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
    /// Which engine was used.
    pub engine: ScriptEngine,
    /// Whether execution completed successfully.
    pub success: bool,
    /// Error message if execution failed.
    pub error: Option<String>,
}

/// Trait for script execution engines.
///
/// Implementations provide sandboxed evaluation of code blocks with access
/// to host state via opaque UUID handles.
pub trait ScriptExecutor: Send + Sync {
    /// Evaluate a code string in the sandbox.
    ///
    /// - `code`: The script source to execute.
    /// - `handle_id`: Opaque UUID for accessing host state via imports.
    /// - `host_imports`: Available host functions.
    /// - `timeout_ms`: Optional execution timeout.
    fn evaluate(
        &self,
        code: &str,
        handle_id: Uuid,
        host_imports: &HostImports,
        timeout_ms: Option<u64>,
        extension_callback: Option<ExtensionCallback>,
    ) -> ExecutionResult;
}
