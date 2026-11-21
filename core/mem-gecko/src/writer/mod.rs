//! The mem-gecko [`EpistemicWriter`](gecko_extension_api::EpistemicWriter) +
//! [`EpistemicReader`](gecko_extension_api::EpistemicReader) implementation.
//!
//! This is the load-bearing substrate-reasoning piece. Writes are **real TypeDB
//! writes** through the neutral [`GraphStore`] seam (mem owns its TQL; the engine
//! owns the driver). The graph write is authoritative and commits *first*;
//! embedding + index upsert (and remove-on-supersede) is **best-effort after
//! commit** — any embed/upsert/remove failure calls [`MemWriter::mark_dirty`] and
//! **never** rolls back or fails the graph write (invariant 8).
//!
//! Reads invoke the persisted substrate functions (`is-superseded`, `believed-at`,
//! `derivation-chain`, `gate`, `retrieval-score`, …) from read transactions and
//! layer the recency/ACT-R decay that TypeQL cannot express **here, in the Rust
//! Reader**. [`recall`](gecko_extension_api::EpistemicReader::recall) is index-seeded: the ANN only
//! *generates candidates*; every candidate is fetched authoritatively and **gated**
//! so a superseded/out-of-scope id can never leak (invariant 8, the crux).
//!
//! The implementation is split by concern: `index` (dirty set, rebuild, index
//! hooks), `reader` (persisted-function reads and recall), `beliefs` (the writer
//! trait), `edges` (the per-extension belief-edge writer), `populations` (the
//! populations writer) and `consolidation`.
//! Query text lives in the crate's `tql` module.

mod beliefs;
mod consolidation;
mod edges;
mod index;
mod populations;
mod reader;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub use consolidation::DuplicateCandidate;
pub use index::IndexOutcome;

use gecko_extension_api::{
    BeliefEdgeWriter, ConceptId, DateTime, Embedder, EpistemicError, GraphStore, MemId,
    SemanticIndex,
};
use serde_json::Value;

use crate::belief_edge::BeliefEdgeRegistry;

type Result<T> = std::result::Result<T, EpistemicError>;

/// Mints a fresh mem-id under `prefix` (e.g. `mem/ep`, `mem/bel`, `mem/anom`).
fn mint(prefix: &str) -> MemId {
    MemId(format!("{prefix}/{}", ulid::Ulid::generate()))
}

/// mem-gecko's epistemic writer + reader.
///
/// Holds the [`GraphStore`] (the sole production coupling to a graph backend) plus
/// an optional [`Embedder`] + [`SemanticIndex`]: when either is `None` the
/// embed/upsert path is skipped and `recall` uses the non-vector fallback. The
/// dirty set records concept-ids whose index vector is stale and awaits background
/// reindex.
pub struct MemWriter {
    graph: Arc<dyn GraphStore>,
    embedder: Option<Arc<dyn Embedder>>,
    index: Option<Arc<dyn SemanticIndex>>,
    dirty: Mutex<HashSet<ConceptId>>,
    /// When on, `recall` stamps a `retrieval-event` + `surfaced` edges and
    /// `assert_belief` links the synthesized belief back to the retrieval(s) whose
    /// gated candidates overlap its evidence. Off ⇒ beliefs simply lack the
    /// retrieval axis and no retrieval-events are minted (no-drag preserved).
    record_retrieval_provenance: bool,
    /// One lock per population `spec-hash`, held across a populations-writer
    /// method's read, plan and write so passes from this writer never interleave.
    population_locks: Mutex<HashMap<String, Arc<futures_util::lock::Mutex<()>>>>,
    /// The belief-edge relations each extension may write through its bound handle.
    belief_edges: BeliefEdgeRegistry,
    /// Relations whose declaration has passed the live-schema check against the
    /// database.
    checked_relations: Mutex<HashSet<String>>,
}

impl MemWriter {
    /// Constructs a writer over the given graph seam and optional index stack.
    pub fn new(
        graph: Arc<dyn GraphStore>,
        embedder: Option<Arc<dyn Embedder>>,
        index: Option<Arc<dyn SemanticIndex>>,
    ) -> Self {
        Self {
            graph,
            embedder,
            index,
            dirty: Mutex::new(HashSet::new()),
            record_retrieval_provenance: false,
            population_locks: Mutex::new(HashMap::new()),
            belief_edges: BeliefEdgeRegistry::default(),
            checked_relations: Mutex::new(HashSet::new()),
        }
    }

    /// A writer over `graph` with no index configured (index disabled).
    pub fn without_index(graph: Arc<dyn GraphStore>) -> Self {
        Self::new(graph, None, None)
    }

    /// Enables/disables the retrieval-provenance write path (default off).
    /// Builder-style so `gecko-bin` can flip it on when the index + config request
    /// it, before wrapping the writer in an `Arc`.
    pub fn with_retrieval_provenance(mut self, on: bool) -> Self {
        self.record_retrieval_provenance = on;
        self
    }

    /// Sets the belief-edge registry the bound edge writers check requests against.
    pub fn with_belief_edges(mut self, registry: BeliefEdgeRegistry) -> Self {
        self.belief_edges = registry;
        self
    }

    /// A belief-edge writer bound to `extension`: it writes only the relations the
    /// registry records as owned by that extension. Fails when the extension owns none.
    pub fn edge_writer_for(self: &Arc<Self>, extension: &str) -> Result<Arc<dyn BeliefEdgeWriter>> {
        if !self.belief_edges.declares_any(extension) {
            return Err(EpistemicError::InvalidInput(format!(
                "extension `{extension}` declares no belief edges"
            )));
        }
        Ok(Arc::new(edges::BoundEdgeWriter::new(
            self.clone(),
            extension,
        )))
    }
}

// ── JSON extraction + decay helpers (the Rust Reader's math) ─────────────────

fn str_field(d: &Value, k: &str) -> String {
    d.get(k)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn f64_field(d: &Value, k: &str) -> Option<f64> {
    d.get(k).and_then(|v| v.as_f64())
}

fn dt_field(d: &Value, k: &str) -> Option<DateTime> {
    d.get(k).and_then(|v| v.as_str()).and_then(parse_dt)
}

/// Parses a datetime from a TypeDB fetch result (ISO, with or without fractional
/// seconds / offset), normalising to UTC.
fn parse_dt(s: &str) -> Option<DateTime> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(chrono::DateTime::from_naive_utc_and_offset(
                ndt,
                chrono::Utc,
            ));
        }
    }
    None
}

/// Collects the `"id"` field from each doc, de-duplicated, order-preserving. Borrows
/// each id as `&str` for the seen-set and allocates only when pushing a new id.
fn dedup_ids(docs: &[Value]) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for d in docs {
        let id = d.get("id").and_then(|v| v.as_str()).unwrap_or_default();
        if !id.is_empty() && seen.insert(id) {
            out.push(id.to_string());
        }
    }
    out
}

/// Reads `docs`' de-duplicated `"id"` fields and wraps each with `f` (the newtype
/// constructor). Factors the `dedup_ids(&docs).into_iter().map(..).collect()` tail
/// shared by the persisted-function readers.
fn dedup_mapped<T>(docs: &[Value], f: impl Fn(String) -> T) -> Vec<T> {
    dedup_ids(docs).into_iter().map(f).collect()
}
