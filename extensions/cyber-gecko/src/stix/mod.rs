use chrono::{DateTime, Utc};
use gecko_extension_api::okf::{OkfBundle, OkfConcept, TypedAttrValue, TypedAttribute};
use gecko_extension_api::{GraphValue, GraphWrite};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub mod normalize;

fn get_str<'a>(val: &'a Value, field: &str) -> Option<&'a str> {
    val.get(field).and_then(Value::as_str)
}

fn str_attr(name: &str, value: &str) -> TypedAttribute {
    TypedAttribute {
        name: name.to_string(),
        value: TypedAttrValue::String(value.to_string()),
    }
}

/// Derives a bundle-relative concept ID from a STIX id (`indicator--<uuid>` ->
/// `stix/indicator/<uuid>`). A STIX id embeds its own type and a UUIDv4, so the
/// derived ID is collision-free within the graph.
fn derive_id(stix_id: &str) -> String {
    match stix_id.split_once("--") {
        Some((ty, uuid)) => format!("stix/{ty}/{uuid}"),
        None => format!("stix/{stix_id}"),
    }
}

/// Maps a STIX Cyber-observable Object (SCO) to a normalized `(ioc-type,
/// ioc-value)` pair, or `None` if the object is not a supported observable. The
/// ioc-type tokens match the `ioc-type` `@values` set in the cyber schema.
fn sco_ioc(type_str: &str, obj: &Value) -> Option<(&'static str, String)> {
    let (ioc_type, raw) = match type_str {
        "ipv4-addr" => ("ipv4", get_str(obj, "value")?),
        "ipv6-addr" => ("ipv6", get_str(obj, "value")?),
        "domain-name" => ("domain", get_str(obj, "value")?),
        "url" => ("url", get_str(obj, "value")?),
        "email-addr" => ("email-addr", get_str(obj, "value")?),
        "mutex" => ("mutex", get_str(obj, "name")?),
        "windows-registry-key" => ("registry-key", get_str(obj, "key")?),
        "file" => {
            // One observable carries one (type, value); prefer the strongest hash.
            let hashes = obj.get("hashes")?;
            [
                ("SHA-256", "file-sha256"),
                ("SHA-1", "file-sha1"),
                ("MD5", "file-md5"),
            ]
            .into_iter()
            .find_map(|(algo, ty)| get_str(hashes, algo).map(|h| (ty, h)))?
        }
        _ => return None,
    };
    Some((ioc_type, normalize::normalize_ioc(ioc_type, raw)))
}

/// Maps a STIX object type to the cyber-schema entity subtype it is inserted as.
fn sdo_type_hint(type_str: &str) -> &'static str {
    match type_str {
        "attack-pattern" => "attack-pattern",
        "intrusion-set" => "intrusion-set",
        "threat-actor" => "threat-actor",
        "campaign" => "campaign",
        "malware" => "malware",
        "tool" => "tool",
        "indicator" => "indicator",
        "course-of-action" => "course-of-action",
        "x-mitre-data-source" => "data-source",
        "x-mitre-data-component" => "data-component",
        _ => "cyber-object",
    }
}

