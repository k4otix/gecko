//! Sandbox → mem host-function surface and run-id auto-binding (plan A1.6).
//!
//! Exec-docs call a small set of mem host functions —
//! `remember`/`recall`/`derive`/`supersede`/`contest` — which the host delegates
//! to the injected [`EpistemicWriter`]/[`EpistemicReader`]. The security spine
//! (invariant 2) is that the **host** binds the run's [`RunContext`]: sandbox code
//! supplies only the payload args and **cannot** supply or override the
//! `run_id`/`actor`/`source`. This module implements that binding mechanism and
//! its forgery resistance.
//!
//! Both bridges are host-side wired: the write ops route to [`dispatch_mem_call`]
//! and `recall` routes to [`dispatch_mem_recall`] (an [`EpistemicReader`], shared
//! per-run scratch → A5.7). The guest binding that lets exec-doc JS call these via a
//! `mem.*` global now ships (`core/js-sandbox/src/main.rs`, compiled into the
//! committed `quickjs.wasm`), and the live end-to-end path — `mem.recall` → later
//! `mem.derive`, correlated by A5.7 — is proven in
//! `gecko-bin/tests/mem_recall_e2e_test.rs`.
//!
//! [`EpistemicWriter`]: gecko_extension_api::EpistemicWriter
//! [`EpistemicReader`]: gecko_extension_api::EpistemicReader

use std::sync::Arc;

use gecko_extension_api::{
    ActorId, BeliefDraft, ContextBudget, DateTime, DerivationMethod, EpisodeDraft, EpistemicReader,
    EpistemicWriter, MemId, ProvenanceSource, RecallQuery, RunContext, RunId, Visibility,
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
            let id = writer.observe(ctx, ep).await.map_err(err)?;
            Ok(json!({ "id": id.0 }))
        }
        MemHostFn::Derive => {
            let belief = BeliefDraft {
                text: req_str(p, "text")?,
                owner: ctx.actor.clone(),
                visibility: Visibility::Private,
                confidence: p.get("confidence").and_then(Value::as_f64),
                // Entrenchment for a fresh assertion is derived from the method
                // (see `entrenchment_for`); the sandbox does not set it here.
                entrenchment: None,
            };
            let evidence = mem_ids(p, "evidence");
            let method = p
                .get("method")
                .and_then(Value::as_str)
                .and_then(DerivationMethod::from_str)
                .unwrap_or(DerivationMethod::LlmSynthesis);
            let id = writer
                .assert_belief(ctx, belief, &evidence, method)
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
                // None ⇒ supersede inherits the old belief's entrenchment tier and
                // enforces the invariant-7 guard against a downgrade.
                entrenchment: None,
            };
            let reason = p.get("reason").and_then(Value::as_str).unwrap_or("");
            let id = writer
                .supersede(ctx, old, belief, reason)
                .await
                .map_err(err)?;
            Ok(json!({ "id": id.0 }))
        }
        MemHostFn::Contest => {
            let claims = mem_ids(p, "claims");
            let anomaly = writer.contest(ctx, &claims).await.map_err(err)?;
            Ok(json!({ "anomaly": anomaly.0 }))
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
/// The reader shares the SAME per-run [`RunContext`] as the write bridge, so a
/// recall stamps its `retrieval-event` into `ctx.scratch` and a later
/// `assert_belief` in the run reads it back — that shared scratch is the A5.7
/// retrieval-provenance correlation seam. The sandbox supplies only the query
/// payload; `run_id`/`actor`/`source` are the host's (already stripped + stamped by
/// [`bind_mem_call`]). Returns `{ "chunks": [{ id, text, score }, …] }`.
pub async fn dispatch_mem_recall(
    reader: &Arc<dyn EpistemicReader>,
    ctx: &RunContext,
    call: StampedMemCall,
) -> Result<Value, String> {
    let p = &call.payload;

    let text = req_str(p, "text")?;

    // `as_of`, when present, MUST be a valid RFC3339 timestamp — a malformed value
    // is an error, never a silent fall-through to present-state recall.
    let as_of = match p.get("as_of") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| "mem.recall 'as_of' must be an RFC3339 string".to_string())?;
            let dt = chrono::DateTime::parse_from_rfc3339(s)
                .map_err(|e| format!("mem.recall 'as_of' is not valid RFC3339: {e}"))?;
            Some(dt.with_timezone(&chrono::Utc))
        }
    };

    let budget = ContextBudget {
        max_chunks: p
            .get("max_chunks")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_RECALL_MAX_CHUNKS),
        max_tokens: p
            .get("max_tokens")
            .and_then(Value::as_u64)
            .map(|n| n as usize),
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
                return tokio::task::block_in_place(|| {
                    handle.block_on(async {
                        match reader.as_ref() {
                            Some(r) => dispatch_mem_recall(r, &ctx, call).await,
                            None => Err("mem.recall is unavailable: no epistemic \
                                         reader is wired for this run"
                                .to_string()),
                        }
                    })
                });
            }
            let writer = writer.clone();
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

    // ── Reader bridge (mem.recall) ──────────────────────────────────────────

    use gecko_extension_api::{BeliefQuery, Chunk, EpistemicError, EpistemicReader};

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
            _at: DateTime,
            _q: BeliefQuery,
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
}
