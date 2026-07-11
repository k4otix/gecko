//! Schema-probing rig for the W2/W3 conflict work — a recreation of Deliverable-3
//! (`docs/refactor/CONFLICT-REPORT.md`, Appendix).
//!
//! It composes the production schema — core, then mem (always-on substrate), then
//! cyber — as three separate schema transactions (faithful to `apply_all_schemas`)
//! against a throwaway TypeDB, then hand-authors raw TypeQL (bypassing the stubbed
//! Rust writers — the D3 method) to reproduce ratified conflict findings and to
//! probe the W2 divergence-exception candidate shapes on top.
//!
//! - **T2** — `blast-radius` misses a cohort belief: its traversal set is
//!   `rests-on ∪ derivation`, so a belief attached to a population via `scoped`
//!   is never swept up when a member is retracted.
//! - **T5** — `population-members` ignores time: it takes no `$t`, so a belief
//!   `believed-at(t0)` is reconstructed against *today's* cohort.
//! - **W2** — the belief-anchored `divergence-exception`/`divergence-precedence`
//!   candidate (`probes/w2_precedence.tql`) and the entrenchment-predicate
//!   candidate (`probes/w2_disposition.tql`), probed as Mode-A deltas on top of
//!   the composed schema. A probe that *fails as expected* (P-DISP-2) is a verdict,
//!   not a bug — it certifies the persisted-vs-host split.
//!
//! The T2/T5 baselines PASS by REPRODUCING the documented conflict (a green
//! baseline is what makes the rig trustworthy to later validate fixes). Nothing
//! here is run by CI. Run against the live TypeDB with:
//!
//! ```text
//! cargo test --test schema_probe_w2w3 --no-default-features --features cyber \
//!     -- --ignored --test-threads=1
//! ```
//!
//! Gated on `cyber` (the rig composes the cyber schema) and needs the multi-thread
//! runtime the TypeDB driver expects.
#![cfg(feature = "cyber")]

use chrono::NaiveDateTime;
use cyber_gecko::CyberGecko;
use futures_util::StreamExt;
use gecko_engine::db::router::DbError;
use gecko_engine::extension::GeckoExtension;
use mem_gecko::MemGecko;

mod common;
use common::TestDb;

const CORE_SCHEMA: &str = include_str!("../../core/gecko-engine/schema/core_schema.tql");

/// Candidate **C1** — the belief-anchored precedence shape (`divergence-exception
/// sub belief` + the 4-role `divergence-precedence` relation). Applied as a Mode-A
/// delta on top of the composed schema.
const W2_PRECEDENCE: &str = include_str!("probes/w2_precedence.tql");

/// Candidate **C2** — the entrenchment ordering as a `return check` predicate
/// (`out-entrenches`). Applied as a Mode-A delta on top of the composed schema.
const W2_DISPOSITION: &str = include_str!("probes/w2_disposition.tql");

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

/// Applies a single candidate `.tql` fragment as its own additional schema
/// transaction — the Mode-A "delta-on-top" posture (candidate stacked on the
/// already-composed core→mem→cyber schema, never folded into it). Returns the raw
/// result so a probe can assert the delta is *rejected* (e.g. P-DISP-2, where the
/// pure-TQL ranker is expected to fail to define).
async fn apply_delta(db: &TestDb, delta: &str) -> Result<(), DbError> {
    db.router().apply_schema(delta).await
}

/// Runs a raw-TQL write as one committed transaction against the test DB.
async fn insert(db: &TestDb, query: &str) {
    let mut router = db.router();
    let tx = router.begin_write().await.expect("write tx");
    tx.query(query).await.expect("insert query");
    tx.commit().await.expect("commit");
}

/// Runs a raw-TQL read whose final stage is `select $<col>;` (one string column)
/// and collects the values, sorted for deterministic assertions.
///
/// A `Some(Err(_))` mid-stream is surfaced (via `.expect`), never silently swallowed
/// — an ended-early stream would otherwise mask a DB failure as a short result set.
async fn read_col(db: &TestDb, query: &str, col: &str) -> Vec<String> {
    let mut router = db.router();
    let tx = router.begin_read().await.expect("read tx");
    let answer = tx.query(query).await.expect("read query");
    assert!(answer.is_row_stream(), "select returns a row stream");
    let mut rows = answer.into_rows();
    let mut out = Vec::new();
    while let Some(item) = rows.next().await {
        let row = item.expect("row stream item");
        let val = row
            .get(col)
            .expect("column present")
            .expect("column bound")
            .try_get_string()
            .expect("column is a string")
            .to_string();
        out.push(val);
    }
    out.sort();
    out
}

