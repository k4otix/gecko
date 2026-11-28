//! Sandbox → mem host-function surface and run-id binding.
//!
//! Exec-docs call a small set of mem host functions —
//! `remember`/`recall`/`derive`/`supersede`/`contest`/`retract`/`rests_on` — which the host delegates
//! to the injected [`EpistemicWriter`]/[`EpistemicReader`]. The security spine
//! (invariant 2) is that the **host** binds the run's [`RunContext`]: sandbox code
//! supplies only the payload args and **cannot** supply or override the
//! `run_id`/`actor`/`source`. This module implements that binding mechanism and
//! its forgery resistance.
//!
//! The write ops route to [`dispatch_mem_call`] and `recall` routes to
//! [`dispatch_mem_recall`]. The guest reaches these via a `mem.*` global exposed by
//! the exec-doc JS runtime (`core/js-sandbox/src/main.rs`, compiled into the
//! committed `quickjs.wasm`); `gecko-bin/tests/mem_recall_e2e_test.rs` exercises the
//! full `mem.recall` → later `mem.derive` path end to end.
//!
//! ## Shared per-run scratch
//!
//! The host mints one [`RunContext`] for the whole run and threads the same
//! reference through every mem call. Its `run_id`/`actor`/`source` are the host's,
//! un-forgeable by the sandbox (invariant 2), and its `scratch` is shared across
//! every call in the run. That shared scratch is what lets a `recall` populate
//! `ctx.scratch.retrievals` and a later `assert_belief` in the same run read it
//! back, correlating the synthesized belief to the retrieval that informed it
//! (retrieval-provenance). Both the write bridge and the reader bridge borrow the
//! same `ctx`, so the correlation holds across the reader→writer boundary.
//!
//! [`EpistemicWriter`]: gecko_extension_api::EpistemicWriter
//! [`EpistemicReader`]: gecko_extension_api::EpistemicReader

use std::future::Future;
use std::sync::Arc;

use gecko_extension_api::{
    ActorId, BeliefDraft, ContextBudget, DateTime, DerivationMethod, EpisodeDraft, EpistemicReader,
    EpistemicWriter, MemId, ProvenanceSource, RecallQuery, RunContext, RunId, Visibility, shown,
    whole_number,
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
    /// `mem.retract(...)` → `EpistemicWriter::retract`.
    Retract,
    /// `mem.restsOn(...)` → `EpistemicWriter::rests_on`.
    RestsOn,
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
            Self::Retract => "retract",
            Self::RestsOn => "rests_on",
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
            "retract" => Self::Retract,
            "rests_on" => Self::RestsOn,
            _ => return None,
        })
    }

    /// The full mem host-fn set (used by the name round-trip test, its only caller).
    pub fn all() -> [MemHostFn; 7] {
        [
            Self::Remember,
            Self::Recall,
            Self::Derive,
            Self::Supersede,
            Self::Contest,
            Self::Retract,
            Self::RestsOn,
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
/// This is the forgery-resistance mechanism (invariant 2):
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

// ── Async writer dispatch + the async→sync sandbox bridge ────────────────────

/// The payload keys each mem function accepts, after the reserved provenance keys
/// are stripped. `gecko.callExtension("mem", …)` reaches the dispatchers without the
/// guest's own checks, so the host refuses anything else itself.
fn allowed_keys(func: MemHostFn) -> &'static [&'static str] {
    match func {
        MemHostFn::Remember => &["text", "event_time"],
        MemHostFn::Derive => &["text", "evidence", "method", "confidence"],
        MemHostFn::Supersede => &["old", "text", "reason", "confidence"],
        MemHostFn::Contest => &["claims"],
        MemHostFn::Retract => &["id", "reason"],
        MemHostFn::RestsOn => &["resting", "assumptions"],
        MemHostFn::Recall => &["text", "max_chunks", "as_of"],
    }
}

/// Checks that the payload is a JSON object holding only `func`'s allowed keys.
fn check_payload(func: MemHostFn, p: &Value) -> Result<(), String> {
    let name = func.as_str();
    let obj = p
        .as_object()
        .ok_or_else(|| format!("mem.{name} payload must be a JSON object, got {}", shown(p)))?;
    let allowed = allowed_keys(func);
    match obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!(
            "mem.{name} does not accept the field {}; it accepts {}",
            shown(&Value::String(k.clone())),
            allowed.join(", ")
        )),
        None => Ok(()),
    }
}

/// An optional field's value: `None` when it is missing or `null`.
fn present<'a>(p: &'a Value, key: &str) -> Option<&'a Value> {
    p.get(key).filter(|v| !v.is_null())
}

/// Reads a required string field, erroring with the field name if absent/non-string.
fn req_str(payload: &Value, key: &str) -> Result<String, String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("mem call missing required string field '{key}'"))
}

/// An optional string field; a present non-string is an error naming the field.
fn opt_str<'a>(p: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    present(p, key)
        .map(|v| {
            v.as_str()
                .ok_or_else(|| format!("mem call field '{key}' must be a string, got {}", shown(v)))
        })
        .transpose()
}

