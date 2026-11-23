//! Schema-level acceptance tests (live TypeDB).
//!
//! These prove the mem-gecko substrate schema at the SCHEMA level by
//! driving raw parameterized TQL against a live server (`localhost:1729`,
//! admin/password) — they do NOT exercise the Rust `EpistemicWriter` (see
//! `writer_reader_integration.rs` for those tests).
//!
//! Each test provisions its own throwaway database (unique name), applies the core
//! schema then the mem substrate schema (types + functions, one `define`), inserts a
//! fixture via TQL, reads it back, and drops the database on the way out. The real
//! `gecko` database is never touched.
//!
//! Requires a running TypeDB service (CI provides one). If no server is
//! reachable the tests fail loudly rather than silently skipping.

use futures_util::StreamExt;
use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_engine::db::run_graph_writes;
use mem_gecko::MemGecko;
use ulid::Ulid;

use gecko_extension_api::GeckoExtension;

mod common;
#[path = "common/provenance.rs"]
mod provenance;

/// Core schema (records tier) that the mem substrate extends additively.
const CORE_SCHEMA: &str = include_str!("../../gecko-engine/schema/core_schema.tql");

/// Provisions a fresh throwaway database, applies core + mem schema, and returns a
/// connected router plus the db name (so the caller can drop it).
async fn fresh_db() -> (TypeDbRouter, String) {
    let name = format!("memtest_{}", Ulid::generate().to_string().to_lowercase());
    let mut router = TypeDbRouter::new(DbConfig {
        address: "localhost:1729".to_string(),
        database: name.clone(),
        username: "admin".to_string(),
        password: "password".to_string(),
        tls: TlsMode::Disabled,
    });
    router
        .apply_schema(CORE_SCHEMA)
        .await
        .expect("core schema applies");
    router
        .apply_schema(MemGecko::new().schema())
        .await
        .expect("mem substrate schema applies (types + functions, one define)");
    (router, name)
}

async fn drop_db(mut router: TypeDbRouter, name: &str) {
    common::drop_db_with_retry(&mut router, name).await;
}

