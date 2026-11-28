// Exercises bundle-sync insert/skip/update/delete-citation coverage against
// core+mem schema only — no cyber-specific schema or types are touched, so this
// test runs on every push, including the candle-free Tier-2 command
// (`cargo test --test '*' --no-default-features`).
use futures_util::StreamExt;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[cfg(feature = "cyber")]
use cyber_gecko::CyberGecko;
use gecko_engine::db::router::{DbError, TypeDbRouter};
use gecko_engine::extension::GeckoExtension;
use gecko_engine::okf::parser::parse_bundle;
use gecko_engine::okf::types::{HierarchyEdge, OkfBundle, OkfLink, TypedAttrValue, TypedAttribute};
use gecko_engine::sandbox::wasm_executor::MAX_TIMEOUT_MS;
use gecko_engine::syncer::bundle::sync_bundle;
use gecko_extension_api::GraphWrite;
use mem_gecko::{BELIEF_TIER_ROOT, MemGecko};

#[path = "common/belief.rs"]
mod belief;
mod common;
#[path = "common/provenance.rs"]
mod provenance;
use belief::claim_with_belief;
use common::TestDb;

fn create_test_bundle(dir: &std::path::Path) {
    let concept_dir = dir.join("playbooks");
    fs::create_dir_all(&concept_dir).unwrap();
    fs::write(
        concept_dir.join("test_concept.md"),
        r#"---
type: Playbook
title: Test Concept
description: A concept to test syncing
resource: db://test
tags: [test, sync]
---

This is a test concept. It links to [another](https://example.com).
"#,
    )
    .unwrap();
}

#[tokio::test]
async fn test_bundle_sync() {
    // Self-cleaning database: dropped from the server when `test_db` goes out of
    // scope (declared first so it drops last, after `db` closes).
    let test_db = TestDb::new("gecko_test_sync");
    let mut db = test_db.router();

    let core_schema = include_str!("../../core/gecko-engine/schema/core_schema.tql");
    db.apply_schema(core_schema)
        .await
        .expect("Failed to apply core schema");

    #[allow(unused_mut)]
    let mut extensions: Vec<Box<dyn GeckoExtension>> = vec![Box::new(MemGecko::new())];
    #[cfg(feature = "cyber")]
    extensions.push(Box::new(CyberGecko::new()));

    for ext in extensions {
        let schema = ext.schema();
        if !schema.trim().is_empty() {
            db.apply_schema(schema)
                .await
                .unwrap_or_else(|e| panic!("Failed to apply {} schema: {}", ext.name(), e));
        }
    }

    let temp_dir = TempDir::new().unwrap();
    create_test_bundle(temp_dir.path());

    let bundle = parse_bundle(temp_dir.path()).expect("Failed to parse bundle");

    let result = sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT)
        .await
        .expect("Failed to sync bundle");
    assert_eq!(result.concepts_inserted, 1);
    assert_eq!(result.citations_attempted, 1);
    // Concept IDs are bundle-relative paths with no namespace prefix.
    assert_eq!(bundle.concepts[0].concept_id, "playbooks/test_concept");

    // Re-syncing an unchanged bundle must be idempotent: the concept's
    // content hash matches, so it is skipped rather than re-inserted.
    let result2 = sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT)
        .await
        .expect("Failed to re-sync bundle");
    assert_eq!(
        result2.concepts_inserted, 0,
        "re-sync should insert nothing"
    );
    assert_eq!(
        result2.concepts_skipped, 1,
        "unchanged concept should be skipped"
    );
    assert_eq!(result2.concepts_deleted, 0, "nothing should be GC'd");

    // Editing the concept changes its content hash, exercising the in-place
    // update path. The new body drops the external link, so its citation should
    // be cleared.
    fs::write(
        temp_dir.path().join("playbooks/test_concept.md"),
        r#"---
type: Playbook
title: Test Concept Revised
description: An edited concept
tags: [test, sync]
---

The body has changed.
"#,
    )
    .unwrap();
    let edited = parse_bundle(temp_dir.path()).expect("Failed to re-parse bundle");
    let result3 = sync_bundle(&mut db, &edited, &[], BELIEF_TIER_ROOT)
        .await
        .expect("Failed to sync edited bundle");
    assert_eq!(result3.concepts_updated, 1, "edited concept should update");
    assert_eq!(result3.concepts_inserted, 0);
    assert_eq!(result3.concepts_skipped, 0);

    // Verify the edit landed and there is still exactly one concept.
    let tx = db.begin_read().await.expect("Failed to begin read");
    let answer = tx
        .query(r#"match $c isa concept, has title "Test Concept Revised";"#)
        .await
        .expect("query failed");
    let rows: Vec<_> = answer.into_rows().collect::<Vec<_>>().await;
    assert_eq!(rows.len(), 1, "Edited concept not found in database");

    // In-place update must not orphan or duplicate relations: the single
    // containment survives, and the now-linkless body clears the citation.
    for (typeql, expected, msg) in [
        (
            "match $c isa concept;",
            1,
            "update must not duplicate the concept",
        ),
        (
            "match $r isa containment;",
            1,
            "containment must survive an update, not orphan or duplicate",
        ),
        (
            "match $r isa citation;",
            0,
            "citation should be cleared when the edited body drops its link",
        ),
    ] {
        let tx = db.begin_read().await.expect("Failed to begin read");
        let n = tx
            .query(typeql)
            .await
            .expect("count query failed")
            .into_rows()
            .collect::<Vec<_>>()
            .await
            .len();
        assert_eq!(n, expected, "{msg}");
    }
}

#[tokio::test]
async fn failing_extension_write_rolls_back_the_whole_sync() {
    let test_db = TestDb::new("gecko_test_sync_rollback");
    let mut db = test_db.router();

    let core_schema = include_str!("../../core/gecko-engine/schema/core_schema.tql");
    db.apply_schema(core_schema)
        .await
        .expect("Failed to apply core schema");
    db.apply_schema(MemGecko::new().schema())
        .await
        .expect("Failed to apply mem schema");

    let temp_dir = TempDir::new().unwrap();
    create_test_bundle(temp_dir.path());
    let bundle = parse_bundle(temp_dir.path()).expect("Failed to parse bundle");

    let bad = [GraphWrite::plain("insert $x isa no-such-type;")];
    let result = sync_bundle(&mut db, &bundle, &bad, BELIEF_TIER_ROOT).await;
    assert!(
        result.is_err(),
        "a failing extension write must fail the sync"
    );

    for typeql in ["match $c isa concept;", "match $b isa bundle;"] {
        let tx = db.begin_read().await.expect("Failed to begin read");
        let n = tx
            .query(typeql)
            .await
            .expect("count query failed")
            .into_rows()
            .collect::<Vec<_>>()
            .await
            .len();
        assert_eq!(n, 0, "rolled-back sync must leave nothing for: {typeql}");
    }
}

