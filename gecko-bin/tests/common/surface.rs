//! The live belief-tier surface for tests that inspect a composed schema. Included
//! with `#[path = "common/surface.rs"] mod surface;` beside `mod common;`.

use gecko_engine::db::surface::{BeliefTierSurface, belief_tier_surface};

use super::common::TestDb;

/// The belief-tier surface of `db`'s schema, rooted at mem's
/// [`mem_gecko::BELIEF_TIER_ROOT`]; panics with the error if it cannot be read.
pub async fn live_belief_tier_surface(db: &TestDb) -> BeliefTierSurface {
    let mut router = db.router();
    let tx = router.begin_read().await.expect("read tx");
    belief_tier_surface(&tx, mem_gecko::BELIEF_TIER_ROOT)
        .await
        .unwrap_or_else(|e| panic!("belief-tier surface: {e}"))
}
