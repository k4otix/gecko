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
        // Each migration's replaced text is gone from the shipped schema. A
        // `redefine` installs the annotation the shipped schema defines, so applying
        // the shipped `define` after it is a no-op for that type; a cleanup is a
        // single `match … delete` of the rows the shipped schema refuses.
        for m in CORE_MIGRATIONS {
            assert!(
                !CORE_SCHEMA.contains(m.applies_when),
                "{}: shipped schema still contains the replaced text",
                m.id
            );
            if m.tql.starts_with("match ") {
                assert!(
                    m.tql.ends_with(" delete $r;") && m.tql.matches("delete").count() == 1,
                    "{}: a cleanup is one `match … delete $r;` statement",
                    m.id
                );
                continue;
            }
            let installed = m
                .tql
                .strip_prefix("redefine ")
                .and_then(|t| t.strip_suffix(';'))
                .expect("migration is a single redefine or a match-delete cleanup");
            assert!(
                CORE_SCHEMA.contains(installed),
                "{}: shipped schema does not define {installed}",
                m.id
            );
        }
    }
}
