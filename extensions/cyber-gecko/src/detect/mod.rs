//! Detection-engineering host imports, backed by the epistemic substrate and the
//! graph (not fabricated).
//!
//! These functions need graph reads and the belief-write path, which the plain
//! synchronous [`GeckoExtension::call_import`] seam cannot supply. They are
//! dispatched instead through [`cyber_extension_callback`], a host bridge wired at
//! run time (parallel to mem's `mem.*` bridge) that captures a read-only
//! [`GraphReader`], a [`BeliefEdgeWriter`] bound to cyber-gecko's declared belief
//! edges, and the host-minted [`RunContext`]. Every write is a belief committed
//! atomically with its edges; the bridge holds no raw graph write path.
//!
//! - `claim` asserts a detection belief with a `technique-detection` edge to its
//!   logic and technique.
//! - `disposition` records an analyst verdict as a belief with an
//!   `alert-disposition` edge carrying the verdict, so `precision` can count it.
//! - `coverage_gaps` runs the persisted `undetected-techniques()` TypeDB function.
//! - `blinded` checks each asserted claim against ATT&CK's detection model and the
//!   synced sensor files. A claim's technique has detection strategies, each carried
//!   out by one analytic per platform, and an analytic reads log sources for the data
//!   components its requirements name. On each platform a sensor file declares,
//!   `claim-platforms()` marks the claim *covered* when the technique's live
//!   analytic there can run (it has a requirement, and a live sensor on that
//!   platform collects each requirement's log source or covers its component as a
//!   fallback), *blind* when it cannot, and *unassessed* when ATT&CK gives no live
//!   analytic there or the technique itself is retired, revoked or deprecated. The
//!   report lists each claim with a blind or unassessed platform, what each blind
//!   analytic lacks (`analytic-gaps`, grouped by log source), and the sensors'
//!   fallback components that match no live data component. A graph with no sensor
//!   file or no detection strategies is an error, never an empty report.
//! - `recommend_sources` turns the blind claims into an ordered list of ATT&CK log
//!   sources to add. Each step adds one analytic's remaining missing log sources on
//!   one platform, chosen greedily for the most (claim, platform) pairs unblinded per
//!   new source. Claims no log source can fix are listed with a reason:
//!   `no-requirements` when every live analytic there names no live log source, and
//!   `no-analytic` when the platform is unassessed. It errors on the same graphs as
//!   `blinded`.
//! - `precision` counts dispositions in a time window and computes true-positive
//!   precision host-side.
//!
//! [`GeckoExtension::call_import`]: gecko_extension_api::GeckoExtension::call_import

mod recommend;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use gecko_extension_api::{
    BeliefDraft, BeliefEdge, BeliefEdgeWriter, DerivationMethod, ExtensionCallback, GraphReader,
    GraphValue, MemId, RunContext, Visibility,
};
use serde_json::{Value, json};

const CYBER_EXT: &str = "cyber-gecko";
const DETECT_FNS: [&str; 6] = [
    "claim",
    "disposition",
    "coverage_gaps",
    "blinded",
    "recommend_sources",
    "precision",
];

/// The analyst verdicts accepted by `disposition`, matching the schema's
/// `detection-verdict @values(...)`.
const VERDICTS: [&str; 3] = ["true-positive", "false-positive", "benign-true"];

/// Wraps `base` so cyber-gecko's detect host imports reach the read-only `graph`
/// and the belief-edge writer `edges`. Every other extension call falls through to
/// `base`.
///
/// This is the sync→async seam: the sandbox host-call bridge is synchronous but
/// the graph and writer are async, so each detect call is driven to completion on
/// the ambient multi-thread runtime via `block_in_place` + `block_on` (the same
/// mechanism the mem bridge uses; requires the multi-thread runtime the binary
/// and sandbox already run on).
pub fn cyber_extension_callback(
    graph: Arc<dyn GraphReader>,
    edges: Arc<dyn BeliefEdgeWriter>,
    ctx: RunContext,
    base: ExtensionCallback,
) -> ExtensionCallback {
    let handle = tokio::runtime::Handle::current();
    Arc::new(move |ext_name: &str, func_name: &str, args: Value| {
        if ext_name == CYBER_EXT && DETECT_FNS.contains(&func_name) {
            let graph = graph.clone();
            let edges = edges.clone();
            let ctx = ctx.clone();
            let func = func_name.to_string();
            return tokio::task::block_in_place(|| {
                handle.block_on(dispatch(&graph, &edges, &ctx, &func, args))
            });
        }
        base(ext_name, func_name, args)
    })
}

async fn dispatch(
    graph: &Arc<dyn GraphReader>,
    edges: &Arc<dyn BeliefEdgeWriter>,
    ctx: &RunContext,
    func: &str,
    args: Value,
) -> Result<Value, String> {
    match func {
        "claim" => claim(graph, edges, ctx, &args).await,
        "disposition" => disposition(graph, edges, ctx, &args).await,
        "coverage_gaps" => coverage_gaps(graph).await,
        "blinded" => blinded(graph).await,
        "recommend_sources" => recommend_sources(graph).await,
        "precision" => precision(graph, ctx, &args).await,
        other => Err(format!("unknown detect function {other:?}")),
    }
}

