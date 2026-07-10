use gecko_engine::okf::types::{OkfBundle, OkfConcept};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub mod normalize;

fn get_str<'a>(val: &'a Value, field: &str) -> Option<&'a str> {
    val.get(field).and_then(Value::as_str)
}

fn derive_id(stix_id: &str) -> String {
    // "indicator--uuid" -> "stix/indicator/uuid"
    let parts: Vec<&str> = stix_id.split("--").collect();
    if parts.len() == 2 {
        format!("stix/{}/{}", parts[0], parts[1])
    } else {
        format!("stix/{}", stix_id)
    }
}

pub struct TypedRelation {
    pub source: String,
    pub target: String,
    pub rel_type: String,
}

pub fn to_okf(bundle_json: &str) -> Result<(OkfBundle, Vec<TypedRelation>), String> {
    let parsed: Value = serde_json::from_str(bundle_json).map_err(|e| e.to_string())?;

    let objects = parsed
        .get("objects")
        .and_then(Value::as_array)
        .ok_or_else(|| "No 'objects' array in STIX bundle".to_string())?;

    let mut concepts = Vec::new();
    let mut sros = Vec::new();

    for obj in objects {
        let type_str = get_str(obj, "type").unwrap_or("");
        let id_str = get_str(obj, "id").unwrap_or("");

        if type_str == "relationship" {
            sros.push(obj);
            continue;
        }

        let concept_id = derive_id(id_str);
        let title = get_str(obj, "name").map(String::from);
        let description = get_str(obj, "description")
            .map(String::from)
            .unwrap_or_default();

        let mut tags = Vec::new();
        if let Some(labels) = obj.get("labels").and_then(Value::as_array) {
            tags.extend(labels.iter().filter_map(Value::as_str).map(String::from));
        }
        if let Some(phases) = obj.get("kill_chain_phases").and_then(Value::as_array) {
            tags.extend(
                phases
                    .iter()
                    .filter_map(|p| get_str(p, "phase_name"))
                    .map(String::from),
            );
        }

        let file_hash = {
            let mut hasher = Sha256::new();
            hasher.update(obj.to_string().as_bytes());
            format!("{:x}", hasher.finalize())
        };

        let mut extra_metadata = HashMap::new();
        extra_metadata.insert("metadata_json".to_string(), obj.to_string());
        extra_metadata.insert("stix-id".to_string(), id_str.to_string());

        if let Some(pattern) = get_str(obj, "pattern") {
            extra_metadata.insert("stix-pattern".to_string(), pattern.to_string());
        }
        if let Some(valid_until) = get_str(obj, "valid_until") {
            extra_metadata.insert("valid-until".to_string(), valid_until.to_string());
        }
        if let Some(revoked) = obj.get("revoked").and_then(Value::as_bool) {
            extra_metadata.insert("revoked".to_string(), revoked.to_string());
        }
        if let Some(ext_refs) = obj.get("external_references").and_then(Value::as_array) {
            for r in ext_refs {
                if get_str(r, "source_name") == Some("mitre-attack") {
                    if let Some(ext_id) = get_str(r, "external_id") {
                        extra_metadata.insert("attack-id".to_string(), ext_id.to_string());
                    }
                }
            }
        }
        if let Some(platforms) = obj.get("x_mitre_platforms").and_then(Value::as_array) {
            let p_str = platforms
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",");
            extra_metadata.insert("platform".to_string(), p_str);
        }

        if type_str == "observable" || type_str == "ipv4-addr" || type_str == "domain-name" {
            let ioc_type = match type_str {
                "ipv4-addr" => "ipv4",
                "domain-name" => "domain",
                _ => "unknown",
            };
            if let Some(val) = get_str(obj, "value") {
                extra_metadata.insert("ioc-type".to_string(), ioc_type.to_string());
                extra_metadata.insert(
                    "ioc-value".to_string(),
                    normalize::normalize_ioc(ioc_type, val),
                );
                extra_metadata.insert("type_hint".to_string(), "observable".to_string());
            }
        } else {
            let type_hint = match type_str {
                "attack-pattern" | "intrusion-set" | "threat-actor" | "campaign" | "malware"
                | "tool" | "indicator" | "course-of-action" => type_str.to_string(),
                "x-mitre-data-source" => "data-source".to_string(),
                "x-mitre-data-component" => "data-component".to_string(),
                _ => "cyber-object".to_string(),
            };
            extra_metadata.insert("type_hint".to_string(), type_hint);
        }

        let type_hint = extra_metadata.get("type_hint").cloned();

        concepts.push(OkfConcept {
            concept_id: concept_id.clone(),
            concept_type: type_str.to_string(),
            type_hint,
            title,
            description: Some(description),
            resource_uri: None,
            tags,
            timestamp: None,
            body: "".to_string(),
            program: None,
            source_path: "".to_string(),
            consumes: Vec::new(),
            produces: Vec::new(),
            engine: None,
            scopes: Vec::new(),
            timeout_ms: None,
            file_hash,
            extra_metadata,
        });
    }

    let mut links = Vec::new();
    let mut typed_rels = Vec::new();
    let valid_rels = [
        "indicates",
        "uses",
        "attributed-to",
        "mitigates",
        "observed-as",
        "evidence-of",
        "requires-evidence",
    ];

    for sro in sros {
        let rel_type = get_str(sro, "relationship_type").unwrap_or("");
        let source_ref = get_str(sro, "source_ref").unwrap_or("");
        let target_ref = get_str(sro, "target_ref").unwrap_or("");

        let source_id = derive_id(source_ref);
        let target_id = derive_id(target_ref);

        if valid_rels.contains(&rel_type) {
            typed_rels.push(TypedRelation {
                source: source_id,
                target: target_id,
                rel_type: rel_type.to_string(),
            });
        } else {
            links.push(gecko_engine::okf::types::OkfLink {
                source_id,
                target_id,
                link_text: rel_type.to_string(),
            });
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

pub async fn cyber_post_sync(
    tx: &typedb_driver::transaction::Transaction,
    typed_rels: &[TypedRelation],
) -> Result<(), String> {
    for rel in typed_rels {
        let (rel_name, source_role, target_role) = match rel.rel_type.as_str() {
            "indicates" => ("indicates", "indicator-source", "indicated"),
            "uses" => ("uses", "user", "used"),
            "attributed-to" => ("attributed-to", "attributee", "actor"),
            "mitigates" => ("mitigates", "mitigation", "mitigated"),
            "observed-as" => ("observed-as", "pattern-indicator", "observable-value"),
            "evidence-of" => ("evidence-of", "evidence-component", "evidence-source"),
            "requires-evidence" => ("requires-evidence", "technique", "required"),
            _ => continue,
        };

        let query = format!(
            "match $s isa concept, has concept-id '{}'; $t isa concept, has concept-id '{}'; \
             not {{ $r ({}: $s, {}: $t) isa {}; }}; \
             insert $r ({}: $s, {}: $t) isa {};",
            rel.source,
            rel.target,
            source_role,
            target_role,
            rel_name,
            source_role,
            target_role,
            rel_name
        );
        tx.query(&query).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}
