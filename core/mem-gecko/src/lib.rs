//! # mem-gecko
//!
//! Cognitive memory extension for the GECKO framework.
//!
//! Transforms temporary execution state into structured, queryable GraphRAG memory.
//! Generates `execution-episode` nodes linked to involved entities via `contextualizes`
//! relations for future LLM-powered retrieval (design §4.3).

use std::sync::LazyLock;

use std::sync::Arc;

use gecko_extension_api::{EpistemicWriter, GeckoExtension, HostImportDef, SandboxCtx};

pub mod consolidation;
mod tql;
pub mod writer;

pub use consolidation::{ConsolidationDaemon, ConsolidationReport, NoopConsolidationDaemon};
pub use writer::MemWriter;

/// The mem-gecko substrate schema, split into two source fragments (invariant 3):
/// the type ontology (`mem_types.tql`) and the persisted reasoning functions
/// (`mem_functions.tql`). Types must precede the functions that reference them.
const MEM_TYPES: &str = include_str!("../schema/mem_types.tql");
const MEM_FUNCTIONS: &str = include_str!("../schema/mem_functions.tql");

/// The finalized substrate schema as a SINGLE `define` transaction. TypeDB accepts
/// many statements under one `define` keyword but not two concatenated `define`
/// blocks, so the functions fragment's leading `define` is stripped and its body is
/// appended under the types fragment's `define` (types first, then functions).
static MEM_SCHEMA: LazyLock<String> = LazyLock::new(|| {
    let functions_body = MEM_FUNCTIONS
        .trim_start()
        .strip_prefix("define")
        .unwrap_or(MEM_FUNCTIONS);
    format!("{}\n{}", MEM_TYPES.trim_end(), functions_body)
});

/// Cognitive memory extension.
pub struct MemGecko;

impl MemGecko {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MemGecko {
    fn default() -> Self {
        Self::new()
    }
}

/// Construct the mem-gecko substrate extension as a boxed [`GeckoExtension`].
///
/// mem-gecko is always-on: the Assembler registers this unconditionally, before
/// any optional domain extension, so mem's schema is applied first.
pub fn extension() -> Box<dyn GeckoExtension> {
    Box::new(MemGecko::new())
}

impl GeckoExtension for MemGecko {
    fn name(&self) -> &'static str {
        "mem-gecko"
    }

    fn schema(&self) -> &str {
        &MEM_SCHEMA
    }

    fn host_imports(&self) -> Vec<HostImportDef> {
        vec![]
    }

    fn call_import(
        &self,
        name: &str,
        _args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        Err(format!(
            "mem-gecko exposes no plain host imports; use the mem.* bindings (name: {name})"
        ))
    }