/// Asserts "logic detects technique" as a belief committed with its
/// `technique-detection` edge. Errors (never fabricates) on a missing/invalid
/// argument or when the referenced logic or technique is absent from the graph.
async fn claim(
    graph: &Arc<dyn GraphReader>,
    edges: &Arc<dyn BeliefEdgeWriter>,
    ctx: &RunContext,
    args: &Value,
) -> Result<Value, String> {
    let logic_id = req_str(args, "logic_id")?;
    let technique_id = req_str(args, "technique_id")?;
    let method_str = req_str(args, "method")?;
    let method = DerivationMethod::from_str(&method_str)
        .ok_or_else(|| format!("unknown detection method {method_str:?}"))?;
    let confidence = req_f64(args, "confidence")?;
    if !(0.0..=1.0).contains(&confidence) {
        return Err(format!("confidence {confidence} is outside [0.0, 1.0]"));
    }
    let evidence = mem_ids(args, "evidence")?;

    // Refuse to assert a claim about a logic or technique that does not exist.
    ensure_logic_and_technique(graph, &logic_id, &technique_id).await?;

    let belief = BeliefDraft {
        text: format!(
            "detection '{logic_id}' detects technique '{technique_id}' (confidence {confidence})"
        ),
        owner: ctx.actor.clone(),
        visibility: Visibility::Private,
        confidence: Some(confidence),
        entrenchment: None,
    };
    // The belief plays detection-claim; logic and technique are matched by key.
    let edge = BeliefEdge {
        relation: "technique-detection".into(),
        players: vec![
            (
                "detecting-logic".into(),
                GraphValue::from(logic_id.as_str()),
            ),
            (
                "detected-technique".into(),
                GraphValue::from(technique_id.as_str()),
            ),
        ],
        attrs: vec![],
    };
    let belief_id = edges
        .assert_belief_with_edges(ctx, belief, &evidence, method, &[edge])
        .await
        .map_err(|e| e.to_string())?;

    Ok(json!({
        "belief_id": belief_id.0,
        "logic_id": logic_id,
        "technique_id": technique_id,
    }))
}

/// Records an analyst verdict on a detection as a belief committed with its
/// `alert-disposition` edge carrying the verdict.
async fn disposition(
    graph: &Arc<dyn GraphReader>,
    edges: &Arc<dyn BeliefEdgeWriter>,
    ctx: &RunContext,
    args: &Value,
) -> Result<Value, String> {
    let logic_id = req_str(args, "logic_id")?;
    let verdict = req_str(args, "verdict")?;
    if !VERDICTS.contains(&verdict.as_str()) {
        return Err(format!(
            "unknown verdict {verdict:?}; expected one of {VERDICTS:?}"
        ));
    }
    // Alert/sighting references, if supplied, back the disposition belief.
    let evidence = mem_ids(args, "evidence")?;
    ensure_concept(graph, &logic_id).await?;

    let belief = BeliefDraft {
        text: format!("disposition of detection '{logic_id}': {verdict}"),
        owner: ctx.actor.clone(),
        visibility: Visibility::Private,
        confidence: None,
        entrenchment: None,
    };
    // The belief plays disposition-belief; the logic is matched by key.
    let edge = BeliefEdge {
        relation: "alert-disposition".into(),
        players: vec![(
            "dispositioned-logic".into(),
            GraphValue::from(logic_id.as_str()),
        )],
        attrs: vec![(
            "detection-verdict".into(),
            GraphValue::from(verdict.as_str()),
        )],
    };
    let belief_id = edges
        .assert_belief_with_edges(
            ctx,
            belief,
            &evidence,
            DerivationMethod::HumanAssertion,
            &[edge],
        )
        .await
        .map_err(|e| e.to_string())?;

    Ok(json!({ "belief_id": belief_id.0, "logic_id": logic_id, "verdict": verdict }))
}

/// Techniques with no asserted detection claim (the `undetected-techniques()`
/// TypeDB function).
async fn coverage_gaps(graph: &Arc<dyn GraphReader>) -> Result<Value, String> {
    let rows = graph
        .read(
            "match let $t in undetected-techniques(); \
             fetch { \"concept_id\": $t.concept-id, \"attack_id\": $t.attack-id };",
            &[],
            &[],
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({ "undetected_techniques": rows }))
}

/// The pattern that skips `$var` when it has `retired-at`, `revoked true` or
/// `deprecated true`. `var` is a variable name from this module, never a value.
fn live(var: &str) -> String {
    format!(
        "not {{ ${var} has revoked true; }}; \
         not {{ ${var} has deprecated true; }}; \
         not {{ ${var} has retired-at ${var}_retired; }};"
    )
}

fn row_str(row: &Value, field: &str) -> Result<String, String> {
    row.get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("a graph row has no string `{field}`: {row}"))
}

/// Runs `query` with the string parameters `names` bound to `values`.
async fn read(
    graph: &Arc<dyn GraphReader>,
    query: &str,
    names: &[&str],
    values: &[&str],
) -> Result<Vec<Value>, String> {
    let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    let values: Vec<GraphValue> = values.iter().map(|v| GraphValue::from(*v)).collect();
    graph
        .read(query, &names, &values)
        .await
        .map_err(|e| e.to_string())
}

/// Runs `query` once over every row of `rows`, binding the string parameters
/// `names` to each row's values.
async fn read_rows(
    graph: &Arc<dyn GraphReader>,
    query: &str,
    names: &[&str],
    rows: &[Vec<&str>],
) -> Result<Vec<Value>, String> {
    let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    let rows: Vec<Vec<GraphValue>> = rows
        .iter()
        .map(|row| row.iter().map(|v| GraphValue::from(*v)).collect())
        .collect();
    graph
        .read_rows(query, &names, &rows)
        .await
        .map_err(|e| e.to_string())
}

/// One claim the report covers: its detecting logic and its technique.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ClaimKey {
    concept_id: String,
    technique_concept_id: String,
}

/// What `claim-platforms()` says of one claim: the technique's attack-id, if it
/// still has one, and every status per declared platform.
#[derive(Debug, Default)]
struct ClaimStatuses {
    technique: Option<String>,
    platforms: BTreeMap<String, BTreeSet<String>>,
}

/// A (technique concept id, platform) pair `claim-platforms()` called blind.
type Pair = (String, String);

/// Every (technique, platform) pair some claim is blind on and not covered on.
fn blind_pairs(claims: &BTreeMap<ClaimKey, ClaimStatuses>) -> BTreeSet<Pair> {
    claims
        .iter()
        .flat_map(|(key, statuses)| {
            statuses
                .platforms
                .iter()
                .filter(|(_, status)| status.contains("blind") && !status.contains("covered"))
                .map(|(platform, _)| (key.technique_concept_id.clone(), platform.clone()))
        })
        .collect()
}