/// Applies the core schema and, when `with_mem`, mem's schema.
async fn apply_core(db: &mut TypeDbRouter, with_mem: bool) {
    let core_schema = include_str!("../../core/gecko-engine/schema/core_schema.tql");
    db.apply_schema(core_schema)
        .await
        .expect("Failed to apply core schema");
    if with_mem {
        db.apply_schema(MemGecko::new().schema())
            .await
            .expect("Failed to apply mem schema");
    }
}

/// The number of answers `typeql` matches in a fresh read transaction.
async fn count(db: &mut TypeDbRouter, typeql: &str) -> usize {
    let tx = db.begin_read().await.expect("Failed to begin read");
    tx.query(typeql)
        .await
        .expect("count query failed")
        .into_rows()
        .collect::<Vec<_>>()
        .await
        .len()
}

/// The stored `file-hash` of the database's single concept.
async fn sole_file_hash(db: &mut TypeDbRouter) -> String {
    let tx = db.begin_read().await.expect("Failed to begin read");
    let rows: Vec<_> = tx
        .query("match $c isa concept, has file-hash $h; select $h;")
        .await
        .expect("file-hash query failed")
        .into_rows()
        .collect::<Vec<_>>()
        .await;
    assert_eq!(rows.len(), 1, "expected exactly one stored file-hash");
    let row = rows.into_iter().next().unwrap().expect("file-hash row");
    row.get("h")
        .expect("column h")
        .expect("h bound")
        .try_get_string()
        .expect("h is a string")
        .to_string()
}

#[tokio::test]
async fn extension_write_into_the_belief_tier_is_refused() {
    let test_db = TestDb::new("gecko_test_sync_belief_write");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;

    let temp_dir = TempDir::new().unwrap();
    create_test_bundle(temp_dir.path());
    let bundle = parse_bundle(temp_dir.path()).expect("Failed to parse bundle");

    let raw = GraphWrite::single(
        "given $id: string, $st: string; \
         insert $b isa belief, has concept-id == $id, has belief-state == $st;",
        vec!["id".into(), "st".into()],
        vec!["mem/bel/raw".into(), "asserted".into()],
    )
    .expecting(1);
    let writes = [vec![raw], provenance::seed_provenance()].concat();
    let result = sync_bundle(&mut db, &bundle, &writes, BELIEF_TIER_ROOT).await;
    let Err(DbError::BeliefTierWrite { changes }) = result else {
        panic!("expected BeliefTierWrite, got {result:?}");
    };
    let labels: HashSet<&str> = changes.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(
        labels,
        HashSet::from(["memory-item", "belief", "ownership", "source-link"])
    );

    assert_eq!(
        count(&mut db, "match $c isa concept;").await,
        0,
        "the refused sync must roll back entirely"
    );
}

#[tokio::test]
async fn sync_without_the_belief_tier_root_is_refused() {
    let test_db = TestDb::new("gecko_test_sync_no_root");
    let mut db = test_db.router();
    apply_core(&mut db, false).await;

    let temp_dir = TempDir::new().unwrap();
    create_test_bundle(temp_dir.path());
    let bundle = parse_bundle(temp_dir.path()).expect("Failed to parse bundle");

    let result = sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT).await;
    let Err(DbError::Schema(m)) = result else {
        panic!("expected a schema error, got {result:?}");
    };
    assert!(m.contains("memory-item"), "{m}");
    assert_eq!(count(&mut db, "match $c isa concept;").await, 0);
    assert_eq!(count(&mut db, "match $b isa bundle;").await, 0);
}

#[tokio::test]
async fn unknown_type_hint_is_refused_as_a_schema_error() {
    // Insert path: a new concept with an unknown type_hint.
    {
        let test_db = TestDb::new("gecko_test_sync_hint_insert");
        let mut db = test_db.router();
        apply_core(&mut db, true).await;

        let temp_dir = TempDir::new().unwrap();
        create_test_bundle(temp_dir.path());
        let mut bundle = parse_bundle(temp_dir.path()).expect("Failed to parse bundle");
        bundle.concepts[0].type_hint = Some("no-such-type".into());

        let result = sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT).await;
        let Err(DbError::Schema(m)) = result else {
            panic!("expected a schema error, got {result:?}");
        };
        assert!(m.contains("no-such-type"), "{m}");
        assert_eq!(count(&mut db, "match $c isa concept;").await, 0);
    }

    // Update path: a changed concept whose typed attribute names an unknown type.
    let test_db = TestDb::new("gecko_test_sync_hint_update");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;

    let temp_dir = TempDir::new().unwrap();
    create_test_bundle(temp_dir.path());
    let bundle = parse_bundle(temp_dir.path()).expect("Failed to parse bundle");
    sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT)
        .await
        .expect("Failed to sync bundle");
    let stored_hash = sole_file_hash(&mut db).await;

    let mut changed = bundle.clone();
    changed.concepts[0].file_hash = format!("{stored_hash}-changed");
    changed.concepts[0].type_hint = Some("no-such-type".into());
    changed.concepts[0].typed_attributes = vec![TypedAttribute {
        name: "title".into(),
        value: TypedAttrValue::String("t".into()),
    }];
    let result = sync_bundle(&mut db, &changed, &[], BELIEF_TIER_ROOT).await;
    let Err(DbError::Schema(m)) = result else {
        panic!("expected a schema error, got {result:?}");
    };
    assert!(m.contains("no-such-type"), "{m}");
    assert_eq!(sole_file_hash(&mut db).await, stored_hash);

    // Update path: a changed concept with no typed attribute whose type_hint is
    // unknown, or names a type outside `concept`.
    for hint in ["no-such-type", "bundle"] {
        let mut changed = bundle.clone();
        changed.concepts[0].file_hash = format!("{stored_hash}-changed");
        changed.concepts[0].type_hint = Some(hint.into());
        assert!(changed.concepts[0].typed_attributes.is_empty());
        let result = sync_bundle(&mut db, &changed, &[], BELIEF_TIER_ROOT).await;
        let Err(DbError::Schema(m)) = result else {
            panic!("expected a schema error for type_hint {hint:?}, got {result:?}");
        };
        assert!(
            m.contains(&format!("type_hint {hint:?} is not a subtype of `concept`")),
            "{m}"
        );
        assert_eq!(sole_file_hash(&mut db).await, stored_hash);
    }
}

/// The concept the two-concept bundle of [`synced_with_referenced`] drops.
const REFERENCED: &str = "playbooks/test_concept";

