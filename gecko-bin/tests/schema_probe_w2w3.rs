//! Schema-probing rig for the W2/W3 conflict work — a recreation of Deliverable-3
//! (`docs/refactor/CONFLICT-REPORT.md`, Appendix).
//!
//! It composes the production schema — core, then mem (always-on substrate), then
//! cyber — as three separate schema transactions (faithful to `apply_all_schemas`)
//! against a throwaway TypeDB, then hand-authors raw TypeQL (bypassing the stubbed
//! Rust writers — the D3 method) to reproduce two ratified conflict findings:
//!
//! - **T2** — `blast-radius` misses a cohort belief: its traversal set is
//!   `rests-on ∪ derivation`, so a belief attached to a population via `scoped`
//!   is never swept up when a member is retracted.
//! - **T5** — `population-members` ignores time: it takes no `$t`, so a belief
//!   `believed-at(t0)` is reconstructed against *today's* cohort.
//!
//! Both are `#[ignore]` baselines that PASS by REPRODUCING the documented conflict
//! (a green baseline is what makes the rig trustworthy to later validate fixes).
//! They are not run by CI. Run them against the live TypeDB with:
//!
//! ```text
//! cargo test --test schema_probe_w2w3 --no-default-features --features cyber \
//!     -- --ignored --test-threads=1
//! ```
//!
//! Gated on `cyber` (the rig composes the cyber schema) and needs the multi-thread
//! runtime the TypeDB driver expects.
#![cfg(feature = "cyber")]

use cyber_gecko::CyberGecko;
use futures_util::StreamExt;
use gecko_engine::extension::GeckoExtension;
use mem_gecko::MemGecko;

mod common;
use common::TestDb;

const CORE_SCHEMA: &str = include_str!("../../core/gecko-engine/schema/core_schema.tql");

/// Applies the composed production schema as THREE separate schema transactions in
/// registration order — core, then mem (always-on substrate), then cyber — exactly
/// as the Assembler's `apply_all_schemas` composes them at `SchemaInit` time.
async fn apply_composed_schema(db: &TestDb) {
    let mut router = db.router();
    router
        .apply_schema(CORE_SCHEMA)
        .await
        .expect("core schema applies");
    let mem = MemGecko::new();
    router
        .apply_schema(mem.schema())
        .await
        .expect("mem substrate schema applies");
    let cyber = CyberGecko::new();
    router
        .apply_schema(cyber.schema())
        .await
        .expect("cyber schema applies");
}

/// Runs a raw-TQL write as one committed transaction against the test DB.
async fn insert(db: &TestDb, query: &str) {
    let mut router = db.router();
    let tx = router.begin_write().await.expect("write tx");
    tx.query(query).await.expect("insert query");
    tx.commit().await.expect("commit");
}

/// Runs a raw-TQL read whose final stage is `select $cid;` (one `concept-id`
/// column) and collects the string values, sorted for deterministic assertions.
///
/// A `Some(Err(_))` mid-stream is surfaced (via `.expect`), never silently swallowed
/// — an ended-early stream would otherwise mask a DB failure as a short result set.
async fn read_concept_ids(db: &TestDb, query: &str) -> Vec<String> {
    let mut router = db.router();
    let tx = router.begin_read().await.expect("read tx");
    let answer = tx.query(query).await.expect("read query");
    assert!(answer.is_row_stream(), "select returns a row stream");
    let mut rows = answer.into_rows();
    let mut out = Vec::new();
    while let Some(item) = rows.next().await {
        let row = item.expect("row stream item");
        let cid = row
            .get("cid")
            .expect("cid column present")
            .expect("cid column bound")
            .try_get_string()
            .expect("concept-id is a string")
            .to_string();
        out.push(cid);
    }
    out.sort();
    out
}