/// Runs a raw-TQL read whose final stage is `select $cid;` (one `concept-id`
/// column) and collects the string values, sorted for deterministic assertions.
async fn read_concept_ids(db: &TestDb, query: &str) -> Vec<String> {
    read_col(db, query, "cid").await
}

// ══════════════════════════════════════════════════════════════════════════════
// T2 / T5 baselines — recreate the ratified D3 conflict findings.
// ══════════════════════════════════════════════════════════════════════════════

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

// ══════════════════════════════════════════════════════════════════════════════
// W2 — precedence (candidate C1, `probes/w2_precedence.tql`)
// ══════════════════════════════════════════════════════════════════════════════

/// Reads back the disposition belief anchoring an instance belief's precedence.
/// A probe-scaffolding function taking `($inst: belief)` (P-PREC-2's read-back).
const PREC_DISPOSITION_FN: &str = r#"define
  fun disposition-of($inst: belief) -> { divergence-exception } @doc("The disposition (divergence-exception) belief anchoring a precedence in which $inst is the prevailing belief."):
      match
          (exception-belief: $e, prevailing: $inst) isa divergence-precedence;
      return { $e };
"#;

/// The retractability-aware read: the disposition, but only while its anchoring
/// exception belief is live (not superseded). P-PREC-4's live-disposition read.
const PREC_LIVE_DISPOSITION_FN: &str = r#"define
  fun live-disposition-of($inst: belief) -> { divergence-exception } @doc("The disposition belief anchoring $inst's precedence, only while that exception belief is not superseded."):
      match
          (exception-belief: $e, prevailing: $inst) isa divergence-precedence;
          not { true == is-superseded($e); };
      return { $e };
"#;

/// **P-PREC-1** — the belief-anchored shape is define-clean and instantiable.
///
/// Applying candidate C1 as a delta on the composed schema must succeed, and a
/// full `divergence-precedence` instance (exception belief + prevailing +
/// overridden + scope) must insert and read back — proving the shape is not just
/// well-formed but usable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "W2 precedence probe; needs a live TypeDB — run with --ignored --test-threads=1"]
async fn p_prec_1_belief_anchored_shape_define_clean() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

    apply_delta(&db, W2_PRECEDENCE)
        .await
        .expect("candidate C1 (w2_precedence) applies clean as a delta");

    // A full precedence instance: exception belief E dispositions instance BI over
    // population belief BP, scoped to member C1.
    insert(
        &db,
        r#"insert
             $c1 isa concept, has concept-id "prec1/c1";
             $bp isa belief, has concept-id "prec1/BP", has belief-state "asserted";
             $bi isa belief, has concept-id "prec1/BI", has belief-state "asserted";
             $e isa divergence-exception, has concept-id "prec1/E",
                 has belief-state "asserted", has entrenchment "tool-derived";
             divergence-precedence
                 (exception-belief: $e, prevailing: $bi, overridden: $bp, exception-scope: $c1);"#,
    )
    .await;

    let anchored = read_concept_ids(
        &db,
        r#"match
             (exception-belief: $e, prevailing: $bi, overridden: $bp) isa divergence-precedence;
             $e has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        anchored,
        vec!["prec1/E".to_string()],
        "the exception belief must anchor a readable divergence-precedence; got {anchored:?}"
    );
}

