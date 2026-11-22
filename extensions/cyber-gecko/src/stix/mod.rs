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
}

/// The version of the STIX-to-graph mapping, folded into every concept's content
/// hash so that a mapping change refreshes every concept on the next import.
pub const STIX_MAPPING_VERSION: &str = "2";

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

        if type_hint == "data-component"
            && let Some(name) = get_str(obj, "name")
        {
            typed.push(str_attr("component-key", &name.trim().to_lowercase()));
        }

        // ATT&CK links a strategy to its analytics, and an analytic to the data
        // components it reads, through embedded references rather than
        // relationship objects; synthesize those links here.
        if type_hint == "detection-strategy"
            && let Some(refs) = obj.get("x_mitre_analytic_refs").and_then(Value::as_array)
        {
            for analytic in refs.iter().filter_map(Value::as_str) {
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
        },
        StixRelations {
            typed: typed_rels,
            requirements,
            analytics: sorted(analytics),
            strategies: sorted(strategies),
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
            relation: "strategy-detects",
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

/// Every `(relation, role, players)` the adapter links, for checking the player
/// lists against the schema's `plays` declarations.
pub fn typed_relation_players() -> Vec<(&'static str, &'static str, &'static [&'static str])> {
    REL_SHAPES
        .iter()
        .map(|(_, s)| s)
        .chain([&STRATEGY_ANALYTIC, &ANALYTIC_REQUIREMENT])
        .flat_map(|s| {
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

/// Resolves a [`TypedRelation::rel_type`], a relationship type or a synthesized
/// strategy-to-analytic link, to its relation shape.
fn shape_of(rel_type: &str) -> Option<RelShape> {
    if rel_type == STRATEGY_ANALYTIC_REL {
        Some(STRATEGY_ANALYTIC)
    } else {
        rel_roles(rel_type)
    }
}

/// Builds the writes that link the relations collected by [`to_okf`].
///
/// - **SROs** become one idempotent relation each, inserted under a `not { … }`
///   guard. Relations with an unknown `rel_type` are skipped.
/// - **Strategy analytics and analytic requirements** are replaced: their writes
///   delete every such relation on a strategy or analytic in the bundle,
///   then every such relation left without its analytic (or strategy) player by a
///   deleted concept, and then insert the current set.
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
    writes.extend(replacement_writes(rels));
    writes
}

/// One row per value, for a single-variable write.
fn id_rows<'a>(ids: impl IntoIterator<Item = &'a str>) -> Vec<Vec<GraphValue>> {
    ids.into_iter()
        .map(|id| vec![GraphValue::String(id.to_string())])
        .collect()
}

/// The replace-then-insert writes for `strategy-analytic` and
/// `analytic-requirement`, in the order they must run.
fn replacement_writes(rels: &StixRelations) -> Vec<GraphWrite> {
    let links: Vec<&TypedRelation> = rels
        .typed
        .iter()
        .filter(|r| r.rel_type == STRATEGY_ANALYTIC_REL)
        .collect();

    // Every strategy and analytic in the bundle, so that one which lost all its
    // references also loses its old relations.
    let scope: BTreeSet<&str> = rels
        .strategies
        .iter()
        .chain(&rels.analytics)
        .map(String::as_str)
        .collect();

    let mut channel_rows = Vec::new();
    for r in &rels.requirements {
        for ch in &r.channels {
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
                          { $r isa strategy-analytic, links (strategy: $x); } or \
                          { $r isa strategy-analytic, links (analytic: $x); } or \
                          { $r isa analytic-requirement, links (analytic: $x); }; \
                    delete $r;"
                .to_string(),
            vars: vec!["id".to_string()],
            rows: id_rows(scope.iter().copied()),
            expect_rows: None,
        },
        GraphWrite::plain(
            "match { $r isa analytic-requirement; not { $r links (analytic: $a1); }; } or \
                   { $r isa strategy-analytic; not { $r links (analytic: $a2); }; } or \
                   { $r isa strategy-analytic; not { $r links (strategy: $s3); }; }; \
             delete $r;",
        ),
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
            .unwrap_or_else(|| panic!("{id} not in the bundle"))
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
        let scoped_delete = position("delete $r");
        let orphan_delete = position("not { $r links (analytic:");
        let requirement_insert = position("insert $r isa analytic-requirement");
        let channel_insert = position("has log-source-channel ==");
        let strategy_insert = position("insert $r isa strategy-analytic");
        assert!(scoped_delete < orphan_delete);
        assert!(orphan_delete < requirement_insert.min(strategy_insert));
        assert!(requirement_insert < channel_insert);
        assert_eq!(writes[requirement_insert].rows.len(), 1);
        assert_eq!(writes[channel_insert].rows.len(), 2);
        assert_eq!(writes[strategy_insert].rows.len(), 2);
        assert!(
            writes
                .iter()
                .any(|w| w.query.contains("isa strategy-detects") && w.query.contains("not {")),
            "strategy-detects is insert-only under a not guard"
        );
    }
}
