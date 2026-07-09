//! The activation-time sandbox context (plan A4.1).
//!
//! Registration is the activation gate (invariant 3): `build_extensions` constructs
//! only the enabled extensions, and the Assembler calls
//! [`GeckoExtension::inject_epistemic_host_fns`](crate::GeckoExtension::inject_epistemic_host_fns)
//! on each. That hook is handed a host-constructed [`EpistemicWriter`]; an epistemic
//! extension (the `mem` substrate) binds it into this [`SandboxCtx`] so the engine's
//! sandbox host-call bridge can route the mem host fns
//! (`remember`/`recall`/`derive`/`supersede`/`contest`) to it — each stamped with a
//! host-minted [`RunContext`](crate::RunContext) the sandbox can neither forge nor
//! omit (invariant 2).
//!
//! There is **no second trait**: a disabled extension's hook is simply never called,
//! so activation gating falls out of registration. Non-epistemic extensions inherit
//! the defaulted no-op hook and leave this context untouched.

use std::sync::Arc;

use crate::epistemic::EpistemicWriter;

/// Mutable context threaded to each enabled extension during activation.
///
/// mem stores the injected writer here via
/// [`set_epistemic_writer`](SandboxCtx::set_epistemic_writer); the engine then reads
/// it back with [`epistemic_writer`](SandboxCtx::epistemic_writer) to build the mem
/// host-call bridge. When no epistemic extension is activated, the writer slot stays
/// `None` and no belief-write path is opened.
#[derive(Clone, Default)]
pub struct SandboxCtx {
    writer: Option<Arc<dyn EpistemicWriter>>,
}

impl SandboxCtx {
    /// A fresh context with no epistemic writer bound.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind the epistemic writer that the mem host fns dispatch to. Called by an
    /// epistemic extension's [`inject_epistemic_host_fns`] override.
    ///
    /// [`inject_epistemic_host_fns`]: crate::GeckoExtension::inject_epistemic_host_fns
    pub fn set_epistemic_writer(&mut self, writer: Arc<dyn EpistemicWriter>) {
        self.writer = Some(writer);
    }

    /// The bound epistemic writer, if an epistemic extension was activated. The
    /// engine's sandbox bridge routes mem host fns here; `None` means the mem
    /// host-fn surface is closed for this run.
    pub fn epistemic_writer(&self) -> Option<Arc<dyn EpistemicWriter>> {
        self.writer.clone()
    }

    /// Whether an epistemic writer has been injected.
    pub fn has_epistemic_writer(&self) -> bool {
        self.writer.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epistemic::{BeliefDraft, DerivationMethod, EpisodeDraft, EpistemicWriter, Outcome};
    use crate::error::EpistemicError;
    use crate::ids::{AnomalyId, MemId};
    use crate::provenance::RunContext;
    use async_trait::async_trait;

    type R<T> = std::result::Result<T, EpistemicError>;

    struct DummyWriter;

    #[async_trait]
    impl EpistemicWriter for DummyWriter {
        async fn observe(&self, _c: &RunContext, _e: EpisodeDraft) -> R<MemId> {
            Ok(MemId::new("mem/ep/dummy"))
        }
        async fn assert_belief(
            &self,
            _c: &RunContext,
            _b: BeliefDraft,
            _e: &[MemId],
            _m: DerivationMethod,
        ) -> R<MemId> {
            Ok(MemId::new("mem/bel/dummy"))
        }
        async fn supersede(
            &self,
            _c: &RunContext,
            _o: MemId,
            _n: BeliefDraft,
            _r: &str,
        ) -> R<MemId> {
            Ok(MemId::new("mem/bel/dummy"))
        }
        async fn contest(&self, _c: &RunContext, _claims: &[MemId]) -> R<AnomalyId> {
            Ok(AnomalyId("mem/anom/dummy".into()))
        }
        async fn rests_on(&self, _c: &RunContext, _r: MemId, _a: &[MemId]) -> R<()> {
            Ok(())
        }
        async fn record_prediction(&self, _c: &RunContext, _b: MemId, _p: f64) -> R<()> {
            Ok(())
        }
        async fn resolve_prediction(&self, _c: &RunContext, _b: MemId, _o: Outcome) -> R<()> {
            Ok(())
        }
    }

    #[test]
    fn writer_slot_is_empty_by_default() {
        let ctx = SandboxCtx::new();
        assert!(!ctx.has_epistemic_writer());
        assert!(ctx.epistemic_writer().is_none());
    }

    #[test]
    fn set_then_get_returns_the_writer() {
        let mut ctx = SandboxCtx::new();
        ctx.set_epistemic_writer(Arc::new(DummyWriter));
        assert!(ctx.has_epistemic_writer());
        assert!(ctx.epistemic_writer().is_some());
    }
}