/// Applies the core and mem schemas, syncs a two-concept bundle whose second
/// concept links to (and is the hierarchy parent of) the first, and gives the
/// first a belief with a `source-link` to it. Returns the full bundle, the
/// referenced concept's ID, and the bundle without that concept and its edges.
async fn synced_with_referenced(
    db: &mut TypeDbRouter,
    dir: &Path,
) -> (OkfBundle, String, OkfBundle) {
    apply_core(db, true).await;
    create_test_bundle(dir);
    fs::write(
        dir.join("playbooks/second.md"),
        "---\ntype: Playbook\ntitle: Second\n---\n\nSee [the first](test_concept.md).\n",
    )
    .unwrap();
    let mut full = parse_bundle(dir).expect("Failed to parse bundle");
    let referenced = REFERENCED.to_string();
    let survivor = full
        .concepts
        .iter()
        .find(|c| c.concept_id != referenced)
        .expect("a second concept")
        .concept_id
        .clone();
    full.hierarchy.push(HierarchyEdge {
        parent_id: survivor.clone(),
        child_id: referenced.clone(),
    });
    let r = sync_bundle(db, &full, &[], BELIEF_TIER_ROOT)
        .await
        .expect("Failed to sync bundle");
    assert_eq!(r.concepts_inserted, 2);
    assert_eq!(
        count(
            db,
            &format!(
                r#"match $s isa concept, has concept-id "{survivor}";
                         $t isa concept, has concept-id "{referenced}";
                         $l isa concept-link, links (source: $s, target: $t);"#
            )
        )
        .await,
        1,
        "the second concept links to the first"
    );
    assert_eq!(
        plays(db, &referenced, "hierarchy").await,
        1,
        "the second concept is the first's hierarchy parent"
    );
    assert_eq!(
        plays(db, &referenced, "citation").await,
        1,
        "the first concept cites an external resource"
    );

    // A belief whose source-link names the concept as its origin.
    claim_with_belief(db, "mem/bel/ref", &referenced).await;

    let mut dropped = full.clone();
    dropped.concepts.retain(|c| c.concept_id != referenced);
    dropped
        .links
        .retain(|l| l.source_id != referenced && l.target_id != referenced);
    dropped
        .hierarchy
        .retain(|h| h.parent_id != referenced && h.child_id != referenced);
    dropped.citations.retain(|c| c.source_id != referenced);
    assert_eq!(dropped.concepts.len(), 1);
    (full, referenced, dropped)
}

/// The `retired-at` values of the concept `id`, rendered for comparison.
async fn retired_at(db: &mut TypeDbRouter, id: &str) -> Vec<String> {
    let tx = db.begin_read().await.expect("Failed to begin read");
    let rows: Vec<_> = tx
        .query(format!(
            r#"match $c isa concept, has concept-id "{id}", has retired-at $t; select $t;"#
        ))
        .await
        .expect("retired-at query failed")
        .into_rows()
        .collect::<Vec<_>>()
        .await;
    rows.into_iter()
        .map(|r| {
            let t = r
                .expect("retired-at row")
                .get("t")
                .expect("column t")
                .expect("t bound")
                .try_get_datetime()
                .expect("t is a datetime");
            t.to_string()
        })
        .collect()
}

/// How many relations of type `rel` the concept `id` plays a role in.
async fn plays(db: &mut TypeDbRouter, id: &str, rel: &str) -> usize {
    count(
        db,
        &format!(r#"match $c isa concept, has concept-id "{id}"; $r isa {rel}, links ($c);"#),
    )
    .await
}

#[tokio::test]
async fn gc_of_a_concept_a_belief_edge_references_retires_it() {
    let test_db = TestDb::new("gecko_test_sync_gc_retire");
    let mut db = test_db.router();
    let temp_dir = TempDir::new().unwrap();
    let (_full, referenced, dropped) = synced_with_referenced(&mut db, temp_dir.path()).await;

    let r = sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT)
        .await
        .expect("dropping a referenced concept retires it");
    assert_eq!(r.concepts_retired, 1);
    assert_eq!(r.concepts_deleted, 0);

    let first = retired_at(&mut db, &referenced).await;
    assert_eq!(first.len(), 1, "exactly one retired-at: {first:?}");
    assert_eq!(
        count(
            &mut db,
            &format!(
                r#"match $c isa concept, has concept-id "{referenced}"; $c has $a;
                         not {{ $a isa concept-id; }}; not {{ $a isa retired-at; }};"#
            )
        )
        .await,
        0,
        "a retired concept keeps only its concept-id and retired-at"
    );
    assert_eq!(plays(&mut db, &referenced, "containment").await, 1);
    assert_eq!(
        count(
            &mut db,
            &format!(
                r#"match $c isa concept, has concept-id "{referenced}";
                         $b isa bundle, has bundle-name "{}";
                         $r isa containment, links (container: $b, member: $c);"#,
                dropped.bundle_name
            )
        )
        .await,
        1,
        "the retired concept stays contained by its bundle"
    );
    for rel in ["hierarchy", "concept-link", "citation"] {
        assert_eq!(plays(&mut db, &referenced, rel).await, 0, "{rel}");
    }
    assert_eq!(count(&mut db, "match $l isa source-link;").await, 1);
    assert_eq!(
        count(
            &mut db,
            &format!(
                r#"match $c isa concept, has concept-id "{referenced}";
                         $l isa source-link, links (origin: $c);"#
            )
        )
        .await,
        1,
        "the belief edge still names the retired concept"
    );

    let again = sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT)
        .await
        .expect("re-sync with the concept still retired");
    assert_eq!(again.concepts_retired, 0);
    assert_eq!(again.concepts_deleted, 0);
    assert_eq!(retired_at(&mut db, &referenced).await, first);
}

#[tokio::test]
async fn restoring_a_retired_concepts_file_unretires_it() {
    let test_db = TestDb::new("gecko_test_sync_unretire");
    let mut db = test_db.router();
    let temp_dir = TempDir::new().unwrap();
    let (full, referenced, dropped) = synced_with_referenced(&mut db, temp_dir.path()).await;
    sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT)
        .await
        .expect("retire");

    let r = sync_bundle(&mut db, &full, &[], BELIEF_TIER_ROOT)
        .await
        .expect("restoring the file");
    assert_eq!(r.concepts_updated, 1);
    assert!(retired_at(&mut db, &referenced).await.is_empty());

    let expected_hash = &full
        .concepts
        .iter()
        .find(|c| c.concept_id == referenced)
        .unwrap()
        .file_hash;
    assert_eq!(
        count(
            &mut db,
            &format!(
                r#"match $c isa concept, has concept-id "{referenced}", has file-hash "{expected_hash}";"#
            )
        )
        .await,
        1,
        "the restored concept carries its file's hash"
    );
    let survivor = &dropped.concepts[0].concept_id;
    assert_eq!(
        count(
            &mut db,
            &format!(
                r#"match $s isa concept, has concept-id "{survivor}";
                         $t isa concept, has concept-id "{referenced}";
                         $l isa concept-link, links (source: $s, target: $t);"#
            )
        )
        .await,
        1,
        "the surviving concept's link to the restored one is back"
    );
}

