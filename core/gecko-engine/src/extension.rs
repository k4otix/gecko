//! Extension trait for pluggable domain logic.
//!
//! Extensions implement `GeckoExtension` to register their TypeDB schemas
//! and host-imported functions. The core engine never imports extension code
//! directly — this trait is the only interface (design §2, §4).

use crate::sandbox::engine::HostImportDef;

/// Trait that domain-specific extensions must implement.
///
/// The Assembler binary (`gecko-bin`) instantiates extension structs and passes
/// them to the engine via this trait. This wires TypeDB schemas and host-imported
/// Wasm functions together at startup without violating the dependency graph.
///
/// # The Golden Rule
/// `gecko-engine` MUST NEVER import or reference anything in `extensions/`.
/// If a feature feels like it belongs in the core but requires knowledge of
/// an IP address, an asset, or a SIEM, the interface is abstracted here,
/// and the logic is implemented in the extension.
pub trait GeckoExtension: Send + Sync {
    /// Extension name (e.g., "cyber-gecko", "mem-gecko").
    fn name(&self) -> &str;

    /// TypeQL schema to apply additively on top of the core schema.
    ///
    /// Must use `sub` to inherit from `okf-concept` or `okf-link` types
    /// defined in the core schema (design §4).
    fn schema(&self) -> &str;

    /// Host-imported functions this extension provides to sandboxed scripts.
    ///
    /// These are registered with the sandbox engine and callable from
    /// Rhai/QuickJS code during playbook execution.
    fn host_imports(&self) -> Vec<HostImportDef>;

    /// Optional initialization hook called after schema application.
    /// Extensions can use this to seed initial data or validate configuration.
    fn on_init(&self) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }

    /// Execute a host import function provided by this extension.
    fn call_import(
        &self,
        name: &str,
        _args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        Err(format!("Import '{name}' not implemented"))
    }
}
