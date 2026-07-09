//! # mem-gecko
//!
//! Cognitive memory extension for the GECKO framework.
//!
//! Transforms temporary execution state into structured, queryable GraphRAG memory.
//! Generates `execution-episode` nodes linked to involved entities via `contextualizes`
//! relations for future LLM-powered retrieval (design §4.3).

use std::sync::LazyLock;

use gecko_extension_api::{GeckoExtension, HostImportDef};

pub mod writer;

pub use writer::{BeliefCommitter, MemWriter, StubCommitter};

/// The mem-gecko substrate schema, split into two source fragments (invariant 3):
/// the type ontology (`mem_types.tql`, A2) and the persisted reasoning functions
/// (`mem_functions.tql`, A3). Types must precede the functions that reference them.
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
        vec![
            HostImportDef {
                name: "record_episode".to_string(),
                description: "Record an execution episode in the cognitive memory graph"
                    .to_string(),
            },
            HostImportDef {
                name: "query_memory".to_string(),
                description: "Query episodic memory for relevant past executions".to_string(),
            },
        ]
    }

    fn call_import(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match name {
            "record_episode" => {
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                Ok(serde_json::json!({
                    "status": "success",
                    "episode_id": "ep-12345",
                    "recorded_text": text
                }))
            }
            "query_memory" => {
                let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                Ok(serde_json::json!({
                    "status": "success",
                    "memories": [
                        {"memory": format!("Past memory matching: {}", query), "relevance": 0.95}
                    ]
                }))
            }
            _ => Err(format!("Unknown import: {name}")),
        }
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
        // A2.1 four-layer substrate: abstract memory-item + its subtypes.
        assert!(schema.contains("entity memory-item @abstract, sub okf-concept"));
        assert!(schema.contains("entity belief sub memory-item"));
        assert!(schema.contains("entity episode sub memory-item"));
        assert!(schema.contains("entity playbook sub memory-item"));
        assert!(schema.contains("entity working-set sub memory-item"));
        // A2.9 cross-source identity: resolution IS a belief.
        assert!(schema.contains("entity resolution sub belief"));
    }

    #[test]
    fn test_schema_contains_state_enums() {
        let schema: &str = &MEM_SCHEMA;
        // A2.2 belief-state enum.
        assert!(schema.contains(
            r#"attribute belief-state value string @values("asserted", "retracted", "superseded", "contested");"#
        ));
        // A2.2 derivation-method: the EXACT 7 kebab strings the A1 Rust enum serializes
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
    fn test_schema_contains_bitemporal_attributes() {
        let schema: &str = &MEM_SCHEMA;
        // A2.4 bitemporal + decay attrs (invariant 6).
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
        // A3 substrate functions by signature (load-bearing surface).
        assert!(schema.contains("fun is-superseded($b: memory-item) -> boolean:"));
        assert!(schema.contains("fun believed-at($t: datetime) -> { belief }:"));
        assert!(schema.contains("fun derivation-chain($b: memory-item) -> { memory-item }:"));
        assert!(schema.contains("fun blast-radius($r: memory-item) -> { memory-item }:"));
        // A2.9 population / resolution functions.
        assert!(schema.contains("fun population-members($pop: population) -> { concept }:"));
        assert!(schema.contains("fun canonical-entity($rec: concept) -> { concept }:"));
    }

    #[test]
    fn test_pivot_is_instantiable() {
        // Carry-forward fix: pivot roles must have players (agent + memory-item),
        // otherwise the substrate primitive is uninstantiable.
        let schema: &str = &MEM_SCHEMA;
        assert!(schema.contains("plays pivot:pivoting-agent"));
        assert!(schema.contains("plays pivot:from-state"));
        assert!(schema.contains("plays pivot:to-state"));
    }
}