#[tokio::test]
async fn a_retired_concept_no_longer_referenced_is_deleted() {
    let test_db = TestDb::new("gecko_test_sync_retired_gc");
    let mut db = test_db.router();
    let temp_dir = TempDir::new().unwrap();
    let (_full, referenced, dropped) = synced_with_referenced(&mut db, temp_dir.path()).await;
    sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT)
        .await
        .expect("retire");

    let tx = db.begin_write().await.expect("Failed to begin write");
    for q in [
        "match $l isa source-link; delete $l;",
        r#"match $b isa belief, has concept-id "mem/bel/ref";
                 $o isa ownership, links (owned: $b);
           delete $o;"#,
        r#"match $b isa belief, has concept-id "mem/bel/ref"; delete $b;"#,
    ] {
        tx.query(q).await.expect("raw delete");
    }
    tx.commit().await.expect("commit raw delete");

    let r = sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT)
        .await
        .expect("GC of an unreferenced retired concept");
    assert_eq!(r.concepts_deleted, 1);
    assert_eq!(r.concepts_retired, 0);
    assert_eq!(
        count(
            &mut db,
            &format!(r#"match $c isa concept, has concept-id "{referenced}";"#)
        )
        .await,
        0
    );
}

#[tokio::test]
async fn retired_concept_gets_no_new_links_from_live_files() {
    let test_db = TestDb::new("gecko_test_sync_retired_links");
    let mut db = test_db.router();
    let temp_dir = TempDir::new().unwrap();
    let (_full, referenced, dropped) = synced_with_referenced(&mut db, temp_dir.path()).await;
    sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT)
        .await
        .expect("retire");

    let survivor = dropped.concepts[0].concept_id.clone();
    let mut linking = dropped.clone();
    linking.links.push(OkfLink {
        source_id: survivor.clone(),
        target_id: referenced.clone(),
        link_text: "retired".into(),
    });
    linking.hierarchy.push(HierarchyEdge {
        parent_id: survivor,
        child_id: referenced.clone(),
    });
    sync_bundle(&mut db, &linking, &[], BELIEF_TIER_ROOT)
        .await
        .expect("sync with edges to the retired concept");
    assert_eq!(plays(&mut db, &referenced, "concept-link").await, 0);
    assert_eq!(plays(&mut db, &referenced, "hierarchy").await, 0);
}

#[tokio::test]
async fn a_dropped_concept_another_bundle_contains_is_released_not_retired() {
    let test_db = TestDb::new("gecko_test_sync_release_not_retire");
    let mut db = test_db.router();
    let temp_dir = TempDir::new().unwrap();
    let (_full, referenced, dropped) = synced_with_referenced(&mut db, temp_dir.path()).await;

    let tx = db.begin_write().await.expect("Failed to begin write");
    tx.query(format!(
        r#"match $c isa concept, has concept-id "{referenced}";
           insert $b isa bundle, has bundle-name "beta";
                  $cb isa containment, links (container: $b, member: $c);"#
    ))
    .await
    .expect("raw beta containment");
    tx.commit().await.expect("commit beta containment");

    let r = sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT)
        .await
        .expect("release");
    assert_eq!(r.concepts_released, 1);
    assert_eq!(r.concepts_retired, 0);
    assert!(retired_at(&mut db, &referenced).await.is_empty());
}

#[tokio::test]
async fn one_sync_retires_two_linked_referenced_concepts_and_deletes_an_unreferenced_one() {
    let test_db = TestDb::new("gecko_test_sync_gc_mixed");
    let mut db = test_db.router();
    let temp_dir = TempDir::new().unwrap();
    let (full, referenced, _dropped) = synced_with_referenced(&mut db, temp_dir.path()).await;
    let linker = full
        .concepts
        .iter()
        .find(|c| c.concept_id != referenced)
        .expect("a second concept")
        .concept_id
        .clone();

    // The linking concept gains a belief reference, and a third concept nothing references.
    claim_with_belief(&mut db, "mem/bel/linker", &linker).await;

    let unreferenced = "playbooks/unreferenced".to_string();
    let mut with_third = full.clone();
    let mut third = with_third.concepts[0].clone();
    third.concept_id = unreferenced.clone();
    with_third.concepts.push(third);
    let r = sync_bundle(&mut db, &with_third, &[], BELIEF_TIER_ROOT)
        .await
        .expect("sync the third concept");
    assert_eq!(r.concepts_inserted, 1);

    let mut emptied = with_third.clone();
    emptied.concepts.clear();
    emptied.links.clear();
    emptied.hierarchy.clear();
    emptied.citations.clear();
    let r = sync_bundle(&mut db, &emptied, &[], BELIEF_TIER_ROOT)
        .await
        .expect("dropping every concept");
    assert_eq!(r.concepts_retired, 2);
    assert_eq!(r.concepts_deleted, 1);

    for id in [&referenced, &linker] {
        assert_eq!(retired_at(&mut db, id).await.len(), 1, "{id} is retired");
        assert_eq!(plays(&mut db, id, "concept-link").await, 0, "{id}");
    }
    assert_eq!(
        count(
            &mut db,
            &format!(r#"match $c isa concept, has concept-id "{unreferenced}";"#)
        )
        .await,
        0,
        "the unreferenced concept is deleted"
    );
    assert_eq!(count(&mut db, "match $l isa concept-link;").await, 0);
}

/// A test-only concept subtype a re-sync retypes a concept into.
const RETYPE_PROBE: &str = "retype-probe";

/// Applies the core and mem schemas plus [`RETYPE_PROBE`], and syncs a bundle
/// whose single concept `a` has no `type_hint`. Returns the bundle and its dir.
async fn synced_untyped_a(db: &mut TypeDbRouter) -> (OkfBundle, TempDir) {
    apply_core(db, true).await;
    db.apply_schema(
        "define entity retype-probe sub concept @doc(\"A test-only concept subtype.\");",
    )
    .await
    .expect("Failed to apply the retype-probe schema");
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("a.md"),
        "---\ntype: Playbook\ntitle: First\n---\n\nThe first body.\n",
    )
    .unwrap();
    let bundle = parse_bundle(temp_dir.path()).expect("Failed to parse bundle");
    assert_eq!(bundle.concepts.len(), 1);
    assert_eq!(bundle.concepts[0].concept_id, "a");
    assert!(bundle.concepts[0].type_hint.is_none());
    let r = sync_bundle(db, &bundle, &[], BELIEF_TIER_ROOT)
        .await
        .expect("Failed to sync bundle");
    assert_eq!(r.concepts_inserted, 1);
    (bundle, temp_dir)
}

/// The exact type label of the concept `id`.
async fn exact_type(db: &mut TypeDbRouter, id: &str) -> Vec<String> {
    let tx = db.begin_read().await.expect("Failed to begin read");
    let rows: Vec<_> = tx
        .query(format!(
            r#"match $c isa! $t, has concept-id "{id}"; $t sub concept;
               let $l = label($t); select $l;"#
        ))
        .await
        .expect("type query failed")
        .into_rows()
        .collect::<Vec<_>>()
        .await;
    rows.into_iter()
        .map(|r| {
            r.expect("type row")
                .get("l")
                .expect("column l")
                .expect("l bound")
                .try_get_string()
                .expect("l is a string")
                .to_string()
        })
        .collect()
}

