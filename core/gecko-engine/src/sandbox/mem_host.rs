//! Sandbox → mem host-function surface and run-id auto-binding (plan A1.6).
//!
//! Exec-docs call a small set of mem host functions —
//! `remember`/`recall`/`derive`/`supersede`/`contest` — which the host delegates
//! to the injected [`EpistemicWriter`]/[`EpistemicReader`]. The security spine
//! (invariant 2) is that the **host** binds the run's [`RunContext`]: sandbox code
//! supplies only the payload args and **cannot** supply or override the
//! `run_id`/`actor`/`source`. This module implements that binding mechanism and
//! its forgery resistance; the live-writer delegation bodies land in A2.
//!
//! [`EpistemicWriter`]: gecko_extension_api::EpistemicWriter
//! [`EpistemicReader`]: gecko_extension_api::EpistemicReader

use std::sync::Arc;

use gecko_extension_api::{
    ActorId, BeliefDraft, DateTime, DerivationMethod, EpisodeDraft, EpistemicWriter, MemId,
    ProvenanceSource, RunContext, RunId, Visibility,
};
use serde_json::{Value, json};

use super::engine::ExtensionCallback;

/// The mem host-function set callable from sandboxed exec-docs.
///
/// Names are the sandbox-facing surface; each maps to an
/// [`EpistemicWriter`](gecko_extension_api::EpistemicWriter) /
/// [`EpistemicReader`](gecko_extension_api::EpistemicReader) method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemHostFn {
    /// `mem.remember(...)` → `EpistemicWriter::observe`.
    Remember,
    /// `mem.recall(...)` → `EpistemicReader::recall`.
    Recall,
    /// `mem.derive(...)` → `EpistemicWriter::assert_belief`.
    Derive,
    /// `mem.supersede(...)` → `EpistemicWriter::supersede`.
    Supersede,
    /// `mem.contest(...)` → `EpistemicWriter::contest`.
    Contest,
}

impl MemHostFn {
    /// The sandbox-facing function name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Remember => "remember",
            Self::Recall => "recall",
            Self::Derive => "derive",
            Self::Supersede => "supersede",
            Self::Contest => "contest",
        }
    }

    /// Resolves a sandbox-supplied function name to a known mem host fn.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "remember" => Self::Remember,
            "recall" => Self::Recall,
            "derive" => Self::Derive,
            "supersede" => Self::Supersede,
            "contest" => Self::Contest,
            _ => return None,
        })
    }

    /// The full mem host-fn set (for host-import registration).
    pub fn all() -> [MemHostFn; 5] {
        [
            Self::Remember,
            Self::Recall,
            Self::Derive,
            Self::Supersede,
            Self::Contest,
        ]
    }
}

/// Provenance fields the sandbox is forbidden from supplying — the host stamps
/// these, so any occurrence in sandbox args is stripped before dispatch.
const RESERVED_PROVENANCE_KEYS: [&str; 4] = ["run_id", "actor", "source", "occurred_at"];

/// A host-stamped mem call, ready to dispatch to the injected writer/reader.
///
/// The `run_id`/`actor` are taken from the host-minted [`RunContext`], **never**
/// from the sandbox payload. `payload` is the sandbox args with every reserved
/// provenance key removed.
#[derive(Debug, Clone, PartialEq)]
pub struct StampedMemCall {
    /// Which mem host fn is being invoked.
    pub func: MemHostFn,
    /// The host's run id — authoritative, un-forgeable by the sandbox.
    pub run_id: RunId,
    /// The host's actor.
    pub actor: ActorId,
    /// The host's provenance source.
    pub source: ProvenanceSource,
    /// The host's ingest-time anchor for this run.
    pub occurred_at: DateTime,
    /// Sandbox-supplied payload with reserved provenance keys stripped.
    pub payload: Value,
}

