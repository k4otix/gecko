//! Schema migrations that bring an earlier cyber schema forward: removing the
//! detection types ATT&CK no longer models, and replacing changed `@doc`s.

use gecko_extension_api::SchemaMigration;

/// Every cyber schema migration, oldest first.
///
/// The first seven remove `evidence-of`, `requires-evidence`, `sensor-coverage` and
/// `blinded-detections`, in the order a schema transaction accepts: instances, the
/// function, then the relations (undefining a relation drops its roles and every
/// `plays` of them). The rest each drop one changed `@doc`, which the shipped
/// `define` then reinstalls with its current text. A subtype's `@doc` sits on its
/// `sub` declaration, which `redefine` cannot address (`[REX15]`), so these
/// migrations `undefine` the annotation instead. `cyber-object`'s `@doc` sits on
/// `sub concept`, and the belief-edge registry refuses any migration naming a core
/// type, so that `@doc` is left as first shipped.
pub static CYBER_MIGRATIONS: [SchemaMigration; 10] = [
    SchemaMigration {
        id: "cyber/delete-evidence-of-instances",
        applies_when: "relation evidence-of",
        tql: "match $r isa evidence-of; delete $r;",
    },
    SchemaMigration {
        id: "cyber/delete-requires-evidence-instances",
        applies_when: "relation requires-evidence",
        tql: "match $r isa requires-evidence; delete $r;",
    },
    SchemaMigration {
        id: "cyber/delete-sensor-coverage-instances",
        applies_when: "relation sensor-coverage",
        tql: "match $r isa sensor-coverage; delete $r;",
    },
    SchemaMigration {
        id: "cyber/undefine-blinded-detections",
        applies_when: "fun blinded-detections",
        tql: "undefine fun blinded-detections;",
    },
    SchemaMigration {
        id: "cyber/undefine-evidence-of",
        applies_when: "relation evidence-of",
        tql: "undefine evidence-of;",
    },
    SchemaMigration {
        id: "cyber/undefine-requires-evidence",
        applies_when: "relation requires-evidence",
        tql: "undefine requires-evidence;",
    },
    SchemaMigration {
        id: "cyber/undefine-sensor-coverage",
        applies_when: "relation sensor-coverage",
        tql: "undefine sensor-coverage;",
    },
    SchemaMigration {
        id: "cyber/data-source-reference-doc",
        applies_when: "@doc(\"An ATT&CK data source: a category of telemetry (e.g. Process, Network Traffic) from which technique detection can draw evidence.\")",
        tql: "undefine @doc from data-source sub cyber-object;",
    },
    SchemaMigration {
        id: "cyber/data-component-analytic-doc",
        applies_when: "@doc(\"An ATT&CK data component: a specific facet of a data source (e.g. Process Creation) that a technique's detection requires as evidence.\")",
        tql: "undefine @doc from data-component sub cyber-object;",
    },
    SchemaMigration {
        id: "cyber/sensor-file-doc",
        applies_when: "@doc(\"A deployed telemetry sensor whose coverage of data sources and components determines what detection evidence is available.\")",
        tql: "undefine @doc from sensor sub cyber-object;",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_lead_to_the_shipped_schema() {
        let shipped = crate::CyberGecko::new();
        let schema = gecko_extension_api::GeckoExtension::schema(&shipped);
        for m in &CYBER_MIGRATIONS {
            assert!(
                !schema.contains(m.applies_when),
                "{}: the shipped schema still contains the replaced text",
                m.id
            );
        }
    }
}
