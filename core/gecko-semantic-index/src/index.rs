//! [`HnswIndex`] — the concrete HNSW retrieval accelerator, plus the
//! [`NoopIndex`] used when the accelerator is disabled.
//!
//! `HnswIndex` wraps an `hnsw_rs` ANN graph over **L2-normalized** vectors under a
//! cosine distance, denormalizes a coarse [`FilterMeta`] per id for cheap
//! pre-filtering, and persists a file-serialized snapshot (a header `{model_id,
//! dim}` + the live entries) so a warm start skips the rebuild. It is a **pure,
//! rebuildable accelerator** (invariant 8): a missing/corrupt file or a model-id
//! bump opens empty, and the writer's startup reconciliation refills it from the
//! graph.
//!
//! `hnsw_rs` has no in-place update or delete, so this wraps it with a live-id map:
//! an `upsert` of an existing concept inserts a fresh internal id and drops the old
//! one from the live set (the stale graph node becomes an inert tombstone), and
//! `remove` drops the concept from the live set. Stale internal ids surfaced by the
//! ANN search are filtered out at query time; `reset` and `remove_many` reclaim them
//! by building a clean graph.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use gecko_extension_api::{ConceptId, EpistemicError, FilterMeta, SemanticIndex, Visibility};
use hnsw_rs::prelude::{DistCosine, Hnsw, Neighbour};
use serde::{Deserialize, Serialize};

use crate::params;

type Result<T> = std::result::Result<T, EpistemicError>;

/// Distinguishes the temp files of successive persists within one process.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// One live vector + its denormalized pre-filter.
struct LiveEntry {
    concept: ConceptId,
    meta: FilterMeta,
    vector: Vec<f32>,
}

/// The persisted snapshot: the header the rebuild path checks, plus every live
/// entry (sorted by concept-id so any reconstruction replays in an identical order
/// and yields an identical ANN graph → identical recall).
#[derive(Serialize, Deserialize)]
struct Snapshot {
    model_id: String,
    dim: usize,
    entries: Vec<SnapEntry>,
}

#[derive(Serialize, Deserialize)]
struct SnapEntry {
    concept_id: ConceptId,
    meta: FilterMeta,
    vector: Vec<f32>,
}

/// The mutable interior of an [`HnswIndex`], guarded by a single lock.
struct Inner {
    hnsw: Hnsw<'static, f32, DistCosine>,
    dim: usize,
    /// concept-id → its current live internal id.
    active: HashMap<ConceptId, usize>,
    /// live internal id → entry (stale/tombstoned ids are absent).
    entries: HashMap<usize, LiveEntry>,
    /// Monotonic internal-id allocator (never reused, so tombstones stay inert).
    next_id: usize,
}

impl Inner {
    fn fresh_hnsw() -> Hnsw<'static, f32, DistCosine> {
        Hnsw::new(
            params::M,
            params::MAX_ELEMENTS_HINT,
            params::MAX_LAYER,
            params::EF_CONSTRUCTION,
            DistCosine {},
        )
    }

    fn empty(dim: usize) -> Self {
        Self {
            hnsw: Self::fresh_hnsw(),
            dim,
            active: HashMap::new(),
            entries: HashMap::new(),
            next_id: 0,
        }
    }

    /// Inserts a vector under a freshly minted internal id, tombstoning any prior
    /// live id for the same concept.
    fn insert_entry(&mut self, concept: ConceptId, meta: FilterMeta, vector: Vec<f32>) {
        if let Some(old) = self.active.remove(&concept) {
            self.entries.remove(&old); // tombstone the superseded internal id
        }
        let id = self.next_id;
        self.next_id += 1;
        self.hnsw.insert((vector.as_slice(), id));
        self.active.insert(concept.clone(), id);
        self.entries.insert(
            id,
            LiveEntry {
                concept,
                meta,
                vector,
            },
        );
    }
}