/// An optional number field; a present non-number is an error naming the field.
fn opt_number(p: &Value, key: &str) -> Result<Option<f64>, String> {
    present(p, key)
        .map(|v| {
            v.as_f64()
                .ok_or_else(|| format!("mem call field '{key}' must be a number, got {}", shown(v)))
        })
        .transpose()
}

/// An optional RFC 3339 datetime field; a present value that is not one is an error.
fn opt_rfc3339(p: &Value, key: &str) -> Result<Option<DateTime>, String> {
    opt_str(p, key)
        .map_err(|_| format!("mem call field '{key}' must be an RFC3339 string"))?
        .map(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .map_err(|e| format!("mem call field '{key}' is not valid RFC3339: {e}"))
        })
        .transpose()
}

/// An optional whole number (see [`whole_number`]) that must be at least 1.
fn opt_positive_int(p: &Value, key: &str) -> Result<Option<usize>, String> {
    let Some(v) = present(p, key) else {
        return Ok(None);
    };
    let n = whole_number(v).and_then(|n| usize::try_from(n).ok());
    match n {
        Some(n) if n >= 1 => Ok(Some(n)),
        _ => Err(format!(
            "mem call field '{key}' must be an integer of at least 1, got {}",
            shown(v)
        )),
    }
}

/// Collects a `[MemId]` from an optional string-array field: absent or `null` ⇒ empty,
/// and a non-array or a non-string entry is an error naming the field.
fn mem_ids(payload: &Value, key: &str) -> Result<Vec<MemId>, String> {
    let Some(field) = present(payload, key) else {
        return Ok(Vec::new());
    };
    let arr = field
        .as_array()
        .ok_or_else(|| format!("mem call field '{key}' must be an array of id strings"))?;
    arr.iter()
        .enumerate()
        .map(|(i, v)| {
            v.as_str().map(MemId::new).ok_or_else(|| {
                format!(
                    "mem call field '{key}' must be an array of id strings; [{i}] is {}",
                    shown(v)
                )
            })
        })
        .collect()
}

/// Refuses an empty id in field `key`; any other id is returned as given.
fn non_empty_id(id: String, key: &str) -> Result<String, String> {
    if id.is_empty() {
        return Err(format!("mem call field '{key}' must be a non-empty id"));
    }
    Ok(id)
}

/// Collects a required `[MemId]` field: missing and `null` are both errors.
fn required_mem_ids(payload: &Value, key: &str) -> Result<Vec<MemId>, String> {
    if present(payload, key).is_none() {
        return Err(format!(
            "mem call missing required field '{key}' (an array of id strings)"
        ));
    }
    mem_ids(payload, key)
}

