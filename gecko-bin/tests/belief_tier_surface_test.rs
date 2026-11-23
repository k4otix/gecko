//! The introspected belief-tier surface and its fingerprint, against a live schema.
//!
//! Requires a live TypeDB service (`localhost:1729`, admin/password). Each test
//! provisions a throwaway database and drops it; the real `gecko` db is untouched.

use std::collections::BTreeSet;

use gecko_engine::db::router::DbError;
use gecko_engine::db::surface::{
    BeliefTierChange, BeliefTierMeasure, BeliefTierSurface, belief_tier_fingerprint,
    belief_tier_surface,
};
use gecko_engine::db::{CORE_SCHEMA, run_graph_writes};
use gecko_extension_api::GeckoExtension;
use mem_gecko::{BELIEF_TIER_ROOT, MemGecko};
use typedb_driver::concept::Value;
use typedb_driver::given::{GivenRowEntry, GivenRows};

mod common;
#[path = "common/provenance.rs"]
mod provenance;
use common::TestDb;

/// A fresh database with the core schema and, when `with_mem`, mem's schema.
async fn schema_db(prefix: &str, with_mem: bool) -> TestDb {
    let db = TestDb::new(prefix);
    let mut router = db.router();
    router
        .apply_schema(CORE_SCHEMA)
        .await
        .expect("core schema applies");
    if with_mem {
        router
            .apply_schema(MemGecko::new().schema())
            .await
            .expect("mem substrate schema applies");
    }
    db
}