/// Binds a host-minted [`RunContext`] to a sandbox-supplied call.
///
/// This is the forgery-resistance mechanism (invariant 2 / acceptance bullet 3):
/// the returned [`StampedMemCall`] always carries the host's `run_id`, even if
/// `args` tries to smuggle a different `run_id`/`actor`/`source`. Those keys are
/// stripped from the payload and replaced by the context's values.
pub fn bind_mem_call(ctx: &RunContext, func: MemHostFn, mut args: Value) -> StampedMemCall {
    if let Some(obj) = args.as_object_mut() {
        for key in RESERVED_PROVENANCE_KEYS {
            obj.remove(key);
        }
    }
    StampedMemCall {
        func,
        run_id: ctx.run_id,
        actor: ctx.actor.clone(),
        source: ctx.source.clone(),
        occurred_at: ctx.occurred_at,
        payload: args,
    }
}

// ── Async writer dispatch + the async→sync sandbox bridge (plan A4.2) ─────────

/// Reads a required string field, erroring with the field name if absent/non-string.
fn req_str(payload: &Value, key: &str) -> Result<String, String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("mem call missing required string field '{key}'"))
}

/// Collects a `[MemId]` from an optional string-array field (absent ⇒ empty).
fn mem_ids(payload: &Value, key: &str) -> Vec<MemId> {
    payload
        .get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(MemId::new))
                .collect()
        })
        .unwrap_or_default()
}

/// Parses an optional RFC3339 datetime field, falling back to `default`.
fn opt_dt(payload: &Value, key: &str, default: DateTime) -> DateTime {
    payload
        .get(key)
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or(default)
}

/// Dispatches a host-stamped mem call to the injected [`EpistemicWriter`].
///
/// `ctx` is the **per-run** [`RunContext`] the host minted once for the whole run
/// (never reconstructed per call) — so its `run_id`/`actor`/`source` are the
/// host's, un-forgeable by the sandbox (invariant 2), and its `scratch` is shared
/// across every mem call in the run. That shared scratch is what lets a `recall`
/// populate `ctx.scratch.retrievals` and a later `assert_belief` in the same run
/// read it back (A5.7 retrieval-provenance correlation). `call` only supplies the
/// already-stamped `run_id`/`actor`/`source`/`payload` (see [`bind_mem_call`]); it
/// must have been stamped from this same `ctx`. `recall` is a *reader* op and is
/// not reachable through this write bridge (the reader path is wired separately).
pub async fn dispatch_mem_call(
    writer: &Arc<dyn EpistemicWriter>,
    ctx: &RunContext,
    call: StampedMemCall,
) -> Result<Value, String> {
    let p = &call.payload;
    let err = |e: gecko_extension_api::EpistemicError| e.to_string();

    match call.func {
        MemHostFn::Remember => {
            let text = req_str(p, "text")?;
            let ep = EpisodeDraft {
                text,
                event_time: opt_dt(p, "event_time", ctx.occurred_at),
                ingest_time: ctx.occurred_at,
            };
            let id = writer.observe(&ctx, ep).await.map_err(err)?;
            Ok(json!({ "id": id.0 }))
        }
        MemHostFn::Derive => {
            let belief = BeliefDraft {
                text: req_str(p, "text")?,
                owner: ctx.actor.clone(),
                visibility: Visibility::Private,
                confidence: p.get("confidence").and_then(Value::as_f64),
            };
            let evidence = mem_ids(p, "evidence");
            let method = p
                .get("method")
                .and_then(Value::as_str)
                .and_then(DerivationMethod::from_str)
                .unwrap_or(DerivationMethod::LlmSynthesis);
            let id = writer
                .assert_belief(&ctx, belief, &evidence, method)
                .await
                .map_err(err)?;
            Ok(json!({ "id": id.0 }))
        }
        MemHostFn::Supersede => {
            let old = MemId::new(req_str(p, "old")?);
            let belief = BeliefDraft {
                text: req_str(p, "text")?,
                owner: ctx.actor.clone(),
                visibility: Visibility::Private,
                confidence: p.get("confidence").and_then(Value::as_f64),
            };
            let reason = p.get("reason").and_then(Value::as_str).unwrap_or("");
            let id = writer
                .supersede(&ctx, old, belief, reason)
                .await
                .map_err(err)?;
            Ok(json!({ "id": id.0 }))
        }
        MemHostFn::Contest => {
            let claims = mem_ids(p, "claims");
            let anomaly = writer.contest(&ctx, &claims).await.map_err(err)?;
            Ok(json!({ "anomaly": anomaly.0 }))
        }
        MemHostFn::Recall => Err(
            "mem.recall is a reader operation and is not exposed on the epistemic write bridge"
                .to_string(),
        ),
    }
}

