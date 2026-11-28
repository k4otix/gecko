use chrono::{DateTime, Utc};
use gecko_extension_api::okf::{OkfBundle, OkfConcept, TypedAttrValue, TypedAttribute};
use gecko_extension_api::{GraphValue, GraphWrite};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};

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
        "x-mitre-detection-strategy" => "detection-strategy",
        "x-mitre-analytic" => "analytic",
        "x-mitre-tactic" => "tactic",
        _ => "cyber-object",
    }
}

/// Whether the cyber-schema subtype `type_hint` owns `platform`.
fn owns_platform(type_hint: &str) -> bool {
    matches!(
        type_hint,
        "attack-pattern" | "malware" | "tool" | "data-source" | "analytic"
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

/// A log source an analytic reads for a data component: one per (analytic,
/// component, trimmed name) across the analytic's `x_mitre_log_source_references`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticRequirement {
    /// The analytic's concept id.
    pub analytic: String,
    /// The data component's concept id.
    pub component: String,
    /// The log-source name as ATT&CK spells it, trimmed.
    pub name: String,
    /// The trimmed, lowercased name that sensors are matched on.
    pub key: String,
    /// Every distinct channel ATT&CK gives for this log source, sorted; a channel of
    /// `"None"` or an absent channel contributes none.
    pub channels: Vec<String>,
}

/// The relations [`to_okf`] collects for [`cyber_post_sync`] to write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StixRelations {
    /// Relations between two in-bundle concepts whose subtypes play the roles.
    pub typed: Vec<TypedRelation>,
    /// Every analytic's log-source requirements on in-bundle data components.
    pub requirements: Vec<AnalyticRequirement>,
    /// The concept id of every analytic in the bundle, sorted.
    pub analytics: Vec<String>,
    /// The concept id of every detection strategy in the bundle, sorted.
    pub strategies: Vec<String>,
    /// The concept id of every attack-pattern in the bundle, sorted.
    pub techniques: Vec<String>,
    /// The concept id of every attack-pattern, intrusion set, malware and tool in
    /// the bundle, sorted: the objects a `revoked-by` can retire.
    pub revocable: Vec<String>,
}

/// The version of the STIX-to-graph mapping, folded into every concept's content
/// hash so that a mapping change refreshes every concept on the next import.
pub const STIX_MAPPING_VERSION: &str = "3";

/// The content hash of a STIX object under mapping `version`: SHA-256 over the
/// version, then the object's JSON. serde_json serializes object keys in sorted
/// order (it is built without `preserve_order`), so the hash is stable across
/// re-parses.
fn concept_hash(version: &str, obj: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(obj.to_string().as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids.dedup();
    ids
}

/// The relation label of a synthesized strategy-to-analytic link, used as its
/// [`TypedRelation::rel_type`].
const STRATEGY_ANALYTIC_REL: &str = "strategy-analytic";

/// The relation label of a sub-technique's link to its parent technique.
pub(crate) const SUBTECHNIQUE_OF_REL: &str = "subtechnique-of";

/// The relation label of a revoked object's link to its replacement.
pub(crate) const REVOKED_BY_REL: &str = "revoked-by";

/// The relation label of a technique's link to a tactic.
pub(crate) const TECHNIQUE_TACTIC_REL: &str = "technique-tactic";

/// The relation a strategy's `detects` SRO becomes, which a re-import replaces.
const STRATEGY_DETECTS_REL: &str = "strategy-detects";

/// A requirement's grouping key: (analytic id, component id, trimmed name).
type RequirementKey = (String, String, String);

/// Collects an analytic's `x_mitre_log_source_references` into `out`, grouped by
/// (analytic, component, trimmed name) with their channels. A reference without a
/// component ref or a non-empty name, or with a non-string channel, is refused.
fn collect_requirements(
    analytic_id: &str,
    stix_id: &str,
    obj: &Value,
    out: &mut BTreeMap<RequirementKey, BTreeSet<String>>,
) -> Result<(), String> {
    let Some(refs) = obj.get("x_mitre_log_source_references") else {
        return Ok(());
    };
    let refs = refs
        .as_array()
        .ok_or_else(|| format!("analytic {stix_id} has non-array x_mitre_log_source_references"))?;
    for r in refs {
        let component = get_str(r, "x_mitre_data_component_ref").ok_or_else(|| {
            format!(
                "analytic {stix_id} has a log-source reference without x_mitre_data_component_ref"
            )
        })?;
        let name = get_str(r, "name")
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .ok_or_else(|| {
                format!("analytic {stix_id} has a log-source reference without a name")
            })?;
        let channels = out
            .entry((
                analytic_id.to_string(),
                derive_id(component),
                name.to_string(),
            ))
            .or_default();
        match r.get("channel") {
            None | Some(Value::Null) => {}
            Some(Value::String(ch)) => {
                let ch = ch.trim();
                if !ch.is_empty() && ch != "None" {
                    channels.insert(ch.to_string());
                }
            }
            Some(other) => {
                return Err(format!(
                    "analytic {stix_id} has a log-source reference with a non-string channel {other}"
                ));
            }
        }
    }
    Ok(())
}

/// The kill chain each ATT&CK domain's techniques name in `kill_chain_phases`, by
/// the domain a tactic lists in `x_mitre_domains`.
const KILL_CHAINS: [(&str, &str); 3] = [
    ("enterprise-attack", "mitre-attack"),
    ("mobile-attack", "mitre-mobile-attack"),
    ("ics-attack", "mitre-ics-attack"),
];

/// A tactic's index key: (kill-chain name, shortname).
type TacticKey = (String, String);

/// Indexes every tactic in `objects`, live or retired, by the kill chain of each
/// of its domains and its shortname, to its (concept id, STIX id). A tactic
/// without a shortname or a domain, with a domain outside [`KILL_CHAINS`], or
/// sharing a key with another tactic is refused.
fn tactic_index(objects: &[Value]) -> Result<BTreeMap<TacticKey, (String, String)>, String> {
    let mut index: BTreeMap<TacticKey, (String, String)> = BTreeMap::new();
    for obj in objects
        .iter()
        .filter(|o| get_str(o, "type") == Some("x-mitre-tactic"))
    {
        let stix_id =
            get_str(obj, "id").ok_or("STIX x-mitre-tactic object missing required 'id'")?;
        let shortname = get_str(obj, "x_mitre_shortname")
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("tactic {stix_id} has no x_mitre_shortname"))?;
        let domains = obj
            .get("x_mitre_domains")
            .and_then(Value::as_array)
            .filter(|d| !d.is_empty())
            .ok_or_else(|| format!("tactic {stix_id} has no x_mitre_domains"))?;
        for domain in domains {
            let chain = domain
                .as_str()
                .and_then(|d| KILL_CHAINS.iter().find(|(name, _)| *name == d))
                .map(|(_, chain)| *chain)
                .ok_or_else(|| {
                    format!(
                        "tactic {stix_id} has the domain {domain}, which names no ATT&CK kill chain"
                    )
                })?;
            let key = (chain.to_string(), shortname.to_string());
            if let Some((_, other)) = index.get(&key)
                && other != stix_id
            {
                return Err(format!(
                    "tactics {other} and {stix_id} both name the kill-chain phase {chain}:{shortname}"
                ));
            }
            index.insert(key, (derive_id(stix_id), stix_id.to_string()));
        }
    }
    Ok(index)
}