/// **T2 baseline** — `blast-radius` misses the cohort belief.
///
/// Ground state (raw TQL against the composed schema): a cohort `{P, C1, B}` — C1 a
/// member of population P via `population-member`, belief B `scoped` to P — plus a
/// `rests-on` chain where belief D rests on assumption A. `blast-radius(A)` traverses
/// only `rests-on ∪ derivation`, so it returns D and never reaches B (which attaches
/// to the cohort solely through `scoped`→P→`population-member`). Reproduces the
/// D3-confirmed finding: `blast-radius(A)` = `{t2/bel/D}` only, cohort belief B absent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "D3 fidelity baseline; needs a live TypeDB — run with --ignored --test-threads=1"]
async fn p_base_t2_blast_radius_misses_cohort_belief() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

    // Cohort {P, C1, B-scoped-to-P} is fully present; the rests-on chain D->A is what
    // blast-radius can actually traverse.
    insert(
        &db,
        r#"insert
             $p isa population, has spec-hash "t2/pop/P";
             $c1 isa concept, has concept-id "t2/c1";
             $b isa belief, has concept-id "t2/bel/B", has belief-state "asserted";
             $a isa belief, has concept-id "t2/bel/A", has belief-state "asserted";
             $d isa belief, has concept-id "t2/bel/D", has belief-state "asserted";
             scoped (scoped-belief: $b, population: $p);
             population-member (owning-population: $p, member: $c1);
             rests-on (resting: $d, assumption: $a);"#,
    )
    .await;

    let reachable = read_concept_ids(
        &db,
        r#"match
             $a isa belief, has concept-id "t2/bel/A";
             let $m in blast-radius($a);
             $m has concept-id $cid;
           select $cid;"#,
    )
    .await;

    // The documented T2 conflict: D (via rests-on) is returned; the cohort belief B
    // (reachable only via the untraversed scoped/population-member edges) is NOT.
    assert_eq!(
        reachable,
        vec!["t2/bel/D".to_string()],
        "blast-radius(A) must return D only; got {reachable:?}"
    );
    assert!(
        !reachable.contains(&"t2/bel/B".to_string()),
        "cohort belief B must be absent from blast-radius(A) — the T2 traversal wall"
    );
}

/// **T5 baseline** — `population-members` ignores time.
///
/// Ground state: belief B `scoped` to population P and valid from t0; the cohort grew
/// *after* t0 — member C1 was `computed-at` t0, member C2 was `computed-at` today.
/// `believed-at(t0)` is temporal and returns B (valid at t0), but `population-members`
/// takes no `$t` and returns today's full cohort `{C1, C2}`. Reproduces the
/// D3-confirmed finding: `believed-at(t0)` = `{B}` while `population-members(P)` =
/// `{C1, C2}` — B is reconstructed against the wrong-time membership.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "D3 fidelity baseline; needs a live TypeDB — run with --ignored --test-threads=1"]
async fn p_base_t5_population_members_ignores_time() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

    // B scoped to P, valid from t0. Cohort: C1 computed at t0, C2 computed today.
    insert(
        &db,
        r#"insert
             $p isa population, has spec-hash "t5/pop/P";
             $b isa belief, has concept-id "t5/bel/B", has belief-state "asserted",
                 has valid-from 2026-01-01T00:00:00;
             $c1 isa concept, has concept-id "t5/c1";
             $c2 isa concept, has concept-id "t5/c2";
             scoped (scoped-belief: $b, population: $p);
             $pm1 isa population-member,
                 links (owning-population: $p, member: $c1),
                 has computed-at 2026-01-01T00:00:00;
             $pm2 isa population-member,
                 links (owning-population: $p, member: $c2),
                 has computed-at 2026-07-11T00:00:00;"#,
    )
    .await;

    // believed-at(t0) is temporal: it sees only the belief valid at t0.
    let believed = read_concept_ids(
        &db,
        r#"match
             let $bel in believed-at(2026-01-01T00:00:00);
             $bel has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        believed,
        vec!["t5/bel/B".to_string()],
        "believed-at(t0) must return B only; got {believed:?}"
    );

    // population-members(P) takes no $t — it returns today's full cohort, both members.
    let members = read_concept_ids(
        &db,
        r#"match
             $p isa population, has spec-hash "t5/pop/P";
             let $m in population-members($p);
             $m has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        members,
        vec!["t5/c1".to_string(), "t5/c2".to_string()],
        "population-members(P) must return today's full cohort {{C1, C2}}; got {members:?}"
    );
}