/// **P-PREC-2** — full T4 population-vs-instance ground state; the population
/// belief stays `"asserted"` for other members, and the disposition is readable
/// back via a fn taking `($inst: belief)`.
///
/// The exception is member-scoped (it carries the scope, not a supersession), so
/// overriding the population belief for C1 leaves it asserted for C2. This is the
/// R3 clause: member-scoping is on the exception, never on `supersession`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "W2 precedence probe; needs a live TypeDB — run with --ignored --test-threads=1"]
async fn p_prec_2_member_scoped_exception_leaves_population_asserted() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;
    apply_delta(&db, W2_PRECEDENCE)
        .await
        .expect("candidate C1 applies");
    apply_delta(&db, PREC_DISPOSITION_FN)
        .await
        .expect("disposition-of read-back fn defines");

    // T4 ground state: population P with members C1, C2. Population belief BP scoped
    // to P; instance belief BI diverges for C1; exception belief E dispositions BI
    // over BP, scoped to C1. BP stays asserted (it still governs C2).
    insert(
        &db,
        r#"insert
             $p isa population, has spec-hash "prec2/pop/P";
             $c1 isa concept, has concept-id "prec2/c1";
             $c2 isa concept, has concept-id "prec2/c2";
             $bp isa belief, has concept-id "prec2/BP", has belief-state "asserted";
             $bi isa belief, has concept-id "prec2/BI", has belief-state "asserted";
             $e isa divergence-exception, has concept-id "prec2/E",
                 has belief-state "asserted", has entrenchment "tool-derived";
             scoped (scoped-belief: $bp, population: $p);
             population-member (owning-population: $p, member: $c1);
             population-member (owning-population: $p, member: $c2);
             divergence-precedence
                 (exception-belief: $e, prevailing: $bi, overridden: $bp, exception-scope: $c1);"#,
    )
    .await;

    // The population belief is untouched — still "asserted" for the other members.
    let bp_state = read_col(
        &db,
        r#"match
             $bp isa belief, has concept-id "prec2/BP", has belief-state $st;
           select $st;"#,
        "st",
    )
    .await;
    assert_eq!(
        bp_state,
        vec!["asserted".to_string()],
        "population belief BP must stay asserted despite the member-scoped exception; got {bp_state:?}"
    );

    // The disposition is readable back through the ($inst: belief) function.
    let disposition = read_concept_ids(
        &db,
        r#"match
             $bi isa belief, has concept-id "prec2/BI";
             let $e in disposition-of($bi);
             $e has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        disposition,
        vec!["prec2/E".to_string()],
        "disposition-of(BI) must surface the exception belief E; got {disposition:?}"
    );
}

/// **P-PREC-3** — the reified exception can be a contradiction claim (closes T4
/// problem 3).
///
/// `divergence-exception sub belief` inherits `plays contradiction:claim`, so a
/// `contradiction` hub can group the population belief, the instance belief and the
/// exception belief as co-claims — something a `canonical-entity` *view* (a
/// population, not a belief) can never do. `contradicts(E, BP)` is then true.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "W2 precedence probe; needs a live TypeDB — run with --ignored --test-threads=1"]
async fn p_prec_3_reified_exception_is_a_contradiction_claim() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;
    apply_delta(&db, W2_PRECEDENCE)
        .await
        .expect("candidate C1 applies");

    // A contradiction hub over the population belief, the instance belief, and the
    // reified exception belief. The insert only commits if E can play claim.
    insert(
        &db,
        r#"insert
             $hub isa anomaly, has concept-id "prec3/hub";
             $bp isa belief, has concept-id "prec3/BP", has belief-state "asserted";
             $bi isa belief, has concept-id "prec3/BI", has belief-state "asserted";
             $e isa divergence-exception, has concept-id "prec3/E",
                 has belief-state "asserted", has entrenchment "tool-derived";
             contradiction (hub: $hub, claim: $bp, claim: $bi, claim: $e);"#,
    )
    .await;

    // contradicts(E, BP) is true — the exception belief is a genuine co-claim.
    let contradicting = read_concept_ids(
        &db,
        r#"match
             $e isa belief, has concept-id "prec3/E";
             $bp isa belief, has concept-id "prec3/BP";
             true == contradicts($e, $bp);
             $e has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        contradicting,
        vec!["prec3/E".to_string()],
        "the reified exception must be a contradiction claim (contradicts(E,BP)=true); got {contradicting:?}"
    );
}

