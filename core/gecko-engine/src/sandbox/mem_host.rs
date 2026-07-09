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

use gecko_extension_api::{ActorId, ProvenanceSource, RunContext, RunId};
use serde_json::Value;

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
        payload: args,
    }
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
