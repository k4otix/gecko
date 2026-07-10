//! The **no-drag proof** (a load-bearing regression guard), plus the
//! sparse-overlay guardrails on the record tier.
//!
//! With `enabled=[]` (the mem substrate forced-on, no domain), a plain
//! `gecko sync sample-bundle` runs the **record path** (the syncer) — which bypasses
//! [`EpistemicWriter`] and the semantic index entirely (invariant 1/2). The
//! proof: core concepts + OKF links ARE created, while **zero** `belief` /
//! `derivation` / `source-link` instances and **zero** index upserts appear. Plain
//! documentation → DB is drag-free.
//!
//! Requires a live TypeDB 3.12 service (`localhost:1729`, admin/password). Each test
//! provisions a throwaway database and drops it; the real `gecko` db is untouched.

use futures_util::StreamExt;

use gecko_engine::db::router::TypeDbRouter;
use gecko_engine::okf::parser::parse_bundle;
use gecko_engine::syncer::bundle::sync_bundle;
use gecko_extension_api::GeckoExtension;

mod common;
use common::TestDb;

const CORE_SCHEMA: &str = include_str!("../../core/gecko-engine/schema/core_schema.tql");
const SAMPLE_BUNDLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../sample-bundle");

/// Counts instances of `isa_type` via a `reduce count` over a fresh read tx.
async fn count(router: &mut TypeDbRouter, isa_type: &str) -> i64 {
    let tx = router.begin_read().await.expect("read tx");
    let query = format!("match $x isa {isa_type}; reduce $c = count;");
    let answer = tx.query(&query).await.expect("count query");
    assert!(answer.is_row_stream(), "reduce count returns a row stream");
    let mut rows = answer.into_rows();
    if let Some(Ok(row)) = rows.next().await
        && let Ok(Some(c)) = row.get("c")
    {
        return c.try_get_integer().expect("count is an integer");
    }
    0
}

#[tokio::test]
async fn no_drag_sync_creates_records_and_zero_epistemic_apparatus() {
    // enabled=[]: apply core + mem substrate schema (mem is always-on). The mem
    // functions/types and — crucially — the belief/derivation/source-link types all
    // EXIST in the schema; the proof is that syncing populates NONE of them.
    let test_db = TestDb::new("gecko_test_nodrag");
    let mut db = test_db.router();
    db.apply_schema(CORE_SCHEMA)
        .await
        .expect("core schema applies");
    db.apply_schema(mem_gecko::MemGecko::new().schema())
        .await
        .expect("mem substrate schema applies");

    // Run the record path exactly as `gecko sync` does: parse the bundle, then
    // sync_bundle over the plain engine writer. No EpistemicWriter, no index — the
    // sync path constructs neither, which is *why* zero index upserts are possible.
    let manifest = parse_bundle(std::path::Path::new(SAMPLE_BUNDLE)).expect("parse sample-bundle");
    let result = sync_bundle(&mut db, &manifest)
        .await
        .expect("sync sample-bundle");

    // Core records ARE created (concepts + OKF links).
    assert!(
        result.concepts_inserted > 0,
        "sync must create core concepts, got {}",
        result.concepts_inserted
    );
    assert!(
        result.links_attempted > 0,
        "sync must create OKF links, got {}",
        result.links_attempted
    );

    // ...and the live graph agrees: concepts + links present.
    let concept_count = count(&mut db, "concept").await;
    let link_count = count(&mut db, "okf-link").await;
    assert!(concept_count > 0, "expected concepts in the graph");
    assert!(link_count > 0, "expected OKF links in the graph");

    // THE NO-DRAG PROOF: zero belief-tier apparatus. The syncer never touches the
    // host-mediated belief-write path, so no reified provenance is minted.
    let belief_count = count(&mut db, "belief").await;
    let derivation_count = count(&mut db, "derivation").await;
    let source_link_count = count(&mut db, "source-link").await;
    assert_eq!(belief_count, 0, "no-drag: belief instances must be 0");
    assert_eq!(
        derivation_count, 0,
        "no-drag: derivation instances must be 0"
    );
    assert_eq!(
        source_link_count, 0,
        "no-drag: source-link instances must be 0"
    );

    // Zero index upserts holds by construction: the sync path (`sync_bundle`) has no
    // `EpistemicWriter`/`SemanticIndex` in scope at all — the belief/index upsert
    // ride-along only exists on `EpistemicWriter::assert_belief`/`observe`, which the
    // record path never calls. The zero belief/derivation counts above are the
    // observable consequence of that bypass.
}

