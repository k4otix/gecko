//! Detection-engineering host imports, backed by the epistemic substrate and the
//! graph (not fabricated).
//!
//! These functions need a graph connection and the belief-write path, which the
//! plain synchronous [`GeckoExtension::call_import`] seam cannot supply. They are
//! dispatched instead through [`cyber_extension_callback`], a host bridge wired at
//! run time (parallel to mem's `mem.*` bridge) that captures a [`GraphStore`], an
//! [`EpistemicWriter`], and the host-minted [`RunContext`].
//!
//! - `claim` asserts a detection belief and links it to its logic and technique
//!   via the `technique-detection` relation.
//! - `disposition` records an analyst verdict as a belief plus an
//!   `alert-disposition` relation, so `precision` can count it.
//! - `coverage_gaps` / `blinded` run the persisted `undetected-techniques()` /
//!   `blinded-detections()` TypeDB functions.
//! - `precision` counts dispositions in a time window and computes true-positive
//!   precision host-side.

use std::sync::Arc;

use gecko_engine::sandbox::engine::ExtensionCallback;
use gecko_extension_api::{
    BeliefDraft, DerivationMethod, EpistemicWriter, GraphStore, GraphValue, GraphWrite, MemId,
    RunContext, Visibility,
};
use serde_json::{Value, json};

const CYBER_EXT: &str = "cyber-gecko";
const DETECT_FNS: [&str; 5] = [
    "claim",
    "disposition",
    "coverage_gaps",
    "blinded",
    "precision",
];

/// The analyst verdicts accepted by `disposition`, matching the schema's
/// `detection-verdict @values(...)`.
const VERDICTS: [&str; 3] = ["true-positive", "false-positive", "benign-true"];

/// Wraps `base` so cyber-gecko's detect host imports reach the graph and the
/// belief-write path. Every other extension call falls through to `base`.
///
/// This is the sync→async seam: the sandbox host-call bridge is synchronous but
/// the graph and writer are async, so each detect call is driven to completion on
/// the ambient multi-thread runtime via `block_in_place` + `block_on` (the same
/// mechanism the mem bridge uses; requires the multi-thread runtime the binary
/// and sandbox already run on).
pub fn cyber_extension_callback(
    graph: Arc<dyn GraphStore>,
    writer: Arc<dyn EpistemicWriter>,
    ctx: RunContext,
    base: ExtensionCallback,
) -> ExtensionCallback {
    let handle = tokio::runtime::Handle::current();
    Arc::new(move |ext_name: &str, func_name: &str, args: Value| {
        if ext_name == CYBER_EXT && DETECT_FNS.contains(&func_name) {
            let graph = graph.clone();
            let writer = writer.clone();
            let ctx = ctx.clone();
            let func = func_name.to_string();
            return tokio::task::block_in_place(|| {
                handle.block_on(dispatch(&graph, &writer, &ctx, &func, args))
            });
        }
        base(ext_name, func_name, args)
    })
}

async fn dispatch(
    graph: &Arc<dyn GraphStore>,
    writer: &Arc<dyn EpistemicWriter>,
    ctx: &RunContext,
    func: &str,
    args: Value,
) -> Result<Value, String> {
    match func {
        "claim" => claim(graph, writer, ctx, &args).await,
        "disposition" => disposition(graph, writer, ctx, &args).await,
        "coverage_gaps" => coverage_gaps(graph).await,
        "blinded" => blinded(graph).await,
        "precision" => precision(graph, ctx, &args).await,
        other => Err(format!("unknown detect function {other:?}")),
    }
}

