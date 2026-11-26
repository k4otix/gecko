//! The sensor file: the platforms a user runs and the sensors that watch them.
//!
//! [`parse`] reads and validates the YAML file, [`to_okf`] converts it to an OKF
//! bundle holding one `monitored-environment` and one `sensor` per entry,
//! [`check_attack`] refuses a database without ATT&CK detection data,
//! [`check_bundle_name`] refuses a bundle name that already holds other records,
//! and [`check_against_graph`] and [`notices`] compare the file with the ATT&CK
//! data in the target database before and after `gecko sync-sensors` syncs it.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use gecko_extension_api::okf::{OkfBundle, OkfConcept, TypedAttrValue, TypedAttribute};
use gecko_extension_api::{GraphReader, GraphValue};
use serde_json::Value as Json;
use serde_yaml::{Mapping, Value};
use sha2::{Digest, Sha256};

/// The bundle name a sensor file syncs under when none is given.
pub const DEFAULT_BUNDLE_NAME: &str = "sensors";

/// Versions the sensor-to-OKF mapping inside every concept's content hash, so a
/// mapping change re-writes concepts whose file entry is unchanged.
const SENSOR_MAPPING_VERSION: &str = "sensor-mapping-1";

/// The refusal when the target database holds no ATT&CK detection strategies.
const NO_ATTACK: &str = "the database holds no ATT&CK detection strategies; \
                         sync the ATT&CK bundle into this database first";

/// Binds `$a` to a live analytic of a live detection strategy, where live is
/// neither revoked, deprecated nor retired, as the coverage functions read them.
const LIVE_ANALYTIC: &str = "$a isa analytic; \
     not { $a has revoked true; }; \
     not { $a has deprecated true; }; \
     not { $a has retired-at $ra; }; \
     strategy-analytic (strategy: $st, analytic: $a); \
     not { $st has revoked true; }; \
     not { $st has deprecated true; }; \
     not { $st has retired-at $sra; };";

/// A log source or data component as the sensor file names it: the trimmed
/// display name and the trimmed, lowercased key it is matched on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub name: String,
    pub key: String,
}

/// One sensor entry: its name, the platforms it runs on, the log sources it
/// collects and the data components it declares as a fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sensor {
    pub name: String,
    pub platforms: Vec<String>,
    pub log_sources: Vec<Named>,
    pub components: Vec<Named>,
}

/// A parsed and validated sensor file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorFile {
    pub platforms: Vec<String>,
    pub sensors: Vec<Sensor>,
}

/// Whether `name` is a non-empty run of `[A-Za-z0-9._-]`, the shape of a sensor
/// name and of a sensor bundle name.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Parses and validates a sensor file, naming the first problem it finds.
pub fn parse(yaml: &str) -> Result<SensorFile, String> {
    let doc: Value = serde_yaml::from_str(yaml)
        .map_err(|e| format!("the sensor file is not valid YAML: {e}"))?;
    let top = doc
        .as_mapping()
        .ok_or("the sensor file is not a mapping of `platforms` and `sensors`")?;
    check_fields(top, &["platforms", "sensors"], "the sensor file")?;

    let platforms = match top.get("platforms") {
        None | Some(Value::Null) => return Err("the sensor file has no `platforms`".into()),
        Some(v) => strings(v, "`platforms`")?,
    };
    if platforms.is_empty() {
        return Err("the sensor file's `platforms` is empty".into());
    }

    let sensors = match top.get("sensors") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Sequence(items)) => items
            .iter()
            .enumerate()
            .map(|(i, v)| sensor(i + 1, v))
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("the sensor file's `sensors` is not a list".into()),
    };
    let mut seen = HashSet::new();
    for s in &sensors {
        if !seen.insert(s.name.as_str()) {
            return Err(format!("duplicate sensor name '{}'", s.name));
        }
    }
    Ok(SensorFile { platforms, sensors })
}

/// Refuses a field of `map` outside `allowed`.
fn check_fields(map: &Mapping, allowed: &[&str], context: &str) -> Result<(), String> {
    for key in map.keys() {
        let key = key
            .as_str()
            .ok_or_else(|| format!("{context} has a field name that is not a string"))?;
        if !allowed.contains(&key) {
            return Err(format!("{context} has an unknown field `{key}`"));
        }
    }
    Ok(())
}

