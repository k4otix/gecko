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
//! **Post-fold: these are graduated acceptance tests, not probes.** The W2/W3
//! schema fold landed on `conflict_resolution`, so `apply_composed_schema` now reads
//! the SHIPPED (folded) mem schema — divergence-exception/-precedence, the
//! population-member window, the b1 `blast-radius` body, and the as-of-t functions are
//! all in the base. The graduated tests (no `#[ignore]`) assert the shipped behavior
//! directly against the composed schema and run under `just test-it` / CI's TypeDB job,
//! exactly as `cyber_detect_test` does. They no longer apply the winning candidates as
//! Mode-A deltas — those constructs live in the base now; a re-applied type delta would
//! be a no-op (idempotent `define`) and a re-applied FUNCTION delta would be
//! `redefine`d by `apply_schema` (see `shipped_function_mutation_requires_redefine_not_define`),
//! replacing the shipped body, so the vestigial deltas were removed.
//!
//! Probes that remain `#[ignore]` test constructs that were NOT shipped — the C2
//! `out-entrenches` predicate (`p_disp_1`), the b2 companion (`p_br_3`), the b3
//! resolution-entry (`p_br_4`/`p_br_5`), and the windowed-blast compositional seam
//! (`p_br_6`, the evidence behind the shipped `blast-radius-at`, which
//! `windowed_blast_radius_traversal_respects_membership_window` tests) — kept as manual
//! evidence for the owed/rejected work (SPEC-W2-W3 §5). Run them with:
//!
//! ```text
//! cargo test --test schema_probe_w2w3 --no-default-features --features cyber \
//!     -- --ignored --test-threads=1
//! ```
//!
//! The T2/T5 baselines are now regression floors: `blast-radius` from a NON-member
//! assumption still does not reach the cohort (the b1 population branch is member-keyed;
//! the fix, driving from the member, is `p_br_1`/`p_br_2`), and the present-state
//! `population-members` still ignores time by design (the as-of fix is `p_pm_3`).
//!
//! Gated on `cyber` (the rig composes the cyber schema) and needs the multi-thread
//! runtime the TypeDB driver expects.
#![cfg(feature = "cyber")]

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::NaiveDateTime;
use cyber_gecko::CyberGecko;
use futures_util::StreamExt;
use gecko_engine::db::router::DbError;
use gecko_engine::extension::GeckoExtension;
use mem_gecko::MemGecko;

mod common;
use common::TestDb;

const CORE_SCHEMA: &str = include_str!("../../core/gecko-engine/schema/core_schema.tql");

/// Candidate **C2** — the entrenchment ordering as a `return check` predicate
/// (`out-entrenches`). Applied as a Mode-A delta on top of the composed schema.
const W2_DISPOSITION: &str = include_str!("probes/w2_disposition.tql");

/// Candidate **C3-b2** — a NEW companion `cohort-blast-radius($member: concept)`
/// function (additive `define`, no param widening); the union with `blast-radius` is
/// host-side.
const W3_BLAST_B2: &str = include_str!("probes/w3_blast_radius_b2.tql");

/// Candidate **C3-b3** — `redefine fun blast-radius` keeping the `memory-item` param,
/// adding a `resolves`→`population-member`→`scoped` entry branch so retracting the
/// identity belief that binds a member undermines the cohort.
const W3_BLAST_B3: &str = include_str!("probes/w3_blast_radius_b3.tql");

/// Candidate **C4b** — the as-of-t functions `population-members-at($pop, $t)` and
/// the recursive `canonical-entity-at($rec, $t)`, both reading membership/identity
/// through per-branch validity-window predicates. Requires the window delta first
/// (`population-members-at` reads `population-member`'s `valid-from`/`valid-to`).
const W3_ASOF_FUNCTIONS: &str = include_str!("probes/w3_asof_functions.tql");

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

/// Counts the rows a `select` returns without inspecting the column values — used when
/// the selected column is a datetime (which `read_col`'s `try_get_string` cannot
/// stringify), e.g. reading back `valid-from`/`valid-to`/`computed-at`.
async fn count_rows(db: &TestDb, query: &str) -> usize {
    let mut router = db.router();
    let tx = router.begin_read().await.expect("read tx");
    let answer = tx.query(query).await.expect("read query");
    assert!(answer.is_row_stream(), "select returns a row stream");
    let mut rows = answer.into_rows();
    let mut n = 0usize;
    while let Some(item) = rows.next().await {
        item.expect("row stream item");
        n += 1;
    }
    n
}

// ══════════════════════════════════════════════════════════════════════════════
// T2 / T5 baselines — recreate the ratified D3 conflict findings.
// ══════════════════════════════════════════════════════════════════════════════

/// **Regression floor (was the T2 baseline)** — `blast-radius` from a NON-member does
/// not reach the cohort.
///
/// Ground state (raw TQL against the folded composed schema): a cohort `{P, C1, B}` — C1
/// a member of population P via `population-member`, belief B `scoped` to P — plus a
/// `rests-on` chain where belief D rests on assumption A. `blast-radius(A)` is driven
/// from the assumption A, which is NOT a population member, so the folded b1 population
/// branch (`population-member:member`) does not fire: it returns D (via `rests-on`) and
/// never reaches the cohort belief B. The complementary fix — driving from the MEMBER
/// concept C1, where b1 DOES sweep B — is `p_br_1`/`p_br_2`; together they show the b1
/// branch is correctly member-keyed. `blast-radius(A)` = `{t2/bel/D}` only, B absent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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

/// **Regression floor (was the T5 baseline)** — the present-state `population-members`
/// ignores time by design; the as-of variant is the fix.
///
/// Ground state: belief B `scoped` to population P and valid from t0; the cohort grew
/// *after* t0 — member C1 was `computed-at` t0, member C2 was `computed-at` today.
/// `believed-at(t0)` is temporal and returns B (valid at t0); the present-state
/// `population-members` takes no `$t` and returns today's full cohort `{C1, C2}` — the
/// documented fast-path behavior. The as-of fix, `population-members-at(P, t0)`, is
/// `p_pm_3`/`p_t5_fix`. `believed-at(t0)` = `{B}` while `population-members(P)` =
/// `{C1, C2}`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
async fn divergence_exception_is_a_belief_and_plays_contradiction_claim_defines() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

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

    // The exception-scope role binds the scoped member — read it back too (not just
    // proven by the insert committing).
    let scoped_member = read_concept_ids(
        &db,
        r#"match
             (exception-belief: $e, exception-scope: $c) isa divergence-precedence;
             $c has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        scoped_member,
        vec!["prec1/c1".to_string()],
        "the exception-scope role must bind the scoped member C1; got {scoped_member:?}"
    );
}