/// A native-Rust HNSW semantic index.
pub struct HnswIndex {
    inner: RwLock<Inner>,
    /// Immutable after construction — the embedder identity the index is bound to.
    /// Cached on the struct (not behind the lock) so `model_id(&self) -> &str` can
    /// hand back a borrow.
    model_id: String,
    path: PathBuf,
    /// Whether `set_searching_mode(true)` has already been toggled on the current
    /// `Inner.hnsw`. `hnsw_rs::Hnsw::search` takes `&self`, so once this is set,
    /// concurrent queries only need a READ lock; only the one-time toggle (which
    /// needs `&mut self`) takes the WRITE lock. Reset alongside `Inner` (see
    /// [`reset`](SemanticIndex::reset)) since a freshly built `Hnsw` starts with
    /// searching mode unset again.
    searching_mode_set: AtomicBool,
}

impl HnswIndex {
    /// Opens (or cold-starts) an index at `path` for an embedder identified by
    /// `model_id` producing `dim`-dimensional vectors.
    ///
    /// If a snapshot exists and its header matches `{model_id, dim}`, its live
    /// entries are replayed (a warm start). Otherwise — missing file, unreadable /
    /// corrupt snapshot, dimension change, or a **model-id bump** — the index
    /// cold-starts **empty** under the new identity, and the writer's startup
    /// reconciliation refills it from the graph. The header is always
    /// adopted from the caller's `model_id`, so [`model_id`](SemanticIndex::model_id)
    /// matches the embedder after a bump.
    pub fn open(path: impl AsRef<Path>, model_id: &str, dim: usize) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut inner = Inner::empty(dim);

        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(snap) = serde_json::from_slice::<Snapshot>(&bytes)
            && snap.model_id == model_id
            && snap.dim == dim
        {
            // Warm start: replay in the snapshot's (concept-id-sorted) order.
            for e in snap.entries {
                inner.insert_entry(e.concept_id, e.meta, e.vector);
            }
        }
        // A header mismatch or a corrupt snapshot opens empty; reconciliation refills it.

        Ok(Self {
            inner: RwLock::new(inner),
            model_id: model_id.to_string(),
            path,
            searching_mode_set: AtomicBool::new(false),
        })
    }

    /// Serializes the current live state to the configured path (atomic rename via a
    /// sibling temp file). A persist failure surfaces as [`EpistemicError::Index`]; the
    /// in-memory index stays correct, and the next startup reconciliation rewrites it.
    fn persist(&self, inner: &Inner) -> Result<()> {
        let mut entries: Vec<SnapEntry> = inner
            .entries
            .values()
            .map(|e| SnapEntry {
                concept_id: e.concept.clone(),
                meta: e.meta.clone(),
                vector: e.vector.clone(),
            })
            .collect();
        entries.sort_by(|a, b| a.concept_id.0.cmp(&b.concept_id.0));
        let snap = Snapshot {
            model_id: self.model_id.clone(),
            dim: inner.dim,
            entries,
        };
        let bytes = serde_json::to_vec(&snap)
            .map_err(|e| EpistemicError::Index(format!("snapshot serialize: {e}")))?;
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| EpistemicError::Index(format!("mkdir index dir: {e}")))?;
        }
        // A temp name unique to this process and call, so two writers of one file
        // never share a temp file.
        let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let file_name = self
            .path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tmp = self
            .path
            .with_file_name(format!("{file_name}.{}.{n}.tmp", std::process::id()));
        std::fs::write(&tmp, &bytes).map_err(|e| {
            // A partial temp file is useless; leave nothing behind.
            let _ = std::fs::remove_file(&tmp);
            EpistemicError::Index(format!("write index tmp: {e}"))
        })?;
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            EpistemicError::Index(format!("rename index: {e}"))
        })?;
        Ok(())
    }

    /// The coarse, best-effort pre-filter (invariant 8): cheaply drops a candidate
    /// whose denormalized owner/visibility make it obviously out-of-scope for `pre`.
    /// Conservative on purpose — authoritative gating happens in TypeDB after the
    /// fetch, so this only rejects the clearly-private-of-another-owner case.
    fn passes_prefilter(entry: &LiveEntry, pre: &FilterMeta) -> bool {
        !(entry.meta.visibility == Visibility::Private && entry.meta.owner != pre.owner)
    }
}

