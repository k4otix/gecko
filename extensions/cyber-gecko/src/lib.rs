//! # cyber-gecko
//!
//! Cyber threat intelligence extension for the GECKO framework.
//!
//! Provides domain-specific TypeDB schema types (indicators, assets, threat actors)
//! and host-imported functions for security operations (e.g., host isolation).
pub mod detect;
pub mod stix;

use gecko_extension_api::{GeckoExtension, HostImportDef};

/// The cyber-gecko TypeQL schema: cyber entities and relations subtyping the
/// core `concept`, plus the TypeDB functions that back the detect host imports.
const CYBER_SCHEMA: &str = include_str!("../schema/cyber_schema.tql");
const CYBER_FUNCTIONS: &str = include_str!("../schema/cyber_functions.tql");

/// Cyber threat intelligence extension.
pub struct CyberGecko {
    schema_string: String,
}

impl CyberGecko {
    pub fn new() -> Self {
        Self {
            schema_string: format!("{}\n{}", CYBER_SCHEMA, CYBER_FUNCTIONS),
        }
    }
}

impl Default for CyberGecko {
    fn default() -> Self {
        Self::new()
    }
}

/// Construct the cyber-gecko extension as a boxed [`GeckoExtension`], ready for
/// the Assembler to register.
pub fn extension() -> Box<dyn GeckoExtension> {
    Box::new(CyberGecko::new())
}

impl GeckoExtension for CyberGecko {
    fn name(&self) -> &'static str {
        "cyber-gecko"
    }

    fn schema(&self) -> &str {
        &self.schema_string
    }

    fn host_imports(&self) -> Vec<HostImportDef> {
        vec![
            HostImportDef {
                name: "claim".to_string(),
                description: "Assert a detection claim".to_string(),
            },
            HostImportDef {
                name: "disposition".to_string(),
                description: "Observe an alert disposition".to_string(),
            },
            HostImportDef {
                name: "coverage_gaps".to_string(),
                description: "List undetected techniques".to_string(),
            },
            HostImportDef {
                name: "blinded".to_string(),
                description: "List blinded detections".to_string(),
            },
            HostImportDef {
                name: "precision".to_string(),
                description: "Calculate detection precision over a window".to_string(),
            },
        ]
    }

    /// The detect imports need graph + belief-write access, which the plain
    /// synchronous `call_import` seam cannot provide. They are dispatched through
    /// the run-time host bridge ([`crate::detect::cyber_extension_callback`])
    /// instead, so reaching this path means the bridge was not wired.
    fn call_import(
        &self,
        name: &str,
        _args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        Err(format!(
            "cyber-gecko import '{name}' requires the detect host bridge, which is not wired for this run"
        ))
    }
}
