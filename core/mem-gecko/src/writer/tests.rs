use super::beliefs::validate_confidence;
use super::reader::{DEFAULT_HALF_LIFE_HOURS, clamp_similarity, recency_decay};
use super::*;
use DerivationMethod::LlmSynthesis;
use async_trait::async_trait;
use gecko_extension_api::{
    ActorId, BeliefDraft, ConceptId, ContextBudget, DerivationMethod, EpistemicReader,
    EpistemicWriter, FilterMeta, ProvenanceSource, RecallQuery, RunContext, Visibility,
};
use gecko_extension_api::{GraphReader, GraphWrite};
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn confidence_validation_enforces_range_and_invariant_7() {
    use DerivationMethod::*;
    for c in [f64::NAN, -0.1, 1.5, f64::INFINITY] {
        assert!(validate_confidence(Some(c), Some(TypeJoin)).is_err(), "{c}");
        assert!(validate_confidence(Some(c), None).is_err(), "{c}");
    }
    assert!(validate_confidence(Some(1.0), Some(LlmSynthesis)).is_err());
    assert!(validate_confidence(Some(0.99), Some(LlmSynthesis)).is_ok());
    assert!(validate_confidence(Some(1.0), Some(TypeJoin)).is_ok());
    assert!(validate_confidence(Some(0.0), Some(HumanAssertion)).is_ok());
    assert!(validate_confidence(None, Some(LlmSynthesis)).is_ok());
}

#[test]
fn similarity_score_is_clamped() {
    // The index reports `1 - distance` as an f32, which can land just outside the
    // cosine range the schema's `@range(-1.0..1.0)` allows.
    let just_above = 1.0 - (-f32::EPSILON);
    assert!(just_above > 1.0);
    assert_eq!(clamp_similarity(just_above), 1.0);
    assert_eq!(clamp_similarity(-1.0 - f32::EPSILON), -1.0);
    for s in [-1.0, -0.25, 0.0, 0.75, 1.0] {
        assert_eq!(clamp_similarity(s), s);
    }
}

fn ctx() -> RunContext {
    RunContext::new(
        ActorId::new("agent-1"),
        ProvenanceSource::Manual,
        chrono::Utc::now(),
    )
}

fn belief() -> BeliefDraft {
    BeliefDraft {
        text: "the host is compromised".to_string(),
        owner: ActorId::new("agent-1"),
        visibility: Visibility::Private,
        confidence: Some(0.7),
        entrenchment: None,
    }
}

/// A fake [`GraphStore`] that records committed writes and can be forced to
/// fail — enough to unit-test the best-effort index wrapper without a live DB.
#[derive(Default)]
struct RecordingGraphStore {
    writes: Mutex<Vec<GraphWrite>>,
    /// One entry per `write` call: the ops it carried.
    write_calls: Mutex<Vec<Vec<GraphWrite>>>,
    fail_write: bool,
    /// When set, every `write` fails with a row-count mismatch at the op index this
    /// maps the batch length to.
    mismatch_at: Option<fn(usize) -> usize>,
    /// The documents every `read` returns.
    read_reply: Vec<Value>,
    /// How many `read` calls were made.
    reads: Mutex<usize>,
}
impl RecordingGraphStore {
    fn failing() -> Self {
        Self {
            fail_write: true,
            ..Default::default()
        }
    }
}
#[async_trait]
impl GraphStore for RecordingGraphStore {
    async fn write(&self, ops: &[GraphWrite]) -> Result<()> {
        if self.fail_write {
            return Err(EpistemicError::Storage("db down".into()));
        }
        if let Some(at) = self.mismatch_at {
            let mismatch = gecko_extension_api::RowCountMismatch {
                op: at(ops.len()),
                expected: 1,
                got: 0,
            };
            return Err(mismatch.into_error("given $b: string;"));
        }
        self.writes.lock().unwrap().extend_from_slice(ops);
        self.write_calls.lock().unwrap().push(ops.to_vec());
        Ok(())
    }
}
#[async_trait]
impl GraphReader for RecordingGraphStore {
    async fn read(
        &self,
        _query: &str,
        _vars: &[String],
        _row: &[gecko_extension_api::GraphValue],
    ) -> Result<Vec<Value>> {
        *self.reads.lock().unwrap() += 1;
        Ok(self.read_reply.clone())
    }
}

struct OkEmbedder;
impl Embedder for OkEmbedder {
    fn model_id(&self) -> &str {
        "test-embedder"
    }
    fn dim(&self) -> usize {
        3
    }
    fn embed_query(&self, _t: &str) -> Result<Vec<f32>> {
        Ok(vec![0.1, 0.2, 0.3])
    }
    fn embed_document(&self, _t: &str) -> Result<Vec<f32>> {
        Ok(vec![0.1, 0.2, 0.3])
    }
}

