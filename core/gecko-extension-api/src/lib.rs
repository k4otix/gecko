//! # gecko-extension-api
//!
//! The **extension contract** for the GECKO framework — and nothing else.
//!
//! Domain extensions (`cyber-gecko`) and the always-on substrate (`mem-gecko`)
//! implement [`GeckoExtension`] to contribute a TypeQL schema and host-imported
//! functions. This crate isolates that contract from the rest of the engine
//! (parser, sandbox, syncer, router), so an extension needs only this thin
//! crate — never a dependency on the whole engine — to implement one trait.
//!
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

use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ── Epistemic substrate contract ──────────────────────────────────────────────
//
// `RunContext`/`EpistemicWriter`/`EpistemicReader`/`SemanticIndex`/`Embedder` live
// alongside `GeckoExtension`/`HostImportDef` in this same crate. mem-gecko
// implements them; `gecko-engine` re-exports them for sandbox host-fn injection.
pub mod belief_edge;
pub mod epistemic;
pub mod error;
pub mod graph;
pub mod ids;
pub mod okf;
pub mod provenance;
pub mod sandbox_ctx;
pub mod semantic;

pub use belief_edge::{
    AttrDecl, AttrValueType, BeliefEdge, BeliefEdgeDecl, BeliefEdgeWriter, RoleDecl,
};
pub use epistemic::{
    BeliefDraft, BeliefQuery, Chunk, ContextBudget, DerivationMethod, EpisodeDraft,
    EpistemicReader, EpistemicWriter, Outcome, RecallQuery,
};
pub use error::EpistemicError;
pub use graph::{GraphReader, GraphStore, GraphValue, GraphWrite, RowCountMismatch};
pub use ids::{ActorId, AnomalyId, ConceptId, DateTime, MemId, RunId};
pub use provenance::{ProvenanceSource, RunContext, RunScratch, SharedScratch};
pub use sandbox_ctx::SandboxCtx;
pub use semantic::{BeliefState, Embedder, Entrenchment, FilterMeta, SemanticIndex, Visibility};

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

/// Host-side handler for a sandboxed `host_call_extension(ext, func, args)` call,
/// returning its JSON result or an error message.
pub type ExtensionCallback = std::sync::Arc<
    dyn Fn(&str, &str, serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync,
>;

/// A schema change TypeDB's idempotent `define` cannot apply to a database that
/// already holds an earlier version of the schema — such as a changed annotation
/// (`@doc`, `@card`, `@values`), which `define` refuses (`[DEX15]`).
///
/// A migration is self-detecting: it runs only while the database's current schema
/// text still contains `applies_when`, so it is skipped on a fresh database and on one
/// that already holds the new definition (where an identical `redefine` would be
/// refused, `[REX25]`). Pending migrations run, in order, in the same schema
/// transaction as the extension's `define`, before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaMigration {
    /// Stable identifier, namespaced by extension (e.g. `mem/population-member-doc`).
    pub id: &'static str,
    /// Text present in the database's exported schema only before this migration.
    pub applies_when: &'static str,
    /// The TypeQL (`redefine`/`undefine`) that brings the schema forward.
    pub tql: &'static str,
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

    /// Migrations that bring an earlier version of [`schema`](Self::schema) forward on
    /// an existing database, applied before it. Empty when the schema has only ever
    /// changed in ways `define` re-applies.
    fn schema_migrations(&self) -> &[SchemaMigration] {
        &[]
    }

    /// Relations this extension attaches to a belief, written atomically with it through
    /// [`BeliefEdgeWriter`]. Empty when the extension adds no belief edges.
    fn belief_edges(&self) -> &[BeliefEdgeDecl] {
        &[]
    }

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

    /// Activation hook: called during `build_extensions` for each **enabled**
    /// extension, handed a host-constructed [`EpistemicWriter`].
    ///
    /// Registration is the activation gate (invariant 3) — a disabled extension's
    /// hook is never called, so there is **no second trait**. The default is a
    /// no-op: non-epistemic extensions (`cyber-gecko`) inherit it and leave the
    /// context untouched. An epistemic extension (the `mem` substrate) overrides it
    /// to bind the writer into the [`SandboxCtx`] via
    /// [`set_epistemic_writer`](SandboxCtx::set_epistemic_writer), which the engine's
    /// sandbox bridge then routes the mem host fns to — each stamped with a
    /// host-minted [`RunContext`](crate::RunContext) the sandbox cannot forge
    /// (invariant 2).
    fn inject_epistemic_host_fns(&self, _ctx: &mut SandboxCtx, _writer: Arc<dyn EpistemicWriter>) {}
}
