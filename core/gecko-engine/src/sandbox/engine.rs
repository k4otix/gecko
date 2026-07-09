//! Shared sandbox types.
//!
//! Common types used by the single WebAssembly executor ([`super::wasm_executor`]):
//! host-import descriptors, the extension-call bridge, and the execution result.

use serde::{Deserialize, Serialize};

use crate::okf::types::ScriptEngine;

// The host-import descriptor is part of the extension contract, so it lives in
// `gecko-extension-api`. Re-exported here to preserve the
// `gecko_engine::sandbox::engine::HostImportDef` path used across the engine.
pub use gecko_extension_api::HostImportDef;

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