fn set(labels: &[&str]) -> BTreeSet<String> {
    labels.iter().map(|l| l.to_string()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn surface_of_the_mem_schema_has_the_root_closure_and_its_belief_edges() {
    let db = schema_db("gecko_test_surface", true).await;
    let mut router = db.router();
    let tx = router.begin_read().await.expect("read tx");
    let surface = belief_tier_surface(&tx, BELIEF_TIER_ROOT)
        .await
        .expect("surface of the mem schema");

    let entities = set(&[
        "memory-item",
        "belief",
        "episode",
        "anomaly",
        "resolution",
        "retrieval-event",
    ]);
    assert!(
        surface.entities.is_superset(&entities),
        "entities must include {entities:?}; got {:?}",
        surface.entities
    );
    let relations = set(&[
        "supersession",
        "derivation",
        "ownership",
        "source-link",
        "rests-on",
        "surfaced",
        "informs-synthesis",
        "scoped",
        "resolves",
    ]);
    assert!(
        surface.relations.is_superset(&relations),
        "relations must include {relations:?}; got {:?}",
        surface.relations
    );
    for record in ["population-member", "containment", "concept-link"] {
        assert!(
            !surface.relations.contains(record),
            "relations must exclude `{record}`; got {:?}",
            surface.relations
        );
    }
    assert!(
        !surface.entities.contains("concept"),
        "entities must exclude `concept`; got {:?}",
        surface.entities
    );
}

/// A relation subtype inherits its parent's roles, and TypeDB reports each
/// inherited role under the parent's scoped label on both relations, so the
/// surface must keep every relation relating a qualifying role.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn surface_keeps_a_belief_edge_and_its_subtype() {
    let db = schema_db("gecko_test_surface", true).await;
    let mut router = db.router();
    router
        .apply_schema(
            "define relation test-rests-on sub rests-on \
             @doc(\"A test-only subtype of rests-on that inherits its roles.\");",
        )
        .await
        .expect("test subtype of rests-on applies");
    let tx = router.begin_read().await.expect("read tx");
    let surface = belief_tier_surface(&tx, BELIEF_TIER_ROOT)
        .await
        .expect("surface of the mem schema with a rests-on subtype");
    for label in ["rests-on", "test-rests-on"] {
        assert!(
            surface.relations.contains(label),
            "relations must include `{label}`; got {:?}",
            surface.relations
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn surface_without_the_root_is_an_error() {
    let db = schema_db("gecko_test_surface", false).await;
    let mut router = db.router();
    let tx = router.begin_read().await.expect("read tx");
    match belief_tier_surface(&tx, "memory-item").await {
        Err(DbError::Schema(m)) => assert!(
            m.contains("memory-item"),
            "the schema error must name the root; got {m:?}"
        ),
        other => panic!("expected a schema error naming the root; got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fingerprint_of_an_empty_surface_is_an_error() {
    let db = schema_db("gecko_test_surface", true).await;
    let mut router = db.router();
    let tx = router.begin_read().await.expect("read tx");
    let empty = BeliefTierSurface {
        entities: BTreeSet::new(),
        relations: BTreeSet::new(),
        attribute_owners: BTreeSet::new(),
    };
    match belief_tier_fingerprint(&tx, &empty).await {
        Err(DbError::Schema(m)) => assert!(m.contains("inspect nothing"), "{m}"),
        other => panic!("expected a schema error for an empty surface; got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fingerprint_of_an_empty_belief_tier_is_all_zero() {
    let db = schema_db("gecko_test_surface", true).await;
    let mut router = db.router();
    let tx = router.begin_read().await.expect("read tx");
    let surface = belief_tier_surface(&tx, BELIEF_TIER_ROOT)
        .await
        .expect("surface");
    let fp = belief_tier_fingerprint(&tx, &surface)
        .await
        .expect("fingerprint of the empty belief tier");

    let has = |l: &String, m| fp.counts.contains_key(&(l.clone(), m));
    for l in &surface.entities {
        assert!(has(l, BeliefTierMeasure::Instances), "{l} instances key");
        assert!(has(l, BeliefTierMeasure::Attributes), "{l} attributes key");
    }
    for l in &surface.relations {
        assert!(has(l, BeliefTierMeasure::Instances), "{l} instances key");
        assert!(has(l, BeliefTierMeasure::Attributes), "{l} attributes key");
        assert!(has(l, BeliefTierMeasure::Links), "{l} links key");
    }
    assert_eq!(
        fp.counts.len(),
        2 * surface.entities.len() + 3 * surface.relations.len(),
        "exactly the surface's keys"
    );
    assert!(
        fp.counts.values().all(|&n| n == 0),
        "every count is zero on an empty tier; got {:?}",
        fp.counts
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fingerprint_counts_instances_attributes_and_links() {
    let db = schema_db("gecko_test_surface", true).await;
    let mut router = db.router();

    let surface = {
        let tx = router.begin_read().await.expect("read tx");
        belief_tier_surface(&tx, BELIEF_TIER_ROOT)
            .await
            .expect("surface")
    };
    let f0 = {
        let tx = router.begin_read().await.expect("read tx");
        belief_tier_fingerprint(&tx, &surface)
            .await
            .expect("fingerprint before")
    };

    let tx = router.begin_write().await.expect("write tx");
    let mut given = GivenRows::new(
        vec!["a".to_string(), "b".to_string(), "state".to_string()],
        1,
    );
    given
        .push_row(vec![
            GivenRowEntry::Value(Value::String("mem/bel/a".to_string())),
            GivenRowEntry::Value(Value::String("mem/bel/b".to_string())),
            GivenRowEntry::Value(Value::String("asserted".to_string())),
        ])
        .expect("given row");
    tx.query_with_rows(
        "given $a: string, $b: string, $state: string; \
         insert $ba isa belief, has concept-id == $a, has belief-state == $state; \
                $bb isa belief, has concept-id == $b, has belief-state == $state; \
                rests-on (resting: $ba, assumption: $bb);",
        given,
    )
    .await
    .expect("insert two beliefs and a rests-on");
    run_graph_writes(&tx, &provenance::seed_provenance())
        .await
        .expect("seed provenance");
    tx.commit().await.expect("commit");

    let f1 = {
        let tx = router.begin_read().await.expect("read tx");
        belief_tier_fingerprint(&tx, &surface)
            .await
            .expect("fingerprint after")
    };

    let change = |label: &str, measure, before, after| BeliefTierChange {
        label: label.to_string(),
        measure,
        before,
        after,
    };
    let got: BTreeSet<String> = f0.changes_to(&f1).iter().map(|c| c.to_string()).collect();
    let want: BTreeSet<String> = [
        change("belief", BeliefTierMeasure::Instances, 0, 2),
        change("belief", BeliefTierMeasure::Attributes, 0, 12),
        change("memory-item", BeliefTierMeasure::Instances, 0, 2),
        change("memory-item", BeliefTierMeasure::Attributes, 0, 12),
        change("ownership", BeliefTierMeasure::Instances, 0, 2),
        change("ownership", BeliefTierMeasure::Attributes, 0, 2),
        change("ownership", BeliefTierMeasure::Links, 0, 4),
        change("rests-on", BeliefTierMeasure::Instances, 0, 1),
        change("rests-on", BeliefTierMeasure::Links, 0, 2),
        change("source-link", BeliefTierMeasure::Instances, 0, 2),
        change("source-link", BeliefTierMeasure::Attributes, 0, 2),
        change("source-link", BeliefTierMeasure::Links, 0, 4),
    ]
    .iter()
    .map(|c| c.to_string())
    .collect();
    assert_eq!(got, want);
}