/// **P-PREC-2** — full T4 population-vs-instance ground state; the population
/// belief stays `"asserted"` for other members, and the disposition is readable
/// back via a fn taking `($inst: belief)`.
///
/// The exception is member-scoped (it carries the scope, not a supersession), so
/// overriding the population belief for C1 leaves it asserted for C2. This is the
/// R3 clause: member-scoping is on the exception, never on `supersession`.
///
/// **Graduation scope (owed half).** This gate asserts only the *schema* half of
/// the §5 row — the member-scoped exception shape defines, inserts, and reads back.
/// The owed behavioral half is the co-mint writer that mints the exception belief
/// and its `divergence-precedence` atomically through the W5b seam (ledger #8),
/// plus the anchor-less-write refusal (ledger #13); both are unbuilt, so a green
/// here is not evidence any writer mints this shape.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_scoped_exception_leaves_population_belief_asserted_for_others() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;
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

    // Member-scoping is real per-member: E is scoped to the SPECIFIC member C1 (not
    // C2), so the override applies to C1 alone while BP stays asserted for C2.
    let scope_of_e = read_concept_ids(
        &db,
        r#"match
             $e isa belief, has concept-id "prec2/E";
             (exception-belief: $e, exception-scope: $c) isa divergence-precedence;
             $c has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        scope_of_e,
        vec!["prec2/c1".to_string()],
        "the exception E must be scoped to the specific member C1, not C2; got {scope_of_e:?}"
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
async fn divergence_exception_is_a_belief_and_plays_contradiction_claim_holds() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

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
///
/// **Graduation scope (owed half).** This gate asserts only the *read* half of the
/// §5 row — supersession of the anchoring belief flips the live-disposition read to
/// empty, with no bare-edge delete. The owed behavioral half is a writer that
/// performs the retraction-with-lineage (the populations writer closing/superseding
/// on divergence, ledger #9; the co-mint writer, ledger #8); it is unbuilt, so a
/// green here does not verify any writer retracts with correct provenance.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retracting_a_precedence_supersedes_the_exception_belief_with_lineage() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;
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
async fn anchorless_divergence_precedence_rejected_at_commit() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

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

    // The cardinality violation surfaces at commit (the insert above is accepted —
    // that insert-ok/commit-err timing is the novel claim).
    let commit_err = tx
        .commit()
        .await
        .expect_err("committing an anchor-less divergence-precedence must fail at commit");
    // Pin the SPECIFIC exception-belief @card(1) violation, not any commit failure.
    let msg = format!("{commit_err:?}");
    eprintln!("P-PREC-5 commit error: {msg}");
    assert!(
        msg.contains("exception-belief") || msg.contains("@card") || msg.contains("CNT5"),
        "the commit must fail specifically on the exception-belief @card(1) violation \
         (expected 'exception-belief'/'@card'/'[CNT5]' in the error); got: {msg}"
    );
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
#[ignore = "C2 out-entrenches is scaffolding, NOT shipped (SPEC-W2-W3 §2.3) — evidence-only probe; run with --ignored --test-threads=1"]
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
async fn single_persisted_disposition_ranker_is_not_expressible() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;
    apply_delta(&db, W2_DISPOSITION)
        .await
        .expect("candidate C2 applies (divergence-winner depends on out-entrenches)");

    let define_result = apply_delta(&db, DIVERGENCE_WINNER_FN).await;

    match define_result {
        Err(e) => {
            // Branch 1 (observed): the ranker is rejected at define — a concept
            // variable bound only inside or-branches (`$winner is $a`) is unbound.
            // Pin the SPECIFIC unbound-variable rejection, not merely "some error":
            // a future schema change must not keep this green while no longer
            // certifying the inexpressibility this probe exists to prove.
            let msg = e.to_string();
            eprintln!("P-DISP-2 define REJECTED: {msg}");
            assert!(
                msg.contains("winner") || msg.contains("REP25"),
                "the define must be rejected specifically for the unbound or-branch-bound \
                 winner (expected 'winner'/'[REP25]' in the error); got: {msg}"
            );
        }
        Ok(()) => {
            // Branch 2: the ranker defines; call it on a fixture where A clearly wins
            // (human-assertion beats llm-synthesis on the first key). If the or-branch
            // binding is lost, the call returns the wrong/empty result — it must NOT
            // correctly return the single winner {A}.
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
            assert_ne!(
                winners,
                vec!["disp2/A".to_string()],
                "the pure-TQL divergence-winner must NOT correctly return the single \
                 winner — the or-branch-bound winner is unbound at return (g1 finding #2), \
                 forcing the host-side ranking split"
            );
        }
    }
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
///
/// **Graduation scope (owed half).** This gate asserts only the *ranking algorithm*
/// — the 4-key ordering reproduced host-side over graph fixtures. The owed
/// behavioral half is the host disposition engine (ledger #10) that, on a winner,
/// mints the divergence-exception, and on a tie writes `belief-state "contested"`
/// and mints the contested-anomaly hub; it is unbuilt, so a green here does not
/// verify the engine performs those writes.
///
/// The §5 graduated name `empty_winner_yields_contested_anomaly_hub_no_exception_minted`
/// is parked until the disposition engine (ledger #10) lands the hub-minting; this test
/// keeps a name reflecting the ranking half it actually asserts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_side_ranking_reproduces_dispositions() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

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

// ══════════════════════════════════════════════════════════════════════════════
// W3 — blast-radius fix (C3: b1/b2/b3) + as-of-t populations / canonical (C4)
//
// The T2 baseline drives blast-radius from an assumption A with NO path to the
// cohort; these fix probes drive from the MEMBER concept C1, the only node reaching
// the cohort belief B through population-member → scoped. b1's whole point is
// widening the param so blast-radius(C1: concept) type-checks — concept and
// memory-item are SIBLINGS under okf-concept, the central type-inference risk.
// ══════════════════════════════════════════════════════════════════════════════

/// **P-BR-1 (C3-b1)** — the widened-param `redefine` type-checks AND admits a
/// `concept` (population member) argument.
///
/// b1 widens `blast-radius`'s param `memory-item` → `okf-concept` so a member concept
/// C1 (sibling of `memory-item`) can be the retractee. Per-branch role narrowing
/// (`rests-on:assumption`/`derivation:source` want a `memory-item`;
/// `population-member:member` wants a `concept`) is the central type-inference risk —
/// and TypeDB 3.12 admits it. The redefine is ASSERTED to be accepted (a rejection
/// fails the test, not merely logged: `apply_schema` collapses every failure to
/// `DbError::Schema`, so an `Err`-tolerant match could not certify acceptance), then
/// `blast-radius(C1: concept)` must return the cohort belief B (exact set). This is
/// the assertion-backed evidence that b1's cross-sibling widening is viable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blast_radius_widens_param_and_reaches_cohort() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

    // The `okf-concept`-widened redefine with per-branch sibling narrowing is accepted
    // by TypeDB 3.12 — a rejection here is a hard test failure, not an open verdict.

    // b1 admits a `concept` argument and sweeps the cohort belief B from the member C1.
    insert(
        &db,
        r#"insert
             $p isa population, has spec-hash "br1/pop/P";
             $c1 isa concept, has concept-id "br1/c1";
             $b isa belief, has concept-id "br1/B", has belief-state "asserted";
             scoped (scoped-belief: $b, population: $p);
             population-member (owning-population: $p, member: $c1);"#,
    )
    .await;

    let reachable = read_concept_ids(
        &db,
        r#"match
             $c1 isa concept, has concept-id "br1/c1";
             let $m in blast-radius($c1);
             $m has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        reachable,
        vec!["br1/B".to_string()],
        "b1 blast-radius(C1: concept) must sweep the cohort belief B; got {reachable:?}"
    );
}

