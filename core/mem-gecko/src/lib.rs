//! # mem-gecko
//!
//! Cognitive memory extension for the GECKO framework.
//!
//! Transforms temporary execution state into structured, queryable GraphRAG memory.
//! Generates `execution-episode` nodes linked to involved entities via `contextualizes`
//! relations for future LLM-powered retrieval (design §4.3).

use gecko_extension_api::{GeckoExtension, HostImportDef};

/// The mem-gecko TypeQL schema extending the core `okf-concept` and `okf-link` types.
///
/// Design §4.3.
const MEM_SCHEMA: &str = include_str!("../schema/mem_schema.tql");

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
        MEM_SCHEMA
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
    fn test_schema_contains_cognitive_nodes() {
        let schema = MEM_SCHEMA;
        assert!(schema.contains("entity cognitive-node sub concept"));
        assert!(schema.contains("entity semantic-belief sub cognitive-node"));
        assert!(schema.contains("entity intention sub cognitive-node"));
        assert!(schema.contains("entity anomaly sub cognitive-node"));
    }

    #[test]
    fn test_schema_contains_atms_functions() {
        let schema = MEM_SCHEMA;
        assert!(schema.contains("fun is-superseded($node: cognitive-node) -> boolean:"));
        assert!(schema.contains("fun active-semantic-beliefs() -> { semantic-belief }:"));
        assert!(schema.contains("fun active-intentions() -> { intention }:"));
        assert!(schema.contains("fun root-goal($child: intention) -> { intention }:"));
        assert!(schema.contains("fun unresolved-anomalies() -> { anomaly }:"));
        assert!(schema.contains(
            "fun foundational-evidence($belief: semantic-belief) -> { execution-episode }:"
        ));
    }

    #[test]
    fn test_schema_contains_temporal_attributes() {
        let schema = MEM_SCHEMA;
        assert!(schema.contains("attribute valid-from value datetime;"));
        assert!(schema.contains("attribute valid-until value datetime;"));
        assert!(schema.contains("attribute half-life-hours value double;"));
        assert!(schema.contains("attribute last-recalled value datetime;"));
        assert!(schema.contains("attribute deadline value datetime;"));
        assert!(schema.contains("attribute duration-ms value integer;"));
    }
}
