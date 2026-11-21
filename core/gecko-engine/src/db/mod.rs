//! TypeDB transaction routing and connection management.

pub mod graph_store;
pub mod label;
pub mod required;
pub mod router;
pub mod schema_apply;
pub mod surface;

use gecko_extension_api::SchemaMigration;

/// The core schema text the engine applies before any extension fragment.
pub const CORE_SCHEMA: &str = include_str!("../../schema/core_schema.tql");

/// Schema migrations for databases initialized with an earlier core schema, oldest
/// first. Each runs only while the database still holds the text it replaces.
pub const CORE_MIGRATIONS: &[SchemaMigration] = &[SchemaMigration {
    id: "core/bundle-shared-database-doc",
    applies_when: "entity bundle @doc(\"The single OKF documentation bundle ingested into this database, keyed by its declared name; bundle-path is machine-local provenance, not identity.\")",
    tql: "redefine entity bundle @doc(\"An OKF documentation bundle synced into this database, keyed by its declared name and containing its concepts; several bundles may share a database, and bundle-path is machine-local provenance, not identity.\");",
}];

pub use graph_store::{RouterGraphStore, run_graph_writes};
pub use label::validate_schema_label;
pub use router::{GraphHandle, WriteTransaction};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_migrations_lead_to_the_shipped_schema() {
        // Each migration's replaced text is gone from the shipped schema, and the
        // annotation it installs is the one the shipped schema defines, so applying
        // the shipped `define` after it is a no-op for that type.
        for m in CORE_MIGRATIONS {
            assert!(
                !CORE_SCHEMA.contains(m.applies_when),
                "{}: shipped schema still contains the replaced text",
                m.id
            );
            let installed = m
                .tql
                .strip_prefix("redefine ")
                .and_then(|t| t.strip_suffix(';'))
                .expect("migration is a single redefine statement");
            assert!(
                CORE_SCHEMA.contains(installed),
                "{}: shipped schema does not define {installed}",
                m.id
            );
        }
    }
}
