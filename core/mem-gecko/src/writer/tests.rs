use super::beliefs::validate_confidence;
use super::reader::{DEFAULT_HALF_LIFE_HOURS, recency_decay};
use super::*;
use async_trait::async_trait;
use gecko_extension_api::GraphWrite;
use gecko_extension_api::{
    ActorId, BeliefDraft, DerivationMethod, EpistemicWriter, FilterMeta, ProvenanceSource,
    RunContext, Visibility,
};
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
/// fail — enough to unit-test the best-effort/dirty wrapper without a live DB.
#[derive(Default)]
struct RecordingGraphStore {
    writes: Mutex<Vec<GraphWrite>>,
    /// One entry per `write` call: the ops it carried.
    write_calls: Mutex<Vec<Vec<GraphWrite>>>,
    fail_write: bool,
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
        self.writes.lock().unwrap().extend_from_slice(ops);
        self.write_calls.lock().unwrap().push(ops.to_vec());
        Ok(())
    }
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

#[tokio::test]
async fn assert_belief_survives_failing_index_and_marks_dirty() {
    let idx = Arc::new(FailingUpsertIndex::default());
    let w = MemWriter::new(
        Arc::new(RecordingGraphStore::default()),
        Some(Arc::new(OkEmbedder)),
        Some(idx.clone()),
    );
    let id = w
        .assert_belief(&ctx(), belief(), &[], DerivationMethod::LlmSynthesis)
        .await
        .expect("belief write must succeed even when the index fails");
    assert_eq!(idx.upserts.load(Ordering::SeqCst), 1);
    assert!(w.is_dirty(&ConceptId(id.0.clone())));
    assert_eq!(w.dirty_len(), 1);
}

#[tokio::test]
async fn assert_belief_survives_failing_embed_and_marks_dirty() {
    let idx = Arc::new(FailingUpsertIndex::default());
    let w = MemWriter::new(
        Arc::new(RecordingGraphStore::default()),
        Some(Arc::new(FailingEmbedder)),
        Some(idx.clone()),
    );
    let id = w
        .assert_belief(&ctx(), belief(), &[], DerivationMethod::LlmSynthesis)
        .await
        .expect("belief write must succeed even when embedding fails");
    assert_eq!(
        idx.upserts.load(Ordering::SeqCst),
        0,
        "embed failed ⇒ no upsert attempt"
    );
    assert!(w.is_dirty(&ConceptId(id.0)));
}

#[tokio::test]
async fn assert_belief_with_working_index_is_not_dirty() {
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
    assert_eq!(w.dirty_len(), 0);
    assert_eq!(idx.upserts.lock().unwrap().as_slice(), &[ConceptId(id.0)]);
}

#[tokio::test]
async fn no_index_configured_skips_embed_and_never_dirties() {
    let w = MemWriter::without_index(Arc::new(RecordingGraphStore::default()));
    let id = w
        .assert_belief(&ctx(), belief(), &[], DerivationMethod::TypeJoin)
        .await
        .unwrap();
    assert!(id.0.starts_with("mem/bel/"));
    assert_eq!(w.dirty_len(), 0);
}

#[tokio::test]
async fn authoritative_commit_failure_fails_the_write_and_dirties_nothing() {
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
    assert_eq!(w.dirty_len(), 0);
}

#[tokio::test]
async fn supersede_marks_old_dirty_when_remove_fails() {
    let idx = Arc::new(FailingUpsertIndex::default());
    let w = MemWriter::new(
        Arc::new(RecordingGraphStore::default()),
        Some(Arc::new(OkEmbedder)),
        Some(idx.clone()),
    );
    let old = MemId::new("mem/bel/OLD");
    let new_id = w
        .supersede(
            &ctx(),
            old.clone(),
            belief(),
            "found contradicting evidence",
        )
        .await
        .expect("supersede must succeed even when the index errors");
    assert_eq!(idx.removes.load(Ordering::SeqCst), 1);
    assert!(w.is_dirty(&ConceptId(old.0)));
    assert!(w.is_dirty(&ConceptId(new_id.0)));
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
    assert_eq!(w.dirty_len(), 0);
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
                    &[edge.clone()],
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
                edge(&[subject.clone()], &[verdict.clone()]),
                "probe-object",
            ),
            (
                "duplicate role",
                edge(
                    &[subject.clone(), object.clone(), subject.clone()],
                    &[verdict.clone()],
                ),
                "probe-subject",
            ),
            (
                "unknown role",
                edge(
                    &[subject.clone(), object.clone(), ("probe-claim", s("x"))],
                    &[verdict.clone()],
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
        let base = w.belief_ops(&c, &belief(), &id, &[], DerivationMethod::TypeJoin);
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
