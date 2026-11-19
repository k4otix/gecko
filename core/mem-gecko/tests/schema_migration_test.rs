//! Schema re-application on an already-initialized database (live TypeDB 3.12).
//!
//! Applying the schema again must succeed, and a function whose shipped body
//! changed must take effect on a database that already stores the old body.
//! Each test provisions its own throwaway database and drops it; the real
//! `gecko` database is never touched.

use futures_util::StreamExt;
use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_extension_api::GeckoExtension;
use ulid::Ulid;

mod common;

const CORE_SCHEMA: &str = include_str!("../../gecko-engine/schema/core_schema.tql");

/// The mem schema as released before the population-member window landed.
const PREVIOUS_MEM_SCHEMA: &str = include_str!("fixtures/mem_schema_95ebac1.tql");

/// The shipped `population-members` body, and the earlier body that did not
/// exclude closed membership rows.
const NEW_BODY: &str =
    "          $pm isa population-member, links (owning-population: $pop, member: $m);
          not { $pm has valid-to $vt; };";
const OLD_BODY: &str = "          (owning-population: $pop, member: $m) isa population-member;";

fn router() -> (TypeDbRouter, String) {
    let name = format!("migtest_{}", Ulid::generate().to_string().to_lowercase());
    let router = TypeDbRouter::new(DbConfig {
        address: "localhost:1729".to_string(),
        database: name.clone(),
        username: "admin".to_string(),
        password: "password".to_string(),
        tls: TlsMode::Disabled,
    });
    (router, name)
}

async fn members(router: &mut TypeDbRouter) -> Vec<String> {
    let tx = router.begin_read().await.expect("read tx");
    let answer = tx
        .query(
            r#"match $p isa population, has spec-hash "pop/m";
                     let $m in population-members($p);
                     $m has concept-id $id;
               fetch { "id": $id };"#,
        )
        .await
        .expect("read query");
    let mut out = Vec::new();
    let mut stream = answer.into_documents();
    while let Some(doc) = stream.next().await {
        let v: serde_json::Value =
            serde_json::from_str(&doc.expect("document").into_json().to_string()).unwrap();
        out.push(v["id"].as_str().unwrap().to_string());
    }
    out.sort();
    out
}

#[tokio::test]
async fn reapplying_the_schema_is_idempotent() {
    let (mut router, name) = router();
    let mem = mem_gecko::MemGecko::new();
    for _ in 0..2 {
        router.apply_schema(CORE_SCHEMA).await.expect("core schema");
        router
            .apply_schema_with_migrations(mem.schema(), mem.schema_migrations())
            .await
            .expect("mem schema");
    }
    common::drop_db_with_retry(&mut router, &name).await;
}

#[tokio::test]
async fn previous_release_upgrades_to_the_shipped_schema() {
    let (mut router, name) = router();
    let mem = mem_gecko::MemGecko::new();
    router.apply_schema(CORE_SCHEMA).await.expect("core schema");
    router
        .apply_schema(PREVIOUS_MEM_SCHEMA)
        .await
        .expect("previous mem schema");

    // A changed type annotation is refused by a plain re-define…
    let err = router
        .apply_schema(mem.schema())
        .await
        .expect_err("a changed @doc cannot be re-defined");
    assert!(format!("{err}").contains("DEX15"), "{err}");

    // …and lands through the migration, together with the new functions and the
    // window attributes, in one transaction. A second apply skips the migration.
    for _ in 0..2 {
        router.apply_schema(CORE_SCHEMA).await.expect("core schema");
        router
            .apply_schema_with_migrations(mem.schema(), mem.schema_migrations())
            .await
            .expect("upgrade to the shipped mem schema");
    }
    let schema = router.database_schema().await.expect("schema");
    for m in mem.schema_migrations() {
        assert!(!schema.contains(m.applies_when), "{} left old text", m.id);
    }
    assert!(schema.contains("fun blast-radius-at("));
    assert!(schema.contains("fun population-members-at("));

    common::drop_db_with_retry(&mut router, &name).await;
}

#[tokio::test]
async fn changed_function_body_reaches_an_initialized_database() {
    let (mut router, name) = router();
    let shipped = mem_gecko::MemGecko::new().schema().to_string();
    assert!(
        shipped.contains(NEW_BODY),
        "shipped body drifted from the test"
    );
    let old_schema = shipped.replace(NEW_BODY, OLD_BODY);

    router.apply_schema(CORE_SCHEMA).await.expect("core schema");
    router
        .apply_schema(&old_schema)
        .await
        .expect("old mem schema");

    {
        let tx = router.begin_write().await.expect("write tx");
        tx.query(
            r#"insert
                 $p isa population, has spec-hash "pop/m";
                 $a isa concept, has concept-id "m/a";
                 $b isa concept, has concept-id "m/b";
                 (owning-population: $p, member: $a) isa population-member,
                     has valid-from 2026-01-01T00:00:00, has valid-to 2026-02-01T00:00:00;
                 (owning-population: $p, member: $b) isa population-member,
                     has valid-from 2026-01-01T00:00:00;"#,
        )
        .await
        .expect("seed rows");
        tx.commit().await.expect("commit");
    }
    assert_eq!(
        members(&mut router).await,
        ["m/a", "m/b"],
        "the old body returns closed rows"
    );

    router
        .apply_schema(&shipped)
        .await
        .expect("re-apply shipped schema");
    assert_eq!(
        members(&mut router).await,
        ["m/b"],
        "the shipped body replaced the stored one"
    );

    common::drop_db_with_retry(&mut router, &name).await;
}

#[tokio::test]
async fn trailing_statement_after_a_function_reapplies() {
    let (mut router, name) = router();
    let fragment = r#"define
  entity widget, owns label;
  attribute label, value string;
  attribute tag, value string;
  fun widgets() -> { widget }:
      match $w isa widget;
      return { $w };
  # an additive statement after the last function
  widget owns tag @card(0..1);
"#;
    for _ in 0..3 {
        router.apply_schema(fragment).await.expect("apply fragment");
    }
    let schema = router.database_schema().await.expect("schema");
    assert!(schema.contains("owns tag @card(0..1)"), "{schema}");
    common::drop_db_with_retry(&mut router, &name).await;
}

#[tokio::test]
async fn callee_signature_change_lands_before_its_caller() {
    let (mut router, name) = router();
    let before = r#"define
  entity widget, owns label;
  attribute label, value string;
  fun pick($w: widget) -> { widget }:
      match $w isa widget;
      return { $w };
  fun uses-pick($w: widget) -> boolean:
      match let $x in pick($w);
      return check;
"#;
    // The caller precedes the callee, and the callee gains a parameter.
    let after = r#"define
  entity widget, owns label;
  attribute label, value string;
  fun uses-pick($w: widget) -> boolean:
      match let $x in pick($w, "a");
      return check;
  fun pick($w: widget, $l: string) -> { widget }:
      match $w isa widget, has label == $l;
      return { $w };
"#;
    router.apply_schema(before).await.expect("first release");
    for _ in 0..2 {
        router.apply_schema(after).await.expect("second release");
    }
    let schema = router.database_schema().await.expect("schema");
    assert!(
        schema.contains("fun pick($w: widget, $l: string)"),
        "{schema}"
    );
    assert!(schema.contains("pick($w, \"a\")"), "{schema}");
    common::drop_db_with_retry(&mut router, &name).await;
}
