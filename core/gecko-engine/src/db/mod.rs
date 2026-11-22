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
pub const CORE_MIGRATIONS: &[SchemaMigration] = &[
    SchemaMigration {
        id: "core/bundle-shared-database-doc",
        applies_when: "entity bundle @doc(\"The single OKF documentation bundle ingested into this database, keyed by its declared name; bundle-path is machine-local provenance, not identity.\")",
        tql: "redefine entity bundle @doc(\"An OKF documentation bundle synced into this database, keyed by its declared name and containing its concepts; several bundles may share a database, and bundle-path is machine-local provenance, not identity.\");",
    },
    // Cleanups: each deletes the rows missing a role the shipped schema requires,
    // so the `define` that adds the role's card commits. Each runs while the role
    // still prints without a card. `concept-link` inherits `okf-link`'s roles, so
    // its trigger is `okf-link`'s; `citation` specializes them and carries its own.
    SchemaMigration {
        id: "core/containment-required-roles",
        applies_when: "relates container @doc(\"The bundle that contains the members.\")",
        tql: "match $r isa containment; { not { $r links (container: $p0); }; } or { not { $r links (member: $p1); }; }; delete $r;",
    },
    SchemaMigration {
        id: "core/hierarchy-required-roles",
        applies_when: "relates child @doc(\"The nested concept.\")",
        tql: "match $r isa hierarchy; { not { $r links (parent: $p0); }; } or { not { $r links (child: $p1); }; }; delete $r;",
    },
    SchemaMigration {
        id: "core/concept-link-required-roles",
        applies_when: "relates source @doc(\"The concept the link originates from.\")",
        tql: "match $r isa concept-link; { not { $r links (source: $p0); }; } or { not { $r links (target: $p1); }; }; delete $r;",
    },
    SchemaMigration {
        id: "core/citation-required-roles",
        applies_when: "relates citing-concept as source @doc(\"The concept making the citation.\")",
        tql: "match $r isa citation; { not { $r links (citing-concept: $p0); }; } or { not { $r links (cited-resource: $p1); }; }; delete $r;",
    },
];

pub use graph_store::{RouterGraphStore, run_graph_writes};
pub use label::validate_schema_label;
pub use router::{GraphHandle, WriteTransaction};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_migrations_lead_to_the_shipped_schema() {
        // Each cleanup deletes only rows missing a role the shipped schema requires,
        // and each `redefine` installs text the shipped schema defines.
        gecko_extension_api::migration::check_migrations(CORE_MIGRATIONS, CORE_SCHEMA)
            .unwrap_or_else(|e| {
                panic!("a core migration is out of step with the shipped schema: {e}")
            });
    }
}