/// The readings gathered for the blind `platform`, empty when none were read,
/// which the builders report as a changed graph. A pair that was never gathered
/// is an internal error: every pair a caller asks for is seeded.
fn readings_for<'a>(
    readings: &'a BTreeMap<Pair, Vec<Reading>>,
    technique_concept_id: &str,
    platform: &str,
) -> Result<&'a [Reading], String> {
    readings
        .get(&(technique_concept_id.to_string(), platform.to_string()))
        .map(Vec::as_slice)
        .ok_or_else(|| {
            format!(
                "internal: readings were not gathered for technique '{technique_concept_id}', platform '{platform}'"
            )
        })
}

/// For each asserted detection claim with a declared platform it is blind on or
/// cannot assess: the blind platforms with what each lacks, and the unassessed
/// platforms. Errors on a graph it cannot judge: one with no sensor file, or with
/// no current ATT&CK detection data.
async fn blinded(graph: &Arc<dyn GraphReader>) -> Result<Value, String> {
    require_judgeable(graph).await?;
    let claims = claim_statuses(graph).await?;
    let readings = analytic_readings(graph, &blind_pairs(&claims)).await?;

    let mut detections = Vec::new();
    for (key, statuses) in &claims {
        let mut blind_platforms = Vec::new();
        let mut unassessed = Vec::new();
        for (platform, status) in &statuses.platforms {
            // A platform with an analytic that can run is not blind, whatever
            // another analytic there lacks.
            if status.contains("covered") {
                continue;
            }
            if status.contains("blind") {
                blind_platforms.extend(blind_analytics(
                    platform,
                    readings_for(&readings, &key.technique_concept_id, platform)?,
                )?);
            } else {
                unassessed.push(platform.clone());
            }
        }
        if blind_platforms.is_empty() && unassessed.is_empty() {
            continue;
        }
        detections.push(json!({
            "concept_id": key.concept_id,
            "technique": statuses.technique,
            "technique_concept_id": key.technique_concept_id,
            "blind_platforms": blind_platforms,
            "unassessed_platforms": unassessed,
        }));
    }

    Ok(json!({
        "blinded_detections": detections,
        "stale_sensor_components": stale_sensor_components(graph).await?,
    }))
}

/// Errs on a graph the blindness checks cannot judge: one with no live sensor-file
/// environment, or with no current ATT&CK detection strategies.
async fn require_judgeable(graph: &Arc<dyn GraphReader>) -> Result<(), String> {
    let env = read(
        graph,
        &format!(
            "match $e isa monitored-environment; {} limit 1; fetch {{ \"id\": $e.concept-id }};",
            live("e")
        ),
        &[],
        &[],
    )
    .await?;
    if env.is_empty() {
        return Err(
            "no sensor file synced: sync one with `gecko sync-sensors` before checking blindness"
                .into(),
        );
    }
    let strategies = read(
        graph,
        "match strategy-detects (strategy: $s, technique: $t); limit 1; \
         fetch { \"id\": $s.concept-id };",
        &[],
        &[],
    )
    .await?;
    if strategies.is_empty() {
        return Err(
            "no current ATT&CK detection data: the graph holds no detection strategies \
             (ATT&CK v18 or later); sync or re-sync the ATT&CK bundle into this database, \
             which an upgraded database needs before its first check"
                .into(),
        );
    }
    Ok(())
}

/// Every asserted claim's `claim-platforms()` statuses, grouped by claim and by
/// declared platform.
async fn claim_statuses(
    graph: &Arc<dyn GraphReader>,
) -> Result<BTreeMap<ClaimKey, ClaimStatuses>, String> {
    let rows = read(
        graph,
        "match let $d, $t, $p, $s in claim-platforms(); \
         fetch { \"concept_id\": $d.concept-id, \"technique_concept_id\": $t.concept-id, \
                 \"technique\": $t.attack-id, \"platform\": $p, \"status\": $s };",
        &[],
        &[],
    )
    .await?;
    let mut claims: BTreeMap<ClaimKey, ClaimStatuses> = BTreeMap::new();
    for row in &rows {
        let key = ClaimKey {
            concept_id: row_str(row, "concept_id")?,
            technique_concept_id: row_str(row, "technique_concept_id")?,
        };
        let entry = claims.entry(key).or_default();
        if let Some(t) = row.get("technique").and_then(Value::as_str) {
            entry.technique = Some(t.to_string());
        }
        entry
            .platforms
            .entry(row_str(row, "platform")?)
            .or_default()
            .insert(row_str(row, "status")?);
    }
    Ok(claims)
}

/// One `analytic-gaps` row of an analytic on a platform: an uncovered requirement.
#[derive(Debug, Clone)]
struct GapRow {
    /// The log source's spelling.
    name: String,
    /// The log source's matching key.
    key: String,
    /// The log source's channels.
    channels: Vec<String>,
    /// The trimmed title of the data component it serves, if it has one.
    component: Option<String>,
}

/// One live analytic of a technique on a platform `claim-platforms()` called blind.
#[derive(Debug, Clone)]
struct Reading {
    /// The analytic's attack-id, or null when it has none.
    analytic: Value,
    /// The analytic's concept id.
    concept_id: String,
    /// Whether the analytic has a requirement on a live data component.
    has_requirements: bool,
    /// Its `analytic-gaps` on the platform, read only when it has requirements.
    gaps: Vec<GapRow>,
}

