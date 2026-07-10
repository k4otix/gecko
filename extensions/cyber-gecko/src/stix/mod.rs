use chrono::{DateTime, Utc};
use gecko_engine::okf::types::{OkfBundle, OkfConcept, TypedAttrValue, TypedAttribute};
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
        if let Some(ext_refs) = obj.get("external_references").and_then(Value::as_array) {
            for r in ext_refs {
                if get_str(r, "source_name") == Some("mitre-attack")
                    && let Some(ext_id) = get_str(r, "external_id")
                {
                    typed.push(str_attr("attack-id", ext_id));
                }
            }
        }
        // platform and kill-chain-phase are @card(0..) — one typed attribute each.
        if let Some(platforms) = obj.get("x_mitre_platforms").and_then(Value::as_array) {
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
            format!("{:x}", hasher.finalize())
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
            links.push(gecko_engine::okf::types::OkfLink {
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
    let mut typed_rels = Vec::new();
    for (source, target, rel_type) in raw_rels {
        match (id_types.get(source.as_str()), id_types.get(target.as_str())) {
            (Some(source_type), Some(target_type)) => typed_rels.push(TypedRelation {
                source,
                target,
                source_type: source_type.to_string(),
                target_type: target_type.to_string(),
                rel_type,
            }),
            _ => links.push(gecko_engine::okf::types::OkfLink {
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

/// A resolved relation shape: `(relation-label, source-role, target-role)`.
type RelShape = (&'static str, &'static str, &'static str);

/// Resolves a STIX/ATT&CK relationship type to its TypeDB relation label and the
/// source/target role labels. These labels are fixed schema identifiers (never
/// user data), so they are the only part safe to interpolate into query text.
fn rel_roles(rel_type: &str) -> Option<RelShape> {
    match rel_type {
        "indicates" => Some(("indicates", "indicator-source", "indicated")),
        "uses" => Some(("uses", "user", "used")),
        "attributed-to" => Some(("attributed-to", "attributee", "actor")),
        "mitigates" => Some(("mitigates", "mitigation", "mitigated")),
        "targets" => Some(("targets", "threat-source", "target")),
        "detects" => Some(("requires-evidence", "required", "technique")),
        "evidence-of" => Some(("evidence-of", "evidence-component", "evidence-source")),
        _ => None,
    }
}

/// Inserts the typed SROs collected by [`to_okf`], one idempotent relation per
/// STIX relationship. The concept IDs are the untrusted STIX
/// `source_ref`/`target_ref` and flow through the parameterized `given` stage;
/// only fixed schema labels — the relation, the roles, and the resolved role-
/// player entity subtypes — are interpolated. The role players are bound with
/// `isa <subtype>` because binding them to the generic `concept` type would make
/// TypeDB reject the role assignment (only the subtypes play the roles).
pub async fn cyber_post_sync(
    tx: &typedb_driver::transaction::Transaction,
    typed_rels: &[TypedRelation],
) -> Result<(), String> {
    use typedb_driver::concept::Value;
    use typedb_driver::given::{GivenRowEntry, GivenRows};

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

    for (((rel_name, source_role, target_role), source_type, target_type), pairs) in by_shape {
        let query = format!(
            "given $src: string, $tgt: string; \
             match $s isa {source_type}, has concept-id $sid; $sid == $src; \
                   $t isa {target_type}, has concept-id $tid; $tid == $tgt; \
                   not {{ $r isa {rel_name}, links ({source_role}: $s, {target_role}: $t); }}; \
             insert $nr isa {rel_name}, links ({source_role}: $s, {target_role}: $t);"
        );
        let mut given = GivenRows::new(vec!["src".to_string(), "tgt".to_string()], pairs.len());
        for (src, tgt) in pairs {
            given
                .push_row(vec![
                    GivenRowEntry::Value(Value::String(src.to_string())),
                    GivenRowEntry::Value(Value::String(tgt.to_string())),
                ])
                .map_err(|e| e.to_string())?;
        }
        tx.query_with_rows(&query, given)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