/// **P-BR-2 (C3-b1)** — recursion through the new population branch: a scoped belief
/// feeds back into `blast-radius`.
///
/// The b1 redefine is ASSERTED accepted (same evidentiary reason as P-BR-1), then
/// recursion is proven. Fixture: member C1 of population P, belief B scoped to P, and
/// belief D `rests-on` B. b1 sweeps B (direct population branch) AND D (recursion: the
/// scoped belief B re-enters `blast-radius`, whose `rests-on` branch reaches D).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blast_radius_widens_param_and_reaches_cohort_recursive() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

    // C1 → member → P → scoped → B (the scoped belief), and D rests-on B.
    insert(
        &db,
        r#"insert
             $p isa population, has spec-hash "br2/pop/P";
             $c1 isa concept, has concept-id "br2/c1";
             $b isa belief, has concept-id "br2/B", has belief-state "asserted";
             $d isa belief, has concept-id "br2/D", has belief-state "asserted";
             scoped (scoped-belief: $b, population: $p);
             population-member (owning-population: $p, member: $c1);
             rests-on (resting: $d, assumption: $b);"#,
    )
    .await;

    let reachable = read_concept_ids(
        &db,
        r#"match
             $c1 isa concept, has concept-id "br2/c1";
             let $m in blast-radius($c1);
             $m has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        reachable,
        vec!["br2/B".to_string(), "br2/D".to_string()],
        "b1 blast-radius(C1) must sweep B (direct) AND D (recursion via the scoped belief); got {reachable:?}"
    );
}

/// **P-BR-3 (C3-b2)** — the companion `cohort-blast-radius($member: concept)` returns
/// the cohort belief B on the T2 fixture, no param widening.
///
/// b2 is additive (`define`, not `redefine`); the union with `blast-radius` is
/// host-side. Driven from member C1 it must return B directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "non-shipped blast-radius construct (b2 rejected / b3 & windowed OWED, SPEC-W2-W3 §5) — evidence-only probe; run with --ignored --test-threads=1"]
async fn p_br_3_b2_companion_returns_cohort_belief() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;
    apply_delta(&db, W3_BLAST_B2)
        .await
        .expect("candidate C3-b2 (cohort-blast-radius) defines clean as a delta");

    insert(
        &db,
        r#"insert
             $p isa population, has spec-hash "br3/pop/P";
             $c1 isa concept, has concept-id "br3/c1";
             $b isa belief, has concept-id "br3/B", has belief-state "asserted";
             scoped (scoped-belief: $b, population: $p);
             population-member (owning-population: $p, member: $c1);"#,
    )
    .await;

    let reachable = read_concept_ids(
        &db,
        r#"match
             $c1 isa concept, has concept-id "br3/c1";
             let $b in cohort-blast-radius($c1);
             $b has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        reachable,
        vec!["br3/B".to_string()],
        "cohort-blast-radius(C1) must return the cohort belief B; got {reachable:?}"
    );
}

/// **P-BR-4 (C3-b3)** — the resolution-entry branch returns the cohort belief when
/// the binding resolution belief is the retractee.
///
/// b3 keeps the `memory-item` param and adds a `resolves`→`population-member`→`scoped`
/// branch. Retracting the identity belief RES that binds member C1 must undermine the
/// belief B scoped to C1's population.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "non-shipped blast-radius construct (b2 rejected / b3 & windowed OWED, SPEC-W2-W3 §5) — evidence-only probe; run with --ignored --test-threads=1"]
async fn p_br_4_b3_resolution_entry_returns_cohort_belief() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;
    apply_delta(&db, W3_BLAST_B3)
        .await
        .expect("candidate C3-b3 (resolution-entry redefine) applies as a schema tx");

    // RES resolves (record-a: C1, record-b: C2); C1 is a member of P; B is scoped to P.
    insert(
        &db,
        r#"insert
             $p isa population, has spec-hash "br4/pop/P";
             $c1 isa concept, has concept-id "br4/c1";
             $c2 isa concept, has concept-id "br4/c2";
             $b isa belief, has concept-id "br4/B", has belief-state "asserted";
             $res isa resolution, has concept-id "br4/RES", has belief-state "asserted";
             resolves (resolution-belief: $res, record-a: $c1, record-b: $c2);
             scoped (scoped-belief: $b, population: $p);
             population-member (owning-population: $p, member: $c1);"#,
    )
    .await;

    let reachable = read_concept_ids(
        &db,
        r#"match
             $res isa resolution, has concept-id "br4/RES";
             let $m in blast-radius($res);
             $m has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        reachable,
        vec!["br4/B".to_string()],
        "b3 blast-radius(RES) must return the cohort belief B via the resolves entry; got {reachable:?}"
    );
}

/// **P-BR-5** — `redefine fun blast-radius` is accepted as a schema transaction on a
/// DB where the function already exists, and the new body actually takes effect.
///
/// Uses b3 (the `memory-item`-param redefine, whose types are unambiguous) to isolate
/// the redefine MECHANISM from b1's param-widening question. The new body must be in
/// force: `blast-radius(RES)` (a `resolves` entry) returns the cohort belief, which
/// the original (rests-on ∪ derivation only) body could never reach.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "non-shipped blast-radius construct (b2 rejected / b3 & windowed OWED, SPEC-W2-W3 §5) — evidence-only probe; run with --ignored --test-threads=1"]
async fn p_br_5_redefine_blast_radius_accepted() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

    // The original blast-radius is present (defined by the mem substrate schema);
    // redefining it must be accepted as a schema transaction.
    apply_delta(&db, W3_BLAST_B3)
        .await
        .expect("redefine fun blast-radius must be accepted on a DB where the fun exists");

    // Prove the redefined body is in force (not a silent old body): the resolves
    // entry branch — absent from the original — now reaches the cohort belief.
    insert(
        &db,
        r#"insert
             $p isa population, has spec-hash "br5/pop/P";
             $c1 isa concept, has concept-id "br5/c1";
             $c2 isa concept, has concept-id "br5/c2";
             $b isa belief, has concept-id "br5/B", has belief-state "asserted";
             $res isa resolution, has concept-id "br5/RES", has belief-state "asserted";
             resolves (resolution-belief: $res, record-a: $c1, record-b: $c2);
             scoped (scoped-belief: $b, population: $p);
             population-member (owning-population: $p, member: $c1);"#,
    )
    .await;

    let reachable = read_concept_ids(
        &db,
        r#"match
             $res isa resolution, has concept-id "br5/RES";
             let $m in blast-radius($res);
             $m has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        reachable,
        vec!["br5/B".to_string()],
        "the redefined blast-radius body must be in force (reaches B via resolves); got {reachable:?}"
    );
}