/// Whether the cyber-schema subtype `type_hint` owns `platform`.
fn owns_platform(type_hint: &str) -> bool {
    matches!(
        type_hint,
        "attack-pattern" | "malware" | "tool" | "data-source"
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedRelation {
    pub source: String,
    pub target: String,
    /// Entity subtype of the source concept, so the insert can bind the role
    /// player to a type that actually plays the role (see [`cyber_post_sync`]).
    pub source_type: String,
    pub target_type: String,
    pub rel_type: String,
}

pub fn to_okf(bundle_json: &str) -> Result<(OkfBundle, Vec<TypedRelation>), String> {
    let parsed: Value = serde_json::from_str(bundle_json).map_err(|e| e.to_string())?;

    if let Some(bundle_type) = get_str(&parsed, "type")
        && bundle_type != "bundle"
    {
        return Err(format!("expected a STIX bundle, got type {bundle_type:?}"));
    }

    let objects = parsed
        .get("objects")
        .and_then(Value::as_array)
        .ok_or_else(|| "STIX bundle has no 'objects' array".to_string())?;

    let mut concepts = Vec::new();
    let mut links = Vec::new();
    let mut sros = Vec::new();
    // Concept id -> entity subtype, used to type SRO role players (see below).
    let mut id_types: HashMap<String, &'static str> = HashMap::new();
    // Candidate typed relations as (source-id, target-id, rel-type), resolved to
    // typed role players after every concept's subtype is known.
    let mut raw_rels: Vec<(String, String, String)> = Vec::new();

    for obj in objects {
        let type_str = get_str(obj, "type")
            .ok_or_else(|| "STIX object missing required 'type'".to_string())?;

        if type_str == "relationship" {
            sros.push(obj);
            continue;
        }

        let id_str = get_str(obj, "id")
            .ok_or_else(|| format!("STIX {type_str} object missing required 'id'"))?;
        let concept_id = derive_id(id_str);

        let mut typed = vec![str_attr("stix-id", id_str)];

        // Observable (SCO) vs. domain object (SDO). An SCO carries its IOC as the
        // schema-mandated ioc-type/ioc-value pair; an SDO maps to its entity type.
        let type_hint = if let Some((ioc_type, ioc_value)) = sco_ioc(type_str, obj) {
            typed.push(str_attr("ioc-type", ioc_type));
            typed.push(str_attr("ioc-value", &ioc_value));
            "observable"
        } else {
            sdo_type_hint(type_str)
        };

        if let Some(pattern) = get_str(obj, "pattern") {
            typed.push(str_attr("stix-pattern", pattern));
        }
        if let Some(valid_until) = get_str(obj, "valid_until") {
            let dt = DateTime::parse_from_rfc3339(valid_until)
                .map_err(|e| format!("STIX object {id_str} has invalid valid_until: {e}"))?
                .with_timezone(&Utc);
            typed.push(TypedAttribute {
                name: "valid-until".to_string(),
                value: TypedAttrValue::Datetime(dt),
            });
        }
        if let Some(revoked) = obj.get("revoked").and_then(Value::as_bool) {
            typed.push(TypedAttribute {
                name: "revoked".to_string(),
                value: TypedAttrValue::Bool(revoked),
            });
        }
        // ATT&CK reuses an id across a live object and its deprecated or revoked
        // predecessors; only the live object claims the unique attack-id.
        let retired = ["x_mitre_deprecated", "revoked"]
            .iter()
            .any(|k| obj.get(*k).and_then(Value::as_bool) == Some(true));
        if !retired && let Some(ext_refs) = obj.get("external_references").and_then(Value::as_array)
        {
            for r in ext_refs {
                if get_str(r, "source_name") == Some("mitre-attack")
                    && let Some(ext_id) = get_str(r, "external_id")
                {
                    typed.push(str_attr("attack-id", ext_id));
                }
            }
        }
        // platform and kill-chain-phase are @card(0..) — one typed attribute each.
        // Platforms are written only on the subtypes that own `platform`.
        if owns_platform(type_hint)
            && let Some(platforms) = obj.get("x_mitre_platforms").and_then(Value::as_array)
        {
            for p in platforms.iter().filter_map(Value::as_str) {
                typed.push(str_attr("platform", p));
            }
        }
        if let Some(phases) = obj.get("kill_chain_phases").and_then(Value::as_array) {
            for ph in phases.iter().filter_map(|p| get_str(p, "phase_name")) {
                typed.push(str_attr("kill-chain-phase", ph));
            }
        }

        // ATT&CK links a data component to its data source through an embedded
        // reference rather than a relationship object; synthesize the SRO here.
        if type_str == "x-mitre-data-component"
            && let Some(src_ref) = get_str(obj, "x_mitre_data_source_ref")
        {
            raw_rels.push((
                concept_id.clone(),
                derive_id(src_ref),
                "evidence-of".to_string(),
            ));
        }

        id_types.insert(concept_id.clone(), type_hint);

        let mut tags = Vec::new();
        if let Some(labels) = obj.get("labels").and_then(Value::as_array) {
            tags.extend(labels.iter().filter_map(Value::as_str).map(String::from));
        }

        let description = get_str(obj, "description").map(String::from);
        // serde_json serializes object keys in sorted order (it is built without
        // `preserve_order`), so this content hash is stable across re-parses.
        let file_hash = {
            let mut hasher = Sha256::new();
            hasher.update(obj.to_string().as_bytes());
            hasher
                .finalize()
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>()
        };

        let mut extra_metadata = HashMap::new();
        extra_metadata.insert("stix-object".to_string(), obj.to_string());

        concepts.push(OkfConcept {
            concept_id,
            concept_type: type_str.to_string(),
            type_hint: Some(type_hint.to_string()),
            title: get_str(obj, "name").map(String::from),
            body: description.clone().unwrap_or_default(),
            description,
            resource_uri: None,
            tags,
            timestamp: None,
            program: None,
            source_path: String::new(),
            consumes: Vec::new(),
            produces: Vec::new(),
            engine: None,
            scopes: Vec::new(),
            timeout_ms: None,
            file_hash,
            extra_metadata,
            typed_attributes: typed,
        });
    }

    for sro in sros {
        let rel_type = get_str(sro, "relationship_type").unwrap_or("");
        let source_ref = get_str(sro, "source_ref").unwrap_or("");
        let target_ref = get_str(sro, "target_ref").unwrap_or("");
        if rel_type.is_empty() || source_ref.is_empty() || target_ref.is_empty() {
            continue;
        }
        let source_id = derive_id(source_ref);
        let target_id = derive_id(target_ref);

        // A typed relation if the schema models it (single source of truth:
        // `rel_roles`); otherwise a generic concept-link so nothing is dropped.
        if rel_roles(rel_type).is_some() {
            raw_rels.push((source_id, target_id, rel_type.to_string()));
        } else {
            links.push(gecko_extension_api::okf::OkfLink {
                source_id,
                target_id,
                link_text: rel_type.to_string(),
            });
        }
    }

    // Resolve each candidate's role-player types. A relation both of whose ends
    // resolve to an in-bundle concept becomes a typed relation; if an endpoint is
    // an external reference (not in this bundle) its type is unknown and the
    // relation is dropped to a concept-link so the reference is still recorded.
    // A relation whose resolved subtypes cannot play its roles (an ATT&CK shape
    // the cyber schema does not model) is also recorded as a concept-link.
    let mut typed_rels = Vec::new();
    for (source, target, rel_type) in raw_rels {
        match (id_types.get(source.as_str()), id_types.get(target.as_str())) {
            (Some(source_type), Some(target_type))
                if rel_roles(&rel_type)
                    .is_some_and(|shape| shape.admits(source_type, target_type)) =>
            {
                typed_rels.push(TypedRelation {
                    source,
                    target,
                    source_type: source_type.to_string(),
                    target_type: target_type.to_string(),
                    rel_type,
                })
            }
            _ => links.push(gecko_extension_api::okf::OkfLink {
                source_id: source,
                target_id: target,
                link_text: rel_type,
            }),
        }
    }

    Ok((
        OkfBundle {
            bundle_path: "stix-sync".to_string(),
            bundle_name: "stix-sync".to_string(),
            bundle_description: None,
            concepts,
            links,
            citations: Vec::new(),
            hierarchy: Vec::new(),
        },
        typed_rels,
    ))
}

/// A resolved relation shape: the TypeDB relation label, its source and target
/// role labels, and the cyber subtypes that play each role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RelShape {
    relation: &'static str,
    source_role: &'static str,
    target_role: &'static str,
    source_players: &'static [&'static str],
    target_players: &'static [&'static str],
}

