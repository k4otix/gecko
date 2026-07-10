//! A2 schema-level acceptance tests (live TypeDB 3.12).
//!
//! These prove the finalized mem-gecko substrate schema at the SCHEMA level by
//! driving raw parameterized TQL against a live server (`localhost:1729`,
//! admin/password) — they do NOT exercise the Rust `EpistemicWriter` (that is A3).
//!
//! Each test provisions its own throwaway database (unique name), applies the core
//! schema then the mem substrate schema (types + functions, one `define`), inserts a
//! fixture via TQL, reads it back, and drops the database on the way out. The real
//! `gecko` database is never touched.
//!
//! Requires a running TypeDB 3.12 service (CI provides one). If no server is
//! reachable the tests fail loudly rather than silently skipping.

use futures_util::StreamExt;
use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use mem_gecko::MemGecko;
use ulid::Ulid;

use gecko_extension_api::GeckoExtension;

/// Core schema (records tier) that the mem substrate extends additively.
const CORE_SCHEMA: &str = include_str!("../../gecko-engine/schema/core_schema.tql");

/// Provisions a fresh throwaway database, applies core + mem schema, and returns a
/// connected router plus the db name (so the caller can drop it).
async fn fresh_db() -> (TypeDbRouter, String) {
    let name = format!("memtest_{}", Ulid::new().to_string().to_lowercase());
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
    router
        .delete_database(name)
        .await
        .expect("throwaway db drops");
}

/// Runs a write transaction, returning `Ok(())` on commit or the server error text.
async fn write(router: &mut TypeDbRouter, tql: &str) -> Result<(), String> {
    let tx = router.begin_write().await.map_err(|e| e.to_string())?;
    tx.query(tql).await.map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
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

    write(
        &mut router,
        r#"
        insert
          $b1 isa belief, has concept-id "mem/bel/b1", has belief-state "asserted",
              has valid-from 2026-01-01T00:00:00;
          $e1 isa episode, has concept-id "mem/ep/e1",
              has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00;
          (evidenced: $b1, evidence-item: $e1) isa evidence;
          $a1 isa belief, has concept-id "mem/bel/a1", has belief-state "asserted";
          (resting: $b1, assumption: $a1) isa rests-on;
          $b2 isa belief, has concept-id "mem/bel/b2", has belief-state "asserted";
          (superseded: $b1, superseding: $b2) isa supersession,
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
             (evidenced: $b, evidence-item: $e) isa evidence;
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
             (resting: $b, assumption: $a) isa rests-on;
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

    write(
        &mut router,
        r#"
        insert
          $p isa population, has spec-hash "cohort-1", has spec-dialect "typeql-3",
              has spec-text 'match $c isa concept, has tag "internet-facing";';
          $c1 isa concept, has concept-id "asset/c1", has tag "internet-facing";
          $c2 isa concept, has concept-id "asset/c2", has tag "internet-facing";
          $c3 isa concept, has concept-id "asset/c3", has tag "internet-facing";
          # materialized view is STALE: only c1, c2 are cached (c3 tagged later)
          (owning-population: $p, member: $c1) isa population-member, has computed-at 2026-01-01T00:00:00;
          (owning-population: $p, member: $c2) isa population-member, has computed-at 2026-01-01T00:00:00;
          # a belief scopes to the POPULATION, not to each member (invariant 5)
          $b isa belief, has concept-id "mem/bel/pop", has belief-state "asserted";
          (scoped-belief: $b, population: $p) isa scoped;
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
             (scoped-belief: $b, population: $p) isa scoped;
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

    write(
        &mut router,
        r#"
        insert
          $ra isa concept, has concept-id "src-a/evil.com";
          $rb isa concept, has concept-id "src-b/evil.com";
          # a reified resolution BELIEF, asserted, with a PROVABLE derivation-method
          $res isa resolution, has concept-id "mem/bel/res", has belief-state "asserted";
          $ep isa episode, has concept-id "mem/ep/rs",
              has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00;
          (derived: $res, source: $ep) isa derivation, has derivation-method "type-join";
          (resolution-belief: $res, record-a: $ra, record-b: $rb) isa resolves;
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
    write(
        &mut router,
        r#"
        insert
          $p isa population, has spec-hash "cohort-x";
          $b1 isa belief, has concept-id "mem/bel/x1", has belief-state "asserted";
          (scoped-belief: $b1, population: $p) isa scoped;
        "#,
    )
    .await
    .expect("insert population + first belief");

    // Second belief attaches to the SAME population, matched by its @key spec-hash.
    write(
        &mut router,
        r#"
        match $p isa population, has spec-hash "cohort-x";
        insert
          $b2 isa belief, has concept-id "mem/bel/x2", has belief-state "asserted";
          (scoped-belief: $b2, population: $p) isa scoped;
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
             (scoped-belief: $b, population: $p) isa scoped;
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
// A6 Acceptance: the consolidation-state machine (with tombstone semantics) is in
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
        write(
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
    let bad = write(
        &mut router,
        r#"insert $e isa episode, has concept-id "mem/ep/csbad",
             has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00,
             has consolidation-state "dreaming";"#,
    )
    .await;
    assert!(
        bad.is_err(),
        "consolidation-state @values rejects an unknown state"
    );

    drop_db(router, &name).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Acceptance: an illegal enum value is rejected by @values.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn illegal_enum_value_rejected() {
    let (mut router, name) = fresh_db().await;

    let bad = write(
        &mut router,
        r#"insert $b isa belief, has concept-id "mem/bel/bad", has belief-state "bogus";"#,
    )
    .await;
    assert!(bad.is_err(), "belief-state @values rejects \"bogus\"");

    // A legal value still commits (control).
    write(
        &mut router,
        r#"insert $b isa belief, has concept-id "mem/bel/good", has belief-state "asserted";"#,
    )
    .await
    .expect("a legal belief-state commits");

    drop_db(router, &name).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Carry-forward fix: the `pivot` substrate primitive is now instantiable
// (agent + two memory-item states) and reads back.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn pivot_is_instantiable() {
    let (mut router, name) = fresh_db().await;

    write(
        &mut router,
        r#"
        insert
          $ag isa agent, has agent-id "analyst-1";
          $from isa belief, has concept-id "mem/bel/from", has belief-state "asserted";
          $to isa belief, has concept-id "mem/bel/to", has belief-state "asserted";
          (pivoting-agent: $ag, from-state: $from, to-state: $to) isa pivot,
              has pivot-method "hypothesis-refutation", has calibration-score 0.8;
        "#,
    )
    .await
    .expect("insert a fully-played pivot");

    let pivots = fetch(
        &mut router,
        r#"match
             (pivoting-agent: $ag, from-state: $f, to-state: $t) isa pivot;
             $ag has agent-id $aid;
           fetch { "agent": $aid };"#,
    )
    .await;
    assert_eq!(pivots.len(), 1);
    assert_eq!(pivots[0]["agent"], "analyst-1");

    drop_db(router, &name).await;
}