/// Runs a write transaction, returning `Ok(())` on commit or the server error text.
async fn write(router: &mut TypeDbRouter, tql: &str) -> Result<(), String> {
    let tx = router.begin_write().await.map_err(|e| e.to_string())?;
    tx.query(tql).await.map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Runs a write transaction whose raw memory items get their ownership and
/// source-link from [`provenance::seed_provenance`] before the commit.
async fn write_seed(router: &mut TypeDbRouter, tql: &str) -> Result<(), String> {
    let tx = router.begin_write().await.map_err(|e| e.to_string())?;
    tx.query(tql).await.map_err(|e| e.to_string())?;
    run_graph_writes(&tx, &provenance::seed_provenance())
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Whether a refused write's error is an `@values` violation on attribute `attr`:
/// it names both the `@values` constraint and the attribute type.
fn names_values_violation(err: &str, attr: &str) -> bool {
    err.contains("@values") && err.contains(&format!("'{attr}'"))
}

/// Runs a read `fetch` query and returns the result documents as JSON.
async fn fetch(router: &mut TypeDbRouter, tql: &str) -> Vec<serde_json::Value> {
    let tx = router.begin_read().await.expect("read tx");
    let answer = tx.query(tql).await.expect("read query");
    let mut out = Vec::new();
    if answer.is_document_stream() {
        let mut stream = answer.into_documents();
        while let Some(Ok(doc)) = stream.next().await {
            out.push(
                serde_json::from_str(&doc.into_json().to_string())
                    .expect("driver returns valid json"),
            );
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance: schema applies clean (this is `gecko schema-init` at the schema level).
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn schema_applies_clean() {
    let (router, name) = fresh_db().await;
    // If we got here, both core and mem (types + functions) applied in one define
    // each without error. Sanity-read a mem type to confirm the transaction stuck.
    let mut router = router;
    let docs = fetch(
        &mut router,
        r#"match $t label belief; fetch { "kind": "ok" };"#,
    )
    .await;
    assert_eq!(docs.len(), 1, "the mem `belief` type is defined");
    drop_db(router, &name).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance: a belief with evidence + rests-on + supersession round-trips.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn belief_evidence_restson_supersession_roundtrip() {
    let (mut router, name) = fresh_db().await;

    write_seed(
        &mut router,
        r#"
        insert
          $b1 isa belief, has concept-id "mem/bel/b1", has belief-state "asserted",
              has valid-from 2026-01-01T00:00:00;
          $e1 isa episode, has concept-id "mem/ep/e1",
              has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00;
          evidence (evidenced: $b1, evidence-item: $e1);
          $a1 isa belief, has concept-id "mem/bel/a1", has belief-state "asserted";
          rests-on (resting: $b1, assumption: $a1);
          $b2 isa belief, has concept-id "mem/bel/b2", has belief-state "asserted";
          supersession (superseded: $b1, superseding: $b2),
              has created-at 2026-01-02T00:00:00, has supersession-reason "newer evidence";
        "#,
    )
    .await
    .expect("insert belief + evidence + rests-on + supersession");

    // Evidence edge reads back.
    let ev = fetch(
        &mut router,
        r#"match
             $b isa belief, has concept-id "mem/bel/b1";
             evidence (evidenced: $b, evidence-item: $e);
             $e has concept-id $eid;
           fetch { "e": $eid };"#,
    )
    .await;
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["e"], "mem/ep/e1");

    // rests-on assumption reads back.
    let assumptions = fetch(
        &mut router,
        r#"match
             $b isa belief, has concept-id "mem/bel/b1";
             rests-on (resting: $b, assumption: $a);
             $a has concept-id $aid;
           fetch { "a": $aid };"#,
    )
    .await;
    assert_eq!(assumptions.len(), 1);
    assert_eq!(assumptions[0]["a"], "mem/bel/a1");

    // Supersession lineage reads back with its reason.
    let lineage = fetch(
        &mut router,
        r#"match
             $b isa belief, has concept-id "mem/bel/b1";
             $s isa supersession, links (superseded: $b, superseding: $newer);
             $s has supersession-reason $r;
             $newer has concept-id $nid;
           fetch { "newer": $nid, "reason": $r };"#,
    )
    .await;
    assert_eq!(lineage.len(), 1);
    assert_eq!(lineage[0]["newer"], "mem/bel/b2");
    assert_eq!(lineage[0]["reason"], "newer evidence");

    // The is-superseded substrate function agrees.
    let superseded = fetch(
        &mut router,
        r#"match
             $b isa belief, has concept-id "mem/bel/b1";
             true == is-superseded($b);
           fetch { "ok": $b.concept-id };"#,
    )
    .await;
    assert_eq!(superseded.len(), 1, "b1 is superseded per is-superseded()");

    drop_db(router, &name).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance: a population-scoped belief resolves N members via the materialized
// view; a strict read re-evaluates membership by running the stored spec-text
// (which finds MORE than the stale materialized cache — bounded-staleness).
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn population_materialized_and_strict_membership() {
    let (mut router, name) = fresh_db().await;

    write_seed(
        &mut router,
        r#"
        insert
          $p isa population, has spec-hash "cohort-1", has spec-dialect "typeql-3",
              has spec-text 'match $c isa concept, has tag "internet-facing";';
          $c1 isa concept, has concept-id "asset/c1", has tag "internet-facing";
          $c2 isa concept, has concept-id "asset/c2", has tag "internet-facing";
          $c3 isa concept, has concept-id "asset/c3", has tag "internet-facing";
          # materialized view is STALE: only c1, c2 are cached (c3 tagged later)
          population-member (owning-population: $p, member: $c1), has computed-at 2026-01-01T00:00:00;
          population-member (owning-population: $p, member: $c2), has computed-at 2026-01-01T00:00:00;
          # a belief scopes to the POPULATION, not to each member (invariant 5)
          $b isa belief, has concept-id "mem/bel/pop", has belief-state "asserted";
          scoped (scoped-belief: $b, population: $p);
        "#,
    )
    .await
    .expect("insert population + members + scoped belief");

    // Materialized read via the population-members function → 2 (stale cache).
    let materialized = fetch(
        &mut router,
        r#"match
             $p isa population, has spec-hash "cohort-1";
             let $m in population-members($p);
           fetch { "m": $m.concept-id };"#,
    )
    .await;
    assert_eq!(materialized.len(), 2, "materialized view holds 2 members");

    // The belief is scoped to the population.
    let scoped = fetch(
        &mut router,
        r#"match
             $b isa belief, has concept-id "mem/bel/pop";
             scoped (scoped-belief: $b, population: $p);
             $p has spec-hash $h;
           fetch { "h": $h };"#,
    )
    .await;
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0]["h"], "cohort-1");

    // Strict read: fetch the stored spec-text and RUN it verbatim. It re-evaluates
    // membership over live data and finds 3 (c1, c2, c3) — more than the stale cache.
    let spec_docs = fetch(
        &mut router,
        r#"match $p isa population, has spec-hash "cohort-1", has spec-text $t;
           fetch { "t": $t };"#,
    )
    .await;
    let spec_text = spec_docs[0]["t"].as_str().expect("spec-text is a string");
    let strict = fetch(
        &mut router,
        &format!(r#"{spec_text} fetch {{ "c": $c.concept-id }};"#),
    )
    .await;
    assert_eq!(
        strict.len(),
        3,
        "strict re-evaluation finds 3 members (> stale materialized 2)"
    );

    drop_db(router, &name).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance: two source-records with an identical provable `resolution` collapse
// to one canonical-entity view (transitive closure over provable resolutions)
// while BOTH source seams stay queryable (no physical merge — invariant 9).
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn provable_resolution_canonicalizes_without_merge() {
    let (mut router, name) = fresh_db().await;

    write_seed(
        &mut router,
        r#"
        insert
          $ra isa concept, has concept-id "src-a/evil.com";
          $rb isa concept, has concept-id "src-b/evil.com";
          # a reified resolution BELIEF, asserted, with a PROVABLE derivation-method
          $res isa resolution, has concept-id "mem/bel/res", has belief-state "asserted";
          $ep isa episode, has concept-id "mem/ep/rs",
              has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00;
          derivation (derived: $res, source: $ep), has derivation-method "type-join";
          resolves (resolution-belief: $res, record-a: $ra, record-b: $rb);
        "#,
    )
    .await
    .expect("insert two source records + provable resolution");

    // canonical-entity() returns the transitive-closure equivalence class (the
    // fixpoint includes the queried record itself). Both source seams collapse to
    // ONE canonical view: querying either seam yields the same {src-a, src-b} class.
    let mut canon_from_a: Vec<String> = fetch(
        &mut router,
        r#"match
             $r isa concept, has concept-id "src-a/evil.com";
             let $o in canonical-entity($r);
           fetch { "o": $o.concept-id };"#,
    )
    .await
    .into_iter()
    .map(|d| d["o"].as_str().unwrap().to_string())
    .collect();
    canon_from_a.sort();
    canon_from_a.dedup();
    assert_eq!(
        canon_from_a,
        vec!["src-a/evil.com".to_string(), "src-b/evil.com".to_string()],
        "the provable resolution collapses both seams into one canonical class"
    );

    // Querying the OTHER seam yields the identical canonical class — one view.
    let mut canon_from_b: Vec<String> = fetch(
        &mut router,
        r#"match
             $r isa concept, has concept-id "src-b/evil.com";
             let $o in canonical-entity($r);
           fetch { "o": $o.concept-id };"#,
    )
    .await
    .into_iter()
    .map(|d| d["o"].as_str().unwrap().to_string())
    .collect();
    canon_from_b.sort();
    canon_from_b.dedup();
    assert_eq!(
        canon_from_a, canon_from_b,
        "both seams resolve to the same canonical view"
    );

    // Both source seams remain distinct, queryable nodes — no physical merge.
    let seams = fetch(
        &mut router,
        r#"match
             $c isa concept, has concept-id $id;
             { $id == "src-a/evil.com"; } or { $id == "src-b/evil.com"; };
           fetch { "id": $id };"#,
    )
    .await;
    assert_eq!(seams.len(), 2, "both source seams stay queryable");

    drop_db(router, &name).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance: two beliefs over the same `spec-hash` attach to ONE population node
// (spec-hash @key dedups cohorts). A duplicate population insert is rejected.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn same_spec_hash_shares_one_population() {
    let (mut router, name) = fresh_db().await;

    // Create the population once, plus the first scoped belief.
    write_seed(
        &mut router,
        r#"
        insert
          $p isa population, has spec-hash "cohort-x";
          $b1 isa belief, has concept-id "mem/bel/x1", has belief-state "asserted";
          scoped (scoped-belief: $b1, population: $p);
        "#,
    )
    .await
    .expect("insert population + first belief");

    // Second belief attaches to the SAME population, matched by its @key spec-hash.
    write_seed(
        &mut router,
        r#"
        match $p isa population, has spec-hash "cohort-x";
        insert
          $b2 isa belief, has concept-id "mem/bel/x2", has belief-state "asserted";
          scoped (scoped-belief: $b2, population: $p);
        "#,
    )
    .await
    .expect("second belief attaches to existing population");

    // Exactly one population node with that spec-hash…
    let pops = fetch(
        &mut router,
        r#"match $p isa population, has spec-hash "cohort-x";
           fetch { "h": $p.spec-hash };"#,
    )
    .await;
    assert_eq!(pops.len(), 1, "one population node for the spec-hash");

    // …carrying both beliefs.
    let scoped = fetch(
        &mut router,
        r#"match
             $p isa population, has spec-hash "cohort-x";
             scoped (scoped-belief: $b, population: $p);
           fetch { "b": $b.concept-id };"#,
    )
    .await;
    assert_eq!(scoped.len(), 2, "both beliefs attach to the one population");

    // A duplicate population with the same @key spec-hash is rejected.
    let dup = write(
        &mut router,
        r#"insert $p2 isa population, has spec-hash "cohort-x";"#,
    )
    .await;
    assert!(dup.is_err(), "spec-hash @key rejects a duplicate cohort");

    drop_db(router, &name).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance: the consolidation-state machine (with tombstone semantics) is in
// the APPLIED schema — an episode can be marked "tombstoned" and read back, and an
// illegal state is rejected by @values. This is the state-machine-present acceptance.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn consolidation_state_machine_present_with_tombstone() {
    let (mut router, name) = fresh_db().await;

    // Each of the five states is accepted on a memory-item (episode).
    for (i, state) in ["raw", "candidate", "consolidated", "archived", "tombstoned"]
        .iter()
        .enumerate()
    {
        write_seed(
            &mut router,
            &format!(
                r#"insert $e isa episode, has concept-id "mem/ep/cs{i}",
                     has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00,
                     has consolidation-state "{state}";"#
            ),
        )
        .await
        .unwrap_or_else(|e| panic!("consolidation-state \"{state}\" is legal: {e}"));
    }

    // Tombstone semantics read back (the terminal state).
    let tomb = fetch(
        &mut router,
        r#"match $e isa episode, has concept-id "mem/ep/cs4", has consolidation-state $s;
           fetch { "s": $s };"#,
    )
    .await;
    assert_eq!(tomb.len(), 1);
    assert_eq!(tomb[0]["s"], "tombstoned", "the tombstone state is present");

    // An illegal consolidation-state is rejected by @values.
    let bad = write_seed(
        &mut router,
        r#"insert $e isa episode, has concept-id "mem/ep/csbad",
             has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00,
             has consolidation-state "dreaming";"#,
    )
    .await;
    let err = bad.expect_err("consolidation-state @values rejects an unknown state");
    assert!(names_values_violation(&err, "consolidation-state"), "{err}");

    drop_db(router, &name).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance: an illegal enum value is rejected by @values.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn illegal_enum_value_rejected() {
    let (mut router, name) = fresh_db().await;

    let bad = write_seed(
        &mut router,
        r#"insert $b isa belief, has concept-id "mem/bel/bad", has belief-state "bogus";"#,
    )
    .await;
    let err = bad.expect_err("belief-state @values rejects \"bogus\"");
    assert!(names_values_violation(&err, "belief-state"), "{err}");

    // A legal value still commits (control).
    write_seed(
        &mut router,
        r#"insert $b isa belief, has concept-id "mem/bel/good", has belief-state "asserted";"#,
    )
    .await
    .expect("a legal belief-state commits");

    drop_db(router, &name).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance: the `pivot` substrate primitive is instantiable
// (agent + two memory-item states) and reads back.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn pivot_is_instantiable() {
    let (mut router, name) = fresh_db().await;

    write_seed(
        &mut router,
        r#"
        insert
          $ag isa agent, has agent-id "analyst-1";
          $from isa belief, has concept-id "mem/bel/from", has belief-state "asserted";
          $to isa belief, has concept-id "mem/bel/to", has belief-state "asserted";
          pivot (pivoting-agent: $ag, from-state: $from, to-state: $to),
              has pivot-method "hypothesis-refutation", has calibration-score 0.8;
        "#,
    )
    .await
    .expect("insert a fully-played pivot");

    let pivots = fetch(
        &mut router,
        r#"match
             pivot (pivoting-agent: $ag, from-state: $f, to-state: $t);
             $ag has agent-id $aid;
           fetch { "agent": $aid };"#,
    )
    .await;
    assert_eq!(pivots.len(), 1);
    assert_eq!(pivots[0]["agent"], "analyst-1");

    drop_db(router, &name).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance: the schema counts ownership and provenance. A memory item is owned
// exactly once and sourced at least once; anything else is refused at commit.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn a_memory_item_with_two_ownerships_is_refused_at_commit() {
    let (mut router, name) = fresh_db().await;

    let refused = write(
        &mut router,
        r#"
        insert
          $ag isa agent, has agent-id "agent-1";
          $run isa doc-run, has run-id "run-1";
          $b isa belief, has concept-id "mem/bel/two-owners", has belief-state "asserted",
              has valid-from 2026-01-01T00:00:00, has salience 0.5,
              has base-activation 1.0, has last-access 2026-01-01T00:00:00;
          source-link (memory: $b, origin: $run), has origin-kind "test";
          ownership (owned: $b, owner: $ag), has visibility "private";
          ownership (owned: $b, owner: $ag), has visibility "team";
        "#,
    )
    .await;
    let err = refused.expect_err("a second ownership commits");
    assert!(
        err.contains("ownership:owned"),
        "the refusal does not name ownership:owned: {err}"
    );

    drop_db(router, &name).await;
}

#[tokio::test]
async fn a_memory_item_with_no_source_link_is_refused_at_commit() {
    let (mut router, name) = fresh_db().await;

    let refused = write(
        &mut router,
        r#"
        insert
          $ag isa agent, has agent-id "agent-1";
          $b isa belief, has concept-id "mem/bel/unsourced", has belief-state "asserted",
              has valid-from 2026-01-01T00:00:00, has salience 0.5,
              has base-activation 1.0, has last-access 2026-01-01T00:00:00;
          ownership (owned: $b, owner: $ag), has visibility "private";
        "#,
    )
    .await;
    let err = refused.expect_err("a memory item with no source-link commits");
    assert!(
        err.contains("source-link:memory"),
        "the refusal does not name source-link:memory: {err}"
    );

    drop_db(router, &name).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance: every required role of a mem relation is enforced at commit. Each
// case inserts the relation with all its players (a control that commits), then
// again with one required role omitted, and expects the commit to fail naming it.
// ─────────────────────────────────────────────────────────────────────────────

/// The players every role case draws on, inserted once per database.
const ROLE_PLAYERS: &str = r#"
insert
  $b1 isa belief, has concept-id "mem/bel/p1", has belief-state "asserted";
  $b2 isa belief, has concept-id "mem/bel/p2", has belief-state "asserted";
  $ep isa episode, has concept-id "mem/ep/p",
      has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00;
  $an isa anomaly, has concept-id "mem/an/p";
  $res isa resolution, has concept-id "mem/res/p", has belief-state "asserted";
  $de isa divergence-exception, has concept-id "mem/de/p", has belief-state "asserted";
  $re isa retrieval-event, has concept-id "mem/re/p",
      has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00;
  $ag isa agent, has agent-id "agent-p";
  $run isa doc-run, has run-id "run-p";
  $pop isa population, has spec-hash "pop-p";
  $c1 isa concept, has concept-id "c/p1";
  $c2 isa concept, has concept-id "c/p2";
"#;

/// Binds each of [`ROLE_PLAYERS`] to the variable it was inserted under.
const MATCH_PLAYERS: &str = r#"
match
  $b1 isa belief, has concept-id "mem/bel/p1";
  $b2 isa belief, has concept-id "mem/bel/p2";
  $ep isa episode, has concept-id "mem/ep/p";
  $an isa anomaly, has concept-id "mem/an/p";
  $res isa resolution, has concept-id "mem/res/p";
  $de isa divergence-exception, has concept-id "mem/de/p";
  $re isa retrieval-event, has concept-id "mem/re/p";
  $ag isa agent, has agent-id "agent-p";
  $run isa doc-run, has run-id "run-p";
  $pop isa population, has spec-hash "pop-p";
  $c1 isa concept, has concept-id "c/p1";
  $c2 isa concept, has concept-id "c/p2";
"#;

/// One mem relation: its required roles, its other players, the attributes it
/// must own, and a memory item it inserts fresh (`$f`) when a role needs one.
struct RoleCase {
    relation: &'static str,
    required: &'static [(&'static str, &'static str)],
    others: &'static [(&'static str, &'static str)],
    attrs: &'static str,
    fresh: bool,
}

const fn case(
    relation: &'static str,
    required: &'static [(&'static str, &'static str)],
    others: &'static [(&'static str, &'static str)],
    attrs: &'static str,
) -> RoleCase {
    RoleCase {
        relation,
        required,
        others,
        attrs,
        fresh: false,
    }
}

const DERIVATION: RoleCase = case(
    "derivation",
    &[("derived", "$b1")],
    &[("source", "$ep")],
    r#", has derivation-method "type-join""#,
);
const EVIDENCE: RoleCase = case(
    "evidence",
    &[("evidenced", "$b1")],
    &[("evidence-item", "$ep")],
    "",
);
const JUSTIFICATION: RoleCase = case(
    "justification",
    &[("consequent", "$b1")],
    &[("antecedent", "$b2")],
    "",
);
const SUPPORT: RoleCase = case(
    "support",
    &[("supporter", "$b1"), ("supported", "$b2")],
    &[],
    "",
);
const ATTACK: RoleCase = case(
    "attack",
    &[("attacker", "$b1"), ("attacked", "$b2")],
    &[],
    "",
);
const CONTRADICTION: RoleCase = case(
    "contradiction",
    &[("hub", "$an")],
    &[("claim", "$b1"), ("claim", "$b2")],
    "",
);
const SUPERSESSION: RoleCase = case(
    "supersession",
    &[("superseded", "$b1"), ("superseding", "$b2")],
    &[],
    ", has created-at 2026-01-02T00:00:00",
);
const SOURCE_LINK: RoleCase = RoleCase {
    fresh: true,
    ..case(
        "source-link",
        &[("memory", "$f")],
        &[("origin", "$run")],
        r#", has origin-kind "test""#,
    )
};
const OWNERSHIP: RoleCase = RoleCase {
    fresh: true,
    ..case(
        "ownership",
        &[("owned", "$f")],
        &[("owner", "$ag")],
        r#", has visibility "private""#,
    )
};
const RESTS_ON: RoleCase = case(
    "rests-on",
    &[("resting", "$b1")],
    &[("assumption", "$b2")],
    "",
);
const EXECUTION: RoleCase = case(
    "execution",
    &[("run", "$run"), ("episode-step", "$ep")],
    &[],
    "",
);
const CONSOLIDATION_STEP: RoleCase = case(
    "consolidation-step",
    &[("affected", "$b1"), ("run", "$run"), ("performer", "$ag")],
    &[],
    r#", has consolidation-action "dedup", has created-at 2026-01-02T00:00:00"#,
);
const SCOPED: RoleCase = case(
    "scoped",
    &[("scoped-belief", "$b1"), ("population", "$pop")],
    &[],
    "",
);
const POPULATION_MEMBER: RoleCase = case(
    "population-member",
    &[("owning-population", "$pop"), ("member", "$c1")],
    &[],
    "",
);
const RESOLVES: RoleCase = case(
    "resolves",
    &[
        ("resolution-belief", "$res"),
        ("record-a", "$c1"),
        ("record-b", "$c2"),
    ],
    &[],
    "",
);
const SURFACED: RoleCase = case("surfaced", &[("surfacer", "$re"), ("item", "$b1")], &[], "");
const INFORMS_SYNTHESIS: RoleCase = case(
    "informs-synthesis",
    &[("retrieval", "$re"), ("synthesized", "$b1")],
    &[],
    "",
);
const DIVERGENCE_PRECEDENCE: RoleCase = case(
    "divergence-precedence",
    &[("prevailing", "$b1"), ("overridden", "$b2")],
    &[("exception-belief", "$de")],
    "",
);

impl RoleCase {
    /// The insert of this relation with every player except `omit`'s.
    fn insert(&self, omit: Option<&str>) -> String {
        let players: Vec<String> = self
            .required
            .iter()
            .filter(|(role, _)| Some(*role) != omit)
            .chain(self.others)
            .map(|(role, var)| format!("{role}: {var}"))
            .collect();
        let fresh = if self.fresh {
            let id = format!("mem/bel/fresh-{}-{}", self.relation, omit.unwrap_or("all"));
            format!(r#"$f isa belief, has concept-id "{id}", has belief-state "asserted";"#)
        } else {
            String::new()
        };
        format!(
            "{MATCH_PLAYERS} insert {fresh} $r isa {}, links ({}){};",
            self.relation,
            players.join(", "),
            self.attrs
        )
    }
}

/// Seeds the players, commits `case` with every player, and asserts that the
/// commit omitting `role` is refused with an error naming `relation:role`.
async fn assert_missing_role_refused(case: &RoleCase, role: &str) {
    assert!(
        case.required.iter().any(|(r, _)| *r == role),
        "{role} is not a required role of {}",
        case.relation
    );
    let (mut router, name) = fresh_db().await;
    write_seed(&mut router, ROLE_PLAYERS)
        .await
        .expect("insert the role players");
    write_seed(&mut router, &case.insert(None))
        .await
        .unwrap_or_else(|e| panic!("a fully-played {} commits: {e}", case.relation));

    let refused = write_seed(&mut router, &case.insert(Some(role))).await;
    let err = refused.expect_err(&format!("a {} missing its {role} commits", case.relation));
    let named = format!("role type '{}:{role}'", case.relation);
    assert!(
        err.contains(&named),
        "the refusal does not name {named}: {err}"
    );
    drop_db(router, &name).await;
}

macro_rules! missing_role_refused {
    ($($test:ident: $case:ident, $role:literal;)*) => {
        $(
            #[tokio::test]
            async fn $test() {
                assert_missing_role_refused(&$case, $role).await;
            }
        )*
    };
}

missing_role_refused! {
    a_derivation_missing_its_derived_is_refused_at_commit: DERIVATION, "derived";
    a_evidence_missing_its_evidenced_is_refused_at_commit: EVIDENCE, "evidenced";
    a_justification_missing_its_consequent_is_refused_at_commit: JUSTIFICATION, "consequent";
    a_support_missing_its_supporter_is_refused_at_commit: SUPPORT, "supporter";
    a_support_missing_its_supported_is_refused_at_commit: SUPPORT, "supported";
    a_attack_missing_its_attacker_is_refused_at_commit: ATTACK, "attacker";
    a_attack_missing_its_attacked_is_refused_at_commit: ATTACK, "attacked";
    a_contradiction_missing_its_hub_is_refused_at_commit: CONTRADICTION, "hub";
    a_supersession_missing_its_superseded_is_refused_at_commit: SUPERSESSION, "superseded";
    a_supersession_missing_its_superseding_is_refused_at_commit: SUPERSESSION, "superseding";
    a_source_link_missing_its_memory_is_refused_at_commit: SOURCE_LINK, "memory";
    a_ownership_missing_its_owned_is_refused_at_commit: OWNERSHIP, "owned";
    a_rests_on_missing_its_resting_is_refused_at_commit: RESTS_ON, "resting";
    a_execution_missing_its_run_is_refused_at_commit: EXECUTION, "run";
    a_execution_missing_its_episode_step_is_refused_at_commit: EXECUTION, "episode-step";
    a_consolidation_step_missing_its_affected_is_refused_at_commit: CONSOLIDATION_STEP, "affected";
    a_consolidation_step_missing_its_run_is_refused_at_commit: CONSOLIDATION_STEP, "run";
    a_consolidation_step_missing_its_performer_is_refused_at_commit: CONSOLIDATION_STEP, "performer";
    a_scoped_missing_its_scoped_belief_is_refused_at_commit: SCOPED, "scoped-belief";
    a_scoped_missing_its_population_is_refused_at_commit: SCOPED, "population";
    a_population_member_missing_its_owning_population_is_refused_at_commit: POPULATION_MEMBER, "owning-population";
    a_population_member_missing_its_member_is_refused_at_commit: POPULATION_MEMBER, "member";
    a_resolves_missing_its_resolution_belief_is_refused_at_commit: RESOLVES, "resolution-belief";
    a_resolves_missing_its_record_a_is_refused_at_commit: RESOLVES, "record-a";
    a_resolves_missing_its_record_b_is_refused_at_commit: RESOLVES, "record-b";
    a_surfaced_missing_its_surfacer_is_refused_at_commit: SURFACED, "surfacer";
    a_surfaced_missing_its_item_is_refused_at_commit: SURFACED, "item";
    a_informs_synthesis_missing_its_retrieval_is_refused_at_commit: INFORMS_SYNTHESIS, "retrieval";
    a_informs_synthesis_missing_its_synthesized_is_refused_at_commit: INFORMS_SYNTHESIS, "synthesized";
    a_divergence_precedence_missing_its_prevailing_is_refused_at_commit: DIVERGENCE_PRECEDENCE, "prevailing";
    a_divergence_precedence_missing_its_overridden_is_refused_at_commit: DIVERGENCE_PRECEDENCE, "overridden";
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance: a database written under the schema before roles were required,
// holding one-player rows, upgrades to the shipped schema. The upgrade deletes
// the one-player rows and keeps the fully-played ones.
// ─────────────────────────────────────────────────────────────────────────────

/// The core and mem schemas as released before mem's roles were required.
const PRE_CORE: &str =
    include_str!("../../../gecko-bin/tests/fixtures/schemas_pre_tightening/core_schema.tql");
const PRE_MEM_TYPES: &str =
    include_str!("../../../gecko-bin/tests/fixtures/schemas_pre_tightening/mem_types.tql");
const PRE_MEM_FUNCTIONS: &str =
    include_str!("../../../gecko-bin/tests/fixtures/schemas_pre_tightening/mem_functions.tql");

/// The pre-change mem types and functions as one `define`, composed as mem-gecko
/// composes its shipped schema.
fn pre_mem_schema() -> String {
    let functions = PRE_MEM_FUNCTIONS
        .trim_start()
        .strip_prefix("define")
        .expect("the functions fragment starts with define");
    format!("{}\n{functions}", PRE_MEM_TYPES.trim_end())
}

#[tokio::test]
async fn a_database_holding_a_one_player_row_upgrades() {
    let name = format!("memtest_{}", Ulid::generate().to_string().to_lowercase());
    let mut router = TypeDbRouter::new(DbConfig {
        address: "localhost:1729".to_string(),
        database: name.clone(),
        username: "admin".to_string(),
        password: "password".to_string(),
        tls: TlsMode::Disabled,
    });
    router
        .apply_schema(PRE_CORE)
        .await
        .expect("pre-change core");
    router
        .apply_schema(&pre_mem_schema())
        .await
        .expect("pre-change mem");
    write_seed(
        &mut router,
        r#"
        insert
          $b isa belief, has concept-id "mem/bel/up", has belief-state "asserted";
          $ep isa episode, has concept-id "mem/ep/up",
              has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00;
          $p isa population, has spec-hash "pop-up";
          $c isa concept, has concept-id "c/up";
          derivation (derived: $b, source: $ep), has derivation-method "type-join";
          derivation (source: $ep), has derivation-method "type-join";
          population-member (owning-population: $p, member: $c);
          population-member (owning-population: $p);
        "#,
    )
    .await
    .expect("the pre-change schema holds one-player rows");

    let mem = MemGecko::new();
    for _ in 0..2 {
        router
            .apply_schema_with_migrations(CORE_SCHEMA, gecko_engine::db::CORE_MIGRATIONS)
            .await
            .expect("core upgrades");
        router
            .apply_schema_with_migrations(mem.schema(), mem.schema_migrations())
            .await
            .expect("mem upgrades over one-player rows");
    }

    for (rel, full) in [
        ("derivation", "links (derived: $x, source: $y)"),
        (
            "population-member",
            "links (owning-population: $x, member: $y)",
        ),
    ] {
        let all = fetch(
            &mut router,
            &format!(r#"match $r isa {rel}; fetch {{ "kind": "row" }};"#),
        )
        .await;
        let played = fetch(
            &mut router,
            &format!(r#"match $r isa {rel}, {full}; fetch {{ "kind": "row" }};"#),
        )
        .await;
        assert_eq!(all.len(), 1, "{rel}: only the fully-played row is left");
        assert_eq!(played.len(), 1, "{rel}: the fully-played row survives");
    }
    drop_db(router, &name).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance: a belief carries every field recall reads, and scores stay in
// range. A missing field is refused at commit; an out-of-range value at write.
// ─────────────────────────────────────────────────────────────────────────────

/// The fields recall reads on every belief, with a conforming value each.
const BELIEF_FIELDS: [(&str, &str); 4] = [
    ("valid-from", "2026-01-01T00:00:00"),
    ("salience", "0.5"),
    ("base-activation", "1.0"),
    ("last-access", "2026-01-01T00:00:00"),
];

/// A raw belief `id` with its ownership and source-link, every field of
/// [`BELIEF_FIELDS`] except `omit`, and the extra `has` clauses in `extra`.
fn belief_insert(id: &str, omit: Option<&str>, extra: &str) -> String {
    let fields: String = BELIEF_FIELDS
        .iter()
        .filter(|(attr, _)| Some(*attr) != omit)
        .map(|(attr, value)| format!(", has {attr} {value}"))
        .collect();
    format!(
        r#"insert
          $ag isa agent, has agent-id "agent-{id}";
          $run isa doc-run, has run-id "run-{id}";
          $b isa belief, has concept-id "mem/bel/{id}", has belief-state "asserted"{fields}{extra};
          source-link (memory: $b, origin: $run), has origin-kind "test";
          ownership (owned: $b, owner: $ag), has visibility "private";"#
    )
}

/// Commits a belief with every field (the control), then asserts the commit of
/// one without `attr` is refused with an error naming the attribute.
async fn assert_belief_without_refused(attr: &str) {
    let (mut router, name) = fresh_db().await;
    write(&mut router, &belief_insert("full", None, ""))
        .await
        .expect("a belief with every field commits");
    let err = write(&mut router, &belief_insert("partial", Some(attr), ""))
        .await
        .expect_err(&format!("a belief without {attr} commits"));
    assert!(
        err.contains("@card") && err.contains(&format!("type '{attr}'")),
        "the refusal does not name {attr}'s card: {err}"
    );
    drop_db(router, &name).await;
}

#[tokio::test]
async fn a_belief_without_valid_from_is_refused() {
    assert_belief_without_refused("valid-from").await;
}

#[tokio::test]
async fn a_belief_without_salience_is_refused() {
    assert_belief_without_refused("salience").await;
}

#[tokio::test]
async fn a_belief_without_base_activation_is_refused() {
    assert_belief_without_refused("base-activation").await;
}

#[tokio::test]
async fn a_belief_without_last_access_is_refused() {
    assert_belief_without_refused("last-access").await;
}

/// Whether a refused write's error is an `@range` violation on attribute `attr`.
fn names_range_violation(err: &str, attr: &str) -> bool {
    err.contains("@range") && err.contains(&format!("'{attr}'"))
}

#[tokio::test]
async fn a_confidence_of_1_5_is_refused() {
    let (mut router, name) = fresh_db().await;
    write(
        &mut router,
        &belief_insert("edge", None, ", has confidence 1.0"),
    )
    .await
    .expect("a confidence of 1.0 commits");
    let err = write(
        &mut router,
        &belief_insert("over", None, ", has confidence 1.5"),
    )
    .await
    .expect_err("a confidence of 1.5 commits");
    assert!(names_range_violation(&err, "confidence"), "{err}");
    drop_db(router, &name).await;
}

#[tokio::test]
async fn a_negative_candidate_count_is_refused() {
    let (mut router, name) = fresh_db().await;
    let event = |id: &str, count: i64| {
        format!(
            r#"insert $re isa retrieval-event, has concept-id "mem/re/{id}",
                   has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00,
                   has candidate-count {count};"#
        )
    };
    write_seed(&mut router, &event("zero", 0))
        .await
        .expect("a candidate-count of 0 commits");
    let err = write_seed(&mut router, &event("neg", -1))
        .await
        .expect_err("a negative candidate-count commits");
    assert!(names_range_violation(&err, "candidate-count"), "{err}");
    drop_db(router, &name).await;
}

#[tokio::test]
async fn a_similarity_score_outside_minus_1_to_1_is_refused() {
    let (mut router, name) = fresh_db().await;
    write_seed(
        &mut router,
        r#"insert
          $b isa belief, has concept-id "mem/bel/s", has belief-state "asserted";
          $re isa retrieval-event, has concept-id "mem/re/s",
              has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00;"#,
    )
    .await
    .expect("seed the surfacer and item");
    let surfaced = |score: f64| {
        format!(
            r#"match $b isa belief, has concept-id "mem/bel/s";
                     $re isa retrieval-event, has concept-id "mem/re/s";
               insert surfaced (surfacer: $re, item: $b), has similarity-score {score:?};"#
        )
    };
    for score in [-1.0, 1.0] {
        write(&mut router, &surfaced(score))
            .await
            .unwrap_or_else(|e| panic!("a similarity-score of {score} commits: {e}"));
    }
    for score in [-1.01, 1.01] {
        let err = write(&mut router, &surfaced(score))
            .await
            .expect_err(&format!("a similarity-score of {score} commits"));
        assert!(names_range_violation(&err, "similarity-score"), "{err}");
    }
    drop_db(router, &name).await;
}

/// Seeds raw memory items with `seed`, commits `control(ok)` and asserts that
/// `control(bad)` is refused with an `@range` violation naming `attr`.
async fn assert_out_of_range_refused<F>(seed: &str, attr: &str, ok: &str, bad: &str, control: F)
where
    F: Fn(&str) -> String,
{
    let (mut router, name) = fresh_db().await;
    if !seed.is_empty() {
        write_seed(&mut router, seed)
            .await
            .expect("seed the memory items");
    }
    write_seed(&mut router, &control(ok))
        .await
        .unwrap_or_else(|e| panic!("a {attr} of {ok} commits: {e}"));
    let err = write_seed(&mut router, &control(bad))
        .await
        .expect_err(&format!("a {attr} of {bad} commits"));
    assert!(names_range_violation(&err, attr), "{err}");
    drop_db(router, &name).await;
}

#[tokio::test]
async fn a_derivation_confidence_of_1_5_is_refused() {
    assert_out_of_range_refused(
        r#"insert $d isa belief, has concept-id "mem/bel/derived", has belief-state "asserted";
                  $s isa belief, has concept-id "mem/bel/source", has belief-state "asserted";"#,
        "confidence",
        "1.0",
        "1.5",
        |value| {
            format!(
                r#"match $d isa belief, has concept-id "mem/bel/derived";
                         $s isa belief, has concept-id "mem/bel/source";
                   insert derivation (derived: $d, source: $s),
                       has derivation-method "human-assertion", has confidence {value};"#
            )
        },
    )
    .await;
}

#[tokio::test]
async fn a_predicted_probability_of_1_5_is_refused() {
    assert_out_of_range_refused(
        r#"insert $b isa belief, has concept-id "mem/bel/predicted", has belief-state "asserted";
                  $e isa episode, has concept-id "mem/ep/resolving",
                      has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00;"#,
        "predicted-probability",
        "1.0",
        "1.5",
        |value| {
            format!(
                r#"match $b isa belief, has concept-id "mem/bel/predicted";
                         $e isa episode, has concept-id "mem/ep/resolving";
                   insert prediction-resolution (predicted-belief: $b, resolving-episode: $e),
                       has predicted-probability {value};"#
            )
        },
    )
    .await;
}

#[tokio::test]
async fn a_negative_access_count_is_refused() {
    assert_out_of_range_refused("", "access-count", "0", "-1", |value| {
        let id = if value.starts_with('-') {
            "neg"
        } else {
            "zero"
        };
        format!(
            r#"insert $b isa belief, has concept-id "mem/bel/{id}", has belief-state "asserted",
                   has access-count {value};"#
        )
    })
    .await;
}

#[tokio::test]
async fn a_negative_corroboration_count_is_refused() {
    assert_out_of_range_refused("", "corroboration-count", "0", "-1", |value| {
        let id = if value.starts_with('-') { "neg" } else { "zero" };
        format!(
            r#"insert $p isa playbook, has concept-id "mem/pb/{id}", has corroboration-count {value};"#
        )
    })
    .await;
}