struct FailingEmbedder;
impl Embedder for FailingEmbedder {
    fn model_id(&self) -> &str {
        "failing-embedder"
    }
    fn dim(&self) -> usize {
        3
    }
    fn embed_query(&self, _t: &str) -> Result<Vec<f32>> {
        Err(EpistemicError::Embedding("boom".into()))
    }
    fn embed_document(&self, _t: &str) -> Result<Vec<f32>> {
        Err(EpistemicError::Embedding("boom".into()))
    }
}

#[derive(Default)]
struct FailingUpsertIndex {
    upserts: AtomicUsize,
    removes: AtomicUsize,
}
impl SemanticIndex for FailingUpsertIndex {
    fn upsert(&self, _id: ConceptId, _v: &[f32], _m: FilterMeta) -> Result<()> {
        self.upserts.fetch_add(1, Ordering::SeqCst);
        Err(EpistemicError::Index("upsert failed".into()))
    }
    fn remove(&self, _id: ConceptId) -> Result<()> {
        self.removes.fetch_add(1, Ordering::SeqCst);
        Err(EpistemicError::Index("remove failed".into()))
    }
    fn query(&self, _v: &[f32], _k: usize, _p: &FilterMeta) -> Result<Vec<(ConceptId, f32)>> {
        Ok(vec![])
    }
    fn model_id(&self) -> &str {
        "test-embedder"
    }
}

#[derive(Default)]
struct OkIndex {
    upserts: Mutex<Vec<ConceptId>>,
}
impl SemanticIndex for OkIndex {
    fn upsert(&self, id: ConceptId, _v: &[f32], _m: FilterMeta) -> Result<()> {
        self.upserts.lock().unwrap().push(id);
        Ok(())
    }
    fn remove(&self, _id: ConceptId) -> Result<()> {
        Ok(())
    }
    fn query(&self, _v: &[f32], _k: usize, _p: &FilterMeta) -> Result<Vec<(ConceptId, f32)>> {
        Ok(vec![])
    }
    fn model_id(&self) -> &str {
        "test-embedder"
    }
}

/// An index whose `query` surfaces one fixed candidate.
struct OneCandidateIndex;
impl SemanticIndex for OneCandidateIndex {
    fn upsert(&self, _id: ConceptId, _v: &[f32], _m: FilterMeta) -> Result<()> {
        Ok(())
    }
    fn remove(&self, _id: ConceptId) -> Result<()> {
        Ok(())
    }
    fn query(&self, _v: &[f32], _k: usize, _p: &FilterMeta) -> Result<Vec<(ConceptId, f32)>> {
        Ok(vec![(ConceptId("mem/bel/vanishing".into()), 0.9)])
    }
    fn model_id(&self) -> &str {
        "test-embedder"
    }
}

#[tokio::test]
async fn a_retrieval_event_mismatch_names_the_event() {
    let store = Arc::new(RecordingGraphStore {
        mismatch_at: Some(|len| len - 1),
        read_reply: vec![serde_json::json!({ "title": "t", "act": 1.0 })],
        ..Default::default()
    });
    let w = MemWriter::new(
        store,
        Some(Arc::new(OkEmbedder)),
        Some(Arc::new(OneCandidateIndex)),
    )
    .with_retrieval_provenance(true);
    let msg = storage(
        w.recall(
            &ctx(),
            RecallQuery {
                text: "q".into(),
                as_of: None,
            },
            ContextBudget {
                max_chunks: 1,
                max_tokens: None,
            },
        )
        .await
        .expect_err("a vanished candidate fails the retrieval-event write"),
    );
    for want in ["retrieval-event mem/ep/", "nothing was committed"] {
        assert!(msg.contains(want), "{want}: {msg}");
    }
}