/// **P-PREC-4** — the exception is attributed and retractable (the §8.2 clause).
///
/// Superseding the exception belief flips a live-disposition read (gated on
/// `is-superseded`) to empty, and the pre-exception state — the population belief
/// still `"asserted"` — is answerable again. Retracting the disposition is just a
/// supersession of its anchoring belief, no bare-edge deletion.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "W2 precedence probe; needs a live TypeDB — run with --ignored --test-threads=1"]
async fn p_prec_4_exception_is_retractable_via_supersession() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;
    apply_delta(&db, W2_PRECEDENCE)
        .await
        .expect("candidate C1 applies");
    apply_delta(&db, PREC_LIVE_DISPOSITION_FN)
        .await
        .expect("live-disposition-of fn defines");

    insert(
        &db,
        r#"insert
             $bp isa belief, has concept-id "prec4/BP", has belief-state "asserted";
             $bi isa belief, has concept-id "prec4/BI", has belief-state "asserted";
             $e isa divergence-exception, has concept-id "prec4/E",
                 has belief-state "asserted", has entrenchment "tool-derived";
             divergence-precedence (exception-belief: $e, prevailing: $bi, overridden: $bp);"#,
    )
    .await;

    // Before retraction: the live disposition surfaces E.
    let live_before = read_concept_ids(
        &db,
        r#"match
             $bi isa belief, has concept-id "prec4/BI";
             let $e in live-disposition-of($bi);
             $e has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        live_before,
        vec!["prec4/E".to_string()],
        "before retraction the live disposition must surface E; got {live_before:?}"
    );

    // Retract the disposition: a superseding belief E2 supersedes E.
    insert(
        &db,
        r#"match
             $e isa belief, has concept-id "prec4/E";
           insert
             $e2 isa belief, has concept-id "prec4/E2", has belief-state "asserted";
             supersession (superseded: $e, superseding: $e2), has created-at 2026-07-11T00:00:00;"#,
    )
    .await;

    // is-superseded(E) is now true.
    let superseded = read_concept_ids(
        &db,
        r#"match
             $e isa belief, has concept-id "prec4/E";
             true == is-superseded($e);
             $e has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        superseded,
        vec!["prec4/E".to_string()],
        "the exception belief must read as superseded after retraction; got {superseded:?}"
    );

    // The live disposition is now empty — the disposition no longer applies.
    let live_after = read_concept_ids(
        &db,
        r#"match
             $bi isa belief, has concept-id "prec4/BI";
             let $e in live-disposition-of($bi);
             $e has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert!(
        live_after.is_empty(),
        "after retraction the live disposition must be empty; got {live_after:?}"
    );

    // The pre-exception state is answerable again: the population belief still stands.
    let bp_state = read_col(
        &db,
        r#"match
             $bp isa belief, has concept-id "prec4/BP", has belief-state $st;
           select $st;"#,
        "st",
    )
    .await;
    assert_eq!(
        bp_state,
        vec!["asserted".to_string()],
        "the overridden population belief must remain asserted after retraction; got {bp_state:?}"
    );
}

/// **P-PREC-5** — the co-mint structural half: a `divergence-precedence` with no
/// `exception-belief` player is rejected at **commit** (`@card(1)` validates at
/// commit, not at insert).
///
/// An anchor-less relation is the forbidden bare-edge override; the schema refuses
/// it. The insert statement itself is accepted — the cardinality violation only
/// surfaces when the transaction commits. (The procedural residue — that the anchor
/// was minted by the sanctioned writer — is owed to SPEC-W5 §3.2.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "W2 precedence probe; needs a live TypeDB — run with --ignored --test-threads=1"]
async fn p_prec_5_anchorless_precedence_fails_at_commit() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;
    apply_delta(&db, W2_PRECEDENCE)
        .await
        .expect("candidate C1 applies");

    let mut router = db.router();
    let tx = router.begin_write().await.expect("write tx");

    // A precedence with prevailing + overridden but NO exception-belief player.
    let insert_result = tx
        .query(
            r#"insert
                 $bp isa belief, has concept-id "prec5/BP", has belief-state "asserted";
                 $bi isa belief, has concept-id "prec5/BI", has belief-state "asserted";
                 divergence-precedence (prevailing: $bi, overridden: $bp);"#,
        )
        .await;
    assert!(
        insert_result.is_ok(),
        "the insert statement itself must be accepted — @card(1) validates at commit, not insert"
    );

    // The cardinality violation surfaces at commit.
    let commit_result = tx.commit().await;
    assert!(
        commit_result.is_err(),
        "committing an anchor-less divergence-precedence must fail on exception-belief @card(1)"
    );
    // Observed error captured for the probe log.
    eprintln!("P-PREC-5 commit error: {:?}", commit_result.err());
}

// ══════════════════════════════════════════════════════════════════════════════
// W2 — disposition (candidate C2, `probes/w2_disposition.tql`)
// ══════════════════════════════════════════════════════════════════════════════