/// The distinct trimmed strings of the list `value`, in file order.
fn strings(value: &Value, context: &str) -> Result<Vec<String>, String> {
    let items = value
        .as_sequence()
        .ok_or_else(|| format!("{context} is not a list"))?;
    let mut out: Vec<String> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let s = item
            .as_str()
            .ok_or_else(|| format!("{context} entry {} is not a string", i + 1))?
            .trim();
        if s.is_empty() {
            return Err(format!("{context} entry {} is empty", i + 1));
        }
        if !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    }
    Ok(out)
}

/// The optional list field `field` of `map`, as names keyed for matching.
fn named(map: &Mapping, field: &str, context: &str) -> Result<Vec<Named>, String> {
    Ok(match map.get(field) {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => strings(v, &format!("{context}: `{field}`"))?
            .into_iter()
            .map(|name| Named {
                key: name.to_lowercase(),
                name,
            })
            .collect(),
    })
}

/// Validates the `index`th (1-based) sensor entry.
fn sensor(index: usize, value: &Value) -> Result<Sensor, String> {
    let map = value
        .as_mapping()
        .ok_or_else(|| format!("sensor {index} is not a mapping"))?;
    let name = match map.get("name") {
        None | Some(Value::Null) => return Err(format!("sensor {index} has no `name`")),
        Some(v) => v
            .as_str()
            .ok_or_else(|| format!("sensor {index}: `name` is not a string"))?
            .trim()
            .to_string(),
    };
    if !is_valid_name(&name) {
        return Err(format!(
            "sensor name '{name}' is not of the form [A-Za-z0-9._-]+"
        ));
    }
    let context = format!("sensor '{name}'");
    check_fields(
        map,
        &["name", "platforms", "log_sources", "components"],
        &context,
    )?;

    let platforms = match map.get("platforms") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => strings(v, &format!("{context}: `platforms`"))?,
    };
    if platforms.is_empty() {
        return Err(format!("{context} has no `platforms`"));
    }
    let log_sources = named(map, "log_sources", &context)?;
    let components = named(map, "components", &context)?;
    if log_sources.is_empty() && components.is_empty() {
        return Err(format!(
            "{context} has neither `log_sources` nor `components`"
        ));
    }
    Ok(Sensor {
        name,
        platforms,
        log_sources,
        components,
    })
}

fn str_attr(name: &str, value: &str) -> TypedAttribute {
    TypedAttribute {
        name: name.to_string(),
        value: TypedAttrValue::String(value.to_string()),
    }
}

/// Appends one `attr` per distinct value of `values`, in order.
fn push_distinct<'a>(
    attrs: &mut Vec<TypedAttribute>,
    attr: &str,
    values: impl IntoIterator<Item = &'a String>,
) {
    let mut seen = BTreeSet::new();
    for v in values {
        if seen.insert(v.as_str()) {
            attrs.push(str_attr(attr, v));
        }
    }
}

/// A sensor-file concept whose content hash covers its type, title and attributes.
fn concept(
    concept_id: String,
    type_hint: &str,
    title: &str,
    typed_attributes: Vec<TypedAttribute>,
) -> OkfConcept {
    let mut hasher = Sha256::new();
    for part in [SENSOR_MAPPING_VERSION, type_hint, title] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    for attr in &typed_attributes {
        hasher.update(attr.name.as_bytes());
        hasher.update([0]);
        if let TypedAttrValue::String(v) = &attr.value {
            hasher.update(v.as_bytes());
        }
        hasher.update([0]);
    }
    let file_hash = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    OkfConcept {
        concept_id,
        concept_type: type_hint.to_string(),
        type_hint: Some(type_hint.to_string()),
        title: Some(title.to_string()),
        body: String::new(),
        description: None,
        resource_uri: None,
        tags: Vec::new(),
        timestamp: None,
        program: None,
        source_path: String::new(),
        engine: None,
        scopes: Vec::new(),
        timeout_ms: None,
        file_hash,
        extra_metadata: HashMap::new(),
        typed_attributes,
    }
}

