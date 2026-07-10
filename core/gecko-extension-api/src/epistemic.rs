//! The host-mediated epistemic write/read contract.
//!
//! [`EpistemicWriter`] is the **exclusive** belief-write path (invariant 2): every
//! method takes a host-minted [`RunContext`], so every belief write is
//! run-id-stamped and attributable. mem-gecko implements these traits; the engine
//! hands mem's impls to sandbox host-fn injection. Signatures here are the stable
//! contract — graph-coupled bodies may return [`EpistemicError::NotYetImplemented`]
//! until mem-gecko implements them.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::EpistemicError;
use crate::ids::{DateTime, MemId};
use crate::provenance::RunContext;
use crate::semantic::{Entrenchment, Visibility};

// Module-local alias so the trait signatures read exactly like the plan.
type Result<T> = std::result::Result<T, EpistemicError>;

/// How a belief was derived. Governs the provable/probabilistic grading of
/// invariant 7 and mirrors the schema's `derivation-method @values(...)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DerivationMethod {
    /// Type-enforced join. **Provable.**
    TypeJoin,
    /// Cardinality constraint. **Provable.**
    Cardinality,
    /// Functional dependency. **Provable.**
    FunctionalDependency,
    /// A persisted TypeDB function. Varies.
    TypeDbFunction,
    /// An external tool result. Varies.
    ExternalTool,
    /// A human assertion. Varies.
    HumanAssertion,
    /// LLM synthesis. **Probabilistic** (confidence < 1.0, cannot supersede
    /// higher-entrenchment beliefs).
    LlmSynthesis,
}

impl DerivationMethod {
    /// The canonical kebab-case string — the contract the mem schema's
    /// `derivation-method @values(...)` mirrors.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TypeJoin => "type-join",
            Self::Cardinality => "cardinality",
            Self::FunctionalDependency => "functional-dependency",
            Self::TypeDbFunction => "type-db-function",
            Self::ExternalTool => "external-tool",
            Self::HumanAssertion => "human-assertion",
            Self::LlmSynthesis => "llm-synthesis",
        }
    }

    /// Parses the canonical kebab-case string. `None` on an unknown value.
    // Intentionally an inherent method, not `std::str::FromStr` (no `Err` type
    // is warranted — unknown values are just `None`).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "type-join" => Self::TypeJoin,
            "cardinality" => Self::Cardinality,
            "functional-dependency" => Self::FunctionalDependency,
            "type-db-function" => Self::TypeDbFunction,
            "external-tool" => Self::ExternalTool,
            "human-assertion" => Self::HumanAssertion,
            "llm-synthesis" => Self::LlmSynthesis,
            _ => return None,
        })
    }

    /// `true` for the type-enforced derivations that may carry confidence 1.0
    /// (invariant 7): [`TypeJoin`](Self::TypeJoin),
    /// [`Cardinality`](Self::Cardinality),
    /// [`FunctionalDependency`](Self::FunctionalDependency).
    pub fn provable(&self) -> bool {
        matches!(
            self,
            Self::TypeJoin | Self::Cardinality | Self::FunctionalDependency
        )
    }

    /// `true` for derivations structurally forbidden from confidence 1.0 —
    /// currently [`LlmSynthesis`](Self::LlmSynthesis).
    pub fn probabilistic(&self) -> bool {
        matches!(self, Self::LlmSynthesis)
    }
}

/// A draft episode (episodic tier): an observation to be recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpisodeDraft {
    /// The observation text.
    pub text: String,
    /// When the observed event actually occurred (bitemporal event-time).
    pub event_time: DateTime,
    /// When it entered the system (bitemporal ingest-time).
    pub ingest_time: DateTime,
}

/// A draft belief (belief tier): a contested/derived/revisable claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BeliefDraft {
    /// The claim text (also the text the index embeds).
    pub text: String,
    /// Owning actor.
    pub owner: crate::ids::ActorId,
    /// Visibility scope.
    pub visibility: Visibility,
    /// Optional confidence in `[0.0, 1.0]`; provable methods may reach 1.0.
    pub confidence: Option<f64>,
    /// Optional explicit entrenchment tier. Governs the invariant-7 guard in
    /// [`supersede`](EpistemicWriter::supersede): when set, it is the new belief's
    /// entrenchment (and the supersede is rejected if it ranks *below* the belief it
    /// would replace). When `None`, `supersede` inherits the old belief's tier (a
    /// revision preserves entrenchment) and `assert_belief` derives it from the
    /// [`DerivationMethod`].
    #[serde(default)]
    pub entrenchment: Option<Entrenchment>,
}

/// The resolved outcome of a recorded prediction (calibration flywheel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// The prediction held.
    Confirmed,
    /// The prediction was refuted.
    Refuted,
    /// The prediction could not be resolved either way.
    Inconclusive,
}

/// A recall request against the epistemic reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecallQuery {
    /// The natural-language query text (embedded for the present-state path).
    pub text: String,
    /// When set, recall is an **as-of-time-T** query: it routes through the
    /// temporal path ([`believed_at`](EpistemicReader::believed_at)) and **skips
    /// the semantic index entirely** (invariant 8 — the index is present-state
    /// only). `None` is the ordinary present-state recall.
    #[serde(default)]
    pub as_of: Option<DateTime>,
}

impl RecallQuery {
    /// A present-state recall over `text` (no as-of constraint).
    pub fn now(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            as_of: None,
        }
    }
}