/// Asserts that `a` is a [`RETYPE_PROBE`] contained once, carrying `expected`'s
/// title, body and hash and no other copy of them.
async fn assert_retyped(db: &mut TypeDbRouter, expected: &OkfBundle, title: &str) {
    assert_eq!(exact_type(db, "a").await, vec![RETYPE_PROBE.to_string()]);
    assert_eq!(count(db, "match $c isa concept;").await, 1);
    assert_eq!(plays(db, "a", "containment").await, 1);
    let c = &expected.concepts[0];
    for (attr, value) in [
        ("title", title),
        ("file-hash", c.file_hash.as_str()),
        ("concept-type", c.concept_type.as_str()),
    ] {
        assert_eq!(
            count(
                db,
                &format!(r#"match $c isa retype-probe, has concept-id "a", has {attr} $v;"#)
            )
            .await,
            1,
            "`a` owns exactly one {attr}"
        );
        assert_eq!(
            count(
                db,
                &format!(
                    r#"match $c isa retype-probe, has concept-id "a", has {attr} == {value:?};"#
                )
            )
            .await,
            1,
            "`a`'s {attr} is the second manifest's"
        );
    }
}

#[tokio::test]
async fn a_changed_type_hint_retypes_the_concept() {
    let test_db = TestDb::new("gecko_test_sync_retype");
    let mut db = test_db.router();
    let (first, temp_dir) = synced_untyped_a(&mut db).await;

    fs::write(
        temp_dir.path().join("a.md"),
        "---\ntype: Playbook\ntitle: Second\n---\n\nThe second body.\n",
    )
    .unwrap();
    let mut second = parse_bundle(temp_dir.path()).expect("Failed to re-parse bundle");
    assert_ne!(second.concepts[0].file_hash, first.concepts[0].file_hash);
    second.concepts[0].type_hint = Some(RETYPE_PROBE.into());

    let r = sync_bundle(&mut db, &second, &[], BELIEF_TIER_ROOT)
        .await
        .expect("a changed type_hint retypes the concept");
    assert_eq!(r.concepts_updated, 1);
    assert_eq!(r.concepts_inserted, 0);
    assert_eq!(r.concepts_skipped, 0);
    assert_eq!(r.concepts_deleted, 0);
    assert_retyped(&mut db, &second, "Second").await;
}

#[tokio::test]
async fn a_changed_type_hint_with_an_unchanged_hash_still_retypes() {
    let test_db = TestDb::new("gecko_test_sync_retype_same_hash");
    let mut db = test_db.router();
    let (first, _temp_dir) = synced_untyped_a(&mut db).await;

    let mut second = first.clone();
    second.concepts[0].type_hint = Some(RETYPE_PROBE.into());

    let r = sync_bundle(&mut db, &second, &[], BELIEF_TIER_ROOT)
        .await
        .expect("a changed type_hint retypes the concept");
    assert_eq!(r.concepts_updated, 1);
    assert_eq!(r.concepts_inserted, 0);
    assert_eq!(r.concepts_skipped, 0);
    assert_retyped(&mut db, &second, "First").await;

    let again = sync_bundle(&mut db, &second, &[], BELIEF_TIER_ROOT)
        .await
        .expect("re-sync of the retyped concept");
    assert_eq!(again.concepts_skipped, 1);
    assert_eq!(again.concepts_updated, 0);
}

#[tokio::test]
async fn a_changed_type_hint_on_a_referenced_concept_is_refused() {
    let test_db = TestDb::new("gecko_test_sync_retype_referenced");
    let mut db = test_db.router();
    let (first, _temp_dir) = synced_untyped_a(&mut db).await;
    let stored_hash = sole_file_hash(&mut db).await;
    claim_with_belief(&mut db, "mem/bel/retype", "a").await;

    let mut second = first.clone();
    second.concepts[0].file_hash = format!("{stored_hash}-changed");
    second.concepts[0].type_hint = Some(RETYPE_PROBE.into());

    let result = sync_bundle(&mut db, &second, &[], BELIEF_TIER_ROOT).await;
    let Err(DbError::TypeChangeReferenced { ids }) = result else {
        panic!("expected TypeChangeReferenced, got {result:?}");
    };
    assert_eq!(
        ids,
        vec![(
            "a".to_string(),
            "concept".to_string(),
            RETYPE_PROBE.to_string()
        )]
    );
    let message = DbError::TypeChangeReferenced { ids }.to_string();
    assert!(message.contains("`a`"), "{message}");
    assert_eq!(exact_type(&mut db, "a").await, vec!["concept".to_string()]);
    assert_eq!(sole_file_hash(&mut db).await, stored_hash);
}

fn write_concepts(dir: &Path, files: &[(&str, &str)]) {
    fs::create_dir_all(dir.join("notes")).unwrap();
    for (name, body) in files {
        fs::write(
            dir.join("notes").join(format!("{name}.md")),
            format!("---\ntype: Note\ntitle: {name}\n---\n\n{body}\n"),
        )
        .unwrap();
    }
}

#[tokio::test]
async fn a_document_linking_the_same_page_twice_gets_one_link() {
    let test_db = TestDb::new("gecko_test_dup_link");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;
    let dir = TempDir::new().unwrap();
    write_concepts(
        dir.path(),
        &[
            ("a", "See [first](b.md) and again [second](b.md). Line two."),
            ("b", "Target."),
        ],
    );
    let bundle = parse_bundle(dir.path()).expect("parse");
    let r = sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT)
        .await
        .expect("sync");
    assert_eq!(r.links_attempted, 1, "attempted counts deduplicated rows");
    assert_eq!(count(&mut db, "match $l isa concept-link;").await, 1);
    assert_eq!(
        count(
            &mut db,
            r#"match $l isa concept-link, has link-text "first";"#
        )
        .await,
        1,
        "the first occurrence's link text is kept"
    );

    // Re-sync after editing another line: still one link.
    write_concepts(
        dir.path(),
        &[
            ("a", "See [first](b.md) and again [second](b.md). Line TWO."),
            ("b", "Target."),
        ],
    );
    let edited = parse_bundle(dir.path()).expect("parse");
    sync_bundle(&mut db, &edited, &[], BELIEF_TIER_ROOT)
        .await
        .expect("re-sync");
    assert_eq!(count(&mut db, "match $l isa concept-link;").await, 1);
}

#[tokio::test]
async fn a_document_citing_the_same_url_twice_gets_one_citation() {
    let test_db = TestDb::new("gecko_test_dup_citation");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;
    let dir = TempDir::new().unwrap();
    write_concepts(
        dir.path(),
        &[(
            "a",
            "[one](https://example.com/x) and [two](https://example.com/x).",
        )],
    );
    let bundle = parse_bundle(dir.path()).expect("parse");
    let r = sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT)
        .await
        .expect("sync");
    assert_eq!(r.citations_attempted, 1);
    assert_eq!(count(&mut db, "match $c isa citation;").await, 1);
}