impl RelShape {
    /// Whether `source_type` and `target_type` can play this shape's roles.
    fn admits(&self, source_type: &str, target_type: &str) -> bool {
        self.source_players.contains(&source_type) && self.target_players.contains(&target_type)
    }
}

/// Every STIX/ATT&CK relationship type the adapter links as a typed relation.
/// The labels are fixed schema identifiers (never user data), so they are the
/// only part safe to interpolate into query text. The player lists mirror the
/// cyber schema's `plays` declarations.
const REL_SHAPES: &[(&str, RelShape)] = &[
    (
        "indicates",
        RelShape {
            relation: "indicates",
            source_role: "indicator-source",
            target_role: "indicated",
            source_players: &["indicator"],
            target_players: &[
                "attack-pattern",
                "intrusion-set",
                "threat-actor",
                "campaign",
                "malware",
                "tool",
            ],
        },
    ),
    (
        "uses",
        RelShape {
            relation: "uses",
            source_role: "user",
            target_role: "used",
            source_players: &[
                "intrusion-set",
                "threat-actor",
                "campaign",
                "malware",
                "tool",
            ],
            target_players: &["attack-pattern", "malware", "tool"],
        },
    ),
    (
        "attributed-to",
        RelShape {
            relation: "attributed-to",
            source_role: "attributee",
            target_role: "actor",
            source_players: &["intrusion-set", "campaign"],
            target_players: &["intrusion-set", "threat-actor"],
        },
    ),
    (
        "mitigates",
        RelShape {
            relation: "mitigates",
            source_role: "mitigation",
            target_role: "mitigated",
            source_players: &["course-of-action"],
            target_players: &["attack-pattern", "malware", "tool", "indicator"],
        },
    ),
    (
        "targets",
        RelShape {
            relation: "targets",
            source_role: "threat-source",
            target_role: "target",
            source_players: &["intrusion-set", "threat-actor", "campaign"],
            target_players: &["host-asset"],
        },
    ),
    (
        "detects",
        RelShape {
            relation: "requires-evidence",
            source_role: "required",
            target_role: "technique",
            source_players: &["data-component"],
            target_players: &["attack-pattern"],
        },
    ),
    (
        "evidence-of",
        RelShape {
            relation: "evidence-of",
            source_role: "evidence-component",
            target_role: "evidence-source",
            source_players: &["data-component"],
            target_players: &["data-source"],
        },
    ),
];

