//! Schema acceptance rig for the W2/W3 conflict work, a recreation of
//! Deliverable-3 (`docs/refactor/CONFLICT-REPORT.md`, Appendix).
//!
//! It composes the production schema — core, then mem (always-on substrate), then
//! cyber — as three separate schema transactions (faithful to `apply_all_schemas`)
//! against a throwaway TypeDB, then hand-authors raw TypeQL (bypassing the Rust
//! writers — the D3 method) to check the W2 divergence-exception shape
//! (`probes/w2_precedence.tql`), the as-of-t identity functions, the function
//! migration posture and the belief-edge surface.
//!
//! The graduated tests (no `#[ignore]`) assert the shipped mem schema directly:
//! `divergence-exception`/`divergence-precedence` and the as-of-t functions are in
//! it. They run under `just test-it` and CI's TypeDB job, as `cyber_detect_test`
//! does, and apply no candidate delta: a re-applied type delta is a no-op
//! (idempotent `define`), and a re-applied function delta would be `redefine`d by
//! `apply_schema` (see `shipped_function_mutation_requires_redefine_not_define`),
//! replacing the shipped body.
//!
//! The probe that stays `#[ignore]` tests a construct that is not shipped — the C2
//! `out-entrenches` predicate (`probes/w2_disposition.tql`, `p_disp_1`) — kept as
//! manual evidence for the owed and rejected work (SPEC-W2-W3 §5). A probe that
//! fails as expected (P-DISP-2) is a verdict, not a bug: it certifies the
//! persisted-vs-host split. Run the ignored probe with:
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
use gecko_engine::db::run_graph_writes;
use gecko_engine::extension::GeckoExtension;
use mem_gecko::MemGecko;

mod common;
#[path = "common/provenance.rs"]
mod provenance;
#[path = "common/surface.rs"]
mod surface;
use common::TestDb;
use surface::live_belief_tier_surface;

const CORE_SCHEMA: &str = include_str!("../../core/gecko-engine/schema/core_schema.tql");

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

/// Runs a raw-TQL write as one committed transaction against the test DB, giving
/// each raw memory item the ownership and source-link the schema requires.
async fn insert(db: &TestDb, query: &str) {
    let mut router = db.router();
    let tx = router.begin_write().await.expect("write tx");
    tx.query(query).await.expect("insert query");
    run_graph_writes(&tx, &provenance::seed_provenance())
        .await
        .expect("seed provenance");
    tx.commit().await.expect("commit");
}

/// Runs a raw-TQL read whose final stage is `select $cid;` (one `concept-id`
/// column) and collects the string values, sorted for deterministic assertions.
async fn read_concept_ids(db: &TestDb, query: &str) -> Vec<String> {
    read_col(db, query, "cid").await
}

// ══════════════════════════════════════════════════════════════════════════════
// W2 — precedence (candidate C1, `probes/w2_precedence.tql`)
// ══════════════════════════════════════════════════════════════════════════════

/// Reads back the disposition belief anchoring an instance belief's precedence.
/// A probe-scaffolding function taking `($inst: belief)` (P-PREC-2's read-back).
const PREC_DISPOSITION_FN: &str = r#"define
  fun disposition-of($inst: belief) -> { divergence-exception } @doc("The disposition (divergence-exception) belief anchoring a precedence in which $inst is the prevailing belief."):
      match
          divergence-precedence (exception-belief: $e, prevailing: $inst);
      return { $e };
"#;

/// The retractability-aware read: the disposition, but only while its anchoring
/// exception belief is live (not superseded). P-PREC-4's live-disposition read.
const PREC_LIVE_DISPOSITION_FN: &str = r#"define
  fun live-disposition-of($inst: belief) -> { divergence-exception } @doc("The disposition belief anchoring $inst's precedence, only while that exception belief is not superseded."):
      match
          divergence-precedence (exception-belief: $e, prevailing: $inst);
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
    // the general belief BP, scoped to member C1.
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
             divergence-precedence (exception-belief: $e, prevailing: $bi, overridden: $bp);
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
             divergence-precedence (exception-belief: $e, exception-scope: $c);
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

