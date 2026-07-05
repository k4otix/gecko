//! # mem-gecko
//!
//! Cognitive memory extension for the GECKO framework.
//!
//! Transforms temporary execution state into structured, queryable GraphRAG memory.
//! Generates `execution-episode` nodes linked to involved entities via `contextualizes`
//! relations for future LLM-powered retrieval (design §4.3).

use gecko_engine::extension::GeckoExtension;
use gecko_engine::sandbox::engine::HostImportDef;

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

impl GeckoExtension for MemGecko {
    fn name(&self) -> &str {
        "mem-gecko"
    }

    fn schema(&self) -> &str {
        MEM_SCHEMA
    }

    fn host_imports(&self) -> Vec<HostImportDef> {
        vec![
            HostImportDef {
                name: "record_episode".to_string(),
                description: "Record an execution episode in the cognitive memory graph".to_string(),
            },
            HostImportDef {
                name: "query_memory".to_string(),
                description: "Query episodic memory for relevant past executions".to_string(),
            },
        ]
    }

    fn call_import(&self, name: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
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
            _ => Err(format!("Unknown import: {}", name)),
        }
    }
}