/// The live analytics of every pair in `pairs`, each with its requirements and its
/// gaps on the pair's platform, in three reads. Every requested pair is in the map,
/// with no readings when its analytics were not found.
async fn analytic_readings(
    graph: &Arc<dyn GraphReader>,
    pairs: &BTreeSet<Pair>,
) -> Result<BTreeMap<Pair, Vec<Reading>>, String> {
    let pair_rows: Vec<Vec<&str>> = pairs
        .iter()
        .map(|(technique, platform)| vec![technique.as_str(), platform.as_str()])
        .collect();
    let analytics = read_rows(
        graph,
        &format!(
            "given $tid: string, $plat: string; \
             match $t isa attack-pattern, has concept-id $tc; $tc == $tid; \
                   strategy-detects (strategy: $st, technique: $t); {} \
                   strategy-analytic (strategy: $st, analytic: $a); {} \
                   $a has platform $ap; $ap == $plat; $a has concept-id $ac; \
             fetch {{ \"technique\": $tc, \"platform\": $ap, \
                     \"analytic_concept_id\": $ac, \"analytic\": $a.attack-id }};",
            live("st"),
            live("a")
        ),
        &["tid", "plat"],
        &pair_rows,
    )
    .await?;
    let mut readings = readings_from_analytics(pairs, &analytics)?;

    let analytic_ids: BTreeSet<String> = readings
        .values()
        .flatten()
        .map(|r| r.concept_id.clone())
        .collect();
    let id_rows: Vec<Vec<&str>> = analytic_ids.iter().map(|id| vec![id.as_str()]).collect();
    let requirements = read_rows(
        graph,
        &format!(
            "given $aid: string; \
             match $a isa analytic, has concept-id $ac; $ac == $aid; \
                   $r isa analytic-requirement, links (analytic: $a, component: $c); {} \
             fetch {{ \"analytic_concept_id\": $ac }};",
            live("c")
        ),
        &["aid"],
        &id_rows,
    )
    .await?;
    mark_requirements(&mut readings, &requirements)?;

    let gapped: BTreeSet<(String, String)> = readings
        .iter()
        .flat_map(|((_, platform), rs)| {
            rs.iter()
                .filter(|r| r.has_requirements)
                .map(|r| (r.concept_id.clone(), platform.clone()))
        })
        .collect();
    let gapped_rows: Vec<Vec<&str>> = gapped
        .iter()
        .map(|(id, platform)| vec![id.as_str(), platform.as_str()])
        .collect();
    let gaps = read_rows(
        graph,
        "given $aid: string, $plat: string; \
         match $a isa analytic, has concept-id $ac; $ac == $aid; \
               $a has platform $ap; $ap == $plat; \
               let $r in analytic-gaps($a, $plat); \
               $r links (component: $c); \
         fetch { \"analytic_concept_id\": $ac, \"platform\": $ap, \
                 \"name\": $r.log-source-name, \"key\": $r.log-source-key, \
                 \"channels\": [ $r.log-source-channel ], \"component\": $c.title };",
        &["aid", "plat"],
        &gapped_rows,
    )
    .await?;
    attach_gaps(&mut readings, &gaps)?;
    Ok(readings)
}

/// The analytic `rows` of the batched read keyed by their (technique, platform),
/// seeded with every pair in `pairs`. Each pair's analytics are sorted by attack-id,
/// or by concept id for one with no attack-id; of two with one attack-id, the
/// smaller concept id is kept.
fn readings_from_analytics(
    pairs: &BTreeSet<Pair>,
    rows: &[Value],
) -> Result<BTreeMap<Pair, Vec<Reading>>, String> {
    let mut by_pair: BTreeMap<Pair, BTreeMap<String, (Value, String)>> = pairs
        .iter()
        .map(|pair| (pair.clone(), BTreeMap::new()))
        .collect();
    for row in rows {
        let pair = (row_str(row, "technique")?, row_str(row, "platform")?);
        let analytics = by_pair
            .get_mut(&pair)
            .ok_or_else(|| format!("an analytic row matched no requested pair: {row}"))?;
        let concept_id = row_str(row, "analytic_concept_id")?;
        let attack_id = row.get("analytic").cloned().unwrap_or(Value::Null);
        let sort_key = attack_id.as_str().unwrap_or(&concept_id).to_string();
        match analytics.get(&sort_key) {
            Some((_, kept)) if *kept <= concept_id => {}
            _ => {
                analytics.insert(sort_key, (attack_id, concept_id));
            }
        }
    }
    Ok(by_pair
        .into_iter()
        .map(|(pair, analytics)| {
            let readings = analytics
                .into_values()
                .map(|(analytic, concept_id)| Reading {
                    analytic,
                    concept_id,
                    has_requirements: false,
                    gaps: Vec::new(),
                })
                .collect();
            (pair, readings)
        })
        .collect())
}

/// Marks each reading whose analytic appears in the requirement `rows`.
fn mark_requirements(
    readings: &mut BTreeMap<Pair, Vec<Reading>>,
    rows: &[Value],
) -> Result<(), String> {
    let with_requirements: BTreeSet<String> = rows
        .iter()
        .map(|row| row_str(row, "analytic_concept_id"))
        .collect::<Result<_, _>>()?;
    for reading in readings.values_mut().flatten() {
        reading.has_requirements = with_requirements.contains(&reading.concept_id);
    }
    Ok(())
}