/// Builds the sandbox [`ExtensionCallback`] that routes the mem host fns to the
/// injected [`EpistemicWriter`], falling through to `base` for every other
/// extension call.
///
/// This is the async→sync integration seam: the sandbox host-call bridge is a
/// **synchronous** [`ExtensionCallback`], but [`EpistemicWriter`] is **async**. A mem
/// call is stamped with the host's `ctx` ([`bind_mem_call`]) and driven to
/// completion on the ambient Tokio runtime via `block_in_place` + `Handle::block_on`
/// (the callback runs inside the wasm executor's async host bridge, so it is already
/// on a runtime worker; `block_in_place` yields the worker while the writer's DB I/O
/// completes). Requires the multi-threaded runtime the binary and the wasm sandbox
/// already run on.
pub fn epistemic_extension_callback(
    ctx: RunContext,
    writer: Arc<dyn EpistemicWriter>,
    base: ExtensionCallback,
) -> ExtensionCallback {
    let handle = tokio::runtime::Handle::current();
    Arc::new(move |ext_name: &str, func_name: &str, args: Value| {
        if ext_name == "mem"
            && let Some(func) = MemHostFn::from_name(func_name)
        {
            let call = bind_mem_call(&ctx, func, args);
            let writer = writer.clone();
            let handle = handle.clone();
            return tokio::task::block_in_place(|| {
                handle.block_on(async { dispatch_mem_call(&writer, &ctx, call).await })
            });
        }
        base(ext_name, func_name, args)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn host_ctx() -> RunContext {
        RunContext::new(
            ActorId::new("system"),
            ProvenanceSource::ExecutableDoc {
                concept_id: gecko_extension_api::ConceptId::new("okf/run/doc"),
            },
            chrono::Utc::now(),
        )
    }

    #[test]
    fn host_fn_names_round_trip() {
        for f in MemHostFn::all() {
            assert_eq!(MemHostFn::from_name(f.as_str()), Some(f));
        }
        assert_eq!(MemHostFn::from_name("delete_everything"), None);
    }

    #[test]
    fn binding_uses_host_run_id() {
        let ctx = host_ctx();
        let call = bind_mem_call(&ctx, MemHostFn::Remember, json!({"text": "saw X"}));
        assert_eq!(call.run_id, ctx.run_id);
        assert_eq!(call.actor, ctx.actor);
    }

    #[test]
    fn sandbox_cannot_forge_a_different_run_id() {
        let ctx = host_ctx();
        let forged = RunId::new();
        assert_ne!(forged, ctx.run_id);

        // Sandbox tries to smuggle its own provenance in the payload.
        let call = bind_mem_call(
            &ctx,
            MemHostFn::Derive,
            json!({
                "text": "the host is compromised",
                "run_id": forged.to_string(),
                "actor": "attacker",
                "source": "Manual",
            }),
        );

        // The stamped call carries the HOST's run id, not the forged one...
        assert_eq!(call.run_id, ctx.run_id);
        assert_eq!(call.actor, ctx.actor);
        // ...and the smuggled provenance keys are gone from the payload.
        let obj = call.payload.as_object().unwrap();
        assert!(!obj.contains_key("run_id"));
        assert!(!obj.contains_key("actor"));
        assert!(!obj.contains_key("source"));
        // The genuine payload survives.
        assert_eq!(obj.get("text").unwrap(), "the host is compromised");
    }

    #[test]
    fn non_object_payload_is_left_intact() {
        let ctx = host_ctx();
        let call = bind_mem_call(&ctx, MemHostFn::Recall, json!("a bare string query"));
        assert_eq!(call.run_id, ctx.run_id);
        assert_eq!(call.payload, json!("a bare string query"));
    }
}
