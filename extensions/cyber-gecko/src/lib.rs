//! # cyber-gecko
//!
//! Cyber threat intelligence extension for the GECKO framework.
//!
//! Provides domain-specific TypeDB schema types (indicators, assets, threat actors)
//! and host-imported functions for security operations (e.g., host isolation).
pub mod detect;
pub mod migrations;
pub mod sensors;
pub mod stix;

use gecko_extension_api::{
    AttrDecl, AttrValueType, BeliefEdgeDecl, GeckoExtension, HostImportDef, RoleDecl,
    SchemaMigration,
};

/// The cyber-gecko TypeQL schema: cyber entities and relations subtyping the
/// core `concept`, plus the TypeDB functions that back the detect host imports.
const CYBER_SCHEMA: &str = include_str!("../schema/cyber_schema.tql");
const CYBER_FUNCTIONS: &str = include_str!("../schema/cyber_functions.tql");

/// The relations cyber-gecko writes alongside a belief: a detection claim linking
/// detecting logic to an ATT&CK technique, and an analyst verdict on detecting logic.
pub static CYBER_BELIEF_EDGES: [BeliefEdgeDecl; 2] = [
    BeliefEdgeDecl {
        relation: "technique-detection",
        belief_role: "detection-claim",
        roles: &[
            RoleDecl {
                role: "detecting-logic",
                player: "concept",
                key: "concept-id",
            },
            RoleDecl {
                role: "detected-technique",
                player: "attack-pattern",
                key: "attack-id",
            },
        ],
        owned_attrs: &[],
    },
    BeliefEdgeDecl {
        relation: "alert-disposition",
        belief_role: "disposition-belief",
        roles: &[RoleDecl {
            role: "dispositioned-logic",
            player: "concept",
            key: "concept-id",
        }],
        owned_attrs: &[AttrDecl {
            attr: "detection-verdict",
            value: AttrValueType::String,
            required: true,
        }],
    },
];

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

    fn schema_migrations(&self) -> &[SchemaMigration] {
        &migrations::CYBER_MIGRATIONS
    }

    fn belief_edges(&self) -> &[BeliefEdgeDecl] {
        &CYBER_BELIEF_EDGES
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
                description: "Report the declared platforms each detection claim is blind on or cannot assess".to_string(),
            },
            HostImportDef {
                name: "recommend_sources".to_string(),
                description: "Recommend the log sources to add, in greedy order, to unblind the most detection claims".to_string(),
            },
            HostImportDef {
                name: "precision".to_string(),
                description: "Calculate detection precision over a window".to_string(),
            },
        ]
    }

    /// The detect imports need graph reads and the belief-edge write path, which the
    /// plain synchronous `call_import` seam cannot provide. They are dispatched through
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