/// Every `(relation, role, players)` the adapter links, for checking the player
/// lists against the schema's `plays` declarations.
pub fn typed_relation_players() -> Vec<(&'static str, &'static str, &'static [&'static str])> {
    REL_SHAPES
        .iter()
        .flat_map(|(_, s)| {
            [
                (s.relation, s.source_role, s.source_players),
                (s.relation, s.target_role, s.target_players),
            ]
        })
        .collect()
}

/// Resolves a STIX/ATT&CK relationship type to its relation shape.
fn rel_roles(rel_type: &str) -> Option<RelShape> {
    REL_SHAPES
        .iter()
        .find(|(t, _)| *t == rel_type)
        .map(|(_, shape)| *shape)
}

/// Builds the writes that link the typed SROs collected by [`to_okf`], one
/// idempotent relation per STIX relationship. The concept IDs are the untrusted
/// STIX `source_ref`/`target_ref` and travel as parameterized rows; only fixed
/// schema labels — the relation, the roles, and the resolved role-player entity
/// subtypes — are interpolated. The role players are bound with `isa <subtype>`
/// because binding them to the generic `concept` type would make TypeDB reject
/// the role assignment (only the subtypes play the roles). Relations with an
/// unknown `rel_type` are skipped.
pub fn cyber_post_sync(typed_rels: &[TypedRelation]) -> Vec<GraphWrite> {
    // Group by the full query shape — relation, roles, and both role-player
    // subtypes — so each distinct shape is one multi-row parameterized write.
    type Shape<'a> = (RelShape, &'a str, &'a str);
    let mut by_shape: HashMap<Shape, Vec<(&str, &str)>> = HashMap::new();
    for rel in typed_rels {
        let Some(roles) = rel_roles(&rel.rel_type) else {
            continue;
        };
        by_shape
            .entry((roles, rel.source_type.as_str(), rel.target_type.as_str()))
            .or_default()
            .push((rel.source.as_str(), rel.target.as_str()));
    }

    let mut writes: Vec<GraphWrite> = by_shape
        .into_iter()
        .map(
            |((shape, source_type, target_type), pairs)| {
                let RelShape {
                    relation: rel_name,
                    source_role,
                    target_role,
                    ..
                } = shape;
                let query = format!(
                    "given $src: string, $tgt: string; \
                     match $s isa {source_type}, has concept-id $sid; $sid == $src; \
                           $t isa {target_type}, has concept-id $tid; $tid == $tgt; \
                           not {{ $r isa {rel_name}, links ({source_role}: $s, {target_role}: $t); }}; \
                     insert $nr isa {rel_name}, links ({source_role}: $s, {target_role}: $t);"
                );
                GraphWrite {
                    query,
                    vars: vec!["src".to_string(), "tgt".to_string()],
                    rows: pairs
                        .into_iter()
                        .map(|(src, tgt)| {
                            vec![
                                GraphValue::String(src.to_string()),
                                GraphValue::String(tgt.to_string()),
                            ]
                        })
                        .collect(),
                    expect_rows: None,
                }
            },
        )
        .collect();
    // Deterministic order regardless of hash-map iteration.
    writes.sort_by(|a, b| a.query.cmp(&b.query));
    writes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(source: &str, target: &str, st: &str, tt: &str, rel_type: &str) -> TypedRelation {
        TypedRelation {
            source: source.into(),
            target: target.into(),
            source_type: st.into(),
            target_type: tt.into(),
            rel_type: rel_type.into(),
        }
    }

    #[test]
    fn post_sync_groups_relations_into_one_write_per_shape() {
        let rels = vec![
            rel("a1", "b1", "malware", "attack-pattern", "uses"),
            rel("a2", "b2", "malware", "attack-pattern", "uses"),
            rel("i1", "t1", "indicator", "attack-pattern", "indicates"),
            rel("x", "y", "malware", "attack-pattern", "frobnicates"),
        ];
        let writes = cyber_post_sync(&rels);
        assert_eq!(writes.len(), 2);
        let uses = writes
            .iter()
            .find(|w| w.query.contains("isa uses"))
            .expect("uses op");
        assert_eq!(uses.vars, vec!["src".to_string(), "tgt".to_string()]);
        assert_eq!(uses.rows.len(), 2);
        assert_eq!(uses.expect_rows, None);
        assert!(writes.iter().all(|w| !w.query.contains("frobnicates")));
    }
}
