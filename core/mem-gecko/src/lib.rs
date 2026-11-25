//! # mem-gecko
//!
//! GECKO's epistemic memory substrate: the bitemporal episode and belief schema, its
//! persisted functions, and the [`MemWriter`] that writes and recalls through them.

use std::sync::LazyLock;

use gecko_extension_api::{GeckoExtension, HostImportDef, SchemaMigration};

pub mod belief_edge;
mod tql;
pub mod writer;

pub use belief_edge::{BeliefEdgeRegistry, ExtensionFragment, RegistryError};
pub use writer::{IndexReconciliation, MemWriter};

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

pub use tql::MEM_MIGRATIONS;

/// The root type of the belief tier mem's writer owns.
pub const BELIEF_TIER_ROOT: &str = "memory-item";

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

    fn schema_migrations(&self) -> &[SchemaMigration] {
        MEM_MIGRATIONS
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
}

#[cfg(test)]
mod tests {
    use super::*;

    use gecko_extension_api::migration::deleted_labels_absent;

    #[test]
    fn shipped_mem_schema_has_no_population_labels() {
        let schema: &str = &MEM_SCHEMA;
        let found = deleted_labels_absent(
            schema,
            &[
                "entity population",
                "relation scoped",
                "relation population-member",
                "isa population",
                "isa scoped",
                "scoped:",
                "population-member:",
                "scoped-belief",
                "spec-dialect",
                "spec-text",
                "spec-hash",
                "computed-at",
                "population-members",
                "population-members-at",
                "blast-radius-at",
                "blast-step-at",
            ],
        );
        assert!(
            found.is_empty(),
            "the shipped mem schema still declares {found:?}"
        );
        assert!(schema.contains("fun blast-radius($r: memory-item) -> { memory-item }"));
    }

    #[test]
    fn shipped_mem_schema_declares_no_consolidation_labels() {
        let found = deleted_labels_absent(
            &MEM_SCHEMA,
            &[
                "consolidation-state",
                "consolidation-action",
                "relation consolidation-step",
                "consolidation-step:",
            ],
        );
        assert!(
            found.is_empty(),
            "the shipped mem schema still declares {found:?}"
        );
    }

    #[test]
    fn shipped_mem_schema_declares_no_divergence_labels() {
        let found = deleted_labels_absent(
            &MEM_SCHEMA,
            &[
                "entity divergence-exception",
                "relation divergence-precedence",
                "divergence-precedence:",
                "exception-scope",
                "exception-belief",
            ],
        );
        assert!(
            found.is_empty(),
            "the shipped mem schema still declares {found:?}"
        );
    }

    #[test]
    fn shipped_mem_schema_declares_no_diet_labels() {
        let found = deleted_labels_absent(
            &MEM_SCHEMA,
            &[
                "entity intention",
                "entity working-set",
                "entity playbook",
                "entity learned-constraint",
                "relation goal-hierarchy",
                "relation execution",
                "relation tool-invocation",
                "relation contextualizes",
                "relation evidence",
                "relation justification",
                "relation support",
                "relation attack",
                "goal-hierarchy:",
                "execution:",
                "tool-invocation:",
                "contextualizes:",
                "evidence:",
                "justification:",
                "support:",
                "attack:",
                "execution-status",
                "stability-tier",
                "corroboration-count",
                "justification-status",
                "tool-name",
                "attribute deadline",
                "owns deadline",
                "has deadline",
            ],
        );
        assert!(
            found.is_empty(),
            "the shipped mem schema still declares {found:?}"
        );
    }

    #[test]
    fn shipped_queries_use_current_relation_syntax() {
        use gecko_extension_api::relation_syntax::deprecated_relation_lines;
        for (file, text) in [
            ("schema/mem_types.tql", MEM_TYPES),
            ("schema/mem_functions.tql", MEM_FUNCTIONS),
            ("src/tql.rs", include_str!("tql.rs")),
        ] {
            let lines = deprecated_relation_lines(text);
            assert!(
                lines.is_empty(),
                "{file} writes a role list before `isa` at lines {lines:?}; write `rel (roles)`"
            );
        }
    }

    #[test]
    fn migrations_lead_to_the_shipped_schema() {
        // Each cleanup deletes only rows missing a role the shipped schema requires,
        // each update clamps a ranged attribute, each `undefine @doc` clears text the
        // shipped `define` replaces, and each `redefine` installs shipped text.
        gecko_extension_api::migration::check_migrations(MEM_MIGRATIONS, &MEM_SCHEMA)
            .unwrap_or_else(|e| {
                panic!("a mem migration is out of step with the shipped schema: {e}")
            });
    }

    fn shape_of(tql: &'static str) -> Result<(), String> {
        let m = SchemaMigration {
            id: "mem/test-entry",
            applies_when: "text no schema holds",
            tql,
        };
        gecko_extension_api::migration::check_migrations(&[m], &MEM_SCHEMA)
    }

    #[test]
    fn a_cleanup_naming_an_optional_shipped_role_is_rejected() {
        let err = shape_of(
            "match $r isa prediction-resolution; not { $r links (predicted-belief: $x); }; delete $r;",
        )
        .unwrap_err();
        assert!(
            err.contains("does not require role predicted-belief"),
            "{err}"
        );
    }

    #[test]
    fn an_update_reaches_a_range_through_the_supertype_and_refuses_an_unranged_one() {
        shape_of("match $r isa belief, has access-count $c; $c < 0; update $r has access-count 0;")
            .expect("memory-item's `owns access-count` carries a @range");
        let err =
            shape_of("match $r isa belief, has salience $s; $s > 1.0; update $r has salience 1.0;")
                .unwrap_err();
        assert!(err.contains("carries no `@range`"), "{err}");
    }

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
        // The abstract memory-item and its subtypes.
        assert!(schema.contains("entity memory-item @abstract, sub okf-concept"));
        assert!(schema.contains("entity belief sub memory-item"));
        assert!(schema.contains("entity episode sub memory-item"));
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
        // Substrate functions by signature (load-bearing surface). Matched through
        // the return type only, so an appended `@doc("…")` before the `:` is fine.
        assert!(schema.contains("fun is-superseded($b: memory-item) -> boolean"));
        assert!(schema.contains("fun believed-at($t: datetime) -> { belief }"));
        assert!(schema.contains("fun derivation-chain($b: memory-item) -> { memory-item }"));
        assert!(schema.contains("fun blast-radius($r: memory-item) -> { memory-item }"));
        // Resolution functions — present-state fast path and as-of-t variant.
        assert!(schema.contains("fun canonical-entity($rec: concept) -> { concept }"));
        assert!(
            schema.contains("fun canonical-entity-at($rec: concept, $t: datetime) -> { concept }")
        );
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