/// Gives each reading with requirements the gap `rows` of its analytic on its
/// pair's platform.
fn attach_gaps(readings: &mut BTreeMap<Pair, Vec<Reading>>, rows: &[Value]) -> Result<(), String> {
    let mut by_analytic: BTreeMap<(String, String), Vec<GapRow>> = BTreeMap::new();
    for row in rows {
        let channels = row
            .get("channels")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("a gap row has no channel list: {row}"))?
            .iter()
            .map(|c| {
                c.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("a gap row has a non-string channel: {row}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        by_analytic
            .entry((
                row_str(row, "analytic_concept_id")?,
                row_str(row, "platform")?,
            ))
            .or_default()
            .push(GapRow {
                name: row_str(row, "name")?,
                key: row_str(row, "key")?,
                channels,
                component: row
                    .get("component")
                    .and_then(Value::as_str)
                    .map(|c| c.trim().to_string()),
            });
    }
    for ((_, platform), rs) in readings.iter_mut() {
        for reading in rs.iter_mut().filter(|r| r.has_requirements) {
            if let Some(gaps) = by_analytic.get(&(reading.concept_id.clone(), platform.clone())) {
                reading.gaps = gaps.clone();
            }
        }
    }
    Ok(())
}

/// The `blind_platforms` entries for `platform` from its analytics' `readings`:
/// one per live analytic there that cannot run, sorted by analytic.
fn blind_analytics(platform: &str, readings: &[Reading]) -> Result<Vec<Value>, String> {
    let analytics = readings
        .iter()
        .map(|r| {
            Ok(AnalyticReading {
                analytic: r.analytic.clone(),
                has_requirements: r.has_requirements,
                missing: missing_log_sources(r)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    blind_platform_entries(platform, analytics)
}

/// The error for a platform `claim-platforms()` called blind whose analytics no
/// longer read as blind: a sync landed between the reads.
fn graph_changed(platform: &str) -> String {
    format!(
        "the graph changed during the check: platform '{platform}' was blind but its \
         analytics no longer read as blind; retry"
    )
}

/// What the reads after `claim-platforms()` found for one live analytic on a
/// platform it called blind.
#[derive(Debug, Clone)]
struct AnalyticReading {
    /// The analytic's attack-id, or null when it has none.
    analytic: Value,
    /// Whether the analytic has a requirement on a live data component.
    has_requirements: bool,
    /// Its uncovered requirements, grouped by log source.
    missing: Vec<Value>,
}

/// The `blind_platforms` entries for `platform`, which `claim-platforms()` called
/// blind and not covered, so every live analytic there cannot run. Each read is its
/// own snapshot: finding no analytic, or one that can run, means a sync landed
/// between them, and is an error rather than a dropped platform.
fn blind_platform_entries(
    platform: &str,
    readings: Vec<AnalyticReading>,
) -> Result<Vec<Value>, String> {
    let changed = || graph_changed(platform);
    if readings.is_empty() {
        return Err(changed());
    }
    let mut out = Vec::new();
    for r in readings {
        if r.has_requirements && r.missing.is_empty() {
            return Err(changed());
        }
        out.push(json!({
            "platform": platform,
            "analytic": r.analytic,
            "no_log_sources": !r.has_requirements,
            "missing": r.missing,
        }));
    }
    Ok(out)
}

/// The greedy list of log sources to add, per declared platform, that lets the most
/// asserted claims' analytics run, and the blind or unassessed claims no log source
/// can fix. Errors on a graph `blinded` cannot judge.
async fn recommend_sources(graph: &Arc<dyn GraphReader>) -> Result<Value, String> {
    require_judgeable(graph).await?;
    let claims = claim_statuses(graph).await?;
    let readings = analytic_readings(graph, &blind_pairs(&claims)).await?;

    // (platform, log-source key) → (smallest spelling, every channel).
    let mut sources: BTreeMap<(String, String), (String, BTreeSet<String>)> = BTreeMap::new();
    let mut claim_refs = Vec::new();
    let mut pairs = Vec::new();
    let mut unfixable = Vec::new();
    for (key, statuses) in &claims {
        let claim_ref = json!({
            "concept_id": key.concept_id,
            "technique": statuses.technique,
            "technique_concept_id": key.technique_concept_id,
        });
        let unfixable_entry = |platform: &str, reason: &str| {
            let mut entry = claim_ref.clone();
            entry["platform"] = json!(platform);
            entry["reason"] = json!(reason);
            entry
        };
        let claim = claim_refs.len();
        for (platform, status) in &statuses.platforms {
            if status.contains("covered") {
                continue;
            }
            if !status.contains("blind") {
                unfixable.push(unfixable_entry(platform, "no-analytic"));
                continue;
            }
            let analytics = blind_gap_sets(
                platform,
                readings_for(&readings, &key.technique_concept_id, platform)?,
                &mut sources,
            )?;
            if analytics.is_empty() {
                unfixable.push(unfixable_entry(platform, "no-requirements"));
            } else {
                pairs.push(recommend::BlindPair {
                    claim,
                    platform: platform.clone(),
                    analytics,
                });
            }
        }
        claim_refs.push(claim_ref);
    }

    let mut steps = Vec::new();
    for step in recommend::plan(&pairs)? {
        let mut add = Vec::new();
        for k in &step.add {
            let (name, channels) = sources
                .get(&(step.platform.clone(), k.clone()))
                .ok_or_else(|| format!("log source '{k}' on '{}' was never read", step.platform))?;
            add.push(json!({ "log_source": name, "key": k, "channels": channels }));
        }
        let unblinds: Vec<&Value> = step
            .unblinds
            .iter()
            .map(|&i| &claim_refs[pairs[i].claim])
            .collect();
        steps.push(json!({
            "platform": step.platform,
            "add": add,
            "unblinds": unblinds,
            "cumulative_unblinded": step.cumulative_unblinded,
        }));
    }
    Ok(json!({ "steps": steps, "unfixable": unfixable }))
}

/// The missing log-source keys on `platform` of each analytic in `readings` that has
/// a requirement, recording each key's spelling and channels in `sources`. Errors
/// when `claim-platforms()` called the platform blind but the analytics no longer
/// read as blind.
fn blind_gap_sets(
    platform: &str,
    readings: &[Reading],
    sources: &mut BTreeMap<(String, String), (String, BTreeSet<String>)>,
) -> Result<Vec<BTreeSet<String>>, String> {
    let mut gap_readings = Vec::new();
    for reading in readings {
        let mut keys = BTreeSet::new();
        for gap in &reading.gaps {
            let (display, all_channels) = sources
                .entry((platform.to_string(), gap.key.clone()))
                .or_insert_with(|| (gap.name.clone(), BTreeSet::new()));
            if gap.name < *display {
                *display = gap.name.clone();
            }
            all_channels.extend(gap.channels.iter().cloned());
            keys.insert(gap.key.clone());
        }
        gap_readings.push(GapReading {
            has_requirements: reading.has_requirements,
            keys,
        });
    }
    gap_sets(platform, gap_readings)
}

/// What the reads after `claim-platforms()` found for one live analytic on a
/// platform it called blind, for `recommend_sources`.
#[derive(Debug, Clone)]
struct GapReading {
    /// Whether the analytic has a requirement on a live data component.
    has_requirements: bool,
    /// Its missing log-source keys on the platform.
    keys: BTreeSet<String>,
}

/// The planner's gap sets for `platform` from its analytics' `readings`: one per
/// analytic that has a requirement, and none for one that names no live log source.
/// Finding no analytic, or one with a requirement and no gap, means a sync landed
/// between the reads, and is an error.
fn gap_sets(platform: &str, readings: Vec<GapReading>) -> Result<Vec<BTreeSet<String>>, String> {
    if readings.is_empty() {
        return Err(graph_changed(platform));
    }
    let mut sets = Vec::new();
    for r in readings {
        if !r.has_requirements {
            continue;
        }
        if r.keys.is_empty() {
            return Err(graph_changed(platform));
        }
        sets.push(r.keys);
    }
    Ok(sets)
}

/// The gaps of the analytic `reading`, grouped by log-source key: each key's
/// smallest trimmed spelling and the sorted names of the data components it would
/// serve, sorted by spelling.
fn missing_log_sources(reading: &Reading) -> Result<Vec<Value>, String> {
    let mut groups: BTreeMap<&str, (&str, BTreeSet<&str>)> = BTreeMap::new();
    for gap in &reading.gaps {
        let component = gap.component.as_deref().ok_or_else(|| {
            format!(
                "a gap's data component has no name: analytic '{}', log source '{}'",
                reading.concept_id, gap.name
            )
        })?;
        let (display, components) = groups
            .entry(&gap.key)
            .or_insert_with(|| (&gap.name, BTreeSet::new()));
        if gap.name.as_str() < *display {
            *display = &gap.name;
        }
        components.insert(component);
    }

    let mut missing: Vec<(&str, &str, BTreeSet<&str>)> = groups
        .into_iter()
        .map(|(key, (display, components))| (display, key, components))
        .collect();
    missing.sort();
    Ok(missing
        .into_iter()
        .map(|(display, _, components)| {
            json!({ "log_source": display, "data_components": components })
        })
        .collect())
}

/// Each live sensor's fallback component that matches no live data component,
/// sorted by sensor and component.
async fn stale_sensor_components(graph: &Arc<dyn GraphReader>) -> Result<Vec<Value>, String> {
    let declared = read(
        graph,
        &format!(
            "match $s isa sensor, has covered-component-name $n; {} \
             fetch {{ \"sensor\": $s.title, \"component\": $n }};",
            live("s")
        ),
        &[],
        &[],
    )
    .await?;
    if declared.is_empty() {
        return Ok(Vec::new());
    }
    let keys: BTreeSet<String> = read(
        graph,
        &format!(
            "match $c isa data-component, has component-key $k; {} fetch {{ \"key\": $k }};",
            live("c")
        ),
        &[],
        &[],
    )
    .await?
    .iter()
    .map(|row| row_str(row, "key"))
    .collect::<Result<_, _>>()?;

    let mut stale = BTreeSet::new();
    for row in &declared {
        let component = row_str(row, "component")?;
        if !keys.contains(&component.to_lowercase()) {
            stale.insert((row_str(row, "sensor")?, component));
        }
    }
    Ok(stale
        .into_iter()
        .map(|(sensor, component)| json!({ "sensor": sensor, "component": component }))
        .collect())
}

/// True-positive precision of a detection over a trailing time window, computed
/// host-side from the dispositions recorded against it.
async fn precision(
    graph: &Arc<dyn GraphReader>,
    ctx: &RunContext,
    args: &Value,
) -> Result<Value, String> {
    let logic_id = req_str(args, "logic_id")?;
    let window = args
        .get("window")
        .and_then(Value::as_u64)
        .ok_or_else(|| "precision requires an integer 'window' (seconds)".to_string())?;
    if window == 0 {
        return Err("precision 'window' must be greater than zero seconds".to_string());
    }
    let cutoff = ctx.occurred_at - chrono::Duration::seconds(window as i64);

    let rows = graph
        .read(
            "given $l: string, $cut: datetime; \
             match $lc isa concept, has concept-id $lcid; $lcid == $l; \
                   $d isa alert-disposition, \
                     links (dispositioned-logic: $lc, disposition-belief: $bel), \
                     has detection-verdict $v; \
                   $bel has belief-state \"asserted\", has valid-from $t; $t >= $cut; \
             fetch { \"verdict\": $v };",
            &["l".into(), "cut".into()],
            &[
                GraphValue::from(logic_id.as_str()),
                GraphValue::Datetime(cutoff),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;

    let mut true_positives = 0u64;
    let mut false_positives = 0u64;
    for row in &rows {
        match row.get("verdict").and_then(Value::as_str) {
            Some("true-positive") => true_positives += 1,
            Some("false-positive") => false_positives += 1,
            // benign-true is neither a true nor a false positive; excluded.
            _ => {}
        }
    }

    Ok(json!({
        "logic_id": logic_id,
        "window_seconds": window,
        "true_positives": true_positives,
        "false_positives": false_positives,
        "precision": precision_ratio(true_positives, false_positives),
    }))
}

/// Precision = TP / (TP + FP); `None` when there are no positives to score.
fn precision_ratio(true_positives: u64, false_positives: u64) -> Option<f64> {
    let denom = true_positives + false_positives;
    (denom > 0).then(|| true_positives as f64 / denom as f64)
}

// ── argument + graph helpers ────────────────────────────────────────────────

fn req_str(args: &Value, field: &str) -> Result<String, String> {
    args.get(field)
        .and_then(Value::as_str)
        .map(String::from)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing required string argument '{field}'"))
}

fn req_f64(args: &Value, field: &str) -> Result<f64, String> {
    args.get(field)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("missing required numeric argument '{field}'"))
}

/// Parses an optional JSON string array of belief/episode ids into `MemId`s: absent or
/// `null` ⇒ empty, and a non-array or a non-string entry is an error naming the field.
fn mem_ids(args: &Value, field: &str) -> Result<Vec<MemId>, String> {
    let Some(value) = args.get(field).filter(|v| !v.is_null()) else {
        return Ok(Vec::new());
    };
    let arr = value
        .as_array()
        .ok_or_else(|| format!("argument '{field}' must be an array of id strings"))?;
    arr.iter()
        .enumerate()
        .map(|(i, v)| {
            v.as_str().map(MemId::new).ok_or_else(|| {
                format!("argument '{field}' must be an array of id strings; [{i}] is {v}")
            })
        })
        .collect()
}

async fn ensure_concept(graph: &Arc<dyn GraphReader>, concept_id: &str) -> Result<(), String> {
    let rows = graph
        .read(
            "given $l: string; \
             match $c isa concept, has concept-id $cid; $cid == $l; \
             fetch { \"id\": $cid };",
            &["l".into()],
            &[GraphValue::from(concept_id)],
        )
        .await
        .map_err(|e| e.to_string())?;
    if rows.is_empty() {
        return Err(format!("no concept '{concept_id}' in the graph"));
    }
    Ok(())
}

async fn ensure_logic_and_technique(
    graph: &Arc<dyn GraphReader>,
    logic_id: &str,
    technique_id: &str,
) -> Result<(), String> {
    let rows = graph
        .read(
            "given $l: string, $tech: string; \
             match $lc isa concept, has concept-id $lcid; $lcid == $l; \
                   $t isa attack-pattern, has attack-id $taid; $taid == $tech; \
             fetch { \"id\": $lcid };",
            &["l".into(), "tech".into()],
            &[GraphValue::from(logic_id), GraphValue::from(technique_id)],
        )
        .await
        .map_err(|e| e.to_string())?;
    if rows.is_empty() {
        return Err(format!(
            "no detection logic '{logic_id}' or technique '{technique_id}' in the graph"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gap(has_requirements: bool, keys: &[&str]) -> GapReading {
        GapReading {
            has_requirements,
            keys: keys.iter().map(|k| k.to_string()).collect(),
        }
    }

    #[test]
    fn readings_for_an_ungathered_pair_is_an_internal_error() {
        let readings = BTreeMap::new();
        let err = readings_for(&readings, "techniques/t1", "Windows").unwrap_err();
        assert_eq!(
            err,
            "internal: readings were not gathered for technique 'techniques/t1', platform 'Windows'"
        );
        let mut seeded = BTreeMap::new();
        seeded.insert(("techniques/t1".to_string(), "Windows".to_string()), vec![]);
        assert!(
            readings_for(&seeded, "techniques/t1", "Windows")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn gap_sets_with_no_analytic_is_graph_changed() {
        assert_eq!(gap_sets("Windows", vec![]), Err(graph_changed("Windows")));
    }

    #[test]
    fn gap_sets_with_a_covered_analytic_is_graph_changed() {
        assert_eq!(
            gap_sets("Windows", vec![gap(true, &["a"]), gap(true, &[])]),
            Err(graph_changed("Windows"))
        );
    }

    #[test]
    fn gap_sets_skip_a_zero_requirement_analytic_beside_a_gapped_one() {
        let expected: BTreeSet<String> = ["a", "b"].iter().map(|k| k.to_string()).collect();
        assert_eq!(
            gap_sets("Windows", vec![gap(false, &[]), gap(true, &["a", "b"])]),
            Ok(vec![expected])
        );
    }

    #[test]
    fn gap_sets_of_only_zero_requirement_analytics_are_empty() {
        assert_eq!(
            gap_sets("macOS", vec![gap(false, &[]), gap(false, &[])]),
            Ok(vec![])
        );
    }

    fn reading(has_requirements: bool, missing: Vec<Value>) -> AnalyticReading {
        AnalyticReading {
            analytic: json!("AN0001"),
            has_requirements,
            missing,
        }
    }

    #[test]
    fn blind_platform_entries_shape_each_blind_analytic() {
        let gap = json!({ "log_source": "NSM:Flow", "data_components": ["Network Traffic"] });
        let entries = blind_platform_entries(
            "Windows",
            vec![reading(true, vec![gap.clone()]), reading(false, vec![])],
        )
        .expect("blind analytics");
        assert_eq!(
            entries,
            vec![
                json!({ "platform": "Windows", "analytic": "AN0001", "no_log_sources": false, "missing": [gap] }),
                json!({ "platform": "Windows", "analytic": "AN0001", "no_log_sources": true, "missing": [] }),
            ]
        );
    }

    #[test]
    fn blind_platform_with_no_analytic_read_is_an_error() {
        let err =
            blind_platform_entries("Windows", vec![]).expect_err("must not drop the platform");
        assert!(err.contains("retry") && err.contains("Windows"), "{err}");
    }

    #[test]
    fn blind_platform_whose_analytic_reads_as_runnable_is_an_error() {
        let err = blind_platform_entries("Windows", vec![reading(true, vec![])])
            .expect_err("must not drop the platform");
        assert!(err.contains("retry") && err.contains("Windows"), "{err}");
    }

    fn pair(technique: &str, platform: &str) -> Pair {
        (technique.to_string(), platform.to_string())
    }

    fn analytic_row(technique: &str, platform: &str, attack_id: &str, concept_id: &str) -> Value {
        json!({ "technique": technique, "platform": platform,
                "analytic": attack_id, "analytic_concept_id": concept_id })
    }

    fn gap_row(concept_id: &str, platform: &str, key: &str, component: &str) -> Value {
        json!({ "analytic_concept_id": concept_id, "platform": platform, "name": key,
                "key": key, "channels": ["c"], "component": component })
    }

    #[test]
    fn a_requested_pair_with_no_analytic_row_is_graph_changed() {
        let pairs: BTreeSet<Pair> = [pair("t1", "Windows")].into();
        let readings = readings_from_analytics(&pairs, &[]).expect("readings");
        let windows = &readings[&pair("t1", "Windows")];
        assert!(windows.is_empty());
        assert_eq!(
            blind_analytics("Windows", windows),
            Err(graph_changed("Windows"))
        );
        assert_eq!(
            blind_gap_sets("Windows", windows, &mut BTreeMap::new()),
            Err(graph_changed("Windows"))
        );
    }

    #[test]
    fn analytic_rows_key_by_pair_and_a_duplicate_attack_id_keeps_the_smallest_concept_id() {
        let pairs: BTreeSet<Pair> = [pair("t1", "Windows"), pair("t2", "Linux")].into();
        let rows = [
            analytic_row("t1", "Windows", "AN2", "a/z"),
            analytic_row("t1", "Windows", "AN1", "a/y"),
            analytic_row("t1", "Windows", "AN1", "a/x"),
            analytic_row("t1", "Windows", "AN1", "a/y"),
            analytic_row("t2", "Linux", "AN3", "a/w"),
        ];
        let readings = readings_from_analytics(&pairs, &rows).expect("readings");
        let ids = |p: &Pair| -> Vec<(Value, String)> {
            readings[p]
                .iter()
                .map(|r| (r.analytic.clone(), r.concept_id.clone()))
                .collect()
        };
        assert_eq!(
            ids(&pair("t1", "Windows")),
            [(json!("AN1"), "a/x".into()), (json!("AN2"), "a/z".into())]
        );
        assert_eq!(ids(&pair("t2", "Linux")), [(json!("AN3"), "a/w".into())]);
    }

    #[test]
    fn an_analytic_row_for_an_unrequested_pair_is_an_error() {
        let pairs: BTreeSet<Pair> = [pair("t1", "Windows")].into();
        let rows = [analytic_row("t1", "Linux", "AN1", "a/x")];
        assert!(readings_from_analytics(&pairs, &rows).is_err());
    }

    #[test]
    fn gaps_attach_by_analytic_and_platform_only_to_analytics_with_requirements() {
        let pairs: BTreeSet<Pair> = [pair("t1", "Windows"), pair("t1", "Linux")].into();
        let rows = [
            analytic_row("t1", "Windows", "AN1", "a/x"),
            analytic_row("t1", "Windows", "AN2", "a/y"),
            analytic_row("t1", "Linux", "AN1", "a/x"),
        ];
        let mut readings = readings_from_analytics(&pairs, &rows).expect("readings");
        mark_requirements(&mut readings, &[json!({ "analytic_concept_id": "a/x" })])
            .expect("requirements");
        attach_gaps(
            &mut readings,
            &[
                gap_row("a/x", "Windows", "k1", "Process Creation"),
                gap_row("a/x", "Linux", "k2", "Command Execution"),
                gap_row("a/x", "Windows", "k3", "Process Creation"),
            ],
        )
        .expect("gaps");

        let keys = |p: &Pair| -> Vec<(bool, Vec<String>)> {
            readings[p]
                .iter()
                .map(|r| {
                    let keys = r.gaps.iter().map(|g| g.key.clone()).collect();
                    (r.has_requirements, keys)
                })
                .collect()
        };
        assert_eq!(
            keys(&pair("t1", "Windows")),
            [(true, vec!["k1".into(), "k3".into()]), (false, vec![])]
        );
        assert_eq!(keys(&pair("t1", "Linux")), [(true, vec!["k2".into()])]);

        let mut sources = BTreeMap::new();
        let windows = &readings[&pair("t1", "Windows")];
        assert_eq!(
            blind_gap_sets("Windows", windows, &mut sources),
            Ok(vec![["k1", "k3"].iter().map(|k| k.to_string()).collect()])
        );
        assert_eq!(
            blind_analytics("Windows", windows),
            Ok(vec![
                json!({ "platform": "Windows", "analytic": "AN1", "no_log_sources": false,
                        "missing": [
                            { "log_source": "k1", "data_components": ["Process Creation"] },
                            { "log_source": "k3", "data_components": ["Process Creation"] },
                        ] }),
                json!({ "platform": "Windows", "analytic": "AN2", "no_log_sources": true,
                        "missing": [] }),
            ])
        );
    }

    #[test]
    fn a_gap_with_no_component_name_fails_blinded_only() {
        let pairs: BTreeSet<Pair> = [pair("t1", "Windows")].into();
        let rows = [analytic_row("t1", "Windows", "AN1", "a/x")];
        let mut readings = readings_from_analytics(&pairs, &rows).expect("readings");
        mark_requirements(&mut readings, &[json!({ "analytic_concept_id": "a/x" })])
            .expect("requirements");
        let mut row = gap_row("a/x", "Windows", "k1", "unused");
        row.as_object_mut().expect("object").remove("component");
        attach_gaps(&mut readings, &[row]).expect("gaps");
        let windows = &readings[&pair("t1", "Windows")];
        let err = blind_analytics("Windows", windows).expect_err("no component name");
        assert!(err.contains("has no name"), "{err}");
        assert!(blind_gap_sets("Windows", windows, &mut BTreeMap::new()).is_ok());
    }

    #[test]
    fn precision_ratio_scores_positives() {
        assert_eq!(precision_ratio(3, 1), Some(0.75));
        assert_eq!(precision_ratio(0, 0), None);
        assert_eq!(precision_ratio(2, 0), Some(1.0));
        assert_eq!(precision_ratio(0, 4), Some(0.0));
    }

    #[test]
    fn req_str_rejects_missing_and_empty() {
        let args = json!({ "a": "x", "b": "" });
        assert_eq!(req_str(&args, "a").unwrap(), "x");
        assert!(req_str(&args, "b").is_err());
        assert!(req_str(&args, "missing").is_err());
    }

    #[test]
    fn mem_ids_parses_string_array() {
        let args = json!({ "evidence": ["mem/ep/1", "mem/ep/2"] });
        let ids = mem_ids(&args, "evidence").unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0].0, "mem/ep/1");
        assert!(mem_ids(&args, "absent").unwrap().is_empty());
        assert!(
            mem_ids(&json!({ "evidence": null }), "evidence")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn mem_ids_rejects_a_non_array_or_a_non_string_entry() {
        let e = mem_ids(&json!({ "evidence": "mem/ep/1" }), "evidence").unwrap_err();
        assert!(e.contains("'evidence'"), "{e}");
        let e = mem_ids(&json!({ "evidence": ["mem/ep/1", 7] }), "evidence").unwrap_err();
        assert!(e.contains("'evidence'") && e.contains("[1]"), "{e}");
    }
}