/// A budget bounding how much context recall may return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBudget {
    /// Maximum number of chunks to return.
    pub max_chunks: usize,
    /// Optional token ceiling across returned chunks.
    pub max_tokens: Option<usize>,
}

/// A unit of recalled context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    /// The source memory node.
    pub id: MemId,
    /// The recalled text.
    pub text: String,
    /// Retrieval similarity score (episodic provenance, never a derivation).
    pub score: f32,
}

/// A query for beliefs as-of a point in time (temporal path; never the index).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeliefQuery {
    /// The natural-language query text.
    pub text: String,
}

/// The exclusive, host-mediated belief-write path.
///
/// Every method is stamped by the host-minted [`RunContext`]; sandbox code cannot
/// forge or omit provenance (invariant 2).
#[async_trait]
pub trait EpistemicWriter: Send + Sync {
    /// Records an episode in the episodic tier.
    async fn observe(&self, ctx: &RunContext, ep: EpisodeDraft) -> Result<MemId>;

    /// Asserts a new belief backed by `evidence`, derived via `method`.
    async fn assert_belief(
        &self,
        ctx: &RunContext,
        b: BeliefDraft,
        evidence: &[MemId],
        method: DerivationMethod,
    ) -> Result<MemId>;

    /// Supersedes `old` with a new belief, recording the lineage + `reason`.
    async fn supersede(
        &self,
        ctx: &RunContext,
        old: MemId,
        new: BeliefDraft,
        reason: &str,
    ) -> Result<MemId>;

    /// Records a contradiction among `claims`, minting an anomaly.
    async fn contest(&self, ctx: &RunContext, claims: &[MemId]) -> Result<crate::ids::AnomalyId>;

    /// Records that `resting` depends on `assumptions` (assumption graph).
    async fn rests_on(&self, ctx: &RunContext, resting: MemId, assumptions: &[MemId])
    -> Result<()>;

    /// Records a probability `p` predicted for belief `b`.
    async fn record_prediction(&self, ctx: &RunContext, b: MemId, p: f64) -> Result<()>;

    /// Resolves a previously recorded prediction on `b`.
    async fn resolve_prediction(&self, ctx: &RunContext, b: MemId, outcome: Outcome) -> Result<()>;

    /// If this writer is ALSO an [`EpistemicReader`] (mem's `MemWriter` is), returns
    /// it as one so the host can wire the sandbox **reader** bridge (`mem.recall`)
    /// from the same object and the same shared per-run scratch — this is what lets
    /// a recall and a later `assert_belief` in one run correlate. Write-only
    /// writers keep the default `None`, and `mem.recall` stays unavailable for them.
    fn as_epistemic_reader(self: Arc<Self>) -> Option<Arc<dyn EpistemicReader>> {
        None
    }
}

/// The read side of the epistemic substrate.
#[async_trait]
pub trait EpistemicReader: Send + Sync {
    /// Retrieves relevant context for `q` within `budget` (present-state; may use
    /// the index).
    async fn recall(
        &self,
        ctx: &RunContext,
        q: RecallQuery,
        budget: ContextBudget,
    ) -> Result<Vec<Chunk>>;

    /// Walks the derivation chain backing belief `b`.
    async fn derivation_chain(&self, b: MemId) -> Result<Vec<MemId>>;

    /// Beliefs believed as-of `at` — routes through TypeDB's temporal path, never
    /// the index (invariant 8).
    async fn believed_at(&self, at: DateTime, q: BeliefQuery) -> Result<Vec<MemId>>;

    /// The `rests-on` propagation set for a retracted belief.
    async fn blast_radius(&self, retracted: MemId) -> Result<Vec<MemId>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivation_method_roundtrips_all_seven() {
        let all = [
            DerivationMethod::TypeJoin,
            DerivationMethod::Cardinality,
            DerivationMethod::FunctionalDependency,
            DerivationMethod::TypeDbFunction,
            DerivationMethod::ExternalTool,
            DerivationMethod::HumanAssertion,
            DerivationMethod::LlmSynthesis,
        ];
        assert_eq!(all.len(), 7);
        for m in all {
            assert_eq!(DerivationMethod::from_str(m.as_str()), Some(m));
        }
        assert_eq!(DerivationMethod::from_str("no-such-method"), None);
    }

    #[test]
    fn derivation_method_strings_are_kebab_contract() {
        assert_eq!(DerivationMethod::TypeJoin.as_str(), "type-join");
        assert_eq!(
            DerivationMethod::FunctionalDependency.as_str(),
            "functional-dependency"
        );
        assert_eq!(
            DerivationMethod::TypeDbFunction.as_str(),
            "type-db-function"
        );
        assert_eq!(DerivationMethod::LlmSynthesis.as_str(), "llm-synthesis");
    }

    #[test]
    fn provable_classifier_matches_invariant_7() {
        assert!(DerivationMethod::TypeJoin.provable());
        assert!(DerivationMethod::Cardinality.provable());
        assert!(DerivationMethod::FunctionalDependency.provable());
        assert!(!DerivationMethod::LlmSynthesis.provable());
        assert!(!DerivationMethod::HumanAssertion.provable());
    }

    #[test]
    fn probabilistic_classifier_flags_llm_synthesis() {
        assert!(DerivationMethod::LlmSynthesis.probabilistic());
        assert!(!DerivationMethod::TypeJoin.probabilistic());
    }
}