/// **P-BR-6** — the T2×T5 seam: windowed blast propagation. Probes BOTH branches of
/// the §3.1 decision so the verdict is definite regardless of the A/B lean.
///
/// Fixture: member C1 with a CLOSED `population-member` window [t₀, t₁); cohort belief
/// B scoped to P and valid from t₀. Then:
///   - Present-state (b2 `cohort-blast-radius`, window-blind): reaches B even though
///     C1 has left the cohort — the row still exists — so present-state traversal is
///     OVER-broad as-of-now.
///   - Window-respecting entry (`population-members-at(P, t)`): reaches B for
///     t ∈ [t₀, t₁) (C1 was a member) and EXCLUDES it for t ≥ t₁ (C1 left) — the
///     as-of-t-correct behavior. This discrimination is only expressible WITH the
///     window, which is the evidence for §3.1 → (A) as-of-t variants.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "non-shipped blast-radius construct (b2 rejected / b3 & windowed OWED, SPEC-W2-W3 §5) — evidence-only probe; run with --ignored --test-threads=1"]
async fn p_br_6_windowed_blast_propagation_seam() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;
    // The population-member window and the as-of-t functions are FOLDED into the shipped
    // mem schema (Mode B): `apply_composed_schema` already provides them, so re-applying
    // them as a delta would be refused ([FUN5] already-exists — see P-REDEF). Only b2
    // (cohort-blast-radius) stays a genuine delta — it is deliberately NOT folded.
    apply_delta(&db, W3_BLAST_B2)
        .await
        .expect("b2 companion defines");

    // C1 was a member of P only during [2026-01-01, 2026-04-01); B is scoped to P.
    insert(
        &db,
        r#"insert
             $p isa population, has spec-hash "br6/pop/P";
             $c1 isa concept, has concept-id "br6/c1";
             $b isa belief, has concept-id "br6/B", has belief-state "asserted",
                 has valid-from 2026-01-01T00:00:00;
             scoped (scoped-belief: $b, population: $p);
             population-member (owning-population: $p, member: $c1),
                 has valid-from 2026-01-01T00:00:00, has valid-to 2026-04-01T00:00:00;"#,
    )
    .await;

    // Present-state traversal (b2) is window-BLIND: the row still exists, so B is swept
    // even though C1's window is closed — over-broad as-of-now.
    let present = read_concept_ids(
        &db,
        r#"match
             $c1 isa concept, has concept-id "br6/c1";
             let $b in cohort-blast-radius($c1);
             $b has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        present,
        vec!["br6/B".to_string()],
        "present-state cohort-blast-radius is window-blind and still sweeps B; got {present:?}"
    );

    // Window-respecting entry at t ∈ [t₀, t₁): C1 was a member → B is in the windowed blast.
    let in_window = read_concept_ids(
        &db,
        r#"match
             $p isa population, has spec-hash "br6/pop/P";
             let $m in population-members-at($p, 2026-02-01T00:00:00);
             $m has concept-id "br6/c1";
             (scoped-belief: $b, population: $p) isa scoped;
             $b has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        in_window,
        vec!["br6/B".to_string()],
        "a window-respecting blast at t in [t0,t1) must reach B (C1 was a member); got {in_window:?}"
    );

    // Window-respecting entry at t ≥ t₁: C1 has left → the windowed blast excludes B.
    let after_window = read_concept_ids(
        &db,
        r#"match
             $p isa population, has spec-hash "br6/pop/P";
             let $m in population-members-at($p, 2026-05-01T00:00:00);
             $m has concept-id "br6/c1";
             (scoped-belief: $b, population: $p) isa scoped;
             $b has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert!(
        after_window.is_empty(),
        "a window-respecting blast at t >= t1 must exclude B (C1 left the cohort); got {after_window:?}"
    );
}

/// **P-BR-6 graduated** — the shipped `blast-radius-at` traversal gates the cohort
/// edge on the membership window, recursively.
///
/// Fixture: member C1 with a CLOSED window [t₀, t₁); cohort belief B scoped to P; D
/// rests on B. As of t ∈ [t₀, t₁) retracting C1 sweeps B and, recursively, D; as of
/// t ≥ t₁ it sweeps nothing. A row with no `valid-from` (written before windows) is
/// never followed as-of-t.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn windowed_blast_radius_traversal_respects_membership_window() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

    insert(
        &db,
        r#"insert
             $p isa population, has spec-hash "wbr/pop/P";
             $c1 isa concept, has concept-id "wbr/c1";
             $c2 isa concept, has concept-id "wbr/c2";
             $b isa belief, has concept-id "wbr/B", has belief-state "asserted",
                 has valid-from 2026-01-01T00:00:00;
             $d isa belief, has concept-id "wbr/D", has belief-state "asserted";
             scoped (scoped-belief: $b, population: $p);
             rests-on (resting: $d, assumption: $b);
             population-member (owning-population: $p, member: $c1),
                 has valid-from 2026-01-01T00:00:00, has valid-to 2026-04-01T00:00:00;
             population-member (owning-population: $p, member: $c2);"#,
    )
    .await;

    let blast_at = |member: &'static str, t: &'static str| {
        format!(
            r#"match
                 $c isa concept, has concept-id "{member}";
                 let $m in blast-radius-at($c, {t});
                 $m has concept-id $cid;
               select $cid;"#
        )
    };

    let in_window = read_concept_ids(&db, &blast_at("wbr/c1", "2026-02-01T00:00:00")).await;
    assert_eq!(
        in_window,
        vec!["wbr/B".to_string(), "wbr/D".to_string()],
        "inside C1's window the cohort belief and what rests on it are swept"
    );

    let after = read_concept_ids(&db, &blast_at("wbr/c1", "2026-05-01T00:00:00")).await;
    assert!(
        after.is_empty(),
        "after C1 left, nothing is swept; got {after:?}"
    );

    let unwindowed = read_concept_ids(&db, &blast_at("wbr/c2", "2026-02-01T00:00:00")).await;
    assert!(
        unwindowed.is_empty(),
        "a membership row without valid-from is not followed as-of-t; got {unwindowed:?}"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// W3 — population window (C4a) + as-of-t functions (C4b)
// ══════════════════════════════════════════════════════════════════════════════

/// **P-PM-1 (C4a)** — the bitemporal window `owns valid-from, owns valid-to` is
/// additive onto the existing `population-member` relation and coexists with
/// `computed-at`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn population_member_carries_bitemporal_window_and_computed_at() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

    // One membership row carries valid-from, valid-to AND computed-at together.
    insert(
        &db,
        r#"insert
             $p isa population, has spec-hash "pm1/pop/P";
             $c1 isa concept, has concept-id "pm1/c1";
             population-member (owning-population: $p, member: $c1),
                 has valid-from 2026-01-01T00:00:00,
                 has valid-to 2026-04-01T00:00:00,
                 has computed-at 2026-01-01T00:00:00;"#,
    )
    .await;

    // All three temporal attrs read back off the same relation instance.
    let vf = count_rows(
        &db,
        r#"match
             $pm isa population-member, links (member: $c1);
             $c1 has concept-id "pm1/c1";
             $pm has valid-from $x;
           select $x;"#,
    )
    .await;
    let vt = count_rows(
        &db,
        r#"match
             $pm isa population-member, links (member: $c1);
             $c1 has concept-id "pm1/c1";
             $pm has valid-to $x;
           select $x;"#,
    )
    .await;
    let ca = count_rows(
        &db,
        r#"match
             $pm isa population-member, links (member: $c1);
             $c1 has concept-id "pm1/c1";
             $pm has computed-at $x;
           select $x;"#,
    )
    .await;
    assert_eq!(vf, 1, "valid-from reads back; got {vf}");
    assert_eq!(vt, 1, "valid-to reads back; got {vt}");
    assert_eq!(
        ca, 1,
        "computed-at still coexists on the same row; got {ca}"
    );
}

