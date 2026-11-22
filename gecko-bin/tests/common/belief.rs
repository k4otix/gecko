//! A belief with a `source-link` to a concept, for tests of what a sync does to a
//! referenced concept. Included with `#[path = "common/belief.rs"] mod belief;`,
//! beside `#[path = "common/provenance.rs"] mod provenance;`.

use gecko_engine::db::router::TypeDbRouter;
use gecko_engine::db::run_graph_writes;
use gecko_extension_api::GraphWrite;

/// Writes the belief `belief_id`, a `source-link` naming the concept `concept_id`
/// as its origin, and the belief's ownership, in one committed transaction.
pub async fn claim_with_belief(db: &mut TypeDbRouter, belief_id: &str, concept_id: &str) {
    let tx = db.begin_write().await.expect("begin write");
    let setup = [
        GraphWrite::single(
            "given $id: string, $st: string; \
             insert $b isa belief, has concept-id == $id, has belief-state == $st;",
            vec!["id".into(), "st".into()],
            vec![belief_id.into(), "asserted".into()],
        )
        .expecting(1),
        GraphWrite::single(
            "given $bid: string, $cid: string, $kind: string; \
             match $b isa belief, has concept-id $bi; $bi == $bid; \
                   $c isa concept, has concept-id $ci; $ci == $cid; \
             insert $l isa source-link, links (memory: $b, origin: $c), \
                    has origin-kind == $kind;",
            vec!["bid".into(), "cid".into(), "kind".into()],
            vec![belief_id.into(), concept_id.into(), "concept".into()],
        )
        .expecting(1),
    ];
    let setup = [&setup[..], &crate::provenance::seed_provenance()].concat();
    run_graph_writes(&tx, &setup).await.expect("belief setup");
    tx.commit().await.expect("commit belief setup");
}
