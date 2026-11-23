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

/// For each asserted detection claim with a declared platform it is blind on or
/// cannot assess: the blind platforms with what each lacks, and the unassessed
/// platforms. Errors on a graph it cannot judge: one with no sensor file, or with
/// no current ATT&CK detection data.
async fn blinded(graph: &Arc<dyn GraphReader>) -> Result<Value, String> {
    require_judgeable(graph).await?;
    let claims = claim_statuses(graph).await?;

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
                blind_platforms
                    .extend(blind_analytics(graph, &key.technique_concept_id, platform).await?);
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

/// The `blind_platforms` entries for `platform` of the technique
/// `technique_concept_id`: one per live analytic there that cannot run, sorted by
/// analytic.
async fn blind_analytics(
    graph: &Arc<dyn GraphReader>,
    technique_concept_id: &str,
    platform: &str,
) -> Result<Vec<Value>, String> {
    let mut readings = Vec::new();
    for (attack_id, concept_id) in live_analytics(graph, technique_concept_id, platform).await? {
        let has_requirements = has_live_requirement(graph, &concept_id).await?;
        let missing = if has_requirements {
            missing_log_sources(graph, &concept_id, platform).await?
        } else {
            Vec::new()
        };
        readings.push(AnalyticReading {
            analytic: attack_id,
            has_requirements,
            missing,
        });
    }
    blind_platform_entries(platform, readings)
}

/// The live analytics of the technique `technique_concept_id` on `platform`, as
/// (attack-id or null, concept id), sorted by attack-id, or by concept id for one
/// with no attack-id.
async fn live_analytics(
    graph: &Arc<dyn GraphReader>,
    technique_concept_id: &str,
    platform: &str,
) -> Result<Vec<(Value, String)>, String> {
    let analytics = read(
        graph,
        &format!(
            "given $tid: string, $plat: string; \
             match $t isa attack-pattern, has concept-id $tc; $tc == $tid; \
                   strategy-detects (strategy: $st, technique: $t); {} \
                   strategy-analytic (strategy: $st, analytic: $a); {} \
                   $a has platform $ap; $ap == $plat; $a has concept-id $ac; \
             fetch {{ \"analytic_concept_id\": $ac, \"analytic\": $a.attack-id }};",
            live("st"),
            live("a")
        ),
        &["tid", "plat"],
        &[technique_concept_id, platform],
    )
    .await?;

    let mut by_analytic: BTreeMap<String, (Value, String)> = BTreeMap::new();
    for row in &analytics {
        let concept_id = row_str(row, "analytic_concept_id")?;
        let attack_id = row.get("analytic").cloned().unwrap_or(Value::Null);
        let sort_key = attack_id.as_str().unwrap_or(&concept_id).to_string();
        by_analytic.insert(sort_key, (attack_id, concept_id));
    }
    Ok(by_analytic.into_values().collect())
}

/// Whether the analytic `analytic_concept_id` has a requirement on a live data
/// component.
async fn has_live_requirement(
    graph: &Arc<dyn GraphReader>,
    analytic_concept_id: &str,
) -> Result<bool, String> {
    let requirements = read(
        graph,
        &format!(
            "given $aid: string; \
             match $a isa analytic, has concept-id $ac; $ac == $aid; \
                   $r isa analytic-requirement, links (analytic: $a, component: $c); {} \
             limit 1; fetch {{ \"key\": $r.log-source-key }};",
            live("c")
        ),
        &["aid"],
        &[analytic_concept_id],
    )
    .await?;
    Ok(!requirements.is_empty())
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

    // (platform, log-source key) → (smallest trimmed spelling, every channel).
    let mut sources: BTreeMap<(String, String), (String, BTreeSet<String>)> = BTreeMap::new();
    let mut gap_sets: BTreeMap<(String, String), Vec<BTreeSet<String>>> = BTreeMap::new();
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
            let cache_key = (key.technique_concept_id.clone(), platform.clone());
            let analytics = match gap_sets.get(&cache_key) {
                Some(sets) => sets.clone(),
                None => {
                    let sets =
                        blind_gap_sets(graph, &key.technique_concept_id, platform, &mut sources)
                            .await?;
                    gap_sets.insert(cache_key, sets.clone());
                    sets
                }
            };
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

/// The missing log-source keys of each live analytic of the technique
/// `technique_concept_id` on `platform` that has a requirement, recording each key's
/// spelling and channels in `sources`. Errors when `claim-platforms()` called the
/// platform blind but the analytics no longer read as blind.
async fn blind_gap_sets(
    graph: &Arc<dyn GraphReader>,
    technique_concept_id: &str,
    platform: &str,
    sources: &mut BTreeMap<(String, String), (String, BTreeSet<String>)>,
) -> Result<Vec<BTreeSet<String>>, String> {
    let mut readings = Vec::new();
    for (_, concept_id) in live_analytics(graph, technique_concept_id, platform).await? {
        if !has_live_requirement(graph, &concept_id).await? {
            readings.push(GapReading {
                has_requirements: false,
                keys: BTreeSet::new(),
            });
            continue;
        }
        let gaps = read(
            graph,
            "given $aid: string, $plat: string; \
             match $a isa analytic, has concept-id $ac; $ac == $aid; \
                   let $r in analytic-gaps($a, $plat); \
             fetch { \"name\": $r.log-source-name, \"key\": $r.log-source-key, \
                     \"channels\": [ $r.log-source-channel ] };",
            &["aid", "plat"],
            &[&concept_id, platform],
        )
        .await?;
        let mut keys = BTreeSet::new();
        for row in &gaps {
            let key = row_str(row, "key")?;
            let name = row_str(row, "name")?.trim().to_string();
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
            let (display, all_channels) = sources
                .entry((platform.to_string(), key.clone()))
                .or_insert_with(|| (name.clone(), BTreeSet::new()));
            if name < *display {
                *display = name;
            }
            all_channels.extend(channels);
            keys.insert(key);
        }
        readings.push(GapReading {
            has_requirements: true,
            keys,
        });
    }
    gap_sets(platform, readings)
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

/// The `analytic-gaps` of the analytic `analytic_concept_id` on `platform`, grouped
/// by log-source key: each key's smallest trimmed spelling and the sorted names of the data
/// components it would serve, sorted by spelling.
async fn missing_log_sources(
    graph: &Arc<dyn GraphReader>,
    analytic_concept_id: &str,
    platform: &str,
) -> Result<Vec<Value>, String> {
    let gaps = read(
        graph,
        "given $aid: string, $plat: string; \
         match $a isa analytic, has concept-id $ac; $ac == $aid; \
               let $r in analytic-gaps($a, $plat); \
               $r links (component: $c); \
         fetch { \"name\": $r.log-source-name, \"key\": $r.log-source-key, \
                 \"component\": $c.title, \"component_id\": $c.concept-id };",
        &["aid", "plat"],
        &[analytic_concept_id, platform],
    )
    .await?;

    let mut groups: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();
    for row in &gaps {
        let name = row_str(row, "name")?;
        let component = row
            .get("component")
            .and_then(Value::as_str)
            .map(|c| c.trim().to_string())
            .ok_or_else(|| format!("a gap's data component has no name: {row}"))?;
        let (display, components) = groups
            .entry(row_str(row, "key")?)
            .or_insert_with(|| (name.clone(), BTreeSet::new()));
        if name < *display {
            *display = name;
        }
        components.insert(component);
    }

    let mut missing: Vec<(String, String, BTreeSet<String>)> = groups
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
