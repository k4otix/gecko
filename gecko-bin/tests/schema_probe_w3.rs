//! Schema probes over the composed production schema: core, then mem (the
//! always-on substrate), then cyber, applied as three separate schema transactions
//! as `apply_all_schemas` applies them, against a throwaway TypeDB.
//!
//! The probes hand-author raw TypeQL, bypassing the Rust writers, to check that the
//! composition defines cleanly, that the as-of-t identity function
//! `canonical-entity-at` returns the epoch-correct closure, that a persisted
//! function's body changes only through `redefine`, and that the introspected
//! belief-edge surface holds the core and cyber belief-tier relations.
//!
//! Gated on `cyber` (the probes compose the cyber schema) and needs the multi-thread
//! runtime the TypeDB driver expects.
#![cfg(feature = "cyber")]

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

/// Applies a single `.tql` fragment as its own schema transaction through the
/// production `apply_schema` path, on top of the composed schema, and returns the
/// raw result.
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
    let db = TestDb::new("probe_w3");
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
// The shipped shape: composition (P-COMP) and migration posture (P-REDEF)
//
// `apply_composed_schema` reads the shipped schema files, so these probes check the
// shipped shape directly, with no delta on top.
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
    let db = TestDb::new("probe_w3");
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

/// **P-COMP** — the full core → mem → cyber composition is define-clean, and the
/// shipped types carry a contradiction with no delta on top.
///
/// `apply_composed_schema` applies core, then the mem substrate (strip-`define`
/// assembly intact), then cyber, each `.expect`-ing a clean apply — a define collision
/// would panic there. On the composed schema, two plain beliefs grouped under a
/// `contradiction` hub make `contradicts()` hold.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn composed_schema_is_define_clean_with_cyber() {
    let db = TestDb::new("probe_w3");
    // Define-clean: each of core/mem/cyber `.expect`s a clean apply inside this helper;
    // a define collision (or a broken strip-define assembly) panics here.
    apply_composed_schema(&db).await;

    insert(
        &db,
        r#"insert
             $hub isa anomaly, has concept-id "comp/c/hub";
             $bp isa belief, has concept-id "comp/c/BP", has belief-state "asserted";
             $bi isa belief, has concept-id "comp/c/BI", has belief-state "asserted";
             contradiction (hub: $hub, claim: $bp, claim: $bi);"#,
    )
    .await;
    let contradicting = read_concept_ids(
        &db,
        r#"match
             $bi isa belief, has concept-id "comp/c/BI";
             $bp isa belief, has concept-id "comp/c/BP";
             true == contradicts($bi, $bp);
             $bi has concept-id $cid;
           select $cid;"#,
    )
    .await;
    assert_eq!(
        contradicting,
        vec!["comp/c/BI".to_string()],
        "the shipped contradiction must make contradicts(BI,BP) hold; got {contradicting:?}"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// W5 — the belief-edge surface (P-W5-1)
//
// SPEC-W5 §1.1 makes the belief-edge (memory-item-substrate) write surface
// INTROSPECTION-DERIVED, not a hand list: a relation is on the surface iff it owns
// at least one role played EXCLUSIVELY by `memory-item` subtypes. An introspected
// surface catches a new belief-edge relation the instant its schema lands, such as
// cyber's `technique-detection`/`alert-disposition` (written by cyber through the
// belief-edge seam), whereas INVARIANTS §8.4's hand-list omits `resolves`.
// ══════════════════════════════════════════════════════════════════════════════

/// **P-W5-1** — the introspection-derived belief-edge surface, computed from the
/// shipped composed schema with no delta, contains cyber's belief-referencing
/// relations and the core belief-tier relations, including the `resolves` that
/// INVARIANTS §8.4's hand-list omits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p_w5_1_surface_contains_core_and_cyber_relations() {
    let db = TestDb::new("probe_w3");
    apply_composed_schema(&db).await;

    let surface = live_belief_tier_surface(&db).await.relations;
    eprintln!("belief-tier surface (core+mem+cyber) = {surface:?}");

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