#[tokio::test]
async fn assert_belief_survives_failing_index() {
    let idx = Arc::new(FailingUpsertIndex::default());
    let w = MemWriter::new(
        Arc::new(RecordingGraphStore::default()),
        Some(Arc::new(OkEmbedder)),
        Some(idx.clone()),
    );
    w.assert_belief(&ctx(), belief(), &[], DerivationMethod::LlmSynthesis)
        .await
        .expect("belief write must succeed even when the index fails");
    assert_eq!(idx.upserts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn assert_belief_survives_failing_embed() {
    let idx = Arc::new(FailingUpsertIndex::default());
    let w = MemWriter::new(
        Arc::new(RecordingGraphStore::default()),
        Some(Arc::new(FailingEmbedder)),
        Some(idx.clone()),
    );
    w.assert_belief(&ctx(), belief(), &[], DerivationMethod::LlmSynthesis)
        .await
        .expect("belief write must succeed even when embedding fails");
    assert_eq!(
        idx.upserts.load(Ordering::SeqCst),
        0,
        "embed failed ⇒ no upsert attempt"
    );
}

#[tokio::test]
async fn assert_belief_with_working_index_upserts_its_vector() {
    let idx = Arc::new(OkIndex::default());
    let w = MemWriter::new(
        Arc::new(RecordingGraphStore::default()),
        Some(Arc::new(OkEmbedder)),
        Some(idx.clone()),
    );
    let id = w
        .assert_belief(&ctx(), belief(), &[], DerivationMethod::TypeJoin)
        .await
        .unwrap();
    assert_eq!(idx.upserts.lock().unwrap().as_slice(), &[ConceptId(id.0)]);
}

#[tokio::test]
async fn no_index_configured_skips_embed() {
    let w = MemWriter::without_index(Arc::new(RecordingGraphStore::default()));
    let id = w
        .assert_belief(&ctx(), belief(), &[], DerivationMethod::TypeJoin)
        .await
        .unwrap();
    assert!(id.0.starts_with("mem/bel/"));
}

#[tokio::test]
async fn authoritative_commit_failure_fails_the_write_and_indexes_nothing() {
    let idx = Arc::new(OkIndex::default());
    let w = MemWriter::new(
        Arc::new(RecordingGraphStore::failing()),
        Some(Arc::new(OkEmbedder)),
        Some(idx.clone()),
    );
    let err = w
        .assert_belief(&ctx(), belief(), &[], DerivationMethod::TypeJoin)
        .await
        .expect_err("a failed graph commit must fail the write");
    assert!(matches!(err, EpistemicError::Storage(_)));
    assert_eq!(idx.upserts.lock().unwrap().len(), 0);
}

#[tokio::test]
async fn supersede_survives_a_failing_remove() {
    let idx = Arc::new(FailingUpsertIndex::default());
    let w = MemWriter::new(
        Arc::new(RecordingGraphStore::default()),
        Some(Arc::new(OkEmbedder)),
        Some(idx.clone()),
    );
    let old = MemId::new("mem/bel/OLD");
    w.supersede(
        &ctx(),
        old.clone(),
        belief(),
        "found contradicting evidence",
    )
    .await
    .expect("supersede must succeed even when the index errors");
    assert_eq!(idx.removes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn supersede_without_embedder_never_removes() {
    // Old-vector remove is gated on BOTH embedder+index.
    let idx = Arc::new(FailingUpsertIndex::default());
    let w = MemWriter::new(
        Arc::new(RecordingGraphStore::default()),
        None, // no embedder
        Some(idx.clone()),
    );
    w.supersede(&ctx(), MemId::new("mem/bel/OLD"), belief(), "reason")
        .await
        .unwrap();
    assert_eq!(
        idx.removes.load(Ordering::SeqCst),
        0,
        "no embedder ⇒ no remove attempt (matches upsert gating)"
    );
}

#[tokio::test]
async fn contest_rejects_fewer_than_two_claims() {
    let w = MemWriter::without_index(Arc::new(RecordingGraphStore::default()));
    let e = w
        .contest(&ctx(), &[MemId::new("mem/bel/x")])
        .await
        .unwrap_err();
    assert!(matches!(e, EpistemicError::InvalidInput(_)));
}

#[tokio::test]
async fn contest_refuses_a_repeated_claim() {
    let store = Arc::new(RecordingGraphStore::default());
    let w = MemWriter::without_index(store.clone());
    let a = MemId::new("mem/bel/a");
    let e = w
        .contest(&ctx(), &[a.clone(), MemId::new("mem/bel/b"), a])
        .await
        .unwrap_err();
    let EpistemicError::InvalidInput(msg) = &e else {
        panic!("expected InvalidInput, got {e:?}");
    };
    assert!(msg.contains("mem/bel/a"), "{msg}");
    assert!(store.writes.lock().unwrap().is_empty());
}

#[test]
fn id_matching_ops_require_exactly_one_row() {
    let id = MemId::new("mem/bel/1");
    let ids = [MemId::new("mem/ep/1"), MemId::new("mem/ep/2")];
    let deriv =
        crate::tql::derivation_op(&id, &ctx().actor, &ids, LlmSynthesis, Some(0.5)).unwrap();
    assert_eq!(deriv.expect_rows, Some(1));
    let contest = crate::tql::contest_ops(&ctx(), &MemId::new("mem/anom/1"), &ids);
    assert_eq!(contest.last().unwrap().expect_rows, Some(1));
    assert_eq!(
        crate::tql::rests_on_op(&id, &ids).expect_rows,
        Some(1),
        "rests_on"
    );
    assert_eq!(
        crate::tql::record_prediction_op(&id, 0.3).expect_rows,
        Some(1),
        "record_prediction"
    );
    let ev = MemId::new("mem/ep/re");
    assert_eq!(
        crate::tql::informs_synthesis_op(&ev, &id).expect_rows,
        Some(1),
        "informs-synthesis"
    );
    assert_eq!(
        crate::tql::set_was_used_op(&ev, &ids[0]).expect_rows,
        Some(1),
        "was-used"
    );
    let resolve =
        crate::tql::resolve_prediction_ops(&ctx(), &id, &MemId::new("mem/ep/r"), "confirmed");
    assert_eq!(
        resolve.last().unwrap().expect_rows,
        Some(1),
        "resolve_prediction"
    );
    let surfaced = [(ConceptId("mem/bel/1".into()), 0.9_f32)];
    let retrieval = crate::tql::retrieval_event_ops(&ctx(), &ev, "semantic", 1, &surfaced);
    assert_eq!(
        retrieval.last().unwrap().expect_rows,
        Some(1),
        "retrieval-event"
    );
}

#[test]
fn belief_ops_reports_the_derivation_index() {
    let w = MemWriter::without_index(Arc::new(RecordingGraphStore::default()))
        .with_retrieval_provenance(true);
    let id = MemId::new("mem/bel/1");
    let ev = [MemId::new("mem/ep/1")];
    let built = w.belief_ops(&ctx(), &belief(), &id, &ev, LlmSynthesis);
    let at = built.derivation.expect("evidence ⇒ a derivation op");
    assert_eq!(
        built.ops[at],
        crate::tql::derivation_op(&id, &ctx().actor, &ev, LlmSynthesis, belief().confidence)
            .unwrap()
    );
    let none = w.belief_ops(&ctx(), &belief(), &id, &[], LlmSynthesis);
    assert_eq!(none.derivation, None);
}

/// A writer whose every write fails with a row-count mismatch at `at(len)`.
fn mismatching_writer(at: fn(usize) -> usize) -> MemWriter {
    MemWriter::without_index(Arc::new(RecordingGraphStore {
        mismatch_at: Some(at),
        ..Default::default()
    }))
}

fn storage(e: EpistemicError) -> String {
    match e {
        EpistemicError::Storage(m) => m,
        other => panic!("expected Storage, got {other:?}"),
    }
}

#[tokio::test]
async fn a_derivation_mismatch_names_the_evidence() {
    // Without retrieval provenance the derivation is the last op.
    let ev = [MemId::new("mem/ep/1"), MemId::new("mem/ep/gone")];
    let msg = storage(
        mismatching_writer(|len| len - 1)
            .assert_belief(&ctx(), belief(), &ev, LlmSynthesis)
            .await
            .unwrap_err(),
    );
    for want in [
        "mem/ep/1",
        "mem/ep/gone",
        "no memory item",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "{want}: {msg}");
    }
    // A mismatch on another op passes through unchanged.
    let msg = storage(
        mismatching_writer(|_| 0)
            .assert_belief(&ctx(), belief(), &ev, LlmSynthesis)
            .await
            .unwrap_err(),
    );
    assert!(msg.starts_with("write op 0"), "{msg}");
}

#[tokio::test]
async fn a_retrieval_provenance_mismatch_names_the_event_and_item() {
    let ev = MemId::new("mem/ep/re");
    let item = MemId::new("mem/ep/1");
    let c = ctx();
    c.push_retrieval(
        ev.clone(),
        std::iter::once(ConceptId(item.0.clone())).collect(),
    );
    // With one matched retrieval the ops end: derivation, informs-synthesis,
    // was-used, retrieval-provenance.
    let informs = storage(
        mismatching_writer(|len| len - 3)
            .with_retrieval_provenance(true)
            .assert_belief(&c, belief(), std::slice::from_ref(&item), LlmSynthesis)
            .await
            .unwrap_err(),
    );
    for want in [
        "informs-synthesis",
        "mem/ep/re",
        "mem/bel/",
        "nothing was committed",
    ] {
        assert!(informs.contains(want), "{want}: {informs}");
    }
    let used = storage(
        mismatching_writer(|len| len - 2)
            .with_retrieval_provenance(true)
            .assert_belief(&c, belief(), std::slice::from_ref(&item), LlmSynthesis)
            .await
            .unwrap_err(),
    );
    for want in ["was-used", "mem/ep/re", "mem/ep/1", "nothing was committed"] {
        assert!(used.contains(want), "{want}: {used}");
    }
}

#[tokio::test]
async fn a_resolve_prediction_mismatch_names_the_belief() {
    let b = MemId::new("mem/bel/p");
    let msg = storage(
        mismatching_writer(|len| len - 1)
            .resolve_prediction(&ctx(), b, gecko_extension_api::Outcome::Confirmed)
            .await
            .unwrap_err(),
    );
    for want in [
        "mem/bel/p",
        "open prediction(s)",
        "exactly one is required",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "{want}: {msg}");
    }
}

#[tokio::test]
async fn contest_rests_on_and_record_prediction_mismatches_name_their_ids() {
    let a = MemId::new("mem/bel/a");
    let gone = MemId::new("mem/bel/gone");
    let msg = storage(
        mismatching_writer(|len| len - 1)
            .contest(&ctx(), &[a.clone(), gone.clone()])
            .await
            .unwrap_err(),
    );
    for want in [
        "mem/bel/a",
        "mem/bel/gone",
        "no belief",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "contest {want}: {msg}");
    }
    let msg = storage(
        mismatching_writer(|_| 0)
            .rests_on(&ctx(), a.clone(), std::slice::from_ref(&gone))
            .await
            .unwrap_err(),
    );
    for want in [
        "mem/bel/a",
        "mem/bel/gone",
        "no memory item",
        "nothing was committed",
    ] {
        assert!(msg.contains(want), "rests_on {want}: {msg}");
    }
    let msg = storage(
        mismatching_writer(|_| 0)
            .record_prediction(&ctx(), gone, 0.3)
            .await
            .unwrap_err(),
    );
    for want in ["mem/bel/gone", "does not exist", "nothing was committed"] {
        assert!(msg.contains(want), "record_prediction {want}: {msg}");
    }
}

#[test]
fn recency_decay_halves_each_half_life() {
    let t0 = chrono::Utc::now();
    let one_week = t0 + chrono::Duration::hours(168);
    let d = recency_decay(t0, one_week, DEFAULT_HALF_LIFE_HOURS);
    assert!((d - 0.5).abs() < 1e-6, "one half-life ⇒ 0.5, got {d}");
    assert_eq!(recency_decay(one_week, t0, DEFAULT_HALF_LIFE_HOURS), 1.0);
}

// ── The bound belief-edge writer ──────────────────────────────────────────────

mod edges {
    use super::*;
    use crate::belief_edge::{BeliefEdgeRegistry, ExtensionFragment};
    use crate::tql;
    use gecko_extension_api::{
        AttrDecl, AttrValueType, BeliefEdge, BeliefEdgeDecl, GeckoExtension, GraphValue, RoleDecl,
    };
    use serde_json::json;

    const CORE: &str = include_str!("../../../gecko-engine/schema/core_schema.tql");

    const PROBE_SCHEMA: &str = "define
  attribute probe-ref, value string;
  attribute probe-verdict, value string;
  relation probe-link @doc(\"Links a belief to a concept and a probe target.\"),
      relates probe-claim,
      relates probe-subject,
      relates probe-object,
      owns probe-verdict @card(1);
  entity probe-target @doc(\"A probe entity looked up by a unique reference.\"),
      owns probe-ref @unique,
      plays probe-link:probe-object;
  belief plays probe-link:probe-claim;
  concept plays probe-link:probe-subject;
";

    const Y_SCHEMA: &str = "define
  relation y-link @doc(\"Links a belief to a concept.\"),
      relates y-claim,
      relates y-subject;
  belief plays y-link:y-claim;
  concept plays y-link:y-subject;
";

    const PROBE_DECLS: &[BeliefEdgeDecl] = &[BeliefEdgeDecl {
        relation: "probe-link",
        belief_role: "probe-claim",
        roles: &[
            RoleDecl {
                role: "probe-subject",
                player: "concept",
                key: "concept-id",
            },
            RoleDecl {
                role: "probe-object",
                player: "probe-target",
                key: "probe-ref",
            },
        ],
        owned_attrs: &[AttrDecl {
            attr: "probe-verdict",
            value: AttrValueType::String,
            required: true,
        }],
    }];

    const Y_DECLS: &[BeliefEdgeDecl] = &[BeliefEdgeDecl {
        relation: "y-link",
        belief_role: "y-claim",
        roles: &[RoleDecl {
            role: "y-subject",
            player: "concept",
            key: "concept-id",
        }],
        owned_attrs: &[],
    }];

    fn registry() -> BeliefEdgeRegistry {
        let mem = crate::MemGecko::new();
        BeliefEdgeRegistry::build(
            CORE,
            mem.schema(),
            &[
                ExtensionFragment {
                    name: "probe-ext",
                    schema: PROBE_SCHEMA,
                    migrations: &[],
                    decls: PROBE_DECLS,
                },
                ExtensionFragment {
                    name: "y",
                    schema: Y_SCHEMA,
                    migrations: &[],
                    decls: Y_DECLS,
                },
            ],
        )
        .expect("the probe registry builds")
    }

    fn store_with(read_reply: Vec<Value>) -> Arc<RecordingGraphStore> {
        Arc::new(RecordingGraphStore {
            read_reply,
            ..Default::default()
        })
    }

    fn passing_store() -> Arc<RecordingGraphStore> {
        store_with(vec![json!({"ok": true})])
    }

    fn writer_over(store: &Arc<RecordingGraphStore>) -> Arc<MemWriter> {
        Arc::new(MemWriter::without_index(store.clone()).with_belief_edges(registry()))
    }

    fn s(v: &str) -> GraphValue {
        GraphValue::String(v.to_string())
    }

    fn pairs(items: &[(&str, GraphValue)]) -> Vec<(String, GraphValue)> {
        items
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn probe_edge(subject: &str, object: &str, verdict: &str) -> BeliefEdge {
        BeliefEdge {
            relation: "probe-link".to_string(),
            players: pairs(&[("probe-subject", s(subject)), ("probe-object", s(object))]),
            attrs: pairs(&[("probe-verdict", s(verdict))]),
        }
    }

    fn invalid_input(e: EpistemicError) -> String {
        match e {
            EpistemicError::InvalidInput(m) => m,
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    fn queries(ops: &[GraphWrite]) -> Vec<String> {
        ops.iter().map(|o| o.query.clone()).collect()
    }

    #[test]
    fn edge_writer_for_refuses_an_extension_with_no_declarations() {
        let w = writer_over(&passing_store());
        for ext in ["mem-gecko", "nobody"] {
            let e = w
                .edge_writer_for(ext)
                .err()
                .expect("no declarations ⇒ refused");
            let msg = invalid_input(e);
            assert!(msg.contains(ext), "{msg}");
        }
        assert!(w.edge_writer_for("probe-ext").is_ok());
    }

    #[tokio::test]
    async fn bound_writer_refuses_relations_it_does_not_own() {
        let store = passing_store();
        let h = writer_over(&store).edge_writer_for("y").unwrap();
        let cases = [
            (probe_edge("c/a", "t/1", "yes"), "probe-ext"),
            (
                BeliefEdge {
                    relation: "nobody-link".to_string(),
                    players: vec![],
                    attrs: vec![],
                },
                "no extension declares",
            ),
        ];
        for (edge, want) in cases {
            let e = h
                .assert_belief_with_edges(
                    &ctx(),
                    belief(),
                    &[],
                    DerivationMethod::TypeJoin,
                    std::slice::from_ref(&edge),
                )
                .await
                .expect_err("an unowned relation is refused");
            let msg = invalid_input(e);
            assert!(msg.contains(&edge.relation), "{msg}");
            assert!(msg.contains(want), "{msg}");
        }
        assert!(store.writes.lock().unwrap().is_empty());
        assert_eq!(*store.reads.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn ownership_is_refused_before_confidence_is_checked() {
        let store = passing_store();
        let h = writer_over(&store).edge_writer_for("y").unwrap();
        let mut draft = belief();
        draft.confidence = Some(1.5);
        let e = h
            .assert_belief_with_edges(
                &ctx(),
                draft,
                &[],
                DerivationMethod::TypeJoin,
                &[probe_edge("c/a", "t/1", "yes")],
            )
            .await
            .expect_err("an unowned relation is refused");
        let msg = invalid_input(e);
        assert!(msg.contains("probe-link"), "{msg}");
        assert!(msg.contains("probe-ext"), "{msg}");
        assert!(store.writes.lock().unwrap().is_empty());
        assert_eq!(*store.reads.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn a_row_count_mismatch_names_the_failing_edge() {
        let edges = [
            probe_edge("c/a", "t/1", "yes"),
            probe_edge("c/b", "t/2", "no"),
        ];
        let write_failing_at = |at: fn(usize) -> usize| {
            let edges = edges.clone();
            async move {
                let store = Arc::new(RecordingGraphStore {
                    read_reply: vec![json!({"ok": true})],
                    mismatch_at: Some(at),
                    ..Default::default()
                });
                writer_over(&store)
                    .edge_writer_for("probe-ext")
                    .unwrap()
                    .assert_belief_with_edges(
                        &ctx(),
                        belief(),
                        &[],
                        DerivationMethod::TypeJoin,
                        &edges,
                    )
                    .await
                    .expect_err("a row-count mismatch fails the write")
            }
        };

        // The last op is the second edge.
        let e = write_failing_at(|len| len - 1).await;
        let EpistemicError::Storage(msg) = &e else {
            panic!("expected Storage, got {e:?}");
        };
        assert_eq!(
            msg,
            "belief edge probe-link (probe-subject concept-id=c/b, probe-object probe-ref=t/2) \
             matched 0 rows; nothing was committed"
        );

        // A belief op's mismatch passes through unchanged.
        let e = write_failing_at(|_| 0).await;
        let mismatch = gecko_extension_api::RowCountMismatch::from_error(&e);
        assert_eq!(mismatch.map(|m| m.op), Some(0), "{e:?}");
    }

    #[tokio::test]
    async fn a_derivation_mismatch_with_edges_names_the_evidence() {
        let store = Arc::new(RecordingGraphStore {
            read_reply: vec![json!({"ok": true})],
            // The derivation sits just before the two edge ops.
            mismatch_at: Some(|len| len - 3),
            ..Default::default()
        });
        let e = writer_over(&store)
            .edge_writer_for("probe-ext")
            .unwrap()
            .assert_belief_with_edges(
                &ctx(),
                belief(),
                &[MemId::new("mem/ep/gone")],
                DerivationMethod::TypeJoin,
                &[
                    probe_edge("c/a", "t/1", "yes"),
                    probe_edge("c/b", "t/2", "no"),
                ],
            )
            .await
            .expect_err("a derivation mismatch fails the write");
        let EpistemicError::Storage(msg) = &e else {
            panic!("expected Storage, got {e:?}");
        };
        assert!(msg.contains("mem/ep/gone"), "{msg}");
        assert!(msg.contains("no memory item"), "{msg}");
        assert!(!msg.contains("belief edge"), "{msg}");
    }

    #[tokio::test]
    async fn bound_writer_validates_roles_and_attrs() {
        let subject = ("probe-subject", s("c/a"));
        let object = ("probe-object", s("t/1"));
        let verdict = ("probe-verdict", s("yes"));
        let edge = |players: &[(&str, GraphValue)], attrs: &[(&str, GraphValue)]| BeliefEdge {
            relation: "probe-link".to_string(),
            players: pairs(players),
            attrs: pairs(attrs),
        };
        let cases = [
            (
                "missing role",
                edge(
                    std::slice::from_ref(&subject),
                    std::slice::from_ref(&verdict),
                ),
                "probe-object",
            ),
            (
                "duplicate role",
                edge(
                    &[subject.clone(), object.clone(), subject.clone()],
                    std::slice::from_ref(&verdict),
                ),
                "probe-subject",
            ),
            (
                "unknown role",
                edge(
                    &[subject.clone(), object.clone(), ("probe-claim", s("x"))],
                    std::slice::from_ref(&verdict),
                ),
                "probe-claim",
            ),
            (
                "missing required attribute",
                edge(&[subject.clone(), object.clone()], &[]),
                "probe-verdict",
            ),
            (
                "unknown attribute",
                edge(
                    &[subject.clone(), object.clone()],
                    &[verdict.clone(), ("probe-mood", s("calm"))],
                ),
                "probe-mood",
            ),
            (
                "wrong value variant",
                edge(
                    &[subject.clone(), object.clone()],
                    &[("probe-verdict", GraphValue::Long(1))],
                ),
                "probe-verdict",
            ),
        ];
        for (name, edge, label) in cases {
            let store = passing_store();
            let h = writer_over(&store).edge_writer_for("probe-ext").unwrap();
            let e = h
                .assert_belief_with_edges(
                    &ctx(),
                    belief(),
                    &[],
                    DerivationMethod::TypeJoin,
                    &[edge],
                )
                .await
                .expect_err(name);
            let msg = invalid_input(e);
            assert!(msg.contains("probe-link"), "{name}: {msg}");
            assert!(msg.contains(label), "{name}: {msg}");
            assert!(store.writes.lock().unwrap().is_empty(), "{name}");
            assert_eq!(*store.reads.lock().unwrap(), 0, "{name}");
        }
    }

    #[tokio::test]
    async fn bound_writer_writes_belief_and_edges_in_one_call() {
        let store = passing_store();
        let w = writer_over(&store);
        let h = w.edge_writer_for("probe-ext").unwrap();
        let c = ctx();
        let edges = [
            probe_edge("c/a", "t/1", "yes"),
            probe_edge("c/b", "t/2", "no"),
        ];
        let id = h
            .assert_belief_with_edges(&c, belief(), &[], DerivationMethod::TypeJoin, &edges)
            .await
            .expect("a valid belief with edges is written");

        let calls = store.write_calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "one transaction");
        let ops = &calls[0];
        let base = w
            .belief_ops(&c, &belief(), &id, &[], DerivationMethod::TypeJoin)
            .ops;
        assert_eq!(ops.len(), base.len() + 2);
        assert_eq!(queries(&ops[..base.len()]), queries(&base));
        let decl = &PROBE_DECLS[0];
        for (op, edge) in ops[base.len()..].iter().zip(&edges) {
            assert_eq!(op.expect_rows, Some(1));
            assert_eq!(op, &tql::belief_edge_op(decl, &id, edge));
        }
    }

    #[tokio::test]
    async fn empty_edges_matches_assert_belief() {
        let run = |edges: bool| async move {
            let store = passing_store();
            let idx = Arc::new(OkIndex::default());
            let w = Arc::new(
                MemWriter::new(store.clone(), Some(Arc::new(OkEmbedder)), Some(idx.clone()))
                    .with_retrieval_provenance(true)
                    .with_belief_edges(registry()),
            );
            if edges {
                w.edge_writer_for("probe-ext")
                    .unwrap()
                    .assert_belief_with_edges(
                        &ctx(),
                        belief(),
                        &[],
                        DerivationMethod::LlmSynthesis,
                        &[],
                    )
                    .await
                    .unwrap();
            } else {
                w.assert_belief(&ctx(), belief(), &[], DerivationMethod::LlmSynthesis)
                    .await
                    .unwrap();
            }
            let ops = queries(&store.writes.lock().unwrap());
            let upserts = idx.upserts.lock().unwrap().len();
            (ops, upserts)
        };
        let (edge_ops, edge_upserts) = run(true).await;
        let (plain_ops, plain_upserts) = run(false).await;
        assert_eq!(edge_ops, plain_ops);
        assert_eq!(edge_upserts, 1);
        assert_eq!(plain_upserts, 1);
    }

    #[test]
    fn edge_op_keeps_values_out_of_query_text() {
        let hostile = "x\"; delete $b;";
        let op = tql::belief_edge_op(
            &PROBE_DECLS[0],
            &MemId::new("mem/bel/1"),
            &probe_edge("c/a", hostile, "yes"),
        );
        assert!(!op.query.contains(hostile), "{}", op.query);
        assert!(!op.query.contains("yes"), "{}", op.query);
        assert!(op.rows[0].contains(&s(hostile)));
        assert!(op.rows[0].contains(&s("yes")));
        assert_eq!(op.expect_rows, Some(1));
    }

    #[tokio::test]
    async fn live_check_runs_once_and_failures_are_not_cached() {
        let checks = tql::belief_edge_schema_reads(&PROBE_DECLS[0]).len();
        assert!(checks > 0);

        let store = passing_store();
        let h = writer_over(&store).edge_writer_for("probe-ext").unwrap();
        for _ in 0..2 {
            h.assert_belief_with_edges(
                &ctx(),
                belief(),
                &[],
                DerivationMethod::TypeJoin,
                &[probe_edge("c/a", "t/1", "yes")],
            )
            .await
            .expect("the live check passes");
        }
        assert_eq!(*store.reads.lock().unwrap(), checks);

        let store = store_with(vec![]);
        let h = writer_over(&store).edge_writer_for("probe-ext").unwrap();
        for attempt in 1..=2 {
            let before = *store.reads.lock().unwrap();
            let e = h
                .assert_belief_with_edges(
                    &ctx(),
                    belief(),
                    &[],
                    DerivationMethod::TypeJoin,
                    &[probe_edge("c/a", "t/1", "yes")],
                )
                .await
                .expect_err("a failed live check refuses the write");
            assert!(invalid_input(e).contains("probe-link"));
            assert!(store.writes.lock().unwrap().is_empty());
            assert!(
                *store.reads.lock().unwrap() > before,
                "attempt {attempt} reads the schema again"
            );
        }
    }
}

/// An index with a configurable model id that either lists its ids or manages
/// itself (`ids() == None`), recording resets — for the reconciliation branches the
/// real `HnswIndex` never takes.
struct ScriptedIndex {
    model: &'static str,
    self_managed: bool,
    live: Mutex<std::collections::HashSet<ConceptId>>,
    resets: AtomicUsize,
}
impl ScriptedIndex {
    fn new(model: &'static str, self_managed: bool, live: &[&str]) -> Self {
        Self {
            model,
            self_managed,
            live: Mutex::new(live.iter().map(|id| ConceptId((*id).into())).collect()),
            resets: AtomicUsize::new(0),
        }
    }
}
impl SemanticIndex for ScriptedIndex {
    fn upsert(&self, id: ConceptId, _v: &[f32], _m: FilterMeta) -> Result<()> {
        self.live.lock().unwrap().insert(id);
        Ok(())
    }
    fn remove(&self, id: ConceptId) -> Result<()> {
        self.live.lock().unwrap().remove(&id);
        Ok(())
    }
    fn query(&self, _v: &[f32], _k: usize, _p: &FilterMeta) -> Result<Vec<(ConceptId, f32)>> {
        Ok(vec![])
    }
    fn model_id(&self) -> &str {
        self.model
    }
    fn ids(&self) -> Option<std::collections::HashSet<ConceptId>> {
        (!self.self_managed).then(|| self.live.lock().unwrap().clone())
    }
    fn len(&self) -> usize {
        self.live.lock().unwrap().len()
    }
    fn reset(&self) {
        self.resets.fetch_add(1, Ordering::SeqCst);
        self.live.lock().unwrap().clear();
    }
}

/// A graph whose enumerate reads return one belief row, `mem/bel/live`.
fn graph_with_one_item() -> Arc<RecordingGraphStore> {
    Arc::new(RecordingGraphStore {
        read_reply: vec![serde_json::json!({
            "id": "mem/bel/live", "text": "a claim", "owner": "agent-1", "vis": "private"
        })],
        ..Default::default()
    })
}

#[tokio::test]
async fn reconcile_rebuilds_an_index_built_for_another_model() {
    let idx = Arc::new(ScriptedIndex::new("another-model", false, &["mem/bel/old"]));
    let w = MemWriter::new(
        graph_with_one_item(),
        Some(Arc::new(OkEmbedder)),
        Some(idx.clone()),
    );
    let report = w.reconcile_index().await.unwrap();
    assert_eq!(
        idx.resets.load(Ordering::SeqCst),
        1,
        "a model change resets"
    );
    assert_eq!(
        idx.ids(),
        Some([ConceptId("mem/bel/live".into())].into_iter().collect())
    );
    assert_eq!((report.added, report.removed), (1, 1));
}

#[tokio::test]
async fn reconcile_leaves_a_populated_self_managed_index_alone() {
    let idx = Arc::new(ScriptedIndex::new(
        "test-embedder",
        true,
        &["mem/bel/other"],
    ));
    let w = MemWriter::new(
        graph_with_one_item(),
        Some(Arc::new(OkEmbedder)),
        Some(idx.clone()),
    );
    let report = w.reconcile_index().await.unwrap();
    assert_eq!(idx.resets.load(Ordering::SeqCst), 0);
    assert_eq!(idx.len(), 1, "nothing embedded or removed");
    assert_eq!((report.added, report.removed), (0, 0));
}

#[tokio::test]
async fn reconcile_fills_an_empty_self_managed_index() {
    let idx = Arc::new(ScriptedIndex::new("test-embedder", true, &[]));
    let w = MemWriter::new(
        graph_with_one_item(),
        Some(Arc::new(OkEmbedder)),
        Some(idx.clone()),
    );
    let report = w.reconcile_index().await.unwrap();
    assert_eq!(idx.len(), 1, "the graph's item was embedded");
    assert_eq!((report.added, report.removed), (1, 0));
}