/// **P-PM-2 (C4a)** — two `population-member` instances for the SAME (pop, member)
/// with different windows (a left-and-rejoined member) coexist as distinct rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn left_and_rejoined_member_is_two_windowed_epoch_rows() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

    // C1 was a member during [t0,t1), left, then rejoined at t2 — two epoch rows.
    insert(
        &db,
        r#"insert
             $p isa population, has spec-hash "pm2/pop/P";
             $c1 isa concept, has concept-id "pm2/c1";
             population-member (owning-population: $p, member: $c1),
                 has valid-from 2026-01-01T00:00:00, has valid-to 2026-04-01T00:00:00;
             population-member (owning-population: $p, member: $c1),
                 has valid-from 2026-06-01T00:00:00;"#,
    )
    .await;

    // Both epoch windows are present on distinct instances linking the same (P, C1);
    // valid-from is @card(0..1), so two rows imply two distinct membership instances.
    let epochs = count_rows(
        &db,
        r#"match
             $pm isa population-member, links (owning-population: $p, member: $c1);
             $c1 has concept-id "pm2/c1";
             $pm has valid-from $x;
           select $x;"#,
    )
    .await;
    assert_eq!(
        epochs, 2,
        "two distinct windowed membership epochs for (P, C1) must coexist; got {epochs}"
    );
}

/// **P-PM-3 (C4b)** — `population-members-at` (relation-var `has` + negation over
/// relation attrs inside a persisted fn) defines and answers, consuming `$t` (FRP1).
///
/// Fixture: C1 open-ended from t₀, C2 closed at t₁. The function must return {C1, C2}
/// for a t in [t₀, t₁) and {C1} for a t ≥ t₁ — different answers for different `$t`
/// proving the parameter is genuinely consumed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn population_members_at_returns_windowed_membership() {
    let db = TestDb::new("probe_w2w3");
    // The population-member window and the as-of-t functions (C4a + C4b) are FOLDED into
    // the shipped mem schema (Mode B); `apply_composed_schema` provides them, so this
    // probe exercises the SHIPPED `population-members-at` directly — re-applying the
    // folded delta would be refused ([FUN5] already-exists; see P-REDEF).
    apply_composed_schema(&db).await;

    insert(
        &db,
        r#"insert
             $p isa population, has spec-hash "pm3/pop/P";
             $c1 isa concept, has concept-id "pm3/c1";
             $c2 isa concept, has concept-id "pm3/c2";
             population-member (owning-population: $p, member: $c1),
                 has valid-from 2026-01-01T00:00:00;
             population-member (owning-population: $p, member: $c2),
                 has valid-from 2026-01-01T00:00:00, has valid-to 2026-04-01T00:00:00;"#,
    )
    .await;

    let at_feb = read_concept_ids(
        &db,
        r#"match
             $p isa population, has spec-hash "pm3/pop/P";
             let $m in population-members-at($p, 2026-02-01T00:00:00);
             $m has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        at_feb,
        vec!["pm3/c1".to_string(), "pm3/c2".to_string()],
        "at t in [t0,t1) both members are live; got {at_feb:?}"
    );

    let at_may = read_concept_ids(
        &db,
        r#"match
             $p isa population, has spec-hash "pm3/pop/P";
             let $m in population-members-at($p, 2026-05-01T00:00:00);
             $m has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        at_may,
        vec!["pm3/c1".to_string()],
        "at t >= t1 only C1 remains (C2's window closed); got {at_may:?}"
    );
}

/// **P-CE-1 (C4b)** — the recursive `canonical-entity-at` with per-branch window
/// predicates returns the epoch-correct closure on a two-epoch fixture.
///
/// RES1 resolves (C1, C2) valid from t₀; RES2 resolves (C2, C3) valid from a LATER t₂.
/// As of t₁ ∈ [t₀, t₂) only RES1 is valid → the closure of C1 excludes C3; as of
/// t₃ ≥ t₂ both are valid → the closure includes C3. Identity as-of-t is read off the
/// resolution's validity window, never its (mutable) belief-state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canonical_entity_at_recursive_asof_closure() {
    let db = TestDb::new("probe_w2w3");
    // C4a + C4b are FOLDED (Mode B): `apply_composed_schema` provides the window and the
    // as-of-t functions, so this probe reads the SHIPPED `canonical-entity-at` — the
    // folded delta cannot be re-applied ([FUN5] already-exists; see P-REDEF).
    apply_composed_schema(&db).await;

    // Two epochs of provable identity: RES1(C1,C2) from t0; RES2(C2,C3) from t2.
    insert(
        &db,
        r#"insert
             $c1 isa concept, has concept-id "ce1/c1";
             $c2 isa concept, has concept-id "ce1/c2";
             $c3 isa concept, has concept-id "ce1/c3";
             $src isa belief, has concept-id "ce1/src", has belief-state "asserted";
             $res1 isa resolution, has concept-id "ce1/RES1", has belief-state "asserted",
                 has valid-from 2026-01-01T00:00:00;
             $res2 isa resolution, has concept-id "ce1/RES2", has belief-state "asserted",
                 has valid-from 2026-06-01T00:00:00;
             resolves (resolution-belief: $res1, record-a: $c1, record-b: $c2);
             resolves (resolution-belief: $res2, record-a: $c2, record-b: $c3);
             derivation (derived: $res1, source: $src), has derivation-method "type-join";
             derivation (derived: $res2, source: $src), has derivation-method "type-join";"#,
    )
    .await;

    // As of t1 (only RES1 valid): C3 is NOT yet in C1's identity closure.
    let at_t1 = read_concept_ids(
        &db,
        r#"match
             $c1 isa concept, has concept-id "ce1/c1";
             let $m in canonical-entity-at($c1, 2026-03-01T00:00:00);
             $m has concept-id $cid;
           select $cid;"#,
    )
    .await;
    eprintln!("P-CE-1 canonical-entity-at(C1, t1) = {at_t1:?}");
    // The symmetric resolves closure includes C1 itself (via the record-a/record-b
    // back-edge); the epoch-load-bearing fact is that C3 is absent as of t1.
    assert_eq!(
        at_t1,
        vec!["ce1/c1".to_string(), "ce1/c2".to_string()],
        "as of t1 (only RES1 valid) the closure of C1 is {{C1, C2}} — C3 absent; got {at_t1:?}"
    );

    // As of t3 (both valid): C3 enters the closure via the recursive branch.
    let at_t3 = read_concept_ids(
        &db,
        r#"match
             $c1 isa concept, has concept-id "ce1/c1";
             let $m in canonical-entity-at($c1, 2026-08-01T00:00:00);
             $m has concept-id $cid;
           select $cid;"#,
    )
    .await;
    eprintln!("P-CE-1 canonical-entity-at(C1, t3) = {at_t3:?}");
    assert_eq!(
        at_t3,
        vec![
            "ce1/c1".to_string(),
            "ce1/c2".to_string(),
            "ce1/c3".to_string()
        ],
        "as of t3 (both epochs valid) the closure of C1 grows to {{C1, C2, C3}}; got {at_t3:?}"
    );
}