/// Converts a sensor file to the OKF bundle `bundle_name`: one
/// `monitored-environment` concept `<bundle_name>/environment` and one `sensor`
/// concept `<bundle_name>/sensor/<name>` per entry, every field a typed attribute.
pub fn to_okf(file: &SensorFile, bundle_name: &str) -> OkfBundle {
    let mut env_attrs = Vec::new();
    push_distinct(&mut env_attrs, "platform", &file.platforms);
    let mut concepts = vec![concept(
        format!("{bundle_name}/environment"),
        "monitored-environment",
        "Monitored environment",
        env_attrs,
    )];

    for s in &file.sensors {
        let mut attrs = Vec::new();
        push_distinct(&mut attrs, "platform", &s.platforms);
        push_distinct(
            &mut attrs,
            "log-source-name",
            s.log_sources.iter().map(|l| &l.name),
        );
        push_distinct(
            &mut attrs,
            "log-source-key",
            s.log_sources.iter().map(|l| &l.key),
        );
        push_distinct(
            &mut attrs,
            "covered-component-name",
            s.components.iter().map(|c| &c.name),
        );
        push_distinct(
            &mut attrs,
            "covered-component-key",
            s.components.iter().map(|c| &c.key),
        );
        concepts.push(concept(
            format!("{bundle_name}/sensor/{}", s.name),
            "sensor",
            &s.name,
            attrs,
        ));
    }

    OkfBundle {
        bundle_path: bundle_name.to_string(),
        bundle_name: bundle_name.to_string(),
        bundle_description: None,
        concepts,
        links: Vec::new(),
        citations: Vec::new(),
        hierarchy: Vec::new(),
    }
}

/// Runs a parameterless read and returns each row's string field `field`.
async fn read_strings(
    graph: &Arc<dyn GraphReader>,
    query: &str,
    field: &str,
) -> Result<Vec<String>, String> {
    graph
        .read(query, &[], &[])
        .await
        .map_err(|e| e.to_string())?
        .iter()
        .map(|row| string_field(row, field))
        .collect()
}

fn string_field(row: &Json, field: &str) -> Result<String, String> {
    row.get(field)
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("a graph row has no string `{field}`: {row}"))
}

/// Refuses a bundle name whose bundle already holds a concept outside
/// `<bundle_name>/environment` and `<bundle_name>/sensor/*`, which a sensor sync
/// would garbage-collect.
pub async fn check_bundle_name(
    graph: &Arc<dyn GraphReader>,
    bundle_name: &str,
) -> Result<(), String> {
    let ids = graph
        .read(
            "given $bn: string; \
             match $b has bundle-name $n; $n == $bn; \
             containment (container: $b, member: $c); $c has concept-id $id; \
             fetch { \"id\": $id };",
            &["bn".into()],
            &[GraphValue::from(bundle_name)],
        )
        .await
        .map_err(|e| e.to_string())?;
    let environment = format!("{bundle_name}/environment");
    let sensor_prefix = format!("{bundle_name}/sensor/");
    for row in &ids {
        let id = string_field(row, "id")?;
        if id != environment && !id.starts_with(&sensor_prefix) {
            return Err(format!(
                "bundle '{bundle_name}' holds records that are not sensors (such as '{id}'); \
                 a sensor sync under that name would delete them, so choose another \
                 --bundle-name"
            ));
        }
    }
    Ok(())
}

/// Refuses a database that holds no strategy-detects relation, including one whose
/// schema does not define `strategy-detects` (TypeDB's unknown-label error).
pub async fn check_attack(graph: &Arc<dyn GraphReader>) -> Result<(), String> {
    let strategies = read_strings(
        graph,
        "match $d isa strategy-detects, links (strategy: $s); $s has concept-id $id; \
         limit 1; fetch { \"id\": $id };",
        "id",
    )
    .await
    .map_err(|e| {
        if e.contains("[INF2]") {
            NO_ATTACK.to_string()
        } else {
            e
        }
    })?;
    if strategies.is_empty() {
        return Err(NO_ATTACK.to_string());
    }
    Ok(())
}

/// Refuses a database without ATT&CK (see [`check_attack`]), or a file naming a
/// platform that matches a live analytic's platform only ignoring case or a
/// component that does not match exactly one live data component, naming every
/// such entry in one message.
pub async fn check_against_graph(
    graph: &Arc<dyn GraphReader>,
    file: &SensorFile,
) -> Result<(), String> {
    check_attack(graph).await?;

    let mut bad = misspelled_platforms(graph, file).await?;
    let component_errors = component_errors(graph, file).await?;
    if !component_errors.is_empty() {
        bad.push(format!(
            "every fallback component must match exactly one live ATT&CK data component: {}",
            component_errors.join("; ")
        ));
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(bad.join("; "))
    }
}