impl SemanticIndex for HnswIndex {
    fn upsert(&self, id: ConceptId, v: &[f32], meta: FilterMeta) -> Result<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| EpistemicError::Index("index lock poisoned".into()))?;
        if v.len() != inner.dim {
            return Err(EpistemicError::Index(format!(
                "dimension mismatch: index dim {}, vector dim {}",
                inner.dim,
                v.len()
            )));
        }
        inner.insert_entry(id, meta, v.to_vec());
        self.persist(&inner)
    }

    fn upsert_many(&self, items: Vec<(ConceptId, Vec<f32>, FilterMeta)>) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let mut inner = self
            .inner
            .write()
            .map_err(|_| EpistemicError::Index("index lock poisoned".into()))?;
        for (id, v, meta) in items {
            if v.len() != inner.dim {
                return Err(EpistemicError::Index(format!(
                    "dimension mismatch: index dim {}, vector dim {}",
                    inner.dim,
                    v.len()
                )));
            }
            inner.insert_entry(id, meta, v);
        }
        // A single persist for the whole batch — this is the whole point: a bulk
        // load of n vectors costs one snapshot rewrite, not n.
        self.persist(&inner)
    }

    fn remove(&self, id: ConceptId) -> Result<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| EpistemicError::Index("index lock poisoned".into()))?;
        if let Some(old) = inner.active.remove(&id) {
            inner.entries.remove(&old);
            self.persist(&inner)?;
        }
        Ok(())
    }

    fn remove_many(&self, ids: Vec<ConceptId>) -> Result<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| EpistemicError::Index("index lock poisoned".into()))?;
        let doomed: HashSet<ConceptId> = ids
            .into_iter()
            .filter(|id| inner.active.contains_key(id))
            .collect();
        if doomed.is_empty() {
            return Ok(());
        }
        // Compact: rebuild the ANN graph from the retained entries' stored vectors,
        // in concept-id order, so no tombstone outlives the call.
        let mut kept: Vec<LiveEntry> = std::mem::take(&mut inner.entries)
            .into_values()
            .filter(|e| !doomed.contains(&e.concept))
            .collect();
        kept.sort_by(|a, b| a.concept.0.cmp(&b.concept.0));
        let mut fresh = Inner::empty(inner.dim);
        for e in kept {
            fresh.insert_entry(e.concept, e.meta, e.vector);
        }
        *inner = fresh;
        // The fresh `Hnsw` starts with searching mode unset again.
        self.searching_mode_set.store(false, Ordering::Release);
        self.persist(&inner)
    }

    fn ids(&self) -> Option<HashSet<ConceptId>> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        Some(inner.active.keys().cloned().collect())
    }

    fn query(&self, v: &[f32], k: usize, pre: &FilterMeta) -> Result<Vec<(ConceptId, f32)>> {
        // `set_searching_mode` needs `&mut self`; `search` only needs `&self`. Flip
        // the mode exactly once (briefly under the WRITE lock) so every other query
        // can proceed under a READ lock instead of serializing on one writer.
        if !self.searching_mode_set.load(Ordering::Acquire) {
            let mut inner = self
                .inner
                .write()
                .map_err(|_| EpistemicError::Index("index lock poisoned".into()))?;
            inner.hnsw.set_searching_mode(true);
            self.searching_mode_set.store(true, Ordering::Release);
        }

        let inner = self
            .inner
            .read()
            .map_err(|_| EpistemicError::Index("index lock poisoned".into()))?;
        if inner.entries.is_empty() || k == 0 {
            return Ok(Vec::new());
        }
        // Over-search to absorb tombstoned ids the ANN graph still returns, capped
        // at the total inserted points (includes tombstones, not just live ones).
        let nb_point = inner.hnsw.get_nb_point();
        let knbn = k.saturating_mul(2).max(k).min(nb_point.max(1));
        let neighbours: Vec<Neighbour> = inner.hnsw.search(v, knbn, params::EF_SEARCH);

        let mut out: Vec<(ConceptId, f32)> = Vec::new();
        let mut seen: HashSet<ConceptId> = HashSet::new();
        for n in neighbours {
            let Some(entry) = inner.entries.get(&n.d_id) else {
                continue; // tombstoned / stale internal id
            };
            if !Self::passes_prefilter(entry, pre) {
                continue;
            }
            if !seen.insert(entry.concept.clone()) {
                continue;
            }
            // DistCosine yields (1 - cosine); similarity is the cosine.
            let similarity = 1.0 - n.distance;
            out.push((entry.concept.clone(), similarity));
            if out.len() >= k {
                break;
            }
        }
        Ok(out)
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn len(&self) -> usize {
        self.inner.read().map(|i| i.entries.len()).unwrap_or(0)
    }

    fn reset(&self) {
        if let Ok(mut inner) = self.inner.write() {
            let dim = inner.dim;
            *inner = Inner::empty(dim);
            // The fresh `Hnsw` starts with searching mode unset again, so the next
            // `query` must re-toggle it.
            self.searching_mode_set.store(false, Ordering::Relaxed);
            // Persist the now-empty state so a crash mid-rebuild doesn't leave a
            // stale snapshot on disk. Best-effort: ignore a persist error here.
            let _ = self.persist(&inner);
        }
    }
}

