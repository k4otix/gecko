//! Schema migrations that remove the detection types ATT&CK no longer models.

use gecko_extension_api::SchemaMigration;

/// Removes `evidence-of`, `requires-evidence`, `sensor-coverage` and
/// `blinded-detections`, in the order a schema transaction accepts: instances, the
/// function, the `plays`, then the roles and relations.
pub static DETECTION_MODEL_MIGRATIONS: [SchemaMigration; 7] = [
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
];