/// **P-DISP-2** scaffolding — a pure-TQL 4-key `divergence-winner` whose winner is
/// bound only inside `or`-branches (via `is`). Per g1 finding #2 such a variable is
/// unbound at `return`, so this is EXPECTED to be inexpressible: either the define
/// is rejected or the call returns zero rows. It is NOT a shipped candidate — it is
/// the negative probe that certifies the host-side ranking split (risk R2).
const DIVERGENCE_WINNER_FN: &str = r#"define
  fun divergence-winner($a: belief, $b: belief) -> { belief } @doc("Attempted pure-TQL 4-key divergence ranker; probe scaffolding, expected inexpressible because the winner is bound only inside or-branches."):
      match
          not { $a is $b; };
          (derived: $a, source: $sa) isa derivation, has derivation-method $ma;
          (derived: $b, source: $sb) isa derivation, has derivation-method $mb;
          $a has confidence $ca; $b has confidence $cb;
          {
              $ma == "human-assertion"; $mb == "llm-synthesis";
              $winner is $a;
          } or {
              $mb == "human-assertion"; $ma == "llm-synthesis";
              $winner is $b;
          } or {
              true == out-entrenches($a, $b);
              $winner is $a;
          } or {
              true == out-entrenches($b, $a);
              $winner is $b;
          } or {
              $ca > $cb;
              $winner is $a;
          } or {
              $cb > $ca;
              $winner is $b;
          };
      return { $winner };
"#;

/// **P-DISP-1** — the nested-disjunction `return check` predicate compiles and
/// answers correctly.
///
/// `out-entrenches` (candidate C2) ranks entrenchment tiers via a nested `or`. On a
/// fixture (axiom / tool-derived / llm beliefs) it must answer the strict ordering
/// both ways: stronger-over-weaker true, the reverse false.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "W2 disposition probe; needs a live TypeDB — run with --ignored --test-threads=1"]
async fn p_disp_1_nested_disjunction_predicate_answers() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;
    apply_delta(&db, W2_DISPOSITION)
        .await
        .expect("candidate C2 (w2_disposition) applies clean as a delta");

    insert(
        &db,
        r#"insert
             $a isa belief, has concept-id "disp1/A", has belief-state "asserted", has entrenchment "axiom";
             $m isa belief, has concept-id "disp1/M", has belief-state "asserted", has entrenchment "tool-derived";
             $l isa belief, has concept-id "disp1/L", has belief-state "asserted", has entrenchment "llm";"#,
    )
    .await;

    // Helper closure: does out-entrenches(x, y) hold?
    async fn out_entrenches(db: &TestDb, x: &str, y: &str) -> bool {
        let q = format!(
            r#"match
                 $x isa belief, has concept-id "{x}";
                 $y isa belief, has concept-id "{y}";
                 true == out-entrenches($x, $y);
                 $x has concept-id $cid;
               select $cid;"#
        );
        !read_concept_ids(db, &q).await.is_empty()
    }

    assert!(
        out_entrenches(&db, "disp1/A", "disp1/L").await,
        "axiom out-entrenches llm"
    );
    assert!(
        out_entrenches(&db, "disp1/A", "disp1/M").await,
        "axiom out-entrenches tool-derived"
    );
    assert!(
        out_entrenches(&db, "disp1/M", "disp1/L").await,
        "tool-derived out-entrenches llm"
    );
    assert!(
        !out_entrenches(&db, "disp1/L", "disp1/A").await,
        "llm must NOT out-entrench axiom"
    );
    assert!(
        !out_entrenches(&db, "disp1/M", "disp1/A").await,
        "tool-derived must NOT out-entrench axiom"
    );
    assert!(
        !out_entrenches(&db, "disp1/A", "disp1/A").await,
        "a tier must NOT strictly out-entrench itself"
    );
}