/// **P-T5-FIX** — the W3 acceptance shape: on the T5 fixture, `believed-at(t)` and
/// `population-members-at(P, t)` are both as-of-t and AGREE (the belief believed at t₀
/// is reconstructed against the t₀ cohort, not today's).
///
/// The T5 baseline showed the bug: `believed-at(t0)` = {B} while the time-blind
/// `population-members(P)` = {C1, C2} (today's cohort). With the window, at t₀ only C1
/// was a member (C2 joined later), so `population-members-at(P, t0)` = {C1} — the
/// as-of-t0 belief view and membership view are mutually consistent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn believed_at_and_members_at_agree_as_of_t() {
    let db = TestDb::new("probe_w2w3");
    // C4a + C4b are FOLDED (Mode B): `apply_composed_schema` provides the window and the
    // as-of-t functions, so the acceptance shape is proven against the SHIPPED schema
    // with no delta — the folded delta cannot be re-applied ([FUN5]; see P-REDEF).
    apply_composed_schema(&db).await;

    // B scoped to P, valid from t0. C1 a member from t0; C2 joined at today.
    insert(
        &db,
        r#"insert
             $p isa population, has spec-hash "t5fix/pop/P";
             $b isa belief, has concept-id "t5fix/B", has belief-state "asserted",
                 has valid-from 2026-01-01T00:00:00;
             $c1 isa concept, has concept-id "t5fix/c1";
             $c2 isa concept, has concept-id "t5fix/c2";
             scoped (scoped-belief: $b, population: $p);
             population-member (owning-population: $p, member: $c1),
                 has valid-from 2026-01-01T00:00:00, has computed-at 2026-01-01T00:00:00;
             population-member (owning-population: $p, member: $c2),
                 has valid-from 2026-07-11T00:00:00, has computed-at 2026-07-11T00:00:00;"#,
    )
    .await;

    // believed-at(t0) is temporal: it sees the belief valid at t0.
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
        vec!["t5fix/B".to_string()],
        "believed-at(t0) must return B only; got {believed:?}"
    );

    // population-members-at(P, t0) is NOW time-aware: only C1 was a member at t0.
    let members_t0 = read_concept_ids(
        &db,
        r#"match
             $p isa population, has spec-hash "t5fix/pop/P";
             let $m in population-members-at($p, 2026-01-01T00:00:00);
             $m has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        members_t0,
        vec!["t5fix/c1".to_string()],
        "population-members-at(P, t0) must return the t0 cohort {{C1}}, agreeing with believed-at(t0); got {members_t0:?}"
    );

    // Sanity: as of today the cohort has grown to {C1, C2} — the fix is time-varying,
    // not a constant, so the t0 agreement above is a genuine as-of-t result.
    let members_now = read_concept_ids(
        &db,
        r#"match
             $p isa population, has spec-hash "t5fix/pop/P";
             let $m in population-members-at($p, 2026-07-11T00:00:00);
             $m has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        members_now,
        vec!["t5fix/c1".to_string(), "t5fix/c2".to_string()],
        "population-members-at(P, today) must return the full cohort {{C1, C2}}; got {members_now:?}"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// Mode B — the folded shipped shape: composition (P-COMP) + migration posture (P-REDEF)
//
// After the Mode-B fold, `apply_composed_schema` reads the mem schema files that now
// ALREADY carry the winning candidates (divergence-exception + divergence-precedence,
// the population-member window, the b1 blast-radius body, and the as-of-t functions).
// These two probes validate the SHIPPED shape directly — no Mode-A delta on top.
// ══════════════════════════════════════════════════════════════════════════════

/// A blast-radius body that changed from the folded (b1) one: the derivation branch
/// ONLY (no rests-on branch, no population branch, no recursion). Applied through the
/// production `define` path (`apply_schema`) to probe the migration posture — if it
/// takes effect, `blast-radius` over a rests-on fixture returns empty; if the folded
/// body stays, it returns the resting item.
const REDEF_CHANGED_BLAST: &str = r#"define
  fun blast-radius($r: okf-concept) -> { memory-item } @doc("Changed body (derivation branch only) — P-REDEF migration probe."):
      match
          (source: $r, derived: $m) isa derivation;
      return { $m };
"#;

/// **P-REDEF** — the §1.4 migration posture: a persisted function's body can only be
/// mutated with `redefine`, so the production apply path emits one.
///
/// From the FOLDED composed schema (`blast-radius` is already the b1 body):
///   1. a raw `define` of a CHANGED body is refused by TypeDB with `[FUN5]` and the
///      stored body stays in force — the engine fact §1.4 rests on;
///   2. a byte-identical raw `define` of an existing function is ALSO refused —
///      `define fun` is not idempotent the way types are;
///   3. `apply_schema` with the same changed body succeeds and the new body is in
///      force, because it plans a `redefine` for every function the database holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shipped_function_mutation_requires_redefine_not_define() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

    // A rests-on-only fixture: the folded b1 body reaches M (rests-on branch); the
    // changed derivation-only body would NOT (M rests on A, no derivation edge).
    insert(
        &db,
        r#"insert
             $a isa belief, has concept-id "redef/A", has belief-state "asserted";
             $m isa belief, has concept-id "redef/M", has belief-state "asserted";
             rests-on (resting: $m, assumption: $a);"#,
    )
    .await;
    let blast_of_a = r#"match
             $a isa belief, has concept-id "redef/A";
             let $x in blast-radius($a);
             $x has concept-id $cid;
           select $cid;"#;

    // 1. Raw `define` of a changed body: refused, stored body untouched.
    let msg = raw_define_error(&db, REDEF_CHANGED_BLAST).await;
    assert!(
        msg.contains("FUN5") || msg.contains("already exists"),
        "a raw define of an existing fun must be refused with [FUN5]/'already exists'; got {msg}"
    );
    assert_eq!(
        read_concept_ids(&db, blast_of_a).await,
        vec!["redef/M".to_string()],
        "after the refused define the folded b1 body must remain in force"
    );

    // 2. Byte-identical raw `define`: also refused.
    let msg = raw_define_error(&db, W3_ASOF_FUNCTIONS).await;
    assert!(
        msg.contains("FUN5") || msg.contains("already exists"),
        "an identical raw define of an existing fun must also be refused; got {msg}"
    );

    // 3. The production apply path lands the changed body.
    apply_delta(&db, REDEF_CHANGED_BLAST)
        .await
        .expect("apply_schema redefines an existing function");
    assert!(
        read_concept_ids(&db, blast_of_a).await.is_empty(),
        "after apply_schema the changed (derivation-only) body must be in force"
    );
}