#[tokio::test]
async fn garbage_collecting_a_concept_deletes_every_non_surface_relation_it_plays() {
    let test_db = TestDb::new("gecko_test_gc_every_relation");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;
    #[cfg(feature = "cyber")]
    db.apply_schema(CyberGecko::new().schema())
        .await
        .expect("Failed to apply cyber schema");
    let dir = TempDir::new().unwrap();
    write_concepts(dir.path(), &[("a", "Member."), ("b", "Kept.")]);
    let full = parse_bundle(dir.path()).expect("parse");
    #[cfg(feature = "cyber")]
    let full = {
        let mut full = full;
        for c in full
            .concepts
            .iter_mut()
            .filter(|c| c.concept_id == "notes/a")
        {
            c.type_hint = Some("intrusion-set".to_string());
        }
        full
    };
    sync_bundle(&mut db, &full, &[], BELIEF_TIER_ROOT)
        .await
        .expect("sync both concepts");
    // An extension relation between the two concepts that no memory item plays, so
    // it stays off the belief-tier surface.
    db.apply_schema(
        r#"define
             relation test-ext-link @doc("A test-only extension relation between two concepts."),
                 relates left @card(1) @doc("The concept on the link's left."),
                 relates right @card(1) @doc("The concept on the link's right.");
             concept plays test-ext-link:left, plays test-ext-link:right;"#,
    )
    .await
    .expect("define the test link");
    let tx = db.begin_write().await.expect("write tx");
    tx.query(
        r#"match $a isa concept, has concept-id "notes/a";
                 $b isa concept, has concept-id "notes/b";
           insert test-ext-link (left: $a, right: $b);
           insert test-ext-link (left: $b, right: $b);"#,
    )
    .await
    .expect("insert the links");
    // A STIX `uses` whose user is the plain bundle's concept and whose used
    // malware no bundle contains.
    #[cfg(feature = "cyber")]
    tx.query(
        r#"match $a isa intrusion-set, has concept-id "notes/a";
           insert $m isa malware, has concept-id "elsewhere/m";
                  uses (user: $a, used: $m);"#,
    )
    .await
    .expect("insert the cross-bundle uses");
    tx.commit().await.expect("commit the extension relations");
    assert_eq!(count(&mut db, "match $l isa test-ext-link;").await, 2);

    let mut dropped = full.clone();
    dropped.concepts.retain(|c| c.concept_id != "notes/a");
    let r = sync_bundle(&mut db, &dropped, &[], BELIEF_TIER_ROOT)
        .await
        .expect("garbage-collecting a concept that plays extension relations commits");
    assert_eq!(r.concepts_deleted, 1);
    assert_eq!(
        count(&mut db, "match $l isa test-ext-link;").await,
        1,
        "only the link to the collected concept is deleted"
    );
    assert_eq!(
        count(
            &mut db,
            r#"match $l isa test-ext-link (left: $x, right: $y);
               $x isa concept, has concept-id "notes/b";
               $y isa concept, has concept-id "notes/b";"#
        )
        .await,
        1,
        "the surviving link is the one between notes/b and itself"
    );
    #[cfg(feature = "cyber")]
    {
        assert_eq!(
            count(&mut db, "match $u isa uses;").await,
            0,
            "no one-player uses remains"
        );
        assert_eq!(
            count(
                &mut db,
                r#"match $m isa malware, has concept-id "elsewhere/m";"#
            )
            .await,
            1,
            "the other player is kept"
        );
    }
}

/// Seeds a bundle, two concepts and an external resource, then asserts that
/// `insert` (matching `$b`, `$c`, `$d` and `$e`) commits nothing: the commit is
/// refused naming one of `roles`. A citation's specialized role and the `okf-link`
/// role it specializes are both required, and either may be reported.
async fn assert_core_relation_refused(prefix: &str, insert: &str, roles: &[&str]) {
    let test_db = TestDb::new(prefix);
    let mut db = test_db.router();
    apply_core(&mut db, false).await;
    let tx = db.begin_write().await.expect("write tx");
    tx.query(
        "insert $b isa bundle, has bundle-name 'req-b'; \
         $c isa concept, has concept-id 'req/c'; \
         $d isa concept, has concept-id 'req/d'; \
         $e isa external-resource, has target-url 'https://example.com/req';",
    )
    .await
    .expect("seed the players");
    tx.commit().await.expect("commit the players");

    let tx = db.begin_write().await.expect("write tx");
    tx.query(&format!(
        "match $b isa bundle, has bundle-name 'req-b'; \
               $c isa concept, has concept-id 'req/c'; \
               $d isa concept, has concept-id 'req/d'; \
               $e isa external-resource, has target-url 'https://example.com/req'; \
         insert {insert}"
    ))
    .await
    .expect("the insert itself is accepted; the card is checked at commit");
    let err = tx
        .commit()
        .await
        .expect_err("a relation missing a required role commits");
    let msg = err.to_string();
    assert!(
        roles
            .iter()
            .any(|role| msg.contains(&format!("role type '{role}'"))),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn a_concept_link_missing_its_target_is_refused_at_commit() {
    assert_core_relation_refused(
        "gecko_test_link_no_target",
        "concept-link (source: $c);",
        &["okf-link:target"],
    )
    .await;
}

#[tokio::test]
async fn a_concept_link_missing_its_source_is_refused_at_commit() {
    assert_core_relation_refused(
        "gecko_test_link_no_source",
        "concept-link (target: $d);",
        &["okf-link:source"],
    )
    .await;
}

#[tokio::test]
async fn a_citation_missing_its_cited_resource_is_refused_at_commit() {
    assert_core_relation_refused(
        "gecko_test_citation_no_resource",
        "citation (citing-concept: $c);",
        &["citation:cited-resource", "okf-link:target"],
    )
    .await;
}

#[tokio::test]
async fn a_citation_missing_its_citing_concept_is_refused_at_commit() {
    assert_core_relation_refused(
        "gecko_test_citation_no_concept",
        "citation (cited-resource: $e);",
        &["citation:citing-concept", "okf-link:source"],
    )
    .await;
}

#[tokio::test]
async fn a_containment_missing_its_member_is_refused_at_commit() {
    assert_core_relation_refused(
        "gecko_test_containment_no_member",
        "containment (container: $b);",
        &["containment:member"],
    )
    .await;
}

#[tokio::test]
async fn a_containment_missing_its_container_is_refused_at_commit() {
    assert_core_relation_refused(
        "gecko_test_containment_no_container",
        "containment (member: $c);",
        &["containment:container"],
    )
    .await;
}

#[tokio::test]
async fn a_hierarchy_missing_its_child_is_refused_at_commit() {
    assert_core_relation_refused(
        "gecko_test_hierarchy_no_child",
        "hierarchy (parent: $c);",
        &["hierarchy:child"],
    )
    .await;
}

#[tokio::test]
async fn a_hierarchy_missing_its_parent_is_refused_at_commit() {
    assert_core_relation_refused(
        "gecko_test_hierarchy_no_parent",
        "hierarchy (child: $d);",
        &["hierarchy:parent"],
    )
    .await;
}

#[tokio::test]
async fn garbage_collecting_a_linked_concept_commits() {
    let test_db = TestDb::new("gecko_test_gc_linked_concept");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;
    let dir = TempDir::new().unwrap();
    write_concepts(
        dir.path(),
        &[
            ("a", "See [b](b.md) and [x](https://example.com/gc)."),
            ("b", "Linked."),
        ],
    );
    let full = parse_bundle(dir.path()).expect("parse");
    sync_bundle(&mut db, &full, &[], BELIEF_TIER_ROOT)
        .await
        .expect("sync both concepts");
    assert_eq!(count(&mut db, "match $l isa concept-link;").await, 1);
    assert_eq!(count(&mut db, "match $c isa citation;").await, 1);

    // Dropping the link's target deletes the link with it.
    let mut no_target = full.clone();
    no_target.concepts.retain(|c| c.concept_id != "notes/b");
    let r = sync_bundle(&mut db, &no_target, &[], BELIEF_TIER_ROOT)
        .await
        .expect("garbage-collecting a link target commits");
    assert_eq!(r.concepts_deleted, 1);
    assert_eq!(count(&mut db, "match $l isa concept-link;").await, 0);

    // Dropping the linking concept deletes its citation; the resource stays.
    let mut empty = no_target.clone();
    empty.concepts.retain(|c| c.concept_id != "notes/a");
    let r = sync_bundle(&mut db, &empty, &[], BELIEF_TIER_ROOT)
        .await
        .expect("garbage-collecting a citing concept commits");
    assert_eq!(r.concepts_deleted, 1);
    assert_eq!(count(&mut db, "match $c isa citation;").await, 0);
    assert_eq!(count(&mut db, "match $c isa containment;").await, 0);
    assert_eq!(count(&mut db, "match $e isa external-resource;").await, 1);
}

/// Writes a one-playbook bundle into `dir`, with `timeout_line` in its frontmatter.
fn write_timeout_bundle(dir: &Path, timeout_line: &str) {
    let playbooks = dir.join("playbooks");
    fs::create_dir_all(&playbooks).unwrap();
    fs::write(
        playbooks.join("bounded.md"),
        format!(
            "---\ntype: playbook\nengine: quickjs\n{timeout_line}\n---\n\n# Bounded\n\n```javascript\n1\n```\n"
        ),
    )
    .unwrap();
}

/// Every stored `timeout-ms` value, in no particular order.
async fn stored_timeouts(db: &mut TypeDbRouter) -> Vec<i64> {
    let tx = db.begin_read().await.expect("Failed to begin read");
    let rows: Vec<_> = tx
        .query("match $c isa concept, has timeout-ms $t; select $t;")
        .await
        .expect("timeout-ms query failed")
        .into_rows()
        .collect::<Vec<_>>()
        .await;
    rows.into_iter()
        .map(|row| {
            row.expect("timeout row")
                .get("t")
                .expect("column t")
                .expect("t bound")
                .try_get_integer()
                .expect("t is an integer")
        })
        .collect()
}

#[tokio::test]
async fn timeout_ms_round_trips() {
    let test_db = TestDb::new("gecko_test_sync_timeout");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;
    let dir = TempDir::new().unwrap();

    write_timeout_bundle(dir.path(), "timeout-ms: 500");
    let bundle = parse_bundle(dir.path()).expect("parse");
    sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT)
        .await
        .expect("sync");
    assert_eq!(stored_timeouts(&mut db).await, [500]);

    write_timeout_bundle(dir.path(), "timeout-ms: 750");
    let bundle = parse_bundle(dir.path()).expect("parse");
    sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT)
        .await
        .expect("re-sync");
    assert_eq!(
        stored_timeouts(&mut db).await,
        [750],
        "the edit replaces it"
    );

    write_timeout_bundle(dir.path(), "");
    let bundle = parse_bundle(dir.path()).expect("parse");
    sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT)
        .await
        .expect("re-sync");
    assert!(
        stored_timeouts(&mut db).await.is_empty(),
        "removing timeout-ms leaves no stored value"
    );
}

