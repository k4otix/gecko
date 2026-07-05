//! Rhai script executor.
//!
//! Phase 1 implementation: embeds Rhai natively in the host process with
//! step-capped evaluation for deterministic, bounded execution (design §3.2).

use std::time::Instant;

use rhai::Engine;
use tracing::{debug, warn};
use uuid::Uuid;

use super::engine::{ExecutionResult, HostImports, ScriptExecutor};
use crate::okf::types::ScriptEngine;

/// Default maximum number of operations before Rhai aborts.
const DEFAULT_MAX_OPERATIONS: u64 = 100_000;

/// Rhai-based script executor with step-capped evaluation.
pub struct RhaiExecutor {
    max_operations: u64,
}

impl RhaiExecutor {
    pub fn new() -> Self {
        Self {
            max_operations: DEFAULT_MAX_OPERATIONS,
        }
    }

    pub fn with_max_operations(mut self, max: u64) -> Self {
        self.max_operations = max;
        self
    }

    /// Creates a configured Rhai engine with safety limits.
    fn build_engine(&self) -> Engine {
        let mut engine = Engine::new();

        // Safety: cap the number of operations to prevent infinite loops
        engine.set_max_operations(self.max_operations);

        // Safety: limit recursion depth
        engine.set_max_call_levels(32);

        // Safety: limit string size (1MB)
        engine.set_max_string_size(1_048_576);

        // Safety: limit array size
        engine.set_max_array_size(10_000);

        // Safety: limit map size
        engine.set_max_map_size(10_000);

        engine
    }
}

impl Default for RhaiExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptExecutor for RhaiExecutor {
    fn evaluate(
        &self,
        code: &str,
        handle_id: Uuid,
        _host_imports: &HostImports,
        _timeout_ms: Option<u64>,
        _extension_callback: Option<crate::sandbox::engine::ExtensionCallback>,
    ) -> ExecutionResult {
        let start = Instant::now();
        let engine = self.build_engine();

        debug!(handle = %handle_id, "Executing Rhai script");

        match engine.eval::<rhai::Dynamic>(code) {
            Ok(value) => {
                let duration = start.elapsed().as_millis() as u64;
                let output = dynamic_to_json(&value);

                ExecutionResult {
                    output,
                    duration_ms: duration,
                    engine: ScriptEngine::Rhai,
                    success: true,
                    error: None,
                }
            }
            Err(e) => {
                let duration = start.elapsed().as_millis() as u64;
                warn!(handle = %handle_id, error = %e, "Rhai execution failed");

                ExecutionResult {
                    output: serde_json::Value::Null,
                    duration_ms: duration,
                    engine: ScriptEngine::Rhai,
                    success: false,
                    error: Some(e.to_string()),
                }
            }
        }
    }
}

/// Converts a Rhai Dynamic value to serde_json::Value.
fn dynamic_to_json(value: &rhai::Dynamic) -> serde_json::Value {
    if value.is_unit() {
        serde_json::Value::Null
    } else if let Some(b) = value.as_bool().ok() {
        serde_json::Value::Bool(b)
    } else if let Some(i) = value.as_int().ok() {
        serde_json::json!(i)
    } else if let Some(f) = value.as_float().ok() {
        serde_json::json!(f)
    } else if let Some(s) = value.clone().into_string().ok() {
        serde_json::Value::String(s)
    } else {
        // Fallback: use debug representation
        serde_json::Value::String(format!("{:?}", value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rhai_simple_expression() {
        let executor = RhaiExecutor::new();
        let result = executor.evaluate(
            "40 + 2",
            Uuid::new_v4(),
            &HostImports::default(),
            None,
            None,
        );

        assert!(result.success);
        assert_eq!(result.output, serde_json::json!(42));
        assert_eq!(result.engine, ScriptEngine::Rhai);
    }

    #[test]
    fn test_rhai_string_result() {
        let executor = RhaiExecutor::new();
        let result = executor.evaluate(
            r#""hello" + " world""#,
            Uuid::new_v4(),
            &HostImports::default(),
            None,
            None,
        );

        assert!(result.success);
        assert_eq!(result.output, serde_json::json!("hello world"));
    }

    #[test]
    fn test_rhai_operation_limit() {
        let executor = RhaiExecutor::new().with_max_operations(100);
        let result = executor.evaluate(
            "let x = 0; while true { x += 1; } x",
            Uuid::new_v4(),
            &HostImports::default(),
            None,
            None,
        );

        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_rhai_syntax_error() {
        let executor = RhaiExecutor::new();
        let result = executor.evaluate(
            "this is not valid rhai",
            Uuid::new_v4(),
            &HostImports::default(),
            None,
            None,
        );

        assert!(!result.success);
        assert!(result.error.is_some());
    }
}