    /// mem is the epistemic substrate: it binds the host-constructed writer into the
    /// activation context so the engine's sandbox bridge routes the mem host fns
    /// (`remember`/`derive`/`supersede`/`contest`) to it. Because mem is always
    /// registered (forced-on, first), this hook always runs — but only ever with a
    /// writer the host built, and every dispatched call is stamped with a host-minted
    /// [`RunContext`](gecko_extension_api::RunContext) (invariant 2).
    fn inject_epistemic_host_fns(&self, ctx: &mut SandboxCtx, writer: Arc<dyn EpistemicWriter>) {
        ctx.set_epistemic_writer(writer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_is_single_define_block() {
        // The combined schema must be exactly one `define` transaction: one leading
        // `define` keyword, no second `define` block (the functions fragment's own
        // `define` is stripped when concatenated).
        let schema: &str = &MEM_SCHEMA;
        assert!(schema.trim_start().starts_with("define"));
        // Only the single leading `define` may appear as a statement keyword. Count
        // occurrences of a `define` at a line start; there must be exactly one.
        let define_lines = schema
            .lines()
            .filter(|l| l.trim_start().starts_with("define"))
            .count();
        assert_eq!(
            define_lines, 1,
            "combined schema must have a single leading `define`"
        );
        // Types must precede functions that reference them.
        let types_marker = schema.find("entity memory-item @abstract").unwrap();
        let fn_marker = schema.find("fun is-superseded").unwrap();
        assert!(types_marker < fn_marker, "types must precede functions");
    }

    #[test]
    fn test_schema_contains_memory_item_hierarchy() {
        let schema: &str = &MEM_SCHEMA;
        // Four-layer substrate: abstract memory-item + its subtypes.
        assert!(schema.contains("entity memory-item @abstract, sub okf-concept"));
        assert!(schema.contains("entity belief sub memory-item"));
        assert!(schema.contains("entity episode sub memory-item"));
        assert!(schema.contains("entity playbook sub memory-item"));
        assert!(schema.contains("entity working-set sub memory-item"));
        // Cross-source identity: resolution IS a belief.
        assert!(schema.contains("entity resolution sub belief"));
    }

    #[test]
    fn test_schema_contains_state_enums() {
        let schema: &str = &MEM_SCHEMA;
        // belief-state enum.
        assert!(schema.contains(
            r#"attribute belief-state value string @values("asserted", "retracted", "superseded", "contested");"#
        ));
        // derivation-method: the exact 7 kebab strings the Rust enum serializes
        // to (this is the contract). Assert each is present and the count is exactly 7.
        for method in [
            "type-join",
            "cardinality",
            "functional-dependency",
            "type-db-function",
            "external-tool",
            "human-assertion",
            "llm-synthesis",
        ] {
            assert!(
                schema.contains(&format!("\"{method}\"")),
                "derivation-method missing {method}"
            );
        }
        let dm_line = schema
            .lines()
            .find(|l| l.contains("attribute derivation-method"))
            .expect("derivation-method attribute declared");
        assert_eq!(
            dm_line.matches(',').count(),
            6,
            "derivation-method must enumerate exactly 7 @values"
        );
    }

    #[test]
    fn test_schema_contains_consolidation_state_machine() {
        // The consolidation-state machine (with tombstone semantics) must be in
        // the schema so the dreaming daemon slots in without schema churn.
        let schema: &str = &MEM_SCHEMA;
        assert!(schema.contains(
            r#"attribute consolidation-state value string @values("raw", "candidate", "consolidated", "archived", "tombstoned");"#
        ));
        // All five states present, including the tombstone terminal state.
        for state in ["raw", "candidate", "consolidated", "archived", "tombstoned"] {
            assert!(
                schema.contains(&format!("\"{state}\"")),
                "consolidation-state missing {state}"
            );
        }
        // memory-item owns consolidation-state (the state machine is on the substrate).
        assert!(schema.contains("owns consolidation-state @card(0..1)"));
    }

    #[test]
    fn test_schema_contains_bitemporal_attributes() {
        let schema: &str = &MEM_SCHEMA;
        // Bitemporal + decay attrs (invariant 6).
        assert!(schema.contains("attribute event-time value datetime;"));
        assert!(schema.contains("attribute ingest-time value datetime;"));
        assert!(schema.contains("attribute valid-from value datetime;"));
        assert!(schema.contains("attribute valid-to value datetime;"));
        // episode carries both event-time and ingest-time mandatorily.
        assert!(schema.contains("owns event-time @card(1)"));
        assert!(schema.contains("owns ingest-time @card(1)"));
        // Invariant 8: NO vector/embedding storage in the schema.
        assert!(!schema.contains("embedding"));
        assert!(!schema.contains("vector"));
    }

    #[test]
    fn test_schema_contains_key_functions() {
        let schema: &str = &MEM_SCHEMA;
        // Substrate functions by signature (load-bearing surface).
        assert!(schema.contains("fun is-superseded($b: memory-item) -> boolean:"));
        assert!(schema.contains("fun believed-at($t: datetime) -> { belief }:"));
        assert!(schema.contains("fun derivation-chain($b: memory-item) -> { memory-item }:"));
        assert!(schema.contains("fun blast-radius($r: memory-item) -> { memory-item }:"));
        // Population / resolution functions.
        assert!(schema.contains("fun population-members($pop: population) -> { concept }:"));
        assert!(schema.contains("fun canonical-entity($rec: concept) -> { concept }:"));
    }

    #[test]
    fn test_pivot_is_instantiable() {
        // Pivot roles must have players (agent + memory-item), otherwise the
        // substrate primitive is uninstantiable.
        let schema: &str = &MEM_SCHEMA;
        assert!(schema.contains("plays pivot:pivoting-agent"));
        assert!(schema.contains("plays pivot:from-state"));
        assert!(schema.contains("plays pivot:to-state"));
    }
}