/// **P-DISP-2** — the pure-TQL 4-key ranker is inexpressible (EXPECTED FAIL → the
/// verdict).
///
/// The probe passes by DEMONSTRATING the limitation: attempting to define the
/// `divergence-winner` ranker either fails to define, or defines but returns the
/// wrong/empty result when called on a fixture where A clearly wins. In neither
/// case does pure TQL correctly return the single winner — which is the design
/// signal forcing the host-side ranking split (risk R2). The observed outcome is
/// printed for the probe log.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "W2 disposition probe (expected-fail verdict); needs a live TypeDB — run with --ignored --test-threads=1"]
async fn p_disp_2_pure_tql_winner_is_inexpressible() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;
    apply_delta(&db, W2_DISPOSITION)
        .await
        .expect("candidate C2 applies (divergence-winner depends on out-entrenches)");

    let define_result = apply_delta(&db, DIVERGENCE_WINNER_FN).await;

    let correctly_ranked = if let Err(ref e) = define_result {
        // Branch 1: the ranker is rejected at define — the winner cannot be bound
        // conditionally. This is the inexpressibility verdict.
        eprintln!("P-DISP-2 define REJECTED: {e:?}");
        false
    } else {
        // Branch 2: the ranker defines; call it on a fixture where A clearly wins
        // (human-assertion beats llm-synthesis on the first key). If the or-branch
        // binding is lost, the call returns zero rows.
        insert(
            &db,
            r#"insert
                 $src isa belief, has concept-id "disp2/src", has belief-state "asserted";
                 $a isa belief, has concept-id "disp2/A", has belief-state "asserted",
                     has confidence 0.9, has entrenchment "user-stated";
                 $b isa belief, has concept-id "disp2/B", has belief-state "asserted",
                     has confidence 0.2, has entrenchment "llm";
                 derivation (derived: $a, source: $src), has derivation-method "human-assertion";
                 derivation (derived: $b, source: $src), has derivation-method "llm-synthesis";"#,
        )
        .await;

        let winners = read_concept_ids(
            &db,
            r#"match
                 $a isa belief, has concept-id "disp2/A";
                 $b isa belief, has concept-id "disp2/B";
                 let $w in divergence-winner($a, $b);
                 $w has concept-id $cid;
               select $cid;"#,
        )
        .await;
        eprintln!("P-DISP-2 define ACCEPTED; divergence-winner(A,B) = {winners:?}");
        winners == vec!["disp2/A".to_string()]
    };

    assert!(
        !correctly_ranked,
        "the pure-TQL divergence-winner must NOT correctly return the single winner \
         — the or-branch-bound winner is unbound at return (g1 finding #2), forcing \
         the host-side ranking split"
    );
}

/// The 4-key evidence a host-side ranker reads for a divergent belief. The ranking
/// key `derivation-method` lives on the belief's `derivation` relation, so it is
/// joined through `derivation`, not read off the belief.
#[derive(Clone, Debug)]
struct DispKeys {
    derivation_method: String,
    confidence: f64,
    entrenchment: String,
    valid_from: NaiveDateTime,
}

/// Priority of a derivation method (higher = stronger evidence). Provable methods
/// out-rank probabilistic ones.
fn method_rank(m: &str) -> i32 {
    match m {
        "human-assertion" => 6,
        "type-join" => 5,
        "cardinality" => 4,
        "functional-dependency" => 3,
        "type-db-function" => 2,
        "external-tool" => 1,
        "llm-synthesis" => 0,
        _ => -1,
    }
}

/// Priority of an entrenchment tier (higher = stronger), mirroring `out-entrenches`
/// / `Entrenchment::strength()`.
fn entrenchment_rank(e: &str) -> i32 {
    match e {
        "axiom" => 4,
        "user-stated" => 3,
        "tool-derived" => 2,
        "inferred" => 1,
        "llm" => 0,
        _ => -1,
    }
}

/// Host-side 4-key disposition: `(derivation-method, confidence, entrenchment,
/// valid-from)`, most-significant first. Returns the winning concept-id, or `None`
/// when every key ties (→ contested).
fn host_disposition(a: (&str, &DispKeys), b: (&str, &DispKeys)) -> Option<String> {
    use std::cmp::Ordering;
    let (a_id, ak) = a;
    let (b_id, bk) = b;
    let ord = method_rank(&ak.derivation_method)
        .cmp(&method_rank(&bk.derivation_method))
        .then(
            ak.confidence
                .partial_cmp(&bk.confidence)
                .unwrap_or(Ordering::Equal),
        )
        .then(entrenchment_rank(&ak.entrenchment).cmp(&entrenchment_rank(&bk.entrenchment)))
        .then(ak.valid_from.cmp(&bk.valid_from));
    match ord {
        Ordering::Greater => Some(a_id.to_string()),
        Ordering::Less => Some(b_id.to_string()),
        Ordering::Equal => None,
    }
}

