//! Host-minted provenance context threaded through every write-capable op.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::ids::{ActorId, ConceptId, DateTime, MemId, RunId};

/// Per-run correlation state for the retrieval-provenance write path (A5.7).
///
/// `recall` pushes each retrieval-event id together with the set of **gated
/// candidate** concept-ids it surfaced; a later `assert_belief` in the *same run*
/// correlates by evidence overlap to link the synthesized belief back to the
/// retrieval(s) that fed it. The host infers the link — the agent/exec-doc carries
/// no bookkeeping.
#[derive(Debug, Default)]
pub struct RunScratch {
    /// `(retrieval-event mem-id, gated candidate concept-ids)` recorded this run.
    pub retrievals: Vec<(MemId, HashSet<ConceptId>)>,
}

/// Interior-mutable, shared-per-run [`RunScratch`]. Cloning a [`RunContext`] shares
/// the same scratch so a recall and a later assert in one run see the same ledger.
pub type SharedScratch = Arc<Mutex<RunScratch>>;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunContext {
    /// ULID; one per exec-doc run / sync / tool-call / manual op.
    pub run_id: RunId,
    /// The authoring actor: agent | human | system.
    pub actor: ActorId,
    /// What kind of op this run is, and what it is bound to.
    pub source: ProvenanceSource,
    /// Ingest-time anchor for this run.
    pub occurred_at: DateTime,
    /// Per-run retrieval-provenance correlation state (A5.7). Interior-mutable and
    /// shared across clones of this context; **never serialized** (it is transient
    /// run bookkeeping, not persisted provenance).
    #[serde(skip)]
    pub scratch: SharedScratch,
}

impl RunContext {
    /// Convenience constructor for host code minting a run.
    pub fn new(actor: ActorId, source: ProvenanceSource, occurred_at: DateTime) -> Self {
        Self {
            run_id: RunId::new(),
            actor,
            source,
            occurred_at,
            scratch: SharedScratch::default(),
        }
    }

    /// Records that retrieval-event `ev` surfaced the given gated `candidates`
    /// (A5.7). Called by `recall` when retrieval-provenance recording is on.
    pub fn push_retrieval(&self, ev: MemId, candidates: HashSet<ConceptId>) {
        self.scratch
            .lock()
            .expect("run scratch mutex poisoned")
            .retrievals
            .push((ev, candidates));
    }

    /// Correlates this run's recorded retrievals against a belief's `evidence`
    /// (A5.7). Returns, for each retrieval whose gated candidates intersect the
    /// evidence, the retrieval-event id and the overlapping evidence ids.
    pub fn matched_retrievals(&self, evidence: &[MemId]) -> Vec<(MemId, Vec<MemId>)> {
        let scratch = self.scratch.lock().expect("run scratch mutex poisoned");
        let mut out = Vec::new();
        for (ev, cands) in &scratch.retrievals {
            let overlap: Vec<MemId> = evidence
                .iter()
                .filter(|e| cands.contains(&ConceptId(e.0.clone())))
                .cloned()
                .collect();
            if !overlap.is_empty() {
                out.push((ev.clone(), overlap));
            }
        }
        out
    }
}

/// Two run contexts are equal on their **stamped** fields; the transient scratch
/// ledger is excluded (it is run-local bookkeeping, not identity).
impl PartialEq for RunContext {
    fn eq(&self, other: &Self) -> bool {
        self.run_id == other.run_id
            && self.actor == other.actor
            && self.source == other.source
            && self.occurred_at == other.occurred_at
    }
}
impl Eq for RunContext {}