#[tokio::test]
async fn an_out_of_range_timeout_is_refused_before_writing() {
    let test_db = TestDb::new("gecko_test_sync_timeout_range");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;
    let dir = TempDir::new().unwrap();
    write_timeout_bundle(dir.path(), "timeout-ms: 500");
    let mut bundle = parse_bundle(dir.path()).expect("parse");
    bundle.concepts[0].timeout_ms = Some(MAX_TIMEOUT_MS + 1);

    let err = sync_bundle(&mut db, &bundle, &[], BELIEF_TIER_ROOT)
        .await
        .expect_err("an out-of-range timeout");
    assert!(
        matches!(
            &err,
            DbError::InvalidTimeout { concept_id, value }
                if concept_id == "playbooks/bounded" && *value == MAX_TIMEOUT_MS + 1
        ),
        "{err}"
    );
    assert_eq!(
        count(&mut db, "match $c isa concept;").await,
        0,
        "nothing was written"
    );
}

// -- OKF v0.2 provenance --

const V02_AT: &str = "2026-11-28T02:42:19Z";

/// Writes the concept `rel` (a bundle-relative path) with `frontmatter` lines.
fn write_concept_with(dir: &Path, rel: &str, frontmatter: &str) {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        format!("---\ntype: guide\n{frontmatter}\n---\n\nBody.\n"),
    )
    .unwrap();
}

async fn sync_dir(db: &mut TypeDbRouter, dir: &Path) {
    let bundle = parse_bundle(dir).expect("parse");
    sync_bundle(db, &bundle, &[], BELIEF_TIER_ROOT)
        .await
        .expect("sync");
}

#[tokio::test]
async fn provenance_attributes_round_trip() {
    let test_db = TestDb::new("gecko_test_sync_provenance");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;
    let dir = TempDir::new().unwrap();
    write_concept_with(
        dir.path(),
        "page.md",
        &format!(
            "generated: {{ by: openwiki/0.9.0, at: {V02_AT} }}\nstatus: draft\n\
             stale_after: 2027-01-01T00:00:00Z\nverified: {{ by: human:alice, at: {V02_AT} }}"
        ),
    );
    sync_dir(&mut db, dir.path()).await;
    for (query, what) in [
        (
            r#"match $c isa concept, has generated-by "openwiki/0.9.0";"#,
            "generated-by",
        ),
        (
            "match $c isa concept, has timestamp 2026-11-28T02:42:19;",
            "timestamp",
        ),
        (
            r#"match $c isa concept, has okf-status "draft";"#,
            "okf-status",
        ),
        (
            "match $c isa concept, has stale-after 2027-01-01T00:00:00;",
            "stale-after",
        ),
        (
            r#"match $c isa concept, has trust-tier "human-reviewed";"#,
            "trust-tier",
        ),
    ] {
        assert_eq!(count(&mut db, query).await, 1, "{what}");
    }

    write_concept_with(dir.path(), "page.md", "title: Plain");
    sync_dir(&mut db, dir.path()).await;
    for attr in [
        "generated-by",
        "timestamp",
        "okf-status",
        "stale-after",
        "trust-tier",
    ] {
        assert_eq!(
            count(&mut db, &format!("match $c isa concept, has {attr} $v;")).await,
            0,
            "{attr} survives the edit that removed it"
        );
    }
    assert_eq!(count(&mut db, "match $v isa verification;").await, 0);
}