/// Reads a belief's 4 ranking keys from the graph, joining `derivation` for the
/// derivation-method (the key that lives on the relation, not the belief).
async fn read_disp_keys(db: &TestDb, concept_id: &str) -> DispKeys {
    let mut router = db.router();
    let tx = router.begin_read().await.expect("read tx");
    let query = format!(
        r#"match
             $b isa belief, has concept-id "{concept_id}";
             $b has confidence $conf;
             $b has entrenchment $ent;
             $b has valid-from $vf;
             (derived: $b, source: $src) isa derivation, has derivation-method $dm;
           select $dm, $conf, $ent, $vf;"#
    );
    let answer = tx.query(&query).await.expect("keys query");
    assert!(answer.is_row_stream(), "select returns a row stream");
    let mut rows = answer.into_rows();
    let row = rows
        .next()
        .await
        .expect("one key row present")
        .expect("key row item");
    let get = |col: &str| row.get(col).expect("column present").expect("column bound");
    DispKeys {
        derivation_method: get("dm").try_get_string().expect("dm string").to_string(),
        confidence: get("conf").try_get_double().expect("confidence double"),
        entrenchment: get("ent").try_get_string().expect("ent string").to_string(),
        valid_from: get("vf").try_get_datetime().expect("valid-from datetime"),
    }
}

/// **P-DISP-3** — predicates-only in the graph + host-side ranking reproduces the
/// §8.2 dispositions.
///
/// The 4-key ranking `(derivation-method, confidence, entrenchment, valid-from)` is
/// assembled in Rust over the graph fixtures: separated evidence yields a single
/// winner; a total tie yields no winner (→ the host writes `belief-state
/// "contested"`, mints no exception). This is the host-split the schema forces.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "W2 disposition probe; needs a live TypeDB — run with --ignored --test-threads=1"]
async fn p_disp_3_host_side_ranking_reproduces_dispositions() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;
    apply_delta(&db, W2_DISPOSITION)
        .await
        .expect("candidate C2 applies");

    // Two fixtures: SEP — A (human-assertion) clearly out-ranks B (llm-synthesis) on
    // the first key; TIE — TA and TB agree on all four keys.
    insert(
        &db,
        r#"insert
             $src isa belief, has concept-id "disp3/src", has belief-state "asserted";
             $a isa belief, has concept-id "disp3/sep/A", has belief-state "asserted",
                 has confidence 0.9, has entrenchment "user-stated", has valid-from 2026-06-01T00:00:00;
             $b isa belief, has concept-id "disp3/sep/B", has belief-state "asserted",
                 has confidence 0.2, has entrenchment "llm", has valid-from 2026-01-01T00:00:00;
             $ta isa belief, has concept-id "disp3/tie/A", has belief-state "asserted",
                 has confidence 0.5, has entrenchment "llm", has valid-from 2026-03-01T00:00:00;
             $tb isa belief, has concept-id "disp3/tie/B", has belief-state "asserted",
                 has confidence 0.5, has entrenchment "llm", has valid-from 2026-03-01T00:00:00;
             derivation (derived: $a, source: $src), has derivation-method "human-assertion";
             derivation (derived: $b, source: $src), has derivation-method "llm-synthesis";
             derivation (derived: $ta, source: $src), has derivation-method "llm-synthesis";
             derivation (derived: $tb, source: $src), has derivation-method "llm-synthesis";"#,
    )
    .await;

    // Separated evidence → a winner.
    let sep_a = read_disp_keys(&db, "disp3/sep/A").await;
    let sep_b = read_disp_keys(&db, "disp3/sep/B").await;
    let winner = host_disposition(("disp3/sep/A", &sep_a), ("disp3/sep/B", &sep_b));
    assert_eq!(
        winner,
        Some("disp3/sep/A".to_string()),
        "separated evidence must resolve to A (human-assertion out-ranks llm-synthesis); got {winner:?}"
    );

    // A total tie → contested (no winner).
    let tie_a = read_disp_keys(&db, "disp3/tie/A").await;
    let tie_b = read_disp_keys(&db, "disp3/tie/B").await;
    let tie = host_disposition(("disp3/tie/A", &tie_a), ("disp3/tie/B", &tie_b));
    assert_eq!(
        tie, None,
        "a total tie on all four keys must be contested (no winner); got {tie:?}"
    );
}
