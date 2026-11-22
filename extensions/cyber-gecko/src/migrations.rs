//! Schema migrations that bring an earlier cyber schema forward: removing the
//! detection types ATT&CK no longer models, replacing changed `@doc`s, and
//! deleting the relations missing a player the schema requires.

use gecko_extension_api::SchemaMigration;

/// Every cyber schema migration, oldest first.
///
/// The first seven remove `evidence-of`, `requires-evidence`, `sensor-coverage` and
/// `blinded-detections`, in the order a schema transaction accepts: instances, the
/// function, then the relations (undefining a relation drops its roles and every
/// `plays` of them). The next three each drop one changed `@doc`, which the shipped
/// `define` then reinstalls with its current text. A subtype's `@doc` sits on its
/// `sub` declaration, which `redefine` cannot address (`[REX15]`), so these
/// migrations `undefine` the annotation instead. `cyber-object`'s `@doc` sits on
/// `sub concept`, and the belief-edge registry refuses any migration naming a core
/// type, so that `@doc` is left as first shipped. The last eleven delete each
/// typed relation's rows that lack a player for a role the schema requires.
pub static CYBER_MIGRATIONS: [SchemaMigration; 21] = [
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
    // One cleanup per relation whose roles the shipped schema requires: each
    // deletes the rows missing a required player, so the `@card(1)` the shipped
    // `define` adds commits. Each runs while the role still prints without a card.
    SchemaMigration {
        id: "cyber/indicates-required-roles",
        applies_when: "relates indicator-source @doc(\"The indicator doing the signalling.\")",
        tql: "match $r isa indicates; { not { $r links (indicator-source: $p0); }; } or { not { $r links (indicated: $p1); }; }; delete $r;",
    },
    SchemaMigration {
        id: "cyber/uses-required-roles",
        applies_when: "relates used @doc(\"The technique, malware, or tool being used.\")",
        tql: "match $r isa uses; { not { $r links (user: $p0); }; } or { not { $r links (used: $p1); }; }; delete $r;",
    },
    SchemaMigration {
        id: "cyber/attributed-to-required-roles",
        applies_when: "relates actor @doc(\"The actor the activity is attributed to.\")",
        tql: "match $r isa attributed-to; { not { $r links (attributee: $p0); }; } or { not { $r links (actor: $p1); }; }; delete $r;",
    },
    SchemaMigration {
        id: "cyber/targets-required-roles",
        applies_when: "relates threat-source @doc(\"The adversary object doing the targeting.\")",
        tql: "match $r isa targets; { not { $r links (threat-source: $p0); }; } or { not { $r links (target: $p1); }; }; delete $r;",
    },
    SchemaMigration {
        id: "cyber/mitigates-required-roles",
        applies_when: "relates mitigation @doc(\"The course-of-action providing the mitigation.\")",
        tql: "match $r isa mitigates; { not { $r links (mitigation: $p0); }; } or { not { $r links (mitigated: $p1); }; }; delete $r;",
    },
    SchemaMigration {
        id: "cyber/observed-as-required-roles",
        applies_when: "relates pattern-indicator @doc(\"The indicator whose pattern was observed.\")",
        tql: "match $r isa observed-as; { not { $r links (pattern-indicator: $p0); }; } or { not { $r links (observable-value: $p1); }; }; delete $r;",
    },
    SchemaMigration {
        id: "cyber/strategy-detects-required-roles",
        applies_when: "relates technique @doc(\"The technique the strategy detects.\")",
        tql: "match $r isa strategy-detects; { not { $r links (strategy: $p0); }; } or { not { $r links (technique: $p1); }; }; delete $r;",
    },
    SchemaMigration {
        id: "cyber/strategy-analytic-required-roles",
        applies_when: "relates analytic @doc(\"The analytic carrying out the strategy.\")",
        tql: "match $r isa strategy-analytic; { not { $r links (strategy: $p0); }; } or { not { $r links (analytic: $p1); }; }; delete $r;",
    },
    SchemaMigration {
        id: "cyber/analytic-requirement-required-roles",
        applies_when: "relates component @doc(\"The data component the log source provides to the analytic.\")",
        tql: "match $r isa analytic-requirement; { not { $r links (analytic: $p0); }; } or { not { $r links (component: $p1); }; }; delete $r;",
    },
    SchemaMigration {
        id: "cyber/technique-detection-required-roles",
        applies_when: "relates detecting-logic @doc(\"The concept holding the detection logic.\")",
        tql: "match $r isa technique-detection; { not { $r links (detecting-logic: $p0); }; } or { not { $r links (detected-technique: $p1); }; } or { not { $r links (detection-claim: $p2); }; }; delete $r;",
    },
    SchemaMigration {
        id: "cyber/alert-disposition-required-roles",
        applies_when: "relates dispositioned-logic @doc(\"The detecting logic whose alert is being dispositioned.\")",
        tql: "match $r isa alert-disposition; { not { $r links (dispositioned-logic: $p0); }; } or { not { $r links (disposition-belief: $p1); }; }; delete $r;",
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