/// The disabled-accelerator index: every `query` returns empty so `recall` falls
/// back to the non-vector path, and writes are inert. Provided so the trait seam
/// stays swappable even with no backend. `gecko-bin` wires the
/// no-index writer directly when the accelerator is off, but this exists for the
/// contract and for callers that prefer an explicit no-op index.
#[derive(Default)]
pub struct NoopIndex;

impl SemanticIndex for NoopIndex {
    fn upsert(&self, _id: ConceptId, _v: &[f32], _m: FilterMeta) -> Result<()> {
        Ok(())
    }
    fn remove(&self, _id: ConceptId) -> Result<()> {
        Ok(())
    }
    fn query(&self, _v: &[f32], _k: usize, _p: &FilterMeta) -> Result<Vec<(ConceptId, f32)>> {
        Ok(Vec::new())
    }
    fn model_id(&self) -> &str {
        "noop"
    }
    fn len(&self) -> usize {
        0
    }
    fn reset(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use gecko_extension_api::{ActorId, BeliefState};

    fn meta(owner: &str, vis: Visibility) -> FilterMeta {
        FilterMeta {
            owner: ActorId::new(owner),
            visibility: vis,
            belief_state: BeliefState::Asserted,
            valid_from: chrono::Utc::now(),
        }
    }

    fn tmp_path(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("gecko-hnsw-{tag}-{nanos}.hnsw"))
    }

    // Two clearly-separated unit directions in 4-D.
    fn v_a() -> Vec<f32> {
        vec![1.0, 0.0, 0.0, 0.0]
    }
    fn v_b() -> Vec<f32> {
        vec![0.0, 1.0, 0.0, 0.0]
    }