/// **P-PREC-2** — the T4 general-vs-instance ground state: the general belief the
/// exception overrides stays `"asserted"` for other members, and the disposition is
/// readable back via a fn taking `($inst: belief)`.
///
/// The exception is member-scoped (it carries the scope, not a supersession), so
/// overriding the general belief for C1 leaves it asserted for C2. This is the R3
/// clause: member-scoping is on the exception, never on `supersession`.
///
/// **Graduation scope (owed half).** This gate asserts only the *schema* half of
/// the §5 row — the member-scoped exception shape defines, inserts, and reads back.
/// The owed behavioral half is the co-mint writer that mints the exception belief
/// and its `divergence-precedence` atomically through the W5b seam (ledger #8),
/// plus the anchor-less-write refusal (ledger #13); both are unbuilt, so a green
/// here is not evidence any writer mints this shape.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_scoped_exception_leaves_the_overridden_belief_asserted_for_others() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;
    apply_delta(&db, PREC_DISPOSITION_FN)
        .await
        .expect("disposition-of read-back fn defines");

    // T4 ground state: members C1, C2 and the general belief BP; instance belief BI
    // diverges for C1; exception belief E dispositions BI over BP, scoped to C1. BP
    // stays asserted (it still governs C2).
    insert(
        &db,
        r#"insert
             $c1 isa concept, has concept-id "prec2/c1";
             $c2 isa concept, has concept-id "prec2/c2";
             $bp isa belief, has concept-id "prec2/BP", has belief-state "asserted";
             $bi isa belief, has concept-id "prec2/BI", has belief-state "asserted";
             $e isa divergence-exception, has concept-id "prec2/E",
                 has belief-state "asserted", has entrenchment "tool-derived";
             divergence-precedence
                 (exception-belief: $e, prevailing: $bi, overridden: $bp, exception-scope: $c1);"#,
    )
    .await;

    // The general belief is untouched — still "asserted" for the other members.
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
        "the general belief BP must stay asserted despite the member-scoped exception; got {bp_state:?}"
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
             divergence-precedence (exception-belief: $e, exception-scope: $c);
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
/// `contradiction` hub can group the general belief, the instance belief and the
/// exception belief as co-claims — something a `canonical-entity` *view* (a closure
/// of records, not a belief) can never do. `contradicts(E, BP)` is then true.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn divergence_exception_is_a_belief_and_plays_contradiction_claim_holds() {
    let db = TestDb::new("probe_w2w3");
    apply_composed_schema(&db).await;

    // A contradiction hub over the general belief, the instance belief, and the
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
/// `is-superseded`) to empty, and the pre-exception state — the general belief
/// still `"asserted"` — is answerable again. Retracting the disposition is just a
/// supersession of its anchoring belief, no bare-edge deletion.
///
/// **Graduation scope (owed half).** This gate asserts only the *read* half of the
/// §5 row — supersession of the anchoring belief flips the live-disposition read to
/// empty, with no bare-edge delete. The owed behavioral half is a writer that
/// performs the retraction-with-lineage (the co-mint writer, ledger #8); it is
/// unbuilt, so a green here does not verify any writer retracts with correct
/// provenance.
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

    // The pre-exception state is answerable again: the general belief still stands.
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
        "the overridden general belief must remain asserted after retraction; got {bp_state:?}"
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
    // The beliefs get their ownership and source-link, so the anchor is the only
    // cardinality the commit can refuse.
    run_graph_writes(&tx, &provenance::seed_provenance())
        .await
        .expect("seed provenance");

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
        msg.contains("divergence-precedence:exception-belief"),
        "the commit must fail specifically on the exception-belief @card(1) violation; \
         got: {msg}"
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
          derivation (derived: $a, source: $sa), has derivation-method $ma;
          derivation (derived: $b, source: $sb), has derivation-method $mb;
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
             derivation (derived: $b, source: $src), has derivation-method $dm;
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
// W3 — as-of-t identity functions (C4b)
// ══════════════════════════════════════════════════════════════════════════════

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
    // `apply_composed_schema` provides the as-of-t functions, so this probe reads the
    // shipped `canonical-entity-at`; re-applying its definition fails with [FUN5]
    // already-exists (see P-REDEF).
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

// ══════════════════════════════════════════════════════════════════════════════
// Mode B — the folded shipped shape: composition (P-COMP) + migration posture (P-REDEF)
//
// After the Mode-B fold, `apply_composed_schema` reads the mem schema files that now
// ALREADY carry the winning candidates (divergence-exception + divergence-precedence,
// and the as-of-t functions).
// These two probes validate the SHIPPED shape directly — no Mode-A delta on top.
// ══════════════════════════════════════════════════════════════════════════════

/// A `blast-radius` body changed from the shipped one: the derivation branch only, with
/// no rests-on branch and no recursion. Applied through the production `define` path
/// (`apply_schema`) to probe the migration posture: if it takes effect, `blast-radius`
/// over a rests-on fixture returns empty; if the shipped body stays, it returns the
/// resting item.
const REDEF_CHANGED_BLAST: &str = r#"define
  fun blast-radius($r: memory-item) -> { memory-item } @doc("Changed body (derivation branch only) — P-REDEF migration probe."):
      match
          derivation (source: $r, derived: $m);
      return { $m };