/// Runs `schema` as a raw `define` schema transaction (bypassing `apply_schema`'s
/// planning) and returns the error it fails with.
async fn raw_define_error(db: &TestDb, schema: &str) -> String {
    let mut router = db.router();
    let tx = router.begin_schema().await.expect("schema tx");
    let result = match tx.query(schema).await {
        Ok(_) => tx.commit().await.map(|_| ()),
        Err(e) => Err(e),
    };
    format!(
        "{:?}",
        result.expect_err("raw define of an existing fun must fail")
    )
}

/// **P-COMP** — the full core → folded-mem → cyber composition is define-clean, and the
/// SHIPPED (folded) schema delivers the W2/W3 acceptance behavior with NO Mode-A delta.
///
/// `apply_composed_schema` applies core, then the folded mem substrate (strip-`define`
/// assembly intact), then cyber, each `.expect`-ing a clean apply — a define collision
/// would panic there. On the composed schema, with no delta, the three acceptance
/// shapes hold from the shipped functions/types alone:
///   (a) `blast-radius(<member concept>)` sweeps the cohort belief (T2 fix, b1 folded);
///   (b) `believed-at(t)` and `population-members-at(P, t)` agree as-of-t (T5 fix);
///   (c) a `divergence-exception` inserts and can be a `contradiction:claim` (W2, C1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_candidate_stack_composes_define_clean_with_cyber() {
    let db = TestDb::new("probe_w2w3");
    // Define-clean: each of core/mem/cyber `.expect`s a clean apply inside this helper;
    // a folded-in define collision (or a broken strip-define assembly) panics here.
    apply_composed_schema(&db).await;

    // (a) T2 fix from the SHIPPED b1 body: blast-radius(member concept) sweeps the cohort
    //     belief with no delta. Population Pa, member Ca1, belief Ba scoped to Pa.
    insert(
        &db,
        r#"insert
             $pa isa population, has spec-hash "comp/pop/A";
             $ca1 isa concept, has concept-id "comp/a/c1";
             $ba isa belief, has concept-id "comp/a/B", has belief-state "asserted";
             scoped (scoped-belief: $ba, population: $pa);
             population-member (owning-population: $pa, member: $ca1);"#,
    )
    .await;
    let cohort_blast = read_concept_ids(
        &db,
        r#"match
             $ca1 isa concept, has concept-id "comp/a/c1";
             let $m in blast-radius($ca1);
             $m has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        cohort_blast,
        vec!["comp/a/B".to_string()],
        "(a) shipped blast-radius(member) must sweep the cohort belief; got {cohort_blast:?}"
    );

    // (b) T5 fix from the SHIPPED window + as-of-t fn: believed-at(t0) and
    //     population-members-at(Pb, t0) agree. Belief Bb scoped to Pb valid-from t0
    //     (the ONLY belief with a valid-from, so believed-at(t0) = {Bb}); member Cb1
    //     from t0, Cb2 from today.
    insert(
        &db,
        r#"insert
             $pb isa population, has spec-hash "comp/pop/B";
             $bb isa belief, has concept-id "comp/b/B", has belief-state "asserted",
                 has valid-from 2026-01-01T00:00:00;
             $cb1 isa concept, has concept-id "comp/b/c1";
             $cb2 isa concept, has concept-id "comp/b/c2";
             scoped (scoped-belief: $bb, population: $pb);
             population-member (owning-population: $pb, member: $cb1),
                 has valid-from 2026-01-01T00:00:00;
             population-member (owning-population: $pb, member: $cb2),
                 has valid-from 2026-07-11T00:00:00;"#,
    )
    .await;
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
        vec!["comp/b/B".to_string()],
        "(b) believed-at(t0) must return Bb only; got {believed:?}"
    );
    let members_t0 = read_concept_ids(
        &db,
        r#"match
             $pb isa population, has spec-hash "comp/pop/B";
             let $m in population-members-at($pb, 2026-01-01T00:00:00);
             $m has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        members_t0,
        vec!["comp/b/c1".to_string()],
        "(b) population-members-at(Pb, t0) must return the t0 cohort {{Cb1}}, agreeing with believed-at(t0); got {members_t0:?}"
    );
    let members_now = read_concept_ids(
        &db,
        r#"match
             $pb isa population, has spec-hash "comp/pop/B";
             let $m in population-members-at($pb, 2026-07-11T00:00:00);
             $m has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        members_now,
        vec!["comp/b/c1".to_string(), "comp/b/c2".to_string()],
        "(b) population-members-at(Pb, today) must grow to {{Cb1, Cb2}} — the fix is time-varying; got {members_now:?}"
    );

    // (c) W2 from the SHIPPED C1 types: a divergence-exception inserts and plays
    //     contradiction:claim (inherited via `sub belief`), and contradicts() holds.
    insert(
        &db,
        r#"insert
             $hub isa anomaly, has concept-id "comp/c/hub";
             $bp isa belief, has concept-id "comp/c/BP", has belief-state "asserted";
             $bi isa belief, has concept-id "comp/c/BI", has belief-state "asserted";
             $e isa divergence-exception, has concept-id "comp/c/E",
                 has belief-state "asserted", has entrenchment "tool-derived";
             divergence-precedence (exception-belief: $e, prevailing: $bi, overridden: $bp);
             contradiction (hub: $hub, claim: $bp, claim: $e);"#,
    )
    .await;
    let contradicting = read_concept_ids(
        &db,
        r#"match
             $e isa belief, has concept-id "comp/c/E";
             $bp isa belief, has concept-id "comp/c/BP";
             true == contradicts($e, $bp);
             $e has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        contradicting,
        vec!["comp/c/E".to_string()],
        "(c) the shipped divergence-exception must be a live contradiction claim (contradicts(E,BP)=true); got {contradicting:?}"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// W5 — the monopoly-surface ratchet (P-W5-1)
//
// SPEC-W5 §1.1 makes the belief-edge (memory-item-substrate) write surface
// INTROSPECTION-DERIVED, not a hand list: a relation is on the surface iff it owns
// at least one role played EXCLUSIVELY by `memory-item` subtypes. An introspected
// surface catches a new belief-edge relation the instant its schema lands — cyber's
// `technique-detection`/`alert-disposition` (the raw-`graph.write` breach at
// `extensions/cyber-gecko/src/detect/mod.rs:142,196`) today, and W2's
// `divergence-precedence` the day W2 lands — whereas INVARIANTS §8.4's hand-list
// misses `divergence-precedence` (and even omits `resolves`/`scoped`).
//
// This probe lives on `conflict_resolution`, whose `apply_composed_schema` reads the
// UNFOLDED mem schema — so `divergence-precedence` is absent until the W2 candidate
// delta is applied, which is what makes the ratchet observable.
// ══════════════════════════════════════════════════════════════════════════════

/// Introspects the composed schema and returns the **belief-edge relation surface**:
/// every relation owning at least one role played EXCLUSIVELY by `memory-item`
/// subtypes. This is the introspection-derived monopoly surface SPEC-W5 §1.1 makes
/// authoritative — no hand-maintained list, so a newly-landed belief-edge relation is
/// caught with no test edit.
///
/// The query is exactly the §1.1 recipe: `$t sub memory-item; $t plays $r` yields the
/// candidate roles; the full player set of each role (`$t plays $r; $rel relates $r`)
/// decides exclusivity host-side; the owning **relation** label is decoded from
/// `label($rel)`. A relation qualifies on ≥1 exclusive role even if its other roles
/// admit records — `divergence-precedence` is caught via `belief plays :prevailing` /
/// `:overridden` though its `exception-scope` role is played by the record-tier
/// `concept`; `resolves`/`scoped` are caught via their belief role though `record-*` /
/// `population` are record/cache tier.
async fn belief_edge_surface(db: &TestDb) -> BTreeSet<String> {
    // 1. The memory-item subtype closure (reflexive: `sub` includes `memory-item`).
    let mi_subtypes: HashSet<String> = read_col(
        db,
        r#"match $t sub memory-item; let $l = label($t); select $l;"#,
        "l",
    )
    .await
    .into_iter()
    .collect();

    // 2. Every (player-type, role, owning-relation) triple in the schema. Roles are
    //    grouped by their (globally-unique) scoped label; the relation is read
    //    directly off `$rel relates $r`, never string-split from the role label.
    let mut router = db.router();
    let tx = router.begin_read().await.expect("read tx");
    let answer = tx
        .query(
            r#"match
                 $t plays $r;
                 $rel relates $r;
                 let $tl = label($t);
                 let $rl = label($r);
                 let $reln = label($rel);
               select $tl, $rl, $reln;"#,
        )
        .await
        .expect("introspection query");
    assert!(answer.is_row_stream(), "introspection returns a row stream");
    let mut rows = answer.into_rows();
    // scoped-role-label -> (owning-relation-label, set of player type labels)
    let mut role_players: HashMap<String, (String, HashSet<String>)> = HashMap::new();
    while let Some(item) = rows.next().await {
        let row = item.expect("introspection row");
        let get = |c: &str| {
            row.get(c)
                .expect("column present")
                .expect("column bound")
                .try_get_string()
                .expect("label is a string")
                .to_string()
        };
        let (player, role, relation) = (get("tl"), get("rl"), get("reln"));
        let entry = role_players
            .entry(role)
            .or_insert_with(|| (relation, HashSet::new()));
        entry.1.insert(player);
    }

    // 3. A relation is on the surface iff some role it owns is played by at least one
    //    type and ONLY by memory-item subtypes.
    let mut surface = BTreeSet::new();
    for (_role, (relation, players)) in role_players {
        if !players.is_empty() && players.iter().all(|p| mi_subtypes.contains(p)) {
            surface.insert(relation);
        }
    }
    surface
}

/// **P-W5-1** — the live monopoly-surface ratchet (SPEC-W5 §1.1 / §2.3).
///
/// Derives the belief-edge relation surface by introspection (`belief_edge_surface`)
/// instead of a hand list, then turns the ratchet: on the unfolded mem schema this
/// branch composes, the surface must already include cyber's `technique-detection`
/// and `alert-disposition` — the raw-`graph.write` breach W5b must route through the
/// sanctioned seam — and must NOT yet include W2's `divergence-precedence`. Applying
/// the W2 candidate delta (`w2_precedence.tql`) makes `divergence-precedence` appear
/// in the surface with NO edit to this test — catching it the instant W2 lands,
/// whereas INVARIANTS §8.4's hand-list would miss it.
///
/// Runs in ISOLATION on `conflict_resolution`: the folded-schema probes in this file
/// are EXPECTED to fail here (the shipped mem schema is unfolded), so run only:
///
/// ```text
/// cargo test --test schema_probe_w2w3 --no-default-features --features cyber p_w5_1 \
///     -- --ignored --test-threads=1
/// ```
/// **W5a surface — graduated from the P-W5-1 ratchet to surface-MEMBERSHIP.**
///
/// W2 has landed (the fold), so the composed schema permanently carries
/// `divergence-precedence`. The pre-fold form toggled the surface 23→24 by applying the
/// W2 candidate as a delta to prove the introspection *grows* when W2 lands; that
/// surface-growth evidence stands at its ratification anchor `95f0d6e` (unfolded base).
/// Post-fold, the meaningful assertion is surface-MEMBERSHIP: the introspection-derived
/// belief-edge surface, computed from the shipped folded schema with NO delta, CONTAINS
/// `divergence-precedence` (via `belief plays :prevailing`/`:overridden`) alongside
/// cyber's belief-referencing relations and the core belief-tier relations INVARIANTS
/// §8.4's hand-list omits. This is the W5a monopoly surface the writers are checked
/// against — no assertion is weakened, only the now-historical toggle is dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p_w5_1_folded_surface_contains_divergence_precedence() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

    // The introspection-derived belief-edge surface on the SHIPPED (folded) schema.
    let surface = belief_edge_surface(&db).await;
    eprintln!("W5a surface (folded core+mem+cyber) = {surface:?}");

    // Membership: W2 landed, so divergence-precedence is on the surface with no delta and
    // no hand list — it qualifies via `belief plays :prevailing`/`:overridden` even though
    // its `exception-scope` role is played by the record-tier `concept`.
    assert!(
        surface.contains("divergence-precedence"),
        "the folded surface must contain divergence-precedence (belief plays :prevailing/:overridden); got {surface:?}"
    );
    // cyber's belief-referencing relations are on the surface without a hand list — the
    // raw-graph.write breach (detect/mod.rs:142,196) is caught by introspection.
    assert!(
        surface.contains("technique-detection") && surface.contains("alert-disposition"),
        "surface must include cyber's technique-detection/alert-disposition; got {surface:?}"
    );
    // Introspection also catches the core belief-tier relations INVARIANTS §8.4's
    // hand-list omits (`resolves`, `scoped`) alongside the ones it names.
    for expected in [
        "derivation",
        "supersession",
        "surfaced",
        "informs-synthesis",
        "resolves",
        "scoped",
    ] {
        assert!(
            surface.contains(expected),
            "surface must include the belief-tier relation `{expected}`; got {surface:?}"
        );
    }
}
