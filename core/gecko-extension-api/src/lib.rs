//! # gecko-extension-api
//!
//! The **extension contract** for the GECKO framework — and nothing else.
//!
//! Domain extensions (`cyber-gecko`) and the always-on substrate (`mem-gecko`)
//! implement [`GeckoExtension`] to contribute a TypeQL schema and host-imported
//! functions. Historically that trait lived inside `gecko-engine`, which forced
//! every extension to depend on the *entire* engine (parser, sandbox, syncer,
//! router) just to implement one trait.
//!
//! This crate inverts that: it holds only the stabilized, reviewable contract.
//! Extensions depend on this thin crate; `gecko-engine` re-exports these types
//! for its internal plumbing; the Assembler (`gecko-bin`) dispatches over
//! `Box<dyn GeckoExtension>`.
//!
//! ## The security invariant
//! The host mediates every capability. **Registration is the gate**: an
//! extension that `gecko-bin` registers gets its [`host_imports`] loaded and its
//! write-paths opened; an unregistered one gets neither. Extensions never expose
//! a write-path that bypasses host mediation.
//!
//! [`host_imports`]: GeckoExtension::host_imports

use serde::{Deserialize, Serialize};

/// Host-imported function definition exposed to sandboxed scripts.
///
/// Returned by [`GeckoExtension::host_imports`] to declare which functions the
/// extension makes callable from sandboxed playbook code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostImportDef {
    /// Function name callable from the script (e.g., "mde_isolate").
    pub name: String,
    /// Human-readable description.
    pub description: String,
}

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