"#;

/// A shipped mem function, byte-identical to its text in `mem_functions.tql`.
const SHIPPED_CANONICAL_STEP: &str = r#"define
  fun canonical-step($rec: concept) -> { concept } @doc("The source records an asserted, provable resolution belief resolves $rec directly to, in either record role."):
      match
          $res isa resolution, has belief-state "asserted";
          true == provably-derived($res);
          { resolves (resolution-belief: $res, record-a: $rec, record-b: $other); }
          or
          { resolves (resolution-belief: $res, record-a: $other, record-b: $rec); };
      return { $other };
"#;

/// **P-REDEF** — the §1.4 migration posture: a persisted function's body can only be
/// mutated with `redefine`, so the production apply path emits one.
///
/// From the shipped composed schema:
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

    // A rests-on-only fixture: the shipped body reaches M (rests-on branch); the
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
        "after the refused define the shipped body must remain in force"
    );

    // 2. Byte-identical raw `define`: also refused.
    let shipped_body = SHIPPED_CANONICAL_STEP
        .strip_prefix("define\n")
        .expect("a define block");
    assert!(
        MemGecko::new().schema().contains(shipped_body),
        "SHIPPED_CANONICAL_STEP drifted from mem_functions.tql"
    );
    let msg = raw_define_error(&db, SHIPPED_CANONICAL_STEP).await;
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
/// would panic there. On the composed schema, with no delta, the W2 acceptance shape
/// holds from the shipped types alone: a `divergence-exception` inserts and can be a
/// `contradiction:claim` (C1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_candidate_stack_composes_define_clean_with_cyber() {
    let db = TestDb::new("probe_w2w3");
    // Define-clean: each of core/mem/cyber `.expect`s a clean apply inside this helper;
    // a folded-in define collision (or a broken strip-define assembly) panics here.
    apply_composed_schema(&db).await;

    // W2 from the SHIPPED C1 types: a divergence-exception inserts and plays
    // contradiction:claim (inherited via `sub belief`), and contradicts() holds.
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
        "the shipped divergence-exception must be a live contradiction claim (contradicts(E,BP)=true); got {contradicting:?}"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// W5 — the monopoly-surface ratchet (P-W5-1)
//
// SPEC-W5 §1.1 makes the belief-edge (memory-item-substrate) write surface
// INTROSPECTION-DERIVED, not a hand list: a relation is on the surface iff it owns
// at least one role played EXCLUSIVELY by `memory-item` subtypes. An introspected
// surface catches a new belief-edge relation the instant its schema lands — cyber's
// `technique-detection`/`alert-disposition` (written by cyber through the
// belief-edge seam) today, and W2's
// `divergence-precedence` the day W2 lands — whereas INVARIANTS §8.4's hand-list
// misses `divergence-precedence` (and even omits `resolves`).
//
// `apply_composed_schema` composes the shipped mem schema, which has no
// `divergence-precedence`; the ratchet becomes observable once the W2 candidate delta
// is applied.
// ══════════════════════════════════════════════════════════════════════════════

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
    let surface = live_belief_tier_surface(&db).await.relations;
    eprintln!("W5a surface (folded core+mem+cyber) = {surface:?}");

    // Membership: W2 landed, so divergence-precedence is on the surface with no delta and
    // no hand list — it qualifies via `belief plays :prevailing`/`:overridden` even though
    // its `exception-scope` role is played by the record-tier `concept`.
    assert!(
        surface.contains("divergence-precedence"),
        "the folded surface must contain divergence-precedence (belief plays :prevailing/:overridden); got {surface:?}"
    );
    // cyber's belief-referencing relations are on the surface without a hand list;
    // cyber writes them through the belief-edge seam.
    assert!(
        surface.contains("technique-detection") && surface.contains("alert-disposition"),
        "surface must include cyber's technique-detection/alert-disposition; got {surface:?}"
    );
    // Introspection also catches the core belief-tier relations INVARIANTS §8.4's
    // hand-list omits (`resolves`) alongside the ones it names.
    for expected in [
        "derivation",
        "supersession",
        "surfaced",
        "informs-synthesis",
        "resolves",
    ] {
        assert!(
            surface.contains(expected),
            "surface must include the belief-tier relation `{expected}`; got {surface:?}"
        );
    }
}