pub fn to_okf(bundle_json: &str) -> Result<(OkfBundle, StixRelations), String> {
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
    let mut raw_requirements: BTreeMap<RequirementKey, BTreeSet<String>> = BTreeMap::new();
    let mut analytics = Vec::new();
    let mut strategies = Vec::new();
    let mut techniques = Vec::new();
    let mut revocable = Vec::new();
    let tactics = tactic_index(objects)?;
    // Technique-to-tactic relations, resolved from kill-chain phases through
    // `tactics`; both ends are in-bundle, so they need no type resolution.
    let mut tactic_rels = Vec::new();

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
        for (field, attr) in [("revoked", "revoked"), ("x_mitre_deprecated", "deprecated")] {
            if let Some(flag) = obj.get(field).and_then(Value::as_bool) {
                typed.push(TypedAttribute {
                    name: attr.to_string(),
                    value: TypedAttrValue::Bool(flag),
                });
            }
        }
        // ATT&CK reuses an id across a live object and its deprecated or revoked
        // predecessors; only the live object claims the unique attack-id, and a
        // retired one keeps it as the non-unique retired-attack-id.
        let retired = ["x_mitre_deprecated", "revoked"]
            .iter()
            .any(|k| obj.get(*k).and_then(Value::as_bool) == Some(true));
        let id_attr = if retired {
            "retired-attack-id"
        } else {
            "attack-id"
        };
        if let Some(ext_refs) = obj.get("external_references").and_then(Value::as_array) {
            for r in ext_refs {
                if get_str(r, "source_name") == Some("mitre-attack")
                    && let Some(ext_id) = get_str(r, "external_id")
                {
                    typed.push(str_attr(id_attr, ext_id));
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
        // A technique's phase on an ATT&CK kill chain names a tactic in the bundle;
        // a bundle without tactics, or a phase on another kill chain, keeps the
        // phase only as the attribute.
        if let Some(phases) = obj.get("kill_chain_phases").and_then(Value::as_array) {
            for ph in phases {
                let Some(phase) = get_str(ph, "phase_name") else {
                    continue;
                };
                typed.push(str_attr("kill-chain-phase", phase));
                if type_hint != "attack-pattern" || tactics.is_empty() {
                    continue;
                }
                let Some(chain) = get_str(ph, "kill_chain_name")
                    .filter(|c| KILL_CHAINS.iter().any(|(_, chain)| chain == c))
                else {
                    continue;
                };
                let (tactic, _) = tactics
                    .get(&(chain.to_string(), phase.to_string()))
                    .ok_or_else(|| {
                        format!(
                            "technique {id_str} has the kill-chain phase {chain}:{phase}, which \
                             names no tactic in the bundle"
                        )
                    })?;
                tactic_rels.push(TypedRelation {
                    source: concept_id.clone(),
                    target: tactic.clone(),
                    source_type: type_hint.to_string(),
                    target_type: "tactic".to_string(),
                    rel_type: TECHNIQUE_TACTIC_REL.to_string(),
                });
            }
        }
        if type_hint == "tactic" {
            let shortname = get_str(obj, "x_mitre_shortname")
                .ok_or_else(|| format!("tactic {id_str} has no x_mitre_shortname"))?;
            typed.push(str_attr("tactic-shortname", shortname));
        }

        if type_hint == "data-component"
            && let Some(name) = get_str(obj, "name")
        {
            typed.push(str_attr("component-key", &name.trim().to_lowercase()));
        }

        // ATT&CK links a strategy to its analytics, and an analytic to the data
        // components it reads, through embedded references rather than
        // relationship objects; synthesize those links here.
        if type_hint == "detection-strategy"
            && let Some(refs) = obj.get("x_mitre_analytic_refs")
        {
            let refs = refs.as_array().ok_or_else(|| {
                format!("detection strategy {id_str} has non-array x_mitre_analytic_refs")
            })?;
            for analytic in refs {
                let analytic = analytic.as_str().ok_or_else(|| {
                    format!(
                        "detection strategy {id_str} has a non-string entry {analytic} in \
                         x_mitre_analytic_refs"
                    )
                })?;
                raw_rels.push((
                    concept_id.clone(),
                    derive_id(analytic),
                    STRATEGY_ANALYTIC_REL.to_string(),
                ));
            }
        }
        if type_hint == "analytic" {
            collect_requirements(&concept_id, id_str, obj, &mut raw_requirements)?;
        }

        match type_hint {
            "analytic" => analytics.push(concept_id.clone()),
            "detection-strategy" => strategies.push(concept_id.clone()),
            _ => {}
        }
        if type_hint == "attack-pattern" {
            techniques.push(concept_id.clone());
        }
        if REVOKED_BY.source_players.contains(&type_hint) {
            revocable.push(concept_id.clone());
        }
        id_types.insert(concept_id.clone(), type_hint);

        let mut tags = Vec::new();
        if let Some(labels) = obj.get("labels").and_then(Value::as_array) {
            tags.extend(labels.iter().filter_map(Value::as_str).map(String::from));
        }

        let description = get_str(obj, "description").map(String::from);
        let file_hash = concept_hash(STIX_MAPPING_VERSION, obj);

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
            engine: None,
            scopes: Vec::new(),
            timeout_ms: None,
            provenance: Default::default(),
            file_hash,
            extra_metadata,
            typed_attributes: typed,
        });
    }

    // The one target of each (relation, source) a source may name at most once,
    // as the target's STIX id.
    let mut single_targets: HashMap<(&str, String), &str> = HashMap::new();
    for sro in sros {
        let rel_type = get_str(sro, "relationship_type").unwrap_or("");
        let source_ref = get_str(sro, "source_ref").unwrap_or("");
        let target_ref = get_str(sro, "target_ref").unwrap_or("");
        if rel_type.is_empty() || source_ref.is_empty() || target_ref.is_empty() {
            continue;
        }
        let source_id = derive_id(source_ref);
        let target_id = derive_id(target_ref);

        // An object has at most one parent and one replacement among the pairs
        // that become typed relations.
        if let Some(shape) = replaced_sro_shape(rel_type)
            && let (Some(source_type), Some(target_type)) =
                (id_types.get(&source_id), id_types.get(&target_id))
            && shape.admits(source_type, target_type)
            && let Some(first) =
                single_targets.insert((shape.relation, source_id.clone()), target_ref)
            && first != target_ref
        {
            return Err(format!(
                "{source_ref} has two {rel_type} targets, {first} and {target_ref}"
            ));
        }

        // A typed relation if the schema models it (single source of truth:
        // `sro_shape`); otherwise a generic concept-link so nothing is dropped.
        if sro_shape(rel_type).is_some() {
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
    let mut typed_rels = tactic_rels;
    for (source, target, rel_type) in raw_rels {
        match (id_types.get(source.as_str()), id_types.get(target.as_str())) {
            (Some(source_type), Some(target_type))
                if shape_of(&rel_type)
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

    // A requirement is kept only when its component is an in-bundle data
    // component, as for the endpoints of a typed relation.
    let requirements = raw_requirements
        .into_iter()
        .filter(|((_, component, _), _)| {
            id_types.get(component.as_str()) == Some(&"data-component")
        })
        .map(
            |((analytic, component, name), channels)| AnalyticRequirement {
                analytic,
                component,
                key: name.to_lowercase(),
                name,
                channels: channels.into_iter().collect(),
            },
        )
        .collect();

    Ok((
        OkfBundle {
            bundle_path: "stix-sync".to_string(),
            bundle_name: "stix-sync".to_string(),
            bundle_description: None,
            concepts,
            links,
            citations: Vec::new(),
            hierarchy: Vec::new(),
            okf_version: None,
            warnings: Vec::new(),
        },
        StixRelations {
            typed: typed_rels,
            requirements,
            analytics: sorted(analytics),
            strategies: sorted(strategies),
            techniques: sorted(techniques),
            revocable: sorted(revocable),
        },
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
            relation: STRATEGY_DETECTS_REL,
            source_role: "strategy",
            target_role: "technique",
            source_players: &["detection-strategy"],
            target_players: &["attack-pattern"],
        },
    ),
];

/// The strategy-to-analytic link synthesized from `x_mitre_analytic_refs`, which a
/// re-import replaces rather than appends to.
const STRATEGY_ANALYTIC: RelShape = RelShape {
    relation: STRATEGY_ANALYTIC_REL,
    source_role: "strategy",
    target_role: "analytic",
    source_players: &["detection-strategy"],
    target_players: &["analytic"],
};

/// The analytic-to-component requirement synthesized from
/// `x_mitre_log_source_references`, which a re-import replaces.
const ANALYTIC_REQUIREMENT: RelShape = RelShape {
    relation: "analytic-requirement",
    source_role: "analytic",
    target_role: "component",
    source_players: &["analytic"],
    target_players: &["data-component"],
};

/// A sub-technique's link to its parent technique.
const SUBTECHNIQUE_OF: RelShape = RelShape {
    relation: SUBTECHNIQUE_OF_REL,
    source_role: "subtechnique",
    target_role: "parent",
    source_players: &["attack-pattern"],
    target_players: &["attack-pattern"],
};

/// A revoked object's link to the object that replaced it.
const REVOKED_BY: RelShape = RelShape {
    relation: REVOKED_BY_REL,
    source_role: "revoked",
    target_role: "replacement",
    source_players: &["attack-pattern", "intrusion-set", "malware", "tool"],
    target_players: &["attack-pattern", "intrusion-set", "malware", "tool"],
};

/// A technique's link to a tactic named by one of its kill-chain phases.
const TECHNIQUE_TACTIC: RelShape = RelShape {
    relation: TECHNIQUE_TACTIC_REL,
    source_role: "technique",
    target_role: "tactic",
    source_players: &["attack-pattern"],
    target_players: &["tactic"],
};

/// Every `(relation, role, players)` the adapter links, for checking the player
/// lists against the schema's `plays` declarations.
pub fn typed_relation_players() -> Vec<(&'static str, &'static str, &'static [&'static str])> {
    typed_shapes()
        .flat_map(|s| {
            [
                (s.relation, s.source_role, s.source_players),
                (s.relation, s.target_role, s.target_players),
            ]
        })
        .collect()
}

/// The shape of every relation the adapter links, one per relation.
fn typed_shapes() -> impl Iterator<Item = &'static RelShape> {
    REL_SHAPES.iter().map(|(_, s)| s).chain([
        &STRATEGY_ANALYTIC,
        &ANALYTIC_REQUIREMENT,
        &SUBTECHNIQUE_OF,
        &REVOKED_BY,
        &TECHNIQUE_TACTIC,
    ])
}

/// Resolves a STIX/ATT&CK relationship type to its relation shape.
fn rel_roles(rel_type: &str) -> Option<RelShape> {
    REL_SHAPES
        .iter()
        .find(|(t, _)| *t == rel_type)
        .map(|(_, shape)| *shape)
}

/// The relationship types a re-import replaces rather than appends to, whose
/// relation label is the relationship type itself.
const REPLACED_SRO_SHAPES: [RelShape; 2] = [SUBTECHNIQUE_OF, REVOKED_BY];

/// Resolves a relationship type whose relation a re-import replaces to its shape.
fn replaced_sro_shape(rel_type: &str) -> Option<RelShape> {
    REPLACED_SRO_SHAPES
        .into_iter()
        .find(|s| s.relation == rel_type)
}

/// Resolves any relationship type the adapter links as a typed relation.
fn sro_shape(rel_type: &str) -> Option<RelShape> {
    rel_roles(rel_type).or_else(|| replaced_sro_shape(rel_type))
}

/// Resolves a [`TypedRelation::rel_type`], a relationship type or a synthesized
/// strategy-to-analytic link, to its relation shape.
fn shape_of(rel_type: &str) -> Option<RelShape> {
    if rel_type == STRATEGY_ANALYTIC_REL {
        Some(STRATEGY_ANALYTIC)
    } else {
        sro_shape(rel_type)
    }
}

/// Resolves a [`TypedRelation::rel_type`] of a structure relation, which a
/// re-import replaces, to its shape.
fn structure_shape(rel_type: &str) -> Option<RelShape> {
    if rel_type == TECHNIQUE_TACTIC_REL {
        Some(TECHNIQUE_TACTIC)
    } else {
        replaced_sro_shape(rel_type)
    }
}

/// Builds the writes that link the relations collected by [`to_okf`].
///
/// - **First**, every typed relation left without a player by a concept the sync
///   deleted is deleted ([`orphan_sweep`]).
/// - **SROs** other than `detects`, `subtechnique-of` and `revoked-by` become one
///   idempotent relation each, inserted under a `not { … }` guard. Relations with
///   an unknown `rel_type` are skipped.
/// - **Strategy detects, strategy analytics and analytic requirements** are
///   replaced: their writes delete every such relation on a strategy or analytic
///   in the bundle, and then insert the current set.
/// - **Sub-technique, revocation and technique-tactic relations** are replaced the
///   same way, on every technique (and, for `revoked-by`, every revocable object)
///   in the bundle.
///
/// The concept IDs are untrusted STIX references and travel as parameterized rows,
/// as do the log-source names, keys and channels; only fixed schema labels — the
/// relation, the roles, and the resolved role-player entity subtypes — are
/// interpolated. The role players are bound with `isa <subtype>` because binding
/// them to the generic `concept` type would make TypeDB reject the role assignment
/// (only the subtypes play the roles).
pub fn cyber_post_sync(rels: &StixRelations) -> Vec<GraphWrite> {
    let typed_rels = &rels.typed;
    // Group by the full query shape — relation, roles, and both role-player
    // subtypes — so each distinct shape is one multi-row parameterized write.
    type Shape<'a> = (RelShape, &'a str, &'a str);
    let mut by_shape: HashMap<Shape, BTreeSet<(&str, &str)>> = HashMap::new();
    for rel in typed_rels {
        let Some(roles) = rel_roles(&rel.rel_type).filter(|r| r.relation != STRATEGY_DETECTS_REL)
        else {
            continue;
        };
        by_shape
            .entry((roles, rel.source_type.as_str(), rel.target_type.as_str()))
            .or_default()
            .insert((rel.source.as_str(), rel.target.as_str()));
    }

    let mut inserts: Vec<GraphWrite> = by_shape
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
    inserts.sort_by(|a, b| a.query.cmp(&b.query));
    let mut writes = orphan_sweep();
    writes.extend(inserts);
    writes.extend(replacement_writes(rels));
    writes
}

/// The `observed-as` roles, which the schema requires but no adapter writes; its
/// players are STIX concepts, so a re-import can still leave one without a player.
const OBSERVED_AS: (&str, [&str; 2]) = ("observed-as", ["pattern-indicator", "observable-value"]);

/// One delete per typed relation (every relation in [`typed_relation_players`], and
/// `observed-as`) of the rows left without a player for either role. A re-import
/// deletes the concepts it dropped, and only the core relations go with them, so a
/// typed relation they played would otherwise fail its required roles at commit.
fn orphan_sweep() -> Vec<GraphWrite> {
    typed_shapes()
        .map(|s| (s.relation, [s.source_role, s.target_role]))
        .chain([OBSERVED_AS])
        .map(|(relation, [a, b])| {
            GraphWrite::plain(format!(
                "match $r isa {relation}; \
                       {{ not {{ $r links ({a}: $p0); }}; }} or {{ not {{ $r links ({b}: $p1); }}; }}; \
                 delete $r;"
            ))
        })
        .collect()
}

/// One row per value, for a single-variable write.
fn id_rows<'a>(ids: impl IntoIterator<Item = &'a str>) -> Vec<Vec<GraphValue>> {
    ids.into_iter()
        .map(|id| vec![GraphValue::String(id.to_string())])
        .collect()
}

/// The replace-then-insert writes for `strategy-detects`, `strategy-analytic`,
/// `analytic-requirement` and the structure relations, in the order they must run.
fn replacement_writes(rels: &StixRelations) -> Vec<GraphWrite> {
    let mut writes = detection_replacement_writes(rels);
    writes.extend(structure_replacement_writes(rels));
    writes
}

/// The replace-then-insert writes for `subtechnique-of`, `technique-tactic` and
/// `revoked-by`: delete each such relation whose source is in the bundle, then
/// insert the current set, one write per (relation, source subtype, target
/// subtype).
fn structure_replacement_writes(rels: &StixRelations) -> Vec<GraphWrite> {
    // Keyed by labels, so the writes come out in a deterministic order; a pair
    // named twice is written once.
    type Key<'a> = (&'static str, &'a str, &'a str);
    type Pairs<'a> = (RelShape, BTreeSet<(&'a str, &'a str)>);
    let mut by_shape: BTreeMap<Key, Pairs> = BTreeMap::new();
    for r in &rels.typed {
        let Some(shape) = structure_shape(&r.rel_type) else {
            continue;
        };
        by_shape
            .entry((shape.relation, &r.source_type, &r.target_type))
            .or_insert_with(|| (shape, BTreeSet::new()))
            .1
            .insert((&r.source, &r.target));
    }

    let mut writes = vec![
        GraphWrite {
            query: format!(
                "given $id: string; \
                 match $x isa attack-pattern, has concept-id $xid; $xid == $id; \
                       {{ $r isa {SUBTECHNIQUE_OF_REL}, links (subtechnique: $x); }} or \
                       {{ $r isa {TECHNIQUE_TACTIC_REL}, links (technique: $x); }}; \
                 delete $r;"
            ),
            vars: vec!["id".to_string()],
            rows: id_rows(rels.techniques.iter().map(String::as_str)),
            expect_rows: None,
        },
        GraphWrite {
            query: format!(
                "given $id: string; \
                 match $x isa cyber-object, has concept-id $xid; $xid == $id; \
                       $r isa {REVOKED_BY_REL}, links (revoked: $x); \
                 delete $r;"
            ),
            vars: vec!["id".to_string()],
            rows: id_rows(rels.revocable.iter().map(String::as_str)),
            expect_rows: None,
        },
    ];
    writes.extend(by_shape.into_iter().map(
        |((relation, source_type, target_type), (shape, pairs))| {
            let RelShape {
                source_role,
                target_role,
                ..
            } = shape;
            GraphWrite {
                query: format!(
                    "given $s: string, $t: string; \
                         match $a isa {source_type}, has concept-id $aid; $aid == $s; \
                               $b isa {target_type}, has concept-id $bid; $bid == $t; \
                         insert $r isa {relation}, links ({source_role}: $a, {target_role}: $b);"
                ),
                vars: vec!["s".to_string(), "t".to_string()],
                rows: pairs
                    .into_iter()
                    .map(|(s, t)| {
                        vec![
                            GraphValue::String(s.to_string()),
                            GraphValue::String(t.to_string()),
                        ]
                    })
                    .collect(),
                expect_rows: Some(1),
            }
        },
    ));
    writes
}