/// The platforms live analytics run on, as ATT&CK spells them.
async fn live_platforms(graph: &Arc<dyn GraphReader>) -> Result<BTreeSet<String>, String> {
    Ok(read_strings(
        graph,
        &format!("match {LIVE_ANALYTIC} $a has platform $p; fetch {{ \"platform\": $p }};"),
        "platform",
    )
    .await?
    .into_iter()
    .collect())
}

/// One error per declared platform that equals a live analytic's platform
/// ignoring case but none exactly, naming ATT&CK's spelling.
async fn misspelled_platforms(
    graph: &Arc<dyn GraphReader>,
    file: &SensorFile,
) -> Result<Vec<String>, String> {
    let live = live_platforms(graph).await?;
    let spelling = |p: &str| -> Option<String> {
        if live.contains(p) {
            return None;
        }
        let folded = p.to_lowercase();
        let matches: Vec<&str> = live
            .iter()
            .filter(|l| l.to_lowercase() == folded)
            .map(String::as_str)
            .collect();
        (!matches.is_empty()).then(|| {
            format!(
                "platform '{p}' should be spelled '{}'",
                matches.join("' or '")
            )
        })
    };
    let mut out: Vec<String> = file.platforms.iter().filter_map(|p| spelling(p)).collect();
    for s in &file.sensors {
        for p in &s.platforms {
            if let Some(e) = spelling(p) {
                out.push(format!("sensor '{}': {e}", s.name));
            }
        }
    }
    Ok(out)
}