    #[test]
    fn upsert_then_query_returns_nearest() {
        let path = tmp_path("nearest");
        let idx = HnswIndex::open(&path, "m@4", 4).unwrap();
        idx.upsert(
            ConceptId::new("mem/bel/a"),
            &v_a(),
            meta("u", Visibility::Private),
        )
        .unwrap();
        idx.upsert(
            ConceptId::new("mem/bel/b"),
            &v_b(),
            meta("u", Visibility::Private),
        )
        .unwrap();
        let res = idx
            .query(&[0.9, 0.1, 0.0, 0.0], 2, &meta("u", Visibility::Private))
            .unwrap();
        assert_eq!(res[0].0, ConceptId::new("mem/bel/a"), "nearest is a");
        assert_eq!(idx.len(), 2);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn upsert_replaces_the_vector_for_a_concept() {
        let path = tmp_path("replace");
        let idx = HnswIndex::open(&path, "m@4", 4).unwrap();
        let id = ConceptId::new("mem/bel/x");
        idx.upsert(id.clone(), &v_a(), meta("u", Visibility::Private))
            .unwrap();
        // Re-upsert same concept with a different vector.
        idx.upsert(id.clone(), &v_b(), meta("u", Visibility::Private))
            .unwrap();
        assert_eq!(idx.len(), 1, "still one live vector for the concept");
        let res = idx
            .query(&v_b(), 1, &meta("u", Visibility::Private))
            .unwrap();
        assert_eq!(res[0].0, id);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn remove_tombstones_the_vector() {
        let path = tmp_path("remove");
        let idx = HnswIndex::open(&path, "m@4", 4).unwrap();
        let id = ConceptId::new("mem/bel/gone");
        idx.upsert(id.clone(), &v_a(), meta("u", Visibility::Private))
            .unwrap();
        idx.remove(id.clone()).unwrap();
        assert_eq!(idx.len(), 0);
        let res = idx
            .query(&v_a(), 5, &meta("u", Visibility::Private))
            .unwrap();
        assert!(
            !res.iter().any(|(c, _)| c == &id),
            "removed id never returned"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn dimension_mismatch_is_rejected() {
        let path = tmp_path("dim");
        let idx = HnswIndex::open(&path, "m@4", 4).unwrap();
        let err = idx
            .upsert(
                ConceptId::new("x"),
                &[1.0, 2.0],
                meta("u", Visibility::Private),
            )
            .unwrap_err();
        assert!(matches!(err, EpistemicError::Index(_)));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn warm_start_reloads_from_snapshot() {
        let path = tmp_path("warm");
        {
            let idx = HnswIndex::open(&path, "m@4", 4).unwrap();
            idx.upsert(
                ConceptId::new("mem/bel/a"),
                &v_a(),
                meta("u", Visibility::Private),
            )
            .unwrap();
            idx.upsert(
                ConceptId::new("mem/bel/b"),
                &v_b(),
                meta("u", Visibility::Private),
            )
            .unwrap();
        }
        // Re-open: the snapshot header matches ⇒ warm start, non-empty.
        let idx2 = HnswIndex::open(&path, "m@4", 4).unwrap();
        assert_eq!(idx2.len(), 2, "warm start replays the snapshot");
        let res = idx2
            .query(&v_a(), 1, &meta("u", Visibility::Private))
            .unwrap();
        assert_eq!(res[0].0, ConceptId::new("mem/bel/a"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn model_id_bump_cold_starts_empty_for_rebuild() {
        let path = tmp_path("bump");
        {
            let idx = HnswIndex::open(&path, "m@4", 4).unwrap();
            idx.upsert(
                ConceptId::new("mem/bel/a"),
                &v_a(),
                meta("u", Visibility::Private),
            )
            .unwrap();
        }
        // Re-open under a NEW model-id ⇒ header mismatch ⇒ empty (rebuild due).
        let idx2 = HnswIndex::open(&path, "m2@4", 4).unwrap();
        assert!(idx2.is_empty(), "model-id bump forces a rebuild");
        assert_eq!(idx2.model_id(), "m2@4", "adopts the new embedder identity");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn corrupt_file_cold_starts_empty() {
        let path = tmp_path("corrupt");
        std::fs::write(&path, b"not json at all").unwrap();
        let idx = HnswIndex::open(&path, "m@4", 4).unwrap();
        assert!(
            idx.is_empty(),
            "a corrupt snapshot is treated as rebuild-due"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn prefilter_drops_other_owners_private_vectors() {
        let path = tmp_path("prefilter");
        let idx = HnswIndex::open(&path, "m@4", 4).unwrap();
        idx.upsert(
            ConceptId::new("mem/bel/mine"),
            &v_a(),
            meta("me", Visibility::Private),
        )
        .unwrap();
        idx.upsert(
            ConceptId::new("mem/bel/theirs"),
            &v_a(),
            meta("them", Visibility::Private),
        )
        .unwrap();
        let res = idx
            .query(&v_a(), 5, &meta("me", Visibility::Private))
            .unwrap();
        assert!(res.iter().any(|(c, _)| c.0 == "mem/bel/mine"));
        assert!(
            !res.iter().any(|(c, _)| c.0 == "mem/bel/theirs"),
            "another owner's private vector is coarse-filtered out"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn noop_index_always_empty() {
        let n = NoopIndex;
        assert!(n.is_empty());
        assert_eq!(
            n.query(&[1.0], 5, &meta("u", Visibility::Private)).unwrap(),
            vec![]
        );
    }

    /// A distinct unit-ish 4-D vector per index `i`.
    fn v_i(i: usize) -> Vec<f32> {
        let t = i as f32 * 0.1;
        vec![t.cos(), t.sin(), 0.1 * (i % 3) as f32, 0.05]
    }

    fn cid(i: usize) -> ConceptId {
        ConceptId::new(format!("mem/bel/{i:03}"))
    }

    #[test]
    fn ids_lists_the_live_ids() {
        let path = tmp_path("ids");
        let idx = HnswIndex::open(&path, "m@4", 4).unwrap();
        for i in 0..3 {
            idx.upsert(cid(i), &v_i(i), meta("u", Visibility::Private))
                .unwrap();
        }
        idx.remove(cid(1)).unwrap();
        assert_eq!(idx.ids(), Some(HashSet::from([cid(0), cid(2)])));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn remove_many_compacts_and_persists_once() {
        let path = tmp_path("remove-many");
        let idx = HnswIndex::open(&path, "m@4", 4).unwrap();
        let items = (0..50)
            .map(|i| (cid(i), v_i(i), meta("u", Visibility::Private)))
            .collect();
        idx.upsert_many(items).unwrap();
        idx.query(&v_i(0), 3, &meta("u", Visibility::Private))
            .unwrap();
        // Remove the cluster around the query point: without compaction, the
        // over-search would surface only tombstones and miss the retained ids.
        idx.remove_many((5..50).map(cid).collect()).unwrap();
        assert_eq!(idx.len(), 5);
        let kept: HashSet<ConceptId> = (0..5).map(cid).collect();
        let hits: HashSet<ConceptId> = idx
            .query(&v_i(47), 5, &meta("u", Visibility::Private))
            .unwrap()
            .into_iter()
            .map(|(c, _)| c)
            .collect();
        assert_eq!(
            hits, kept,
            "every retained id is reachable after compaction"
        );
        let reopened = HnswIndex::open(&path, "m@4", 4).unwrap();
        assert_eq!(reopened.ids(), Some(kept));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn remove_many_to_empty_persists_the_empty_state() {
        let path = tmp_path("remove-all");
        let idx = HnswIndex::open(&path, "m@4", 4).unwrap();
        for i in 0..3 {
            idx.upsert(cid(i), &v_i(i), meta("u", Visibility::Private))
                .unwrap();
        }
        idx.remove_many((0..3).map(cid).collect()).unwrap();
        let reopened = HnswIndex::open(&path, "m@4", 4).unwrap();
        assert_eq!(reopened.ids(), Some(HashSet::new()));
        assert!(reopened.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn remove_many_of_unknown_ids_is_a_no_op() {
        let path = tmp_path("remove-unknown");
        let idx = HnswIndex::open(&path, "m@4", 4).unwrap();
        idx.upsert(cid(0), &v_i(0), meta("u", Visibility::Private))
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        idx.remove_many(vec![cid(9)]).unwrap();
        assert!(!path.exists(), "removing unknown ids writes nothing");
        assert_eq!(idx.len(), 1);
    }

    #[test]
    fn two_handles_on_one_path_persist_valid_json() {
        let path = tmp_path("two-handles");
        let a = std::sync::Arc::new(HnswIndex::open(&path, "m@4", 4).unwrap());
        let b = std::sync::Arc::new(HnswIndex::open(&path, "m@4", 4).unwrap());
        let spawn = |idx: std::sync::Arc<HnswIndex>, base: usize| {
            std::thread::spawn(move || {
                (0..50)
                    .map(|i| idx.upsert(cid(base + i), &v_i(i), meta("u", Visibility::Private)))
                    .filter(|r| r.is_err())
                    .count()
            })
        };
        let (ta, tb) = (spawn(a, 0), spawn(b, 100));
        let failures = ta.join().unwrap() + tb.join().unwrap();
        assert_eq!(failures, 0, "every upsert persisted");
        let bytes = std::fs::read(&path).unwrap();
        serde_json::from_slice::<serde_json::Value>(&bytes).expect("valid snapshot JSON");
        let dir = path.parent().unwrap();
        let stem = path.file_name().unwrap().to_string_lossy().into_owned();
        let leftovers = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.starts_with(&stem) && n.ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0, "no temp file is left behind");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn noop_index_ids_is_none() {
        assert_eq!(NoopIndex.ids(), None);
    }
}