/// The replace-then-insert writes for `strategy-detects`, `strategy-analytic` and
/// `analytic-requirement`, in the order they must run.
fn detection_replacement_writes(rels: &StixRelations) -> Vec<GraphWrite> {
    let links: Vec<&TypedRelation> = rels
        .typed
        .iter()
        .filter(|r| r.rel_type == STRATEGY_ANALYTIC_REL)
        .collect();
    // Two `detects` SROs may name the same pair; the relation is written once.
    let detects: BTreeSet<(&str, &str)> = rels
        .typed
        .iter()
        .filter(|r| rel_roles(&r.rel_type).is_some_and(|s| s.relation == STRATEGY_DETECTS_REL))
        .map(|r| (r.source.as_str(), r.target.as_str()))
        .collect();

    // Every strategy and analytic in the bundle, so that one which lost all its
    // references also loses its old relations.
    let scope: BTreeSet<&str> = rels
        .strategies
        .iter()
        .chain(&rels.analytics)
        .map(String::as_str)
        .collect();

    // A requirement is inserted with its first channel; only the further channels
    // (rare) are attached afterwards, since matching a requirement back by its
    // name scans every requirement per row.
    let mut channel_rows = Vec::new();
    for r in &rels.requirements {
        for ch in r.channels.iter().skip(1) {
            channel_rows.push(vec![
                GraphValue::String(r.analytic.clone()),
                GraphValue::String(r.component.clone()),
                GraphValue::String(r.name.clone()),
                GraphValue::String(ch.clone()),
            ]);
        }
    }

    vec![
        GraphWrite {
            query: "given $id: string; \
                    match $x isa cyber-object, has concept-id $xid; $xid == $id; \
                          { $r isa strategy-detects, links (strategy: $x); } or \
                          { $r isa strategy-analytic, links (strategy: $x); } or \
                          { $r isa strategy-analytic, links (analytic: $x); } or \
                          { $r isa analytic-requirement, links (analytic: $x); }; \
                    delete $r;"
                .to_string(),
            vars: vec!["id".to_string()],
            rows: id_rows(scope.iter().copied()),
            expect_rows: None,
        },
        GraphWrite {
            query: "given $s: string, $t: string; \
                    match $st isa detection-strategy, has concept-id $sid; $sid == $s; \
                          $te isa attack-pattern, has concept-id $tid; $tid == $t; \
                    insert $r isa strategy-detects, links (strategy: $st, technique: $te);"
                .to_string(),
            vars: vec!["s".to_string(), "t".to_string()],
            rows: detects
                .iter()
                .map(|(s, t)| {
                    vec![
                        GraphValue::String(s.to_string()),
                        GraphValue::String(t.to_string()),
                    ]
                })
                .collect(),
            expect_rows: Some(1),
        },
        GraphWrite {
            query: "given $s: string, $a: string; \
                    match $st isa detection-strategy, has concept-id $sid; $sid == $s; \
                          $an isa analytic, has concept-id $aid; $aid == $a; \
                    insert $r isa strategy-analytic, links (strategy: $st, analytic: $an);"
                .to_string(),
            vars: vec!["s".to_string(), "a".to_string()],
            rows: links
                .iter()
                .map(|r| {
                    vec![
                        GraphValue::String(r.source.clone()),
                        GraphValue::String(r.target.clone()),
                    ]
                })
                .collect(),
            expect_rows: Some(1),
        },
        GraphWrite {
            query: "given $a: string, $c: string, $name: string, $key: string; \
                    match $an isa analytic, has concept-id $aid; $aid == $a; \
                          $co isa data-component, has concept-id $cid; $cid == $c; \
                    insert $r isa analytic-requirement, links (analytic: $an, component: $co), \
                           has log-source-name == $name, has log-source-key == $key;"
                .to_string(),
            vars: ["a", "c", "name", "key"].map(String::from).to_vec(),
            rows: rels
                .requirements
                .iter()
                .filter(|r| r.channels.is_empty())
                .map(|r| {
                    vec![
                        GraphValue::String(r.analytic.clone()),
                        GraphValue::String(r.component.clone()),
                        GraphValue::String(r.name.clone()),
                        GraphValue::String(r.key.clone()),
                    ]
                })
                .collect(),
            expect_rows: Some(1),
        },
        GraphWrite {
            query: "given $a: string, $c: string, $name: string, $key: string, $ch: string; \
                    match $an isa analytic, has concept-id $aid; $aid == $a; \
                          $co isa data-component, has concept-id $cid; $cid == $c; \
                    insert $r isa analytic-requirement, links (analytic: $an, component: $co), \
                           has log-source-name == $name, has log-source-key == $key, has log-source-channel == $ch;"
                .to_string(),
            vars: ["a", "c", "name", "key", "ch"].map(String::from).to_vec(),
            rows: rels
                .requirements
                .iter()
                .filter(|r| !r.channels.is_empty())
                .map(|r| {
                    vec![
                        GraphValue::String(r.analytic.clone()),
                        GraphValue::String(r.component.clone()),
                        GraphValue::String(r.name.clone()),
                        GraphValue::String(r.key.clone()),
                        GraphValue::String(r.channels[0].clone()),
                    ]
                })
                .collect(),
            expect_rows: Some(1),
        },
        GraphWrite {
            query: "given $a: string, $c: string, $name: string, $ch: string; \
                    match $an isa analytic, has concept-id $aid; $aid == $a; \
                          $co isa data-component, has concept-id $cid; $cid == $c; \
                          $r isa analytic-requirement, links (analytic: $an, component: $co), \
                             has log-source-name $n; $n == $name; \
                    insert $r has log-source-channel == $ch;"
                .to_string(),
            vars: ["a", "c", "name", "ch"].map(String::from).to_vec(),
            rows: channel_rows,
            expect_rows: Some(1),
        },
    ]
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
        let rels = StixRelations {
            typed: vec![
                rel("a1", "b1", "malware", "attack-pattern", "uses"),
                rel("a2", "b2", "malware", "attack-pattern", "uses"),
                rel("i1", "t1", "indicator", "attack-pattern", "indicates"),
                rel("x", "y", "malware", "attack-pattern", "frobnicates"),
            ],
            ..StixRelations::default()
        };
        let writes: Vec<GraphWrite> = cyber_post_sync(&rels)
            .into_iter()
            .filter(|w| !w.rows.is_empty() && !w.vars.is_empty())
            .collect();
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

    #[test]
    fn the_orphan_sweep_covers_every_typed_relation_before_any_insert() {
        let rels = StixRelations {
            typed: vec![rel("a1", "b1", "malware", "attack-pattern", "uses")],
            ..StixRelations::default()
        };
        let writes = cyber_post_sync(&rels);
        let mut relations: Vec<&str> = typed_relation_players()
            .into_iter()
            .map(|(relation, _, _)| relation)
            .chain(["observed-as"])
            .collect();
        relations.dedup();
        let sweep = &writes[..relations.len()];
        for (relation, w) in relations.iter().zip(sweep) {
            assert!(
                w.query.starts_with(&format!("match $r isa {relation};"))
                    && w.query.contains("not { $r links (")
                    && w.vars.is_empty(),
                "{relation}: {}",
                w.query
            );
        }
        assert!(
            writes[relations.len()..]
                .iter()
                .all(|w| !w.query.starts_with("match $r isa")),
            "a sweep runs after an insert"
        );
    }

    #[test]
    fn repeated_sros_become_one_row() {
        let rels = StixRelations {
            typed: vec![
                rel("a1", "b1", "malware", "attack-pattern", "uses"),
                rel("a1", "b1", "malware", "attack-pattern", "uses"),
                rel("a1", "b2", "malware", "attack-pattern", "uses"),
            ],
            ..StixRelations::default()
        };
        let writes: Vec<GraphWrite> = cyber_post_sync(&rels)
            .into_iter()
            .filter(|w| !w.rows.is_empty() && !w.vars.is_empty())
            .collect();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].rows.len(), 2, "{:?}", writes[0].rows);
    }

    const STRATEGY: &str = "x-mitre-detection-strategy--00000000-0000-4000-8000-000000000001";
    const WINDOWS: &str = "x-mitre-analytic--00000000-0000-4000-8000-000000000001";
    const LINUX: &str = "x-mitre-analytic--00000000-0000-4000-8000-000000000002";
    const TECHNIQUE: &str = "attack-pattern--00000000-0000-4000-8000-000000000001";
    const COMPONENT: &str = "x-mitre-data-component--00000000-0000-4000-8000-000000000001";
    const OUTSIDE: &str = "x-mitre-data-component--00000000-0000-4000-8000-0000000000ff";

    /// A bundle of a technique, a strategy that detects it through two analytics,
    /// one component, and the given log-source references on the Windows analytic.
    fn detection_bundle(windows_sources: Value) -> String {
        serde_json::json!({
            "type": "bundle",
            "objects": [
                { "type": "attack-pattern", "id": TECHNIQUE, "name": "T" },
                { "type": "x-mitre-detection-strategy", "id": STRATEGY, "name": "S",
                  "x_mitre_analytic_refs": [WINDOWS, LINUX] },
                { "type": "x-mitre-analytic", "id": WINDOWS, "name": "W",
                  "x_mitre_platforms": ["Windows"],
                  "x_mitre_log_source_references": windows_sources },
                { "type": "x-mitre-analytic", "id": LINUX, "name": "L",
                  "x_mitre_platforms": ["Linux"] },
                { "type": "x-mitre-data-component", "id": COMPONENT, "name": " Process Creation " },
                { "type": "relationship", "id": "relationship--00000000-0000-4000-8000-000000000001",
                  "relationship_type": "detects", "source_ref": STRATEGY, "target_ref": TECHNIQUE },
            ]
        })
        .to_string()
    }

    fn concept<'a>(bundle: &'a OkfBundle, stix_id: &str) -> &'a OkfConcept {
        let id = derive_id(stix_id);
        bundle
            .concepts
            .iter()
            .find(|c| c.concept_id == id)
            .expect("the fixture's concept is in the bundle")
    }

    fn attr_values(concept: &OkfConcept, name: &str) -> Vec<TypedAttrValue> {
        concept
            .typed_attributes
            .iter()
            .filter(|a| a.name == name)
            .map(|a| a.value.clone())
            .collect()
    }

    fn source(name: &str, channel: Option<&str>) -> Value {
        let mut r = serde_json::json!({ "x_mitre_data_component_ref": COMPONENT, "name": name });
        if let Some(channel) = channel {
            r["channel"] = channel.into();
        }
        r
    }

    #[test]
    fn detection_strategy_and_analytic_get_their_types() {
        let (bundle, _) = to_okf(&detection_bundle(serde_json::json!([]))).expect("to_okf");
        assert_eq!(
            concept(&bundle, STRATEGY).type_hint.as_deref(),
            Some("detection-strategy")
        );
        assert_eq!(
            concept(&bundle, WINDOWS).type_hint.as_deref(),
            Some("analytic")
        );
    }

    #[test]
    fn analytic_platform_is_written() {
        let (bundle, _) = to_okf(&detection_bundle(serde_json::json!([]))).expect("to_okf");
        assert_eq!(
            attr_values(concept(&bundle, LINUX), "platform"),
            vec![TypedAttrValue::String("Linux".into())]
        );
    }

    #[test]
    fn strategy_analytic_is_synthesized_from_analytic_refs() {
        let (_, rels) = to_okf(&detection_bundle(serde_json::json!([]))).expect("to_okf");
        let mut pairs: Vec<(&str, &str, &str)> = rels
            .typed
            .iter()
            .filter(|r| r.rel_type == "strategy-analytic")
            .map(|r| (r.source.as_str(), r.target.as_str(), r.target_type.as_str()))
            .collect();
        pairs.sort();
        let (s, w, l) = (derive_id(STRATEGY), derive_id(WINDOWS), derive_id(LINUX));
        let mut expected = vec![
            (s.as_str(), w.as_str(), "analytic"),
            (s.as_str(), l.as_str(), "analytic"),
        ];
        expected.sort();
        assert_eq!(pairs, expected);
        assert!(
            rels.typed
                .iter()
                .any(|r| r.rel_type == "detects" && r.source_type == "detection-strategy"),
            "the strategy's detects SRO is a typed relation: {:?}",
            rels.typed
        );
    }

    #[test]
    fn requirements_group_by_analytic_component_and_trimmed_name() {
        let (_, rels) = to_okf(&detection_bundle(serde_json::json!([
            source("saas:audit ", Some("b")),
            source("saas:audit", Some("a")),
        ])))
        .expect("to_okf");
        assert_eq!(
            rels.requirements,
            vec![AnalyticRequirement {
                analytic: derive_id(WINDOWS),
                component: derive_id(COMPONENT),
                name: "saas:audit".into(),
                key: "saas:audit".into(),
                channels: vec!["a".into(), "b".into()],
            }]
        );
    }

    #[test]
    fn a_none_channel_writes_no_channel() {
        let (_, rels) = to_okf(&detection_bundle(serde_json::json!([
            source("WinEventLog:Security", Some("None")),
            source("NSM:Flow", None),
        ])))
        .expect("to_okf");
        assert_eq!(rels.requirements.len(), 2);
        assert!(
            rels.requirements.iter().all(|r| r.channels.is_empty()),
            "{:?}",
            rels.requirements
        );
        assert_eq!(rels.requirements[0].key, "nsm:flow");
        assert_eq!(rels.requirements[1].key, "wineventlog:security");
    }

    #[test]
    fn a_log_source_reference_without_a_name_is_refused() {
        let err = to_okf(&detection_bundle(serde_json::json!([
            { "x_mitre_data_component_ref": COMPONENT }
        ])))
        .unwrap_err();
        assert!(err.contains(WINDOWS) && err.contains("name"), "got: {err}");
    }

    /// A bundle holding one strategy whose `x_mitre_analytic_refs` is `refs`.
    fn strategy_bundle(refs: Value) -> String {
        serde_json::json!({
            "type": "bundle",
            "objects": [
                { "type": "x-mitre-detection-strategy", "id": STRATEGY, "name": "S",
                  "x_mitre_analytic_refs": refs },
                { "type": "x-mitre-analytic", "id": WINDOWS, "name": "W" },
            ]
        })
        .to_string()
    }

    #[test]
    fn non_array_analytic_refs_are_refused() {
        let err = to_okf(&strategy_bundle(serde_json::json!(WINDOWS))).unwrap_err();
        assert!(
            err.contains(STRATEGY) && err.contains("x_mitre_analytic_refs"),
            "got: {err}"
        );
    }

    #[test]
    fn a_non_string_analytic_ref_is_refused() {
        let err = to_okf(&strategy_bundle(serde_json::json!([WINDOWS, 7]))).unwrap_err();
        assert!(
            err.contains(STRATEGY) && err.contains("x_mitre_analytic_refs"),
            "got: {err}"
        );
    }

    #[test]
    fn a_data_component_detects_falls_to_a_concept_link() {
        let bundle = serde_json::json!({
            "type": "bundle",
            "objects": [
                { "type": "attack-pattern", "id": TECHNIQUE, "name": "T" },
                { "type": "x-mitre-data-component", "id": COMPONENT, "name": "C" },
                { "type": "relationship", "id": "relationship--00000000-0000-4000-8000-000000000002",
                  "relationship_type": "detects", "source_ref": COMPONENT, "target_ref": TECHNIQUE },
            ]
        })
        .to_string();
        let (manifest, rels) = to_okf(&bundle).expect("to_okf");
        assert!(rels.typed.is_empty(), "{:?}", rels.typed);
        assert_eq!(manifest.links.len(), 1);
        assert_eq!(manifest.links[0].link_text, "detects");
    }

    #[test]
    fn deprecated_is_a_typed_attribute() {
        let bundle = serde_json::json!({
            "type": "bundle",
            "objects": [
                { "type": "attack-pattern", "id": TECHNIQUE, "x_mitre_deprecated": true },
                { "type": "x-mitre-data-component", "id": COMPONENT, "x_mitre_deprecated": false },
                { "type": "x-mitre-analytic", "id": WINDOWS },
            ]
        })
        .to_string();
        let (manifest, _) = to_okf(&bundle).expect("to_okf");
        assert_eq!(
            attr_values(concept(&manifest, TECHNIQUE), "deprecated"),
            vec![TypedAttrValue::Bool(true)]
        );
        assert_eq!(
            attr_values(concept(&manifest, COMPONENT), "deprecated"),
            vec![TypedAttrValue::Bool(false)]
        );
        assert!(attr_values(concept(&manifest, WINDOWS), "deprecated").is_empty());
    }

    #[test]
    fn a_data_component_carries_its_name_key() {
        let (manifest, _) = to_okf(&detection_bundle(serde_json::json!([]))).expect("to_okf");
        assert_eq!(
            attr_values(concept(&manifest, COMPONENT), "component-key"),
            vec![TypedAttrValue::String("process creation".into())]
        );
    }

    #[test]
    fn the_mapping_version_changes_the_hash() {
        let obj = serde_json::json!({ "type": "attack-pattern", "id": TECHNIQUE });
        assert_ne!(concept_hash("1", &obj), concept_hash("2", &obj));
        assert_eq!(concept_hash("2", &obj), concept_hash("2", &obj.clone()));
        let (manifest, _) =
            to_okf(&serde_json::json!({ "type": "bundle", "objects": [obj.clone()] }).to_string())
                .expect("to_okf");
        assert_eq!(
            manifest.concepts[0].file_hash,
            concept_hash(STIX_MAPPING_VERSION, &obj)
        );
    }

    #[test]
    fn a_requirement_on_an_out_of_bundle_component_is_dropped() {
        let (_, rels) = to_okf(&detection_bundle(serde_json::json!([
            source("WinEventLog:Security", Some("EventCode=4688")),
            { "x_mitre_data_component_ref": OUTSIDE, "name": "NSM:Flow" },
        ])))
        .expect("to_okf");
        assert_eq!(rels.requirements.len(), 1, "{:?}", rels.requirements);
        assert_eq!(rels.requirements[0].component, derive_id(COMPONENT));
    }

    #[test]
    fn every_analytic_and_strategy_in_the_bundle_is_listed() {
        let (_, rels) = to_okf(&detection_bundle(serde_json::json!([]))).expect("to_okf");
        let mut analytics = vec![derive_id(WINDOWS), derive_id(LINUX)];
        analytics.sort();
        assert_eq!(rels.analytics, analytics);
        assert_eq!(rels.strategies, vec![derive_id(STRATEGY)]);
        let scoped = cyber_post_sync(&rels)
            .into_iter()
            .find(|w| w.query.contains("delete $r") && !w.vars.is_empty())
            .expect("the scoped delete");
        assert_eq!(scoped.rows.len(), 3);
    }

    #[test]
    fn post_sync_replaces_strategy_analytics_and_requirements() {
        let (_, rels) = to_okf(&detection_bundle(serde_json::json!([
            source("WinEventLog:Security", Some("EventCode=4688")),
            source("WinEventLog:Security", Some("EventCode=4624")),
        ])))
        .expect("to_okf");
        let writes = cyber_post_sync(&rels);
        let position = |needle: &str| {
            writes
                .iter()
                .position(|w| w.query.contains(needle))
                .unwrap_or_else(|| panic!("no op contains {needle:?}"))
        };
        let orphan_delete = position("not { $r links (analytic:");
        let scoped_delete = position("given $id: string");
        let requirement_insert = position("has log-source-key == $key, has log-source-channel");
        let channel_insert = position("insert $r has log-source-channel");
        let strategy_insert = position("insert $r isa strategy-analytic");
        let detects_insert = position("insert $r isa strategy-detects");
        assert!(orphan_delete < scoped_delete);
        assert!(scoped_delete < requirement_insert.min(strategy_insert).min(detects_insert));
        assert!(requirement_insert < channel_insert);
        assert_eq!(writes[requirement_insert].rows.len(), 1);
        assert_eq!(writes[requirement_insert].rows[0].len(), 5);
        assert_eq!(writes[channel_insert].rows.len(), 1);
        assert_eq!(writes[strategy_insert].rows.len(), 2);
        assert_eq!(writes[detects_insert].rows.len(), 1);
        assert!(
            !writes
                .iter()
                .any(|w| w.query.contains("insert $nr isa strategy-detects")),
            "strategy-detects is replaced, not inserted under a not guard"
        );
    }

    const EXECUTION: &str = "x-mitre-tactic--00000000-0000-4000-8000-000000000001";
    const PERSISTENCE: &str = "x-mitre-tactic--00000000-0000-4000-8000-000000000002";
    const PARENT: &str = "attack-pattern--00000000-0000-4000-8000-000000000002";
    const OTHER_PARENT: &str = "attack-pattern--00000000-0000-4000-8000-000000000003";
    const SUB: &str = "attack-pattern--00000000-0000-4000-8000-000000000004";
    const MALWARE: &str = "malware--00000000-0000-4000-8000-000000000001";
    const TOOL: &str = "tool--00000000-0000-4000-8000-000000000001";
    const OTHER_TOOL: &str = "tool--00000000-0000-4000-8000-000000000002";
    const INTRUSION_SET: &str = "intrusion-set--00000000-0000-4000-8000-000000000001";
    const OLD_MITIGATION: &str = "course-of-action--00000000-0000-4000-8000-000000000001";
    const NEW_MITIGATION: &str = "course-of-action--00000000-0000-4000-8000-000000000002";

    /// An enterprise tactic.
    fn tactic(stix_id: &str, shortname: &str) -> Value {
        serde_json::json!({ "type": "x-mitre-tactic", "id": stix_id, "name": shortname,
                            "x_mitre_shortname": shortname,
                            "x_mitre_domains": ["enterprise-attack"] })
    }

    /// A technique with the given (kill-chain name, phase name) phases.
    fn technique(stix_id: &str, phases: &[(&str, &str)]) -> Value {
        let phases: Vec<Value> = phases
            .iter()
            .map(|(chain, phase)| {
                serde_json::json!({ "kill_chain_name": chain, "phase_name": phase })
            })
            .collect();
        serde_json::json!({ "type": "attack-pattern", "id": stix_id, "name": stix_id,
                            "kill_chain_phases": phases })
    }

    fn sro(n: u32, rel_type: &str, source: &str, target: &str) -> Value {
        serde_json::json!({ "type": "relationship",
                            "id": format!("relationship--00000000-0000-4000-8000-{n:012}"),
                            "relationship_type": rel_type,
                            "source_ref": source, "target_ref": target })
    }

    fn bundle(objects: Vec<Value>) -> String {
        serde_json::json!({ "type": "bundle", "objects": objects }).to_string()
    }

    fn typed_of<'a>(rels: &'a StixRelations, rel_type: &str) -> Vec<&'a TypedRelation> {
        rels.typed
            .iter()
            .filter(|r| r.rel_type == rel_type)
            .collect()
    }

    #[test]
    fn a_tactic_is_typed_with_its_shortname() {
        let (manifest, _) = to_okf(&bundle(vec![tactic(EXECUTION, "execution")])).expect("to_okf");
        let t = concept(&manifest, EXECUTION);
        assert_eq!(t.type_hint.as_deref(), Some("tactic"));
        assert_eq!(
            attr_values(t, "tactic-shortname"),
            vec![TypedAttrValue::String("execution".into())]
        );
    }

    #[test]
    fn a_tactic_without_a_shortname_or_with_an_unmapped_domain_is_refused() {
        let mut no_shortname = tactic(EXECUTION, "execution");
        no_shortname
            .as_object_mut()
            .expect("an object")
            .remove("x_mitre_shortname");
        let err = to_okf(&bundle(vec![no_shortname])).unwrap_err();
        assert!(
            err.contains(EXECUTION) && err.contains("x_mitre_shortname"),
            "got: {err}"
        );

        let mut pre_attack = tactic(EXECUTION, "execution");
        pre_attack["x_mitre_domains"] = serde_json::json!(["pre-attack"]);
        let err = to_okf(&bundle(vec![pre_attack])).unwrap_err();
        assert!(
            err.contains(EXECUTION) && err.contains("pre-attack"),
            "got: {err}"
        );
    }

    #[test]
    fn two_tactics_with_one_kill_chain_phase_are_refused() {
        let err = to_okf(&bundle(vec![
            tactic(EXECUTION, "execution"),
            tactic(PERSISTENCE, "execution"),
        ]))
        .unwrap_err();
        assert!(
            err.contains(EXECUTION) && err.contains(PERSISTENCE),
            "got: {err}"
        );
    }

    #[test]
    fn a_technique_links_to_the_tactic_its_phase_names() {
        let (manifest, rels) = to_okf(&bundle(vec![
            technique(PARENT, &[("mitre-attack", "execution")]),
            tactic(EXECUTION, "execution"),
            tactic(PERSISTENCE, "persistence"),
        ]))
        .expect("to_okf");
        assert_eq!(
            typed_of(&rels, TECHNIQUE_TACTIC_REL),
            vec![&rel(
                &derive_id(PARENT),
                &derive_id(EXECUTION),
                "attack-pattern",
                "tactic",
                TECHNIQUE_TACTIC_REL
            )]
        );
        assert_eq!(
            attr_values(concept(&manifest, PARENT), "kill-chain-phase"),
            vec![TypedAttrValue::String("execution".into())]
        );
    }

    #[test]
    fn a_phase_naming_no_tactic_is_refused() {
        let err = to_okf(&bundle(vec![
            tactic(EXECUTION, "execution"),
            technique(PARENT, &[("mitre-attack", "persistence")]),
        ]))
        .unwrap_err();
        assert!(
            err.contains(PARENT) && err.contains("persistence"),
            "got: {err}"
        );
    }

    #[test]
    fn a_mobile_phase_does_not_link_an_enterprise_tactic() {
        let err = to_okf(&bundle(vec![
            tactic(PERSISTENCE, "persistence"),
            technique(PARENT, &[("mitre-mobile-attack", "persistence")]),
        ]))
        .unwrap_err();
        assert!(
            err.contains(PARENT) && err.contains("mitre-mobile-attack"),
            "got: {err}"
        );
    }

    #[test]
    fn foreign_kill_chains_and_tactic_free_bundles_are_kept_as_attributes() {
        let (manifest, rels) = to_okf(&bundle(vec![
            tactic(EXECUTION, "execution"),
            technique(PARENT, &[("lockheed-martin-cyber-kill-chain", "delivery")]),
        ]))
        .expect("a foreign kill chain is not resolved");
        assert!(typed_of(&rels, TECHNIQUE_TACTIC_REL).is_empty());
        assert_eq!(
            attr_values(concept(&manifest, PARENT), "kill-chain-phase"),
            vec![TypedAttrValue::String("delivery".into())]
        );

        let (manifest, rels) = to_okf(&bundle(vec![technique(
            PARENT,
            &[("mitre-attack", "execution")],
        )]))
        .expect("a bundle without tactics resolves no phase");
        assert!(typed_of(&rels, TECHNIQUE_TACTIC_REL).is_empty());
        assert_eq!(
            attr_values(concept(&manifest, PARENT), "kill-chain-phase"),
            vec![TypedAttrValue::String("execution".into())]
        );
    }

    #[test]
    fn subtechnique_of_and_revoked_by_are_typed() {
        let (manifest, rels) = to_okf(&bundle(vec![
            technique(PARENT, &[]),
            technique(SUB, &[]),
            serde_json::json!({ "type": "malware", "id": MALWARE, "revoked": true }),
            serde_json::json!({ "type": "tool", "id": TOOL }),
            sro(1, "subtechnique-of", SUB, PARENT),
            sro(2, "revoked-by", MALWARE, TOOL),
        ]))
        .expect("to_okf");
        assert!(manifest.links.is_empty(), "{:?}", manifest.links);
        assert_eq!(
            typed_of(&rels, SUBTECHNIQUE_OF_REL),
            vec![&rel(
                &derive_id(SUB),
                &derive_id(PARENT),
                "attack-pattern",
                "attack-pattern",
                SUBTECHNIQUE_OF_REL
            )]
        );
        assert_eq!(
            typed_of(&rels, REVOKED_BY_REL),
            vec![&rel(
                &derive_id(MALWARE),
                &derive_id(TOOL),
                "malware",
                "tool",
                REVOKED_BY_REL
            )]
        );
    }

    #[test]
    fn an_unadmitted_revocation_pair_stays_a_concept_link() {
        let (manifest, rels) = to_okf(&bundle(vec![
            serde_json::json!({ "type": "course-of-action", "id": OLD_MITIGATION, "revoked": true }),
            serde_json::json!({ "type": "course-of-action", "id": NEW_MITIGATION }),
            sro(1, "revoked-by", OLD_MITIGATION, NEW_MITIGATION),
        ]))
        .expect("to_okf");
        assert!(typed_of(&rels, REVOKED_BY_REL).is_empty());
        assert_eq!(manifest.links.len(), 1);
        assert_eq!(manifest.links[0].link_text, "revoked-by");
    }

    #[test]
    fn a_second_parent_or_replacement_is_refused() {
        let err = to_okf(&bundle(vec![
            technique(PARENT, &[]),
            technique(OTHER_PARENT, &[]),
            technique(SUB, &[]),
            sro(1, "subtechnique-of", SUB, PARENT),
            sro(2, "subtechnique-of", SUB, OTHER_PARENT),
        ]))
        .unwrap_err();
        assert!(
            err.contains(SUB) && err.contains("subtechnique-of"),
            "got: {err}"
        );

        let err = to_okf(&bundle(vec![
            serde_json::json!({ "type": "malware", "id": MALWARE, "revoked": true }),
            serde_json::json!({ "type": "tool", "id": TOOL }),
            serde_json::json!({ "type": "tool", "id": OTHER_TOOL }),
            sro(1, "revoked-by", MALWARE, TOOL),
            sro(2, "revoked-by", MALWARE, OTHER_TOOL),
        ]))
        .unwrap_err();
        assert!(
            err.contains(MALWARE) && err.contains("revoked-by"),
            "got: {err}"
        );

        to_okf(&bundle(vec![
            technique(PARENT, &[]),
            technique(SUB, &[]),
            sro(1, "subtechnique-of", SUB, PARENT),
            sro(2, "subtechnique-of", SUB, PARENT),
        ]))
        .expect("a repeated pair names one parent");
    }

    #[test]
    fn a_retired_object_keeps_its_attack_id_as_retired() {
        let with_id = |id: &str, attack_id: &str, revoked: bool| {
            serde_json::json!({ "type": "attack-pattern", "id": id, "revoked": revoked,
                "external_references": [{ "source_name": "mitre-attack", "external_id": attack_id }] })
        };
        let (manifest, _) = to_okf(&bundle(vec![
            with_id(PARENT, "T1000", false),
            with_id(SUB, "T1000.001", true),
        ]))
        .expect("to_okf");
        let live = concept(&manifest, PARENT);
        assert_eq!(
            attr_values(live, "attack-id"),
            vec![TypedAttrValue::String("T1000".into())]
        );
        assert!(attr_values(live, "retired-attack-id").is_empty());
        let retired = concept(&manifest, SUB);
        assert!(attr_values(retired, "attack-id").is_empty());
        assert_eq!(
            attr_values(retired, "retired-attack-id"),
            vec![TypedAttrValue::String("T1000.001".into())]
        );
    }

    #[test]
    fn techniques_and_revocable_list_their_ids() {
        let (_, rels) = to_okf(&bundle(vec![
            technique(SUB, &[]),
            technique(PARENT, &[]),
            serde_json::json!({ "type": "malware", "id": MALWARE }),
            serde_json::json!({ "type": "tool", "id": TOOL }),
            serde_json::json!({ "type": "intrusion-set", "id": INTRUSION_SET }),
            serde_json::json!({ "type": "course-of-action", "id": OLD_MITIGATION }),
            tactic(EXECUTION, "execution"),
        ]))
        .expect("to_okf");
        assert_eq!(
            rels.techniques,
            sorted(vec![derive_id(SUB), derive_id(PARENT)])
        );
        assert_eq!(
            rels.revocable,
            sorted(
                [SUB, PARENT, MALWARE, TOOL, INTRUSION_SET]
                    .map(derive_id)
                    .to_vec()
            )
        );
    }

    #[test]
    fn structure_relations_are_replaced_not_inserted_under_a_guard() {
        let (_, rels) = to_okf(&bundle(vec![
            tactic(EXECUTION, "execution"),
            technique(PARENT, &[("mitre-attack", "execution")]),
            technique(SUB, &[("mitre-attack", "execution")]),
            serde_json::json!({ "type": "malware", "id": MALWARE, "revoked": true }),
            serde_json::json!({ "type": "tool", "id": TOOL }),
            sro(1, "subtechnique-of", SUB, PARENT),
            sro(2, "subtechnique-of", SUB, PARENT),
            sro(3, "revoked-by", MALWARE, TOOL),
        ]))
        .expect("to_okf");
        let writes = cyber_post_sync(&rels);
        let position = |needle: &str| {
            writes
                .iter()
                .position(|w| w.query.contains(needle))
                .unwrap_or_else(|| panic!("no op contains {needle:?}"))
        };
        let technique_delete = position("links (subtechnique: $x); } or");
        let revoked_delete = position("$r isa revoked-by, links (revoked: $x);");
        let subtechnique_insert = position("insert $r isa subtechnique-of");
        let tactic_insert = position("insert $r isa technique-tactic");
        let revoked_insert = position("insert $r isa revoked-by");
        assert!(
            technique_delete.max(revoked_delete)
                < subtechnique_insert.min(tactic_insert).min(revoked_insert)
        );
        assert_eq!(writes[technique_delete].rows.len(), 2);
        assert_eq!(writes[revoked_delete].rows.len(), 4);
        assert_eq!(writes[subtechnique_insert].rows.len(), 1);
        assert_eq!(writes[tactic_insert].rows.len(), 2);
        assert_eq!(writes[revoked_insert].rows.len(), 1);
        assert!(
            writes[revoked_insert].query.contains("isa malware")
                && writes[revoked_insert].query.contains("isa tool"),
            "{}",
            writes[revoked_insert].query
        );
        for relation in [SUBTECHNIQUE_OF_REL, REVOKED_BY_REL, TECHNIQUE_TACTIC_REL] {
            assert!(
                !writes
                    .iter()
                    .any(|w| w.query.contains(&format!("insert $nr isa {relation}"))),
                "{relation} is inserted under a not guard"
            );
        }
    }
}