/// Asserts "logic detects technique" as a belief and wires the
/// `technique-detection` relation. Errors (never fabricates) on a missing/invalid
/// argument or when the referenced logic or technique is absent from the graph.
async fn claim(
    graph: &Arc<dyn GraphStore>,
    writer: &Arc<dyn EpistemicWriter>,
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
    let evidence = mem_ids(args, "evidence");

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
    let belief_id = writer
        .assert_belief(ctx, belief, &evidence, method)
        .await
        .map_err(|e| e.to_string())?;

    // Link the claim: logic (detecting-logic) + technique (detected-technique) +
    // the belief just written (detection-claim). Idempotent via the not-guard.
    let op = GraphWrite::single(
        "given $l: string, $tech: string, $b: string; \
         match $lc isa concept, has concept-id $lcid; $lcid == $l; \
               $t isa attack-pattern, has attack-id $taid; $taid == $tech; \
               $bel isa belief, has concept-id $bid; $bid == $b; \
               not { $r isa technique-detection, \
                     links (detecting-logic: $lc, detected-technique: $t, detection-claim: $bel); }; \
         insert $nr isa technique-detection, \
                links (detecting-logic: $lc, detected-technique: $t, detection-claim: $bel);",
        vec!["l".into(), "tech".into(), "b".into()],
        vec![
            GraphValue::from(logic_id.as_str()),
            GraphValue::from(technique_id.as_str()),
            GraphValue::from(belief_id.0.as_str()),
        ],
    );
    graph.write(&[op]).await.map_err(|e| e.to_string())?;

    Ok(json!({
        "belief_id": belief_id.0,
        "logic_id": logic_id,
        "technique_id": technique_id,
    }))
}

/// Records an analyst verdict on a detection as a belief plus an
/// `alert-disposition` relation carrying the verdict.
async fn disposition(
    graph: &Arc<dyn GraphStore>,
    writer: &Arc<dyn EpistemicWriter>,
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
    let evidence = mem_ids(args, "evidence");
    ensure_concept(graph, &logic_id).await?;

    let belief = BeliefDraft {
        text: format!("disposition of detection '{logic_id}': {verdict}"),
        owner: ctx.actor.clone(),
        visibility: Visibility::Private,
        confidence: None,
        entrenchment: None,
    };
    let belief_id = writer
        .assert_belief(ctx, belief, &evidence, DerivationMethod::HumanAssertion)
        .await
        .map_err(|e| e.to_string())?;

    let op = GraphWrite::single(
        "given $l: string, $b: string, $v: string; \
         match $lc isa concept, has concept-id $lcid; $lcid == $l; \
               $bel isa belief, has concept-id $bid; $bid == $b; \
         insert $r isa alert-disposition, \
                links (dispositioned-logic: $lc, disposition-belief: $bel), \
                has detection-verdict == $v;",
        vec!["l".into(), "b".into(), "v".into()],
        vec![
            GraphValue::from(logic_id.as_str()),
            GraphValue::from(belief_id.0.as_str()),
            GraphValue::from(verdict.as_str()),
        ],
    );
    graph.write(&[op]).await.map_err(|e| e.to_string())?;

    Ok(json!({ "belief_id": belief_id.0, "logic_id": logic_id, "verdict": verdict }))
}

/// Techniques with no asserted detection claim (the `undetected-techniques()`
/// TypeDB function).
async fn coverage_gaps(graph: &Arc<dyn GraphStore>) -> Result<Value, String> {
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

/// Detection logic whose required evidence has no covering sensor (the
/// `blinded-detections()` TypeDB function).
async fn blinded(graph: &Arc<dyn GraphStore>) -> Result<Value, String> {
    let rows = graph
        .read(
            "match let $d in blinded-detections(); \
             fetch { \"concept_id\": $d.concept-id };",
            &[],
            &[],
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({ "blinded_detections": rows }))
}

/// True-positive precision of a detection over a trailing time window, computed
/// host-side from the dispositions recorded against it.
async fn precision(
    graph: &Arc<dyn GraphStore>,
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
                   $bel has valid-from $t; $t >= $cut; \
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

/// Parses an optional JSON string array of belief/episode ids into `MemId`s.
fn mem_ids(args: &Value, field: &str) -> Vec<MemId> {
    args.get(field)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(MemId::new).collect())
        .unwrap_or_default()
}

async fn ensure_concept(graph: &Arc<dyn GraphStore>, concept_id: &str) -> Result<(), String> {
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
    graph: &Arc<dyn GraphStore>,
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
        let ids = mem_ids(&args, "evidence");
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0].0, "mem/ep/1");
        assert!(mem_ids(&args, "absent").is_empty());
    }
}