/// Parses the optional `method` field; absent is [`DerivationMethod::LlmSynthesis`].
fn derivation_method(p: &Value) -> Result<DerivationMethod, String> {
    let accepted = || {
        DerivationMethod::ALL
            .iter()
            .map(DerivationMethod::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };
    match present(p, "method") {
        None => Ok(DerivationMethod::LlmSynthesis),
        Some(v) => v
            .as_str()
            .and_then(DerivationMethod::from_str)
            .ok_or_else(|| {
                format!(
                    "mem call field 'method' must be one of {}, got {}",
                    accepted(),
                    shown(v)
                )
            }),
    }
}

/// Builds a [`BeliefDraft`] from the sandbox payload, owned by the run's actor.
///
/// Entrenchment is left `None`: a fresh assertion derives it from the method (see
/// `entrenchment_for`), and a supersede inherits the old belief's tier while the
/// invariant-7 guard blocks a downgrade. The sandbox never sets it directly.
fn belief_draft_from_payload(ctx: &RunContext, p: &Value) -> Result<BeliefDraft, String> {
    Ok(BeliefDraft {
        text: req_str(p, "text")?,
        owner: ctx.actor.clone(),
        visibility: Visibility::Private,
        confidence: opt_number(p, "confidence")?,
        entrenchment: None,
    })
}

/// Dispatches a host-stamped mem call to the injected [`EpistemicWriter`].
///
/// `ctx` is the per-run [`RunContext`] the host minted once for the whole run; its
/// `run_id`/`actor`/`source` are the host's, un-forgeable by the sandbox
/// (invariant 2). See module docs for the shared-scratch mechanism. `call` only
/// supplies the already-stamped `run_id`/`actor`/`source`/`payload` (see
/// [`bind_mem_call`]); it must have been stamped from this same `ctx`. The payload
/// is validated whole before the writer runs: a non-object, an unknown field or a
/// malformed value is an error, and an absent optional field takes its default.
/// `recall` is a *reader* op and is not reachable through this write bridge.
pub async fn dispatch_mem_call(
    writer: &Arc<dyn EpistemicWriter>,
    ctx: &RunContext,
    call: StampedMemCall,
) -> Result<Value, String> {
    let p = &call.payload;
    let err = |e: gecko_extension_api::EpistemicError| e.to_string();
    if call.func != MemHostFn::Recall {
        check_payload(call.func, p)?;
    }

    match call.func {
        MemHostFn::Remember => {
            let text = req_str(p, "text")?;
            let ep = EpisodeDraft {
                text,
                event_time: opt_rfc3339(p, "event_time")?.unwrap_or(ctx.occurred_at),
                ingest_time: ctx.occurred_at,
            };
            let id = writer.observe(ctx, ep).await.map_err(err)?;
            Ok(json!({ "id": id.0 }))
        }
        MemHostFn::Derive => {
            let belief = belief_draft_from_payload(ctx, p)?;
            let evidence = mem_ids(p, "evidence")?;
            let method = derivation_method(p)?;
            let id = writer
                .assert_belief(ctx, belief, &evidence, method)
                .await
                .map_err(err)?;
            Ok(json!({ "id": id.0 }))
        }
        MemHostFn::Supersede => {
            let old = MemId::new(req_str(p, "old")?);
            let belief = belief_draft_from_payload(ctx, p)?;
            let reason = opt_str(p, "reason")?.unwrap_or("");
            let id = writer
                .supersede(ctx, old, belief, reason)
                .await
                .map_err(err)?;
            Ok(json!({ "id": id.0 }))
        }
        MemHostFn::Contest => {
            let claims = required_mem_ids(p, "claims")?;
            let anomaly = writer.contest(ctx, &claims).await.map_err(err)?;
            Ok(json!({ "anomaly": anomaly.0 }))
        }
        MemHostFn::Retract => {
            let id = req_str(p, "id")?;
            let reason = opt_str(p, "reason")?;
            let radius = writer
                .retract(ctx, MemId::new(id.clone()), reason)
                .await
                .map_err(err)?;
            let radius: Vec<String> = radius.into_iter().map(|m| m.0).collect();
            Ok(json!({ "id": id, "blast_radius": radius }))
        }
        MemHostFn::RestsOn => {
            let resting = non_empty_id(req_str(p, "resting")?, "resting")?;
            let assumptions = required_mem_ids(p, "assumptions")?;
            for a in &assumptions {
                non_empty_id(a.0.clone(), "assumptions")?;
            }
            writer
                .rests_on(ctx, MemId::new(resting.clone()), &assumptions)
                .await
                .map_err(err)?;
            let assumptions: Vec<String> = assumptions.into_iter().map(|m| m.0).collect();
            Ok(json!({ "resting": resting, "assumptions": assumptions }))
        }
        // `mem.recall` is a *reader* op: it is dispatched through the separate
        // reader bridge ([`dispatch_mem_recall`]), which the callback routes to when
        // an [`EpistemicReader`] is wired. It is deliberately unreachable on THIS
        // write bridge — a writer has no reader — so a direct call here is an error.
        MemHostFn::Recall => Err(
            "mem.recall is a reader operation and is not exposed on the epistemic write bridge"
                .to_string(),
        ),
    }
}

/// The default number of chunks a `mem.recall` returns when the sandbox omits an
/// explicit `max_chunks`.
const DEFAULT_RECALL_MAX_CHUNKS: usize = 8;

/// Dispatches a host-stamped `mem.recall` to the injected [`EpistemicReader`].
///
/// The reader shares the SAME per-run [`RunContext`] as the write bridge. See module
/// docs for the shared-scratch mechanism. The sandbox supplies only the query
/// payload, validated whole before the reader runs; `run_id`/`actor`/`source` are
/// the host's (already stripped + stamped by [`bind_mem_call`]). Returns
/// `{ "chunks": [{ id, text, score }, …] }`.
pub async fn dispatch_mem_recall(
    reader: &Arc<dyn EpistemicReader>,
    ctx: &RunContext,
    call: StampedMemCall,
) -> Result<Value, String> {
    let p = &call.payload;
    check_payload(MemHostFn::Recall, p)?;

    let text = req_str(p, "text")?;
    // A malformed `as_of` is an error, never a silent fall-through to present-state recall.
    let as_of = opt_rfc3339(p, "as_of")?;
    let budget = ContextBudget {
        max_chunks: opt_positive_int(p, "max_chunks")?.unwrap_or(DEFAULT_RECALL_MAX_CHUNKS),
    };

    let query = RecallQuery { text, as_of };
    let chunks = reader
        .recall(ctx, query, budget)
        .await
        .map_err(|e| e.to_string())?;

    let chunks: Vec<Value> = chunks
        .into_iter()
        .map(|c| json!({ "id": c.id.0, "text": c.text, "score": c.score }))
        .collect();
    Ok(json!({ "chunks": chunks }))
}

/// Drives a mem-bridge future to completion on the ambient multi-thread runtime.
///
/// The callback runs inside the wasm executor's async host bridge, so it is already
/// on a runtime worker; `block_in_place` hands that worker back to the scheduler
/// while the writer/reader's DB I/O completes, then `block_on` awaits the result.
fn block_on_bridge<F>(handle: &tokio::runtime::Handle, fut: F) -> Result<Value, String>
where
    F: Future<Output = Result<Value, String>>,
{
    tokio::task::block_in_place(|| handle.block_on(fut))
}

/// Builds the sandbox [`ExtensionCallback`] that routes the mem host fns to the
/// injected [`EpistemicWriter`] — and `mem.recall` to the optional
/// [`EpistemicReader`] reader bridge when one is wired — falling through to `base`
/// for every other extension call. When `reader` is `None`, `mem.recall` returns an
/// unavailable error (the writer alone cannot read).
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
    reader: Option<Arc<dyn EpistemicReader>>,
    base: ExtensionCallback,
) -> ExtensionCallback {
    let handle = tokio::runtime::Handle::current();
    // The async→sync bridge below drives the writer with `block_in_place` + `block_on`.
    // `block_in_place` **panics on a current-thread runtime** — it can only hand the
    // worker back to the scheduler when there IS a multi-threaded scheduler. The binary
    // and the wasm sandbox both run on a multi-thread runtime; assert the flavor here so
    // a misconfiguration fails loudly at wiring time rather than deep inside a host call.
    debug_assert_eq!(
        handle.runtime_flavor(),
        tokio::runtime::RuntimeFlavor::MultiThread,
        "epistemic mem host bridge requires a multi-thread tokio runtime \
         (block_in_place + block_on panics on the current-thread scheduler)"
    );
    Arc::new(move |ext_name: &str, func_name: &str, args: Value| {
        if ext_name == "mem"
            && let Some(func) = MemHostFn::from_name(func_name)
        {
            let call = bind_mem_call(&ctx, func, args);
            let handle = handle.clone();
            // Reader op → reader bridge (when wired); everything else → write bridge.
            if func == MemHostFn::Recall {
                let reader = reader.clone();
                return block_on_bridge(&handle, async {
                    match reader.as_ref() {
                        Some(r) => dispatch_mem_recall(r, &ctx, call).await,
                        None => Err("mem.recall is unavailable: no epistemic \
                                     reader is wired for this run"
                            .to_string()),
                    }
                });
            }
            let writer = writer.clone();
            return block_on_bridge(&handle, async {
                dispatch_mem_call(&writer, &ctx, call).await
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

    // ── Reader bridge (mem.recall) ──────────────────────────────────────────

    use gecko_extension_api::{Chunk, EpistemicError, EpistemicReader};

    /// A recording mock reader: captures the last `(query, budget)` it was asked
    /// for and returns a fixed chunk list, so the tests can assert the payload →
    /// `RecallQuery`/`ContextBudget` parse and the chunk → JSON serialization.
    struct MockReader {
        last: std::sync::Mutex<Option<(RecallQuery, ContextBudget)>>,
        chunks: Vec<Chunk>,
    }

    #[async_trait::async_trait]
    impl EpistemicReader for MockReader {
        async fn recall(
            &self,
            _ctx: &RunContext,
            q: RecallQuery,
            budget: ContextBudget,
        ) -> std::result::Result<Vec<Chunk>, EpistemicError> {
            *self.last.lock().unwrap() = Some((q, budget));
            Ok(self.chunks.clone())
        }
        async fn derivation_chain(
            &self,
            _b: MemId,
        ) -> std::result::Result<Vec<MemId>, EpistemicError> {
            Ok(vec![])
        }
        async fn believed_at(
            &self,
            _ctx: &RunContext,
            _at: DateTime,
        ) -> std::result::Result<Vec<MemId>, EpistemicError> {
            Ok(vec![])
        }
        async fn blast_radius(
            &self,
            _retracted: MemId,
        ) -> std::result::Result<Vec<MemId>, EpistemicError> {
            Ok(vec![])
        }
    }

    fn reader_with(chunks: Vec<Chunk>) -> Arc<MockReader> {
        Arc::new(MockReader {
            last: std::sync::Mutex::new(None),
            chunks,
        })
    }

    /// Runs a recall against the mock, returning both the dispatched JSON and the
    /// `(query, budget)` the reader actually received.
    async fn run_recall(
        mock: &Arc<MockReader>,
        ctx: &RunContext,
        payload: Value,
    ) -> Result<Value, String> {
        let reader: Arc<dyn EpistemicReader> = mock.clone();
        let call = bind_mem_call(ctx, MemHostFn::Recall, payload);
        dispatch_mem_recall(&reader, ctx, call).await
    }

    fn captured(mock: &Arc<MockReader>) -> (RecallQuery, ContextBudget) {
        mock.last
            .lock()
            .unwrap()
            .clone()
            .expect("reader was called")
    }

    #[tokio::test]
    async fn recall_parses_query_budget_and_serializes_chunks() {
        let ctx = host_ctx();
        let mock = reader_with(vec![
            Chunk {
                id: MemId::new("mem/ep/a"),
                text: "first".to_string(),
                score: 0.9,
            },
            Chunk {
                id: MemId::new("mem/ep/b"),
                text: "second".to_string(),
                score: 0.5,
            },
        ]);
        let out = run_recall(
            &mock,
            &ctx,
            json!({ "text": "oauth persistence", "max_chunks": 3 }),
        )
        .await
        .unwrap();

        // The query text + budget reached the reader; as_of absent ⇒ present-state.
        let (q, budget) = captured(&mock);
        assert_eq!(q.text, "oauth persistence");
        assert!(q.as_of.is_none());
        assert_eq!(budget.max_chunks, 3);

        // The chunks serialize to the documented shape.
        let chunks = out.get("chunks").and_then(Value::as_array).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].get("id").unwrap(), "mem/ep/a");
        assert_eq!(chunks[0].get("text").unwrap(), "first");
        assert_eq!(chunks[1].get("id").unwrap(), "mem/ep/b");
    }

    #[tokio::test]
    async fn recall_defaults_max_chunks_when_omitted() {
        let ctx = host_ctx();
        let mock = reader_with(vec![]);
        run_recall(&mock, &ctx, json!({ "text": "q" }))
            .await
            .unwrap();
        let (_q, budget) = captured(&mock);
        assert_eq!(budget.max_chunks, DEFAULT_RECALL_MAX_CHUNKS);
    }

    #[tokio::test]
    async fn recall_requires_text_and_rejects_malformed_as_of() {
        let ctx = host_ctx();
        let mock = reader_with(vec![]);

        // Missing 'text' → error naming the field.
        let err = run_recall(&mock, &ctx, json!({ "max_chunks": 2 }))
            .await
            .unwrap_err();
        assert!(err.contains("text"), "got: {err}");

        // Malformed 'as_of' is an error, never a silent present-state fall-through.
        let err = run_recall(
            &mock,
            &ctx,
            json!({ "text": "q", "as_of": "not-a-timestamp" }),
        )
        .await
        .unwrap_err();
        assert!(err.contains("as_of"), "got: {err}");
    }

    #[tokio::test]
    async fn recall_accepts_a_valid_as_of_timestamp() {
        let ctx = host_ctx();
        let mock = reader_with(vec![]);
        run_recall(
            &mock,
            &ctx,
            json!({ "text": "q", "as_of": "2026-01-02T03:04:05Z" }),
        )
        .await
        .unwrap();
        let (q, _budget) = captured(&mock);
        assert!(q.as_of.is_some(), "valid RFC3339 as_of must route as-of-T");
    }

    // ── Writer bridge: id lists ─────────────────────────────────────────────

    #[test]
    fn mem_ids_treats_an_absent_key_as_empty() {
        assert_eq!(mem_ids(&json!({"text": "x"}), "evidence"), Ok(vec![]));
        assert_eq!(mem_ids(&json!({"evidence": null}), "evidence"), Ok(vec![]));
        assert_eq!(
            mem_ids(&json!({"evidence": ["mem/ep/1"]}), "evidence"),
            Ok(vec![MemId::new("mem/ep/1")])
        );
    }

    #[test]
    fn mem_ids_rejects_a_non_array() {
        let e = mem_ids(&json!({"evidence": "mem/ep/1"}), "evidence").unwrap_err();
        assert!(e.contains("'evidence'"), "{e}");
    }

    #[test]
    fn mem_ids_rejects_a_non_string_entry() {
        let e = mem_ids(&json!({"claims": ["mem/bel/1", 42]}), "claims").unwrap_err();
        assert!(e.contains("'claims'"), "{e}");
        assert!(e.contains("[1]"), "{e}");
    }

    /// What a [`ProbeWriter`] method received.
    #[derive(Debug, Clone, PartialEq)]
    enum Probed {
        Observe {
            event_time: DateTime,
        },
        Derive {
            method: DerivationMethod,
            confidence: Option<f64>,
        },
        Supersede {
            reason: String,
            confidence: Option<f64>,
        },
        Contest,
        Retract {
            id: MemId,
            reason: Option<String>,
        },
        RestsOn {
            resting: MemId,
            assumptions: Vec<MemId>,
        },
    }

    /// A writer that records what each write method receives; the methods the
    /// bridge never reaches panic.
    #[derive(Default)]
    struct ProbeWriter {
        evidence: std::sync::Mutex<Option<Vec<MemId>>>,
        probed: std::sync::Mutex<Option<Probed>>,
    }

    impl ProbeWriter {
        fn probed(&self) -> Option<Probed> {
            self.probed.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl EpistemicWriter for ProbeWriter {
        async fn observe(&self, _: &RunContext, ep: EpisodeDraft) -> Result<MemId, EpistemicError> {
            *self.probed.lock().unwrap() = Some(Probed::Observe {
                event_time: ep.event_time,
            });
            Ok(MemId::new("mem/ep/probe"))
        }
        async fn assert_belief(
            &self,
            _: &RunContext,
            belief: BeliefDraft,
            evidence: &[MemId],
            method: DerivationMethod,
        ) -> Result<MemId, EpistemicError> {
            *self.evidence.lock().unwrap() = Some(evidence.to_vec());
            *self.probed.lock().unwrap() = Some(Probed::Derive {
                method,
                confidence: belief.confidence,
            });
            Ok(MemId::new("mem/bel/probe"))
        }
        async fn supersede(
            &self,
            _: &RunContext,
            _: MemId,
            belief: BeliefDraft,
            reason: &str,
        ) -> Result<MemId, EpistemicError> {
            *self.probed.lock().unwrap() = Some(Probed::Supersede {
                reason: reason.to_string(),
                confidence: belief.confidence,
            });
            Ok(MemId::new("mem/bel/probe"))
        }
        async fn contest(
            &self,
            _: &RunContext,
            _: &[MemId],
        ) -> Result<gecko_extension_api::AnomalyId, EpistemicError> {
            *self.probed.lock().unwrap() = Some(Probed::Contest);
            Ok(gecko_extension_api::AnomalyId::new("mem/anom/probe"))
        }
        async fn retract(
            &self,
            _: &RunContext,
            id: MemId,
            reason: Option<&str>,
        ) -> Result<Vec<MemId>, EpistemicError> {
            *self.probed.lock().unwrap() = Some(Probed::Retract {
                id,
                reason: reason.map(str::to_string),
            });
            Ok(vec![MemId::new("mem/bel/dependent")])
        }
        async fn rests_on(
            &self,
            _: &RunContext,
            resting: MemId,
            assumptions: &[MemId],
        ) -> Result<(), EpistemicError> {
            *self.probed.lock().unwrap() = Some(Probed::RestsOn {
                resting,
                assumptions: assumptions.to_vec(),
            });
            Ok(())
        }
        async fn record_prediction(
            &self,
            _: &RunContext,
            _: MemId,
            _: f64,
        ) -> Result<(), EpistemicError> {
            unreachable!("record_prediction")
        }
        async fn resolve_prediction(
            &self,
            _: &RunContext,
            _: MemId,
            _: gecko_extension_api::Outcome,
        ) -> Result<(), EpistemicError> {
            unreachable!("resolve_prediction")
        }
    }

    #[tokio::test]
    async fn a_malformed_id_list_is_refused_before_the_writer_runs() {
        let ctx = host_ctx();
        let probe = Arc::new(ProbeWriter::default());
        let writer: Arc<dyn EpistemicWriter> = probe.clone();
        for (func, args) in [
            (MemHostFn::Derive, json!({"text": "t", "evidence": [42]})),
            (
                MemHostFn::Derive,
                json!({"text": "t", "evidence": "mem/ep/1"}),
            ),
            (MemHostFn::Contest, json!({"claims": {"a": "mem/bel/1"}})),
        ] {
            let call = bind_mem_call(&ctx, func, args.clone());
            let e = dispatch_mem_call(&writer, &ctx, call)
                .await
                .expect_err("a malformed id list is refused");
            assert!(e.contains("must be an array of id strings"), "{args}: {e}");
        }
        assert!(
            probe.evidence.lock().unwrap().is_none(),
            "the writer never ran"
        );
    }

    #[tokio::test]
    async fn derive_without_evidence_reaches_the_writer_with_none() {
        let ctx = host_ctx();
        for args in [json!({"text": "t"}), json!({"text": "t", "evidence": null})] {
            let probe = Arc::new(ProbeWriter::default());
            let writer: Arc<dyn EpistemicWriter> = probe.clone();
            let call = bind_mem_call(&ctx, MemHostFn::Derive, args.clone());
            dispatch_mem_call(&writer, &ctx, call)
                .await
                .expect("a derive without evidence succeeds");
            assert_eq!(*probe.evidence.lock().unwrap(), Some(vec![]), "{args}");
        }
    }

    // ── Payload validation ──────────────────────────────────────────────────

    /// Dispatches `args` as `func` to a fresh [`ProbeWriter`].
    async fn write_call(func: MemHostFn, args: Value) -> (Result<Value, String>, Arc<ProbeWriter>) {
        let ctx = host_ctx();
        let probe = Arc::new(ProbeWriter::default());
        let writer: Arc<dyn EpistemicWriter> = probe.clone();
        let call = bind_mem_call(&ctx, func, args);
        (dispatch_mem_call(&writer, &ctx, call).await, probe)
    }

    /// Asserts `args` is refused as `func` with an error naming `key`, before the writer ran.
    async fn assert_refused(func: MemHostFn, args: Value, key: &str) {
        let (out, probe) = write_call(func, args.clone()).await;
        let e = out.expect_err(&format!("{args} is refused"));
        assert!(e.contains(key), "{args}: {e}");
        assert_eq!(probe.probed(), None, "{args}: the writer never ran");
    }

    #[tokio::test]
    async fn rests_on_reaches_the_writer_and_echoes_its_ids() {
        let args = json!({ "resting": "mem/bel/a", "assumptions": ["mem/bel/b"] });
        let (out, probe) = write_call(MemHostFn::RestsOn, args.clone()).await;
        assert_eq!(out, Ok(args));
        assert_eq!(
            probe.probed(),
            Some(Probed::RestsOn {
                resting: MemId::new("mem/bel/a"),
                assumptions: vec![MemId::new("mem/bel/b")],
            })
        );
    }

    #[tokio::test]
    async fn rests_on_refuses_bad_fields() {
        for (args, key) in [
            (json!({ "assumptions": ["mem/bel/b"] }), "resting"),
            (json!({ "resting": "mem/bel/a" }), "assumptions"),
            (
                json!({ "resting": "mem/bel/a", "assumptions": "x" }),
                "assumptions",
            ),
            (
                json!({ "resting": "mem/bel/a", "assumptions": [1] }),
                "assumptions",
            ),
            (
                json!({ "resting": "", "assumptions": ["mem/bel/b"] }),
                "resting",
            ),
            (
                json!({ "resting": "mem/bel/a", "assumptions": [""] }),
                "assumptions",
            ),
            (
                json!({ "resting": "mem/bel/a", "assumptions": ["mem/bel/b"], "reason": "r" }),
                "reason",
            ),
        ] {
            assert_refused(MemHostFn::RestsOn, args, key).await;
        }
    }

    #[tokio::test]
    async fn a_non_object_payload_is_refused_by_both_dispatchers() {
        let (out, probe) = write_call(MemHostFn::Derive, json!("bare")).await;
        let e = out.unwrap_err();
        assert!(e.contains("must be a JSON object"), "{e}");
        assert_eq!(probe.probed(), None);

        let mock = reader_with(vec![]);
        let e = run_recall(&mock, &host_ctx(), json!("bare"))
            .await
            .unwrap_err();
        assert!(e.contains("must be a JSON object"), "{e}");
        assert!(mock.last.lock().unwrap().is_none(), "the reader never ran");
    }

    #[tokio::test]
    async fn an_unknown_key_is_refused_for_each_mem_function() {
        let mock = reader_with(vec![]);
        for key in ["max_tokens", "max_chunk"] {
            let e = run_recall(&mock, &host_ctx(), json!({"text": "q", key: 3}))
                .await
                .unwrap_err();
            assert!(e.contains(key), "{key}: {e}");
        }
        assert!(mock.last.lock().unwrap().is_none(), "the reader never ran");

        assert_refused(
            MemHostFn::Remember,
            json!({"text": "t", "eventTime": "x"}),
            "eventTime",
        )
        .await;
        assert_refused(
            MemHostFn::Derive,
            json!({"text": "t", "confidnce": 0.5}),
            "confidnce",
        )
        .await;
        assert_refused(
            MemHostFn::Supersede,
            json!({"old": "mem/bel/1", "text": "t", "why": "r"}),
            "why",
        )
        .await;
        assert_refused(
            MemHostFn::Contest,
            json!({"claims": ["mem/bel/1", "mem/bel/2"], "extra": 1}),
            "extra",
        )
        .await;
        assert_refused(
            MemHostFn::Retract,
            json!({"id": "mem/bel/1", "why": "r"}),
            "why",
        )
        .await;
    }

    #[tokio::test]
    async fn remember_refuses_an_unparseable_event_time() {
        for bad in [json!("yesterday"), json!(42)] {
            assert_refused(
                MemHostFn::Remember,
                json!({"text": "t", "event_time": bad}),
                "event_time",
            )
            .await;
        }
        let ctx = host_ctx();
        let probe = Arc::new(ProbeWriter::default());
        let writer: Arc<dyn EpistemicWriter> = probe.clone();
        let call = bind_mem_call(&ctx, MemHostFn::Remember, json!({"text": "t"}));
        dispatch_mem_call(&writer, &ctx, call).await.unwrap();
        assert_eq!(
            probe.probed(),
            Some(Probed::Observe {
                event_time: ctx.occurred_at
            })
        );
    }

    #[tokio::test]
    async fn derive_refuses_an_unknown_method_and_lists_the_accepted_ones() {
        let (out, probe) =
            write_call(MemHostFn::Derive, json!({"text": "t", "method": "guess"})).await;
        let e = out.unwrap_err();
        assert!(e.contains("method"), "{e}");
        for m in DerivationMethod::ALL {
            assert!(e.contains(m.as_str()), "{e} lists {}", m.as_str());
        }
        assert_eq!(probe.probed(), None);

        assert_refused(
            MemHostFn::Derive,
            json!({"text": "t", "method": 7}),
            "method",
        )
        .await;

        let (out, probe) = write_call(MemHostFn::Derive, json!({"text": "t"})).await;
        out.unwrap();
        assert_eq!(
            probe.probed(),
            Some(Probed::Derive {
                method: DerivationMethod::LlmSynthesis,
                confidence: None
            })
        );
    }

    #[tokio::test]
    async fn derive_and_supersede_refuse_a_non_number_confidence() {
        assert_refused(
            MemHostFn::Derive,
            json!({"text": "t", "confidence": "0.5"}),
            "confidence",
        )
        .await;
        assert_refused(
            MemHostFn::Supersede,
            json!({"old": "mem/bel/1", "text": "t", "confidence": "0.5"}),
            "confidence",
        )
        .await;
        let (out, probe) = write_call(
            MemHostFn::Supersede,
            json!({"old": "mem/bel/1", "text": "t"}),
        )
        .await;
        out.unwrap();
        assert_eq!(
            probe.probed(),
            Some(Probed::Supersede {
                reason: String::new(),
                confidence: None
            })
        );
    }

    #[tokio::test]
    async fn supersede_refuses_a_non_string_reason() {
        assert_refused(
            MemHostFn::Supersede,
            json!({"old": "mem/bel/1", "text": "t", "reason": 5}),
            "reason",
        )
        .await;
    }

    #[tokio::test]
    async fn retract_requires_an_id() {
        assert_refused(MemHostFn::Retract, json!({}), "id").await;
        assert_refused(MemHostFn::Retract, json!({"id": 5}), "id").await;
    }

    #[tokio::test]
    async fn retract_refuses_a_non_string_reason() {
        for bad in [json!(5), json!({})] {
            assert_refused(
                MemHostFn::Retract,
                json!({"id": "mem/bel/x", "reason": bad}),
                "reason",
            )
            .await;
        }
    }

    #[tokio::test]
    async fn retract_forwards_id_and_reason_and_replies_with_the_blast_radius() {
        let (out, probe) = write_call(
            MemHostFn::Retract,
            json!({"id": "mem/bel/x", "reason": "r"}),
        )
        .await;
        assert_eq!(
            out.unwrap(),
            json!({"id": "mem/bel/x", "blast_radius": ["mem/bel/dependent"]})
        );
        assert_eq!(
            probe.probed(),
            Some(Probed::Retract {
                id: MemId::new("mem/bel/x"),
                reason: Some("r".to_string()),
            })
        );
        for absent in [
            json!({"id": "mem/bel/x"}),
            json!({"id": "mem/bel/x", "reason": null}),
        ] {
            let (out, probe) = write_call(MemHostFn::Retract, absent.clone()).await;
            out.unwrap();
            assert_eq!(
                probe.probed(),
                Some(Probed::Retract {
                    id: MemId::new("mem/bel/x"),
                    reason: None,
                }),
                "{absent}"
            );
        }
    }

    #[tokio::test]
    async fn contest_requires_claims() {
        assert_refused(MemHostFn::Contest, json!({}), "claims").await;
        assert_refused(MemHostFn::Contest, json!({"claims": null}), "claims").await;
    }

    #[tokio::test]
    async fn recall_max_chunks_must_be_a_positive_integer() {
        let ctx = host_ctx();
        for (given, want) in [(json!(5.0), 5), (json!(5), 5)] {
            let mock = reader_with(vec![]);
            run_recall(&mock, &ctx, json!({"text": "q", "max_chunks": given}))
                .await
                .unwrap();
            assert_eq!(captured(&mock).1.max_chunks, want, "{given}");
        }
        for bad in [
            json!(0),
            json!(-1),
            json!(2.5),
            json!(1e20),
            json!("5"),
            json!(true),
        ] {
            let mock = reader_with(vec![]);
            let e = run_recall(&mock, &ctx, json!({"text": "q", "max_chunks": bad}))
                .await
                .unwrap_err();
            assert!(e.contains("max_chunks"), "{bad}: {e}");
            assert!(
                mock.last.lock().unwrap().is_none(),
                "{bad}: the reader never ran"
            );
        }
        let mock = reader_with(vec![]);
        run_recall(&mock, &ctx, json!({"text": "q", "max_chunks": null}))
            .await
            .unwrap();
        assert_eq!(captured(&mock).1.max_chunks, DEFAULT_RECALL_MAX_CHUNKS);
    }

    #[tokio::test]
    async fn refusals_do_not_echo_a_large_value() {
        let big = "x".repeat(1 << 20);
        let cases = [
            (MemHostFn::Derive, json!(big.clone())),
            (
                MemHostFn::Derive,
                json!({"text": "t", "confidence": big.clone()}),
            ),
            (
                MemHostFn::Derive,
                json!({"text": "t", "method": big.clone()}),
            ),
            (
                MemHostFn::Derive,
                json!({"text": "t", "evidence": [{"k": big.clone()}]}),
            ),
            (
                MemHostFn::Supersede,
                json!({"old": "o", "text": "t", "reason": {"k": big.clone()}}),
            ),
            (
                MemHostFn::Remember,
                json!({"text": "t", "event_time": [big.clone()]}),
            ),
            (MemHostFn::Contest, json!({ big.clone(): 1 })),
        ];
        for (func, args) in cases {
            let (out, _) = write_call(func, args).await;
            let e = out.unwrap_err();
            assert!(e.len() < 300, "{func:?}: a {}-byte error", e.len());
        }
        let e = run_recall(
            &reader_with(vec![]),
            &host_ctx(),
            json!({"text": "q", "max_chunks": big}),
        )
        .await
        .unwrap_err();
        assert!(e.len() < 300, "recall: a {}-byte error", e.len());
    }

    #[test]
    fn a_short_value_is_still_shown_in_a_refusal() {
        let e = opt_number(&json!({"confidence": "0.5"}), "confidence").unwrap_err();
        assert!(e.contains("\"0.5\""), "{e}");
    }
}