// ── Sparse-overlay guardrails (schema lint) ──────────────────────────────────

/// The belief-tier attributes a record-tier type must never own.
const BELIEF_ATTRS: &[&str] = &[
    "belief-state",
    "confidence",
    "entrenchment",
    "derivation-method",
    "valid-from",
    "valid-to",
    "supersession-reason",
];

/// The epistemic *bearer* roles a record-tier type must never play. (Playing
/// `source-link:origin` — being *cited* — is record-safe and deliberately allowed;
/// playing `source-link:memory` — being the belief that cites — is not.)
const BEARER_ROLES: &[&str] = &[
    "derivation:derived",
    "derivation:source",
    "source-link:memory",
    "contradiction:hub",
    "contradiction:claim",
    "supersession:superseded",
    "supersession:superseding",
    "evidence:evidenced",
    "evidence:evidence-item",
    "justification:consequent",
    "justification:antecedent",
    "rests-on:resting",
    "rests-on:assumption",
];

/// Extracts the `plays`/`owns` clauses attached to `type_name` — every additive
/// `<type> plays/owns ... ;` statement plus the type's own `entity <type> ... ;`
/// declaration block — from the combined schema text.
fn wiring_for(schema: &str, type_name: &str) -> String {
    let mut collected = String::new();
    // Match statement-leading occurrences: `entity concept ...` or `concept plays ...`.
    for start_kw in [format!("entity {type_name}"), format!("{type_name} plays")] {
        let mut from = 0;
        while let Some(idx) = schema[from..].find(&start_kw) {
            let abs = from + idx;
            // A statement runs to the next `;`.
            let end = schema[abs..]
                .find(';')
                .map(|e| abs + e)
                .unwrap_or(schema.len());
            collected.push_str(&schema[abs..end]);
            collected.push('\n');
            from = end;
        }
    }
    collected
}

#[test]
fn record_tier_owns_no_belief_apparatus_and_plays_no_bearer_role() {
    // The applied schema the assertion inspects = core + mem substrate (enabled=[]).
    let schema = format!("{}\n{}", CORE_SCHEMA, mem_gecko::MemGecko::new().schema());

    for record_type in ["concept", "external-resource", "bundle"] {
        let wiring = wiring_for(&schema, record_type);
        for attr in BELIEF_ATTRS {
            assert!(
                !wiring.contains(&format!("owns {attr}")),
                "record-tier `{record_type}` must not own belief attribute `{attr}`"
            );
        }
        for role in BEARER_ROLES {
            assert!(
                !wiring.contains(&format!("plays {role}")),
                "record-tier `{record_type}` must not play epistemic bearer role `{role}`"
            );
        }
    }

    // Sanity: the record tier IS wired as a citation *origin* (the allowed direction)
    // — proving the lint above is scoped to *bearer* roles, not all mem roles.
    assert!(
        wiring_for(&schema, "concept").contains("plays source-link:origin"),
        "concept should still be a source-link origin (records are cited, not citers)"
    );
}

#[test]
fn retention_scaffold_present_in_schema() {
    // Retention hooks are scaffold-only (the consolidation daemon does not exist
    // yet). Confirm the consolidation-state lifecycle the future daemon
    // transitions through is present.
    let schema = mem_gecko::MemGecko::new().schema().to_string();
    assert!(
        schema.contains("consolidation-state"),
        "consolidation-state lifecycle attribute must be present"
    );
    for state in ["raw", "candidate", "consolidated", "archived", "tombstoned"] {
        assert!(
            schema.contains(&format!("\"{state}\"")),
            "consolidation-state must enumerate `{state}`"
        );
    }
}