#[tokio::test]
async fn verification_events_round_trip() {
    let test_db = TestDb::new("gecko_test_sync_verification");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;
    let dir = TempDir::new().unwrap();
    let event = |by: &str, at: &str| format!("{{ by: {by}, at: {at} }}");
    write_concept_with(
        dir.path(),
        "page.md",
        &format!(
            "verified: [{}, {}, {}]",
            event("a/1", "2026-01-01T00:00:00Z"),
            event("b/2", "2026-02-01T00:00:00Z"),
            event("a/1", "2026-01-01T00:00:00Z"),
        ),
    );
    sync_dir(&mut db, dir.path()).await;
    assert_eq!(count(&mut db, "match $v isa verification;").await, 2);
    for (by, at) in [
        ("a/1", "2026-01-01T00:00:00"),
        ("b/2", "2026-02-01T00:00:00"),
    ] {
        assert_eq!(
            count(
                &mut db,
                &format!(
                    r#"match $c isa concept, has concept-id "page";
                       $v isa verification, links (verified-concept: $c),
                          has verified-by "{by}", has verified-at {at};"#
                )
            )
            .await,
            1,
            "{by} keeps its own time"
        );
    }

    write_concept_with(
        dir.path(),
        "page.md",
        &format!("verified: [{}]", event("c/3", "2026-03-01T00:00:00Z")),
    );
    sync_dir(&mut db, dir.path()).await;
    assert_eq!(count(&mut db, "match $v isa verification;").await, 1);
    assert_eq!(
        count(
            &mut db,
            r#"match $v isa verification, has verified-by "c/3";"#
        )
        .await,
        1,
        "the edit replaces the events"
    );
}

#[tokio::test]
async fn gc_and_retirement_leave_no_verification() {
    let test_db = TestDb::new("gecko_test_sync_verification_gc");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;
    let dir = TempDir::new().unwrap();
    let verified = format!("verified: {{ by: a/1, at: {V02_AT} }}");
    write_concept_with(dir.path(), "gone.md", &verified);
    write_concept_with(dir.path(), "referenced.md", &verified);
    write_concept_with(dir.path(), "stays.md", "title: Stays");
    sync_dir(&mut db, dir.path()).await;
    assert_eq!(count(&mut db, "match $v isa verification;").await, 2);

    claim_with_belief(&mut db, "mem/bel/ref", "referenced").await;
    fs::remove_file(dir.path().join("gone.md")).unwrap();
    fs::remove_file(dir.path().join("referenced.md")).unwrap();
    sync_dir(&mut db, dir.path()).await;
    assert_eq!(retired_at(&mut db, "referenced").await.len(), 1, "retired");
    assert_eq!(
        count(&mut db, r#"match $c isa concept, has concept-id "gone";"#).await,
        0,
        "deleted"
    );
    assert_eq!(count(&mut db, "match $v isa verification;").await, 0);
}

/// The number of bundles whose `okf-version` is `v`.
async fn okf_versions(db: &mut TypeDbRouter, v: &str) -> usize {
    count(
        db,
        &format!(r#"match $b isa bundle, has okf-version "{v}";"#),
    )
    .await
}

#[tokio::test]
async fn okf_version_follows_the_root_index() {
    let test_db = TestDb::new("gecko_test_sync_okf_version");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;
    let dir = TempDir::new().unwrap();
    write_concept_with(dir.path(), "page.md", "title: Page");

    fs::write(
        dir.path().join("index.md"),
        "---\nokf_version: \"0.2\"\n---\n",
    )
    .unwrap();
    sync_dir(&mut db, dir.path()).await;
    assert_eq!(okf_versions(&mut db, "0.2").await, 1);

    fs::write(
        dir.path().join("index.md"),
        "---\nokf_version: \"9.9\"\n---\n",
    )
    .unwrap();
    sync_dir(&mut db, dir.path()).await;
    assert_eq!(
        okf_versions(&mut db, "9.9").await,
        1,
        "an index-only change re-syncs it"
    );
    assert_eq!(
        count(&mut db, "match $b isa bundle, has okf-version $v;").await,
        1
    );

    fs::write(dir.path().join("index.md"), "# Files\n").unwrap();
    sync_dir(&mut db, dir.path()).await;
    assert_eq!(
        count(&mut db, "match $b isa bundle, has okf-version $v;").await,
        0
    );
}

#[tokio::test]
async fn a_v01_bundle_gains_no_provenance_attributes() {
    let test_db = TestDb::new("gecko_test_sync_v01_provenance");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;
    let dir = TempDir::new().unwrap();
    create_test_bundle(dir.path());
    sync_dir(&mut db, dir.path()).await;
    for query in [
        "match $c isa concept, has trust-tier $v;",
        "match $c isa concept, has generated-by $v;",
        "match $v isa verification;",
        "match $b isa bundle, has okf-version $v;",
    ] {
        assert_eq!(count(&mut db, query).await, 0, "{query}");
    }
}

// -- Which files are concepts (OKF §3.2) --

#[tokio::test]
async fn a_bundle_with_a_readme_syncs() {
    let test_db = TestDb::new("gecko_test_sync_readme");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;
    let dir = TempDir::new().unwrap();
    write_concept_with(dir.path(), "page.md", "title: Page");
    fs::write(dir.path().join("README.md"), "# About this bundle\n").unwrap();
    sync_dir(&mut db, dir.path()).await;
    assert_eq!(count(&mut db, "match $c isa concept;").await, 1);
}

#[tokio::test]
async fn a_concept_that_loses_its_type_is_dropped() {
    let test_db = TestDb::new("gecko_test_sync_loses_type");
    let mut db = test_db.router();
    apply_core(&mut db, true).await;
    let dir = TempDir::new().unwrap();
    write_concept_with(dir.path(), "page.md", "title: Page");
    write_concept_with(dir.path(), "other.md", "title: Other");
    sync_dir(&mut db, dir.path()).await;
    assert_eq!(count(&mut db, "match $c isa concept;").await, 2);

    fs::write(
        dir.path().join("page.md"),
        "---\ntitle: Page\n---\n\nBody.\n",
    )
    .unwrap();
    sync_dir(&mut db, dir.path()).await;
    assert_eq!(
        count(&mut db, r#"match $c isa concept, has concept-id "page";"#).await,
        0,
        "a file that lost its type is garbage-collected"
    );
}

#[tokio::test]
async fn a_referenced_concept_that_loses_its_type_is_retired() {
    let test_db = TestDb::new("gecko_test_sync_loses_type_retire");
    let mut db = test_db.router();
    let temp_dir = TempDir::new().unwrap();
    let (_full, referenced, _dropped) = synced_with_referenced(&mut db, temp_dir.path()).await;

    fs::write(
        temp_dir.path().join(format!("{referenced}.md")),
        "---\ntitle: Test Concept\n---\n\nNo type any more.\n",
    )
    .unwrap();
    sync_dir(&mut db, temp_dir.path()).await;
    assert_eq!(retired_at(&mut db, &referenced).await.len(), 1);
}
