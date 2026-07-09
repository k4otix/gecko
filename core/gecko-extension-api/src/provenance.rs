//! Host-minted provenance context threaded through every write-capable op.

use serde::{Deserialize, Serialize};

use crate::ids::{ActorId, ConceptId, DateTime, RunId};

/// Where a write originated. An open-ended discriminator on the run's provenance:
/// an exec-doc run, a corpus sync, an external tool call, or a manual operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProvenanceSource {
    /// A sandboxed executable-document run, bound to its authoring concept.
    ExecutableDoc { concept_id: ConceptId },
    /// A corpus/bundle sync (record path — see invariant 1/2).
    Sync { bundle: String },
    /// An external tool invocation.
    Tool { name: String },
    /// A direct human/manual operation.
    Manual,
}

/// The provenance stamp minted by the host at the entry of any write-capable op
/// and threaded through the epistemic write path.
///
/// # Security invariant (invariant 2)
/// **Sandbox code never constructs its own `RunContext`.** The engine mints it —
/// binding [`run_id`](RunContext::run_id) to the exec-doc concept + run — and
/// injects it into host-function calls. Sandbox code supplies only the payload
/// args; it cannot forge or omit the `run_id`, `actor`, or `source`. This is what
/// makes [`EpistemicWriter`](crate::EpistemicWriter) the *exclusive*,
/// attributable belief-write path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunContext {
    /// ULID; one per exec-doc run / sync / tool-call / manual op.
    pub run_id: RunId,
    /// The authoring actor: agent | human | system.
    pub actor: ActorId,
    /// What kind of op this run is, and what it is bound to.
    pub source: ProvenanceSource,
    /// Ingest-time anchor for this run.
    pub occurred_at: DateTime,
}

impl RunContext {
    /// Convenience constructor for host code minting a run.
    pub fn new(actor: ActorId, source: ProvenanceSource, occurred_at: DateTime) -> Self {
        Self {
            run_id: RunId::new(),
            actor,
            source,
            occurred_at,
        }
    }
}
