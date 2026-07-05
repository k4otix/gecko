//! # cyber-gecko
//!
//! Cyber threat intelligence extension for the GECKO framework.
//!
//! Provides domain-specific TypeDB schema types (indicators, assets, threat actors)
//! and host-imported functions for security operations (e.g., host isolation).

use gecko_engine::extension::GeckoExtension;
use gecko_engine::sandbox::engine::HostImportDef;

/// The cyber-gecko TypeQL schema extending the core `okf-concept` and `okf-link` types.
///
/// Design §4.2.
const CYBER_SCHEMA: &str = include_str!("../schema/cyber_schema.tql");

/// Cyber threat intelligence extension.
pub struct CyberGecko;

impl CyberGecko {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CyberGecko {
    fn default() -> Self {
        Self::new()
    }
}

impl GeckoExtension for CyberGecko {
    fn name(&self) -> &'static str {
        "cyber-gecko"
    }

    fn schema(&self) -> &str {
        CYBER_SCHEMA
    }

    fn host_imports(&self) -> Vec<HostImportDef> {
        vec![
            HostImportDef {
                name: "mde_isolate".to_string(),
                description: "Isolate a host via Microsoft Defender for Endpoint API".to_string(),
            },
            HostImportDef {
                name: "sentinel_query".to_string(),
                description: "Execute a KQL query against Microsoft Sentinel".to_string(),
            },
        ]
    }

    fn call_import(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match name {
            "mde_isolate" => {
                let machine_id = args
                    .get("machine_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                // Mock isolation
                Ok(serde_json::json!({
                    "status": "success",
                    "action": "isolated",
                    "machine_id": machine_id
                }))
            }
            "sentinel_query" => {
                let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                // Mock query result
                Ok(serde_json::json!({
                    "status": "success",
                    "results": [
                        {"query": query, "found": true}
                    ]
                }))
            }
            _ => Err(format!("Unknown import: {name}")),
        }
    }
}