/// One error per fallback component that does not match exactly one live data
/// component.
async fn component_errors(
    graph: &Arc<dyn GraphReader>,
    file: &SensorFile,
) -> Result<Vec<String>, String> {
    if file.sensors.iter().all(|s| s.components.is_empty()) {
        return Ok(Vec::new());
    }
    let rows = graph
        .read(
            "match $c isa data-component, has component-key $k, has concept-id $id; \
             not { $c has revoked true; }; \
             not { $c has deprecated true; }; \
             not { $c has retired-at $ra; }; \
             fetch { \"key\": $k, \"id\": $id };",
            &[],
            &[],
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut live: HashMap<String, BTreeSet<String>> = HashMap::new();
    for row in &rows {
        live.entry(string_field(row, "key")?)
            .or_default()
            .insert(string_field(row, "id")?);
    }

    let mut bad = Vec::new();
    for s in &file.sensors {
        for c in &s.components {
            match live.get(&c.key).map_or(0, BTreeSet::len) {
                1 => {}
                0 => bad.push(format!(
                    "sensor '{}': component '{}' matches no live ATT&CK data component",
                    s.name, c.name
                )),
                n => bad.push(format!(
                    "sensor '{}': component '{}' matches {n} live ATT&CK data components",
                    s.name, c.name
                )),
            }
        }
    }
    Ok(bad)
}

/// What a sensor file declares that ATT&CK does not use: log sources no live
/// analytic reads, sensor platforms outside the file's `platforms`, and declared
/// platforms no live analytic runs on.
pub async fn notices(
    graph: &Arc<dyn GraphReader>,
    file: &SensorFile,
) -> Result<Vec<String>, String> {
    let used_keys: HashSet<String> = read_strings(
        graph,
        &format!(
            "match {LIVE_ANALYTIC} \
             $r isa analytic-requirement, links (analytic: $a, component: $c), \
                has log-source-key $k; \
             not {{ $c has revoked true; }}; \
             not {{ $c has deprecated true; }}; \
             not {{ $c has retired-at $cra; }}; \
             fetch {{ \"key\": $k }};"
        ),
        "key",
    )
    .await?
    .into_iter()
    .collect();
    let used_platforms = live_platforms(graph).await?;

    let mut out = Vec::new();
    for s in &file.sensors {
        for l in &s.log_sources {
            if !used_keys.contains(&l.key) {
                out.push(format!(
                    "sensor '{}': log source '{}' is read by no live ATT&CK analytic",
                    s.name, l.name
                ));
            }
        }
        for p in &s.platforms {
            if !file.platforms.contains(p) {
                out.push(format!(
                    "sensor '{}': platform '{p}' is not among the file's `platforms`",
                    s.name
                ));
            }
        }
    }
    for p in &file.platforms {
        if !used_platforms.contains(p) {
            out.push(format!("platform '{p}' has no live ATT&CK analytic"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gecko_extension_api::okf::{OkfConcept, TypedAttrValue};

    const FILE: &str = r#"
platforms: [Windows, Linux, macOS]
sensors:
  - name: windows-edr
    platforms: [Windows]
    log_sources: [" WinEventLog:Security", WinEventLog:Sysmon]
    components: [Process Creation]
  - name: linux-audit
    platforms: [Linux]
    log_sources: [auditd:SYSCALL]
"#;

    fn refused(yaml: &str) -> String {
        parse(yaml).expect_err("the file is refused")
    }

    fn assert_names(err: &str, needles: &[&str]) {
        for n in needles {
            assert!(err.contains(n), "{err:?} should name {n:?}");
        }
    }

    #[test]
    fn missing_platforms() {
        let err = refused("sensors: []\n");
        assert_names(&err, &["platforms"]);
    }

    #[test]
    fn empty_platforms() {
        let err = refused("platforms: []\nsensors: []\n");
        assert_names(&err, &["platforms", "empty"]);
    }

    #[test]
    fn sensor_without_name() {
        let err = refused(
            "platforms: [Windows]\nsensors:\n  - platforms: [Windows]\n    log_sources: [a]\n",
        );
        assert_names(&err, &["name"]);
    }

    #[test]
    fn bad_name() {
        let err = refused(
            "platforms: [Windows]\nsensors:\n  - name: a/b\n    platforms: [Windows]\n    log_sources: [a]\n",
        );
        assert_names(&err, &["a/b", "[A-Za-z0-9._-]+"]);
    }

    #[test]
    fn duplicate_name() {
        let err = refused(
            "platforms: [Windows]\nsensors:\n  - name: edr\n    platforms: [Windows]\n    log_sources: [a]\n  - name: edr\n    platforms: [Windows]\n    log_sources: [b]\n",
        );
        assert_names(&err, &["duplicate", "edr"]);
    }

    #[test]
    fn sensor_without_platforms() {
        let err = refused("platforms: [Windows]\nsensors:\n  - name: edr\n    log_sources: [a]\n");
        assert_names(&err, &["edr", "platforms"]);
        let err = refused(
            "platforms: [Windows]\nsensors:\n  - name: edr\n    platforms: []\n    log_sources: [a]\n",
        );
        assert_names(&err, &["edr", "platforms"]);
    }

    #[test]
    fn sensor_with_neither_log_sources_nor_components() {
        let err =
            refused("platforms: [Windows]\nsensors:\n  - name: edr\n    platforms: [Windows]\n");
        assert_names(&err, &["edr", "log_sources", "components"]);
        let err = refused(
            "platforms: [Windows]\nsensors:\n  - name: edr\n    platforms: [Windows]\n    log_sources: []\n    components: []\n",
        );
        assert_names(&err, &["edr", "log_sources", "components"]);
    }

    #[test]
    fn non_string_entry() {
        let err = refused("platforms: [Windows, 3]\nsensors: []\n");
        assert_names(&err, &["platforms", "not a string"]);
        let err = refused(
            "platforms: [Windows]\nsensors:\n  - name: edr\n    platforms: [Windows]\n    log_sources: [{a: b}]\n",
        );
        assert_names(&err, &["edr", "log_sources", "not a string"]);
        let err = refused(
            "platforms: [Windows]\nsensors:\n  - name: 7\n    platforms: [Windows]\n    log_sources: [a]\n",
        );
        assert_names(&err, &["name", "not a string"]);
    }

    #[test]
    fn unknown_field() {
        let err = refused(
            "platforms: [Windows]\nsensors:\n  - name: edr\n    platforms: [Windows]\n    log_source: [a]\n",
        );
        assert_names(&err, &["edr", "log_source"]);
    }

    #[test]
    fn accepts_an_empty_sensor_list() {
        let file = parse("platforms: [Windows]\nsensors: []\n").expect("parses");
        assert_eq!(file.platforms, vec!["Windows".to_string()]);
        assert!(file.sensors.is_empty());
    }

    #[test]
    fn parse_trims_names_and_keys_them() {
        let file = parse(FILE).expect("parses");
        let edr = &file.sensors[0];
        assert_eq!(
            edr.log_sources[0],
            Named {
                name: "WinEventLog:Security".into(),
                key: "wineventlog:security".into(),
            }
        );
        assert_eq!(
            edr.components,
            vec![Named {
                name: "Process Creation".into(),
                key: "process creation".into(),
            }]
        );
    }

    fn concept<'a>(bundle: &'a OkfBundle, id: &str) -> &'a OkfConcept {
        bundle
            .concepts
            .iter()
            .find(|c| c.concept_id == id)
            .unwrap_or_else(|| panic!("no concept {id}"))
    }

    fn strings(concept: &OkfConcept, attr: &str) -> Vec<String> {
        concept
            .typed_attributes
            .iter()
            .filter(|a| a.name == attr)
            .map(|a| match &a.value {
                TypedAttrValue::String(s) => s.clone(),
                other => panic!("{attr} is not a string: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn to_okf_writes_every_field() {
        let file = parse(FILE).expect("parses");
        let bundle = to_okf(&file, "sensors");
        assert_eq!(bundle.bundle_name, "sensors");
        assert_eq!(bundle.concepts.len(), 3);

        let env = concept(&bundle, "sensors/environment");
        assert_eq!(env.type_hint.as_deref(), Some("monitored-environment"));
        assert_eq!(strings(env, "platform"), ["Windows", "Linux", "macOS"]);

        let edr = concept(&bundle, "sensors/sensor/windows-edr");
        assert_eq!(edr.type_hint.as_deref(), Some("sensor"));
        assert_eq!(edr.title.as_deref(), Some("windows-edr"));
        assert_eq!(strings(edr, "platform"), ["Windows"]);
        assert_eq!(
            strings(edr, "log-source-name"),
            ["WinEventLog:Security", "WinEventLog:Sysmon"]
        );
        assert_eq!(
            strings(edr, "log-source-key"),
            ["wineventlog:security", "wineventlog:sysmon"]
        );
        assert_eq!(strings(edr, "covered-component-name"), ["Process Creation"]);
        assert_eq!(strings(edr, "covered-component-key"), ["process creation"]);

        let audit = concept(&bundle, "sensors/sensor/linux-audit");
        assert!(strings(audit, "covered-component-key").is_empty());

        let lab = to_okf(&file, "lab");
        assert_eq!(lab.bundle_name, "lab");
        concept(&lab, "lab/environment");
        concept(&lab, "lab/sensor/windows-edr");
    }

    #[test]
    fn to_okf_writes_each_key_once() {
        let file = parse(
            "platforms: [Windows]\nsensors:\n  - name: edr\n    platforms: [Windows]\n    log_sources: [WinEventLog:Security, wineventlog:security]\n",
        )
        .expect("parses");
        let bundle = to_okf(&file, "sensors");
        let edr = concept(&bundle, "sensors/sensor/edr");
        assert_eq!(strings(edr, "log-source-key"), ["wineventlog:security"]);
    }

    #[test]
    fn to_okf_hash_follows_content() {
        let file = parse(FILE).expect("parses");
        let a = to_okf(&file, "sensors");
        let b = to_okf(&file, "sensors");
        let hashes = |x: &OkfBundle| {
            x.concepts
                .iter()
                .map(|c| c.file_hash.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(hashes(&a), hashes(&b));

        let mut changed = file.clone();
        changed.sensors[0].components.clear();
        let c = to_okf(&changed, "sensors");
        assert_ne!(
            concept(&a, "sensors/sensor/windows-edr").file_hash,
            concept(&c, "sensors/sensor/windows-edr").file_hash
        );
        assert_eq!(
            concept(&a, "sensors/sensor/linux-audit").file_hash,
            concept(&c, "sensors/sensor/linux-audit").file_hash
        );
    }

    #[test]
    fn valid_names() {
        assert!(is_valid_name("windows-edr_1.2"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("a/b"));
        assert!(!is_valid_name("a b"));
    }
}
