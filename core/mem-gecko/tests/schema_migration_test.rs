//! Schema re-application on an already-initialized database (live TypeDB).
//!
//! Applying the schema again must succeed, and a function whose shipped body
//! changed must take effect on a database that already stores the old body.
//! Each test provisions its own throwaway database and drops it; the real
//! `gecko` database is never touched.

use futures_util::StreamExt;
use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_engine::db::schema_apply;
use gecko_extension_api::{GeckoExtension, SchemaMigration};
use ulid::Ulid;

mod common;
#[path = "common/provenance.rs"]
mod provenance;

const CORE_SCHEMA: &str = include_str!("../../gecko-engine/schema/core_schema.tql");

/// The mem schema of an earlier release, before belief's `@doc` changed.
const PREVIOUS_MEM_SCHEMA: &str = include_str!("fixtures/mem_schema_95ebac1.tql");

/// The shipped `canonical-step` body's first line, which matches only asserted
/// resolutions, and a variant that matches resolutions in any belief-state.
const NEW_BODY: &str = "          $res isa resolution, has belief-state \"asserted\";";
const OLD_BODY: &str = "          $res isa resolution;";

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

/// The records `canonical-step` resolves `r/a` to directly, sorted.
async fn steps(router: &mut TypeDbRouter) -> Vec<String> {
    let tx = router.begin_read().await.expect("read tx");
    let answer = tx
        .query(
            r#"match $r isa concept, has concept-id "r/a";
                     let $o in canonical-step($r);
                     $o has concept-id $id;
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

    // The exported functions read back as the shipped text, so a further apply
    // plans no function define or redefine.
    let exported = router.database_schema().await.expect("schema");
    let stored = schema_apply::stored_functions(&exported).expect("stored functions");
    for fragment in [CORE_SCHEMA, mem.schema()] {
        let plan = schema_apply::plan_queries(fragment, &stored).expect("plan");
        let functions: Vec<&String> = plan.iter().filter(|q| q.contains("fun ")).collect();
        assert!(
            functions.is_empty(),
            "a re-apply would rewrite functions: {functions:?}"
        );
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

    // A changed annotation is refused by a plain re-define…
    let err = router
        .apply_schema(mem.schema())
        .await
        .expect_err("a changed @doc cannot be re-defined");
    assert!(format!("{err}").contains("DEX16"), "{err}");

    // …and lands through the migrations, together with the new functions, in one
    // transaction. A second apply skips the migrations.
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
    // The shipped functions land alongside the migrations; the earlier release's
    // population function and relation stay stored and unused.
    for shipped in [
        "fun blast-step($r: memory-item)",
        "fun canonical-entity-at(",
    ] {
        assert!(schema.contains(shipped), "upgrade lacks `{shipped}`");
    }
    for leftover in ["fun population-members(", "relation population-member"] {
        assert!(schema.contains(leftover), "upgrade dropped `{leftover}`");
    }

    common::drop_db_with_retry(&mut router, &name).await;
}

#[tokio::test]
async fn changed_function_body_reaches_an_initialized_database() {
    let (mut router, name) = router();
    let shipped = mem_gecko::MemGecko::new().schema().to_string();
    assert_eq!(
        shipped.matches(NEW_BODY).count(),
        1,
        "shipped body drifted from the test"
    );
    let old_schema = shipped.replace(NEW_BODY, OLD_BODY);

    router.apply_schema(CORE_SCHEMA).await.expect("core schema");
    router
        .apply_schema(&old_schema)
        .await
        .expect("old mem schema");

    // Two provable resolutions of `r/a`: one asserted, one retracted.
    write_seed(
        &mut router,
        r#"insert
             $a isa concept, has concept-id "r/a";
             $b isa concept, has concept-id "r/b";
             $c isa concept, has concept-id "r/c";
             $ep isa episode, has concept-id "mem/ep/r",
                 has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00;
             $live isa resolution, has concept-id "mem/bel/r-live", has belief-state "asserted";
             $gone isa resolution, has concept-id "mem/bel/r-gone", has belief-state "retracted";
             derivation (derived: $live, source: $ep), has derivation-method "type-join";
             derivation (derived: $gone, source: $ep), has derivation-method "type-join";
             resolves (resolution-belief: $live, record-a: $a, record-b: $b);
             resolves (resolution-belief: $gone, record-a: $a, record-b: $c);"#,
    )
    .await
    .expect("seed resolutions");
    assert_eq!(
        steps(&mut router).await,
        ["r/b", "r/c"],
        "the old body follows the retracted resolution"
    );

    router
        .apply_schema(&shipped)
        .await
        .expect("re-apply shipped schema");
    assert_eq!(
        steps(&mut router).await,
        ["r/b"],
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

#[tokio::test]
async fn narrowing_a_callee_under_a_stored_caller_applies() {
    let (mut router, name) = router();
    // Both functions take the supertype, and the caller calls the callee.
    let before = r#"define
  entity gadget, owns label;
  entity widget sub gadget;
  attribute label, value string;
  fun reach($g: gadget) -> { gadget }:
      match let $x in step($g);
      return { $x };
  fun step($g: gadget) -> { gadget }:
      match $g isa gadget;
      return { $g };
"#;
    // Both narrow to the subtype. The plan redefines the callee first, so TypeDB
    // checks the narrowed callee against the stored, still-wide caller.
    let after = r#"define
  entity gadget, owns label;
  entity widget sub gadget;
  attribute label, value string;
  fun reach($w: widget) -> { gadget }:
      match let $x in step($w);
      return { $x };
  fun step($w: widget) -> { gadget }:
      match $w isa widget;
      return { $w };
"#;
    router.apply_schema(before).await.expect("first release");
    write(&mut router, r#"insert $w isa widget, has label "w";"#)
        .await
        .expect("seed a widget");
    for _ in 0..2 {
        router.apply_schema(after).await.expect("narrowed release");
    }
    let schema = router.database_schema().await.expect("schema");
    assert!(schema.contains("fun step($w: widget)"), "{schema}");
    assert!(schema.contains("fun reach($w: widget)"), "{schema}");
    assert_eq!(
        count(
            &mut router,
            r#"match $w isa widget, has label "w"; let $x in reach($w);"#
        )
        .await,
        1,
        "the caller answers through the narrowed callee"
    );
    common::drop_db_with_retry(&mut router, &name).await;
}

/// The probe's schema before tightening. `probe-mark` is owned with an explicit
/// card by two types, as the pre-tightening fixture owns `valid-from`.
const PROBE_BEFORE: &str = r#"define
  attribute probe-weight @doc("A score the probe bounds to [0, 1]."), value double;
  attribute probe-count @doc("A count the probe bounds below by zero."), value integer;
  attribute probe-mark @doc("A mark the probe makes required on one owner."), value string;
  attribute probe-label @doc("A label the probe makes required on a subtype."), value string;
  entity probe-base @abstract @doc("An abstract owner of an optional label."),
      owns probe-label @card(0..1);
  entity probe-leaf sub probe-base @doc("A subtype that comes to require the inherited label.");
  entity probe-node @doc("A node the probe links and scores."),
      owns probe-weight @card(0..1),
      owns probe-count @card(0..1),
      owns probe-mark @card(0..1),
      plays probe-link:probe-from,
      plays probe-link:probe-to,
      plays probe-pair:left,
      plays probe-pair:right;
  relation probe-link @doc("A link whose roles the probe makes required."),
      relates probe-from @doc("The node the link starts at."),
      relates probe-to @doc("The node the link ends at.");
  relation probe-pair @doc("A pair that keeps an optional mark."),
      relates left @doc("The first node of the pair."),
      relates right @doc("The second node of the pair."),
      owns probe-mark @card(0..1);
"#;

/// The probe's schema after tightening: required roles and an inherited owns
/// specialized by additive `define`, ranges beside the existing cards, and a
/// standalone statement for the card a migration replaces.
const PROBE_AFTER: &str = r#"define
  attribute probe-weight @doc("A score the probe bounds to [0, 1]."), value double;
  attribute probe-count @doc("A count the probe bounds below by zero."), value integer;
  attribute probe-mark @doc("A mark the probe makes required on one owner."), value string;
  attribute probe-label @doc("A label the probe makes required on a subtype."), value string;
  entity probe-base @abstract @doc("An abstract owner of an optional label."),
      owns probe-label @card(0..1);
  entity probe-leaf sub probe-base @doc("A subtype that comes to require the inherited label.");
  probe-leaf owns probe-label @card(1);
  entity probe-node @doc("A node the probe links and scores."),
      owns probe-weight @card(0..1) @range(0.0..1.0),
      owns probe-count @card(0..1) @range(0..),
      plays probe-link:probe-from,
      plays probe-link:probe-to,
      plays probe-pair:left,
      plays probe-pair:right;
  probe-node owns probe-mark @card(1);
  relation probe-link @doc("A link whose roles the probe makes required."),
      relates probe-from @card(1) @doc("The node the link starts at."),
      relates probe-to @card(1) @doc("The node the link ends at.");
  relation probe-pair @doc("A pair that keeps an optional mark."),
      relates left @doc("The first node of the pair."),
      relates right @doc("The second node of the pair."),
      owns probe-mark @card(0..1);
"#;

/// The exported text of `probe-node`'s mark before tightening. The export lists a
/// type's owns alphabetically and prints `probe-pair`'s identical line, so the
/// trigger spans to the next line, which only `probe-node` has.
const PROBE_MARK_TRIGGER: &str = "  owns probe-mark @card(0..1),\n  owns probe-weight";

/// The probe's upgrade: drop links missing either role, then replace the mark's
/// explicit card. Each trigger is exported text the shipped schema no longer shows.
const PROBE_MIGRATIONS: &[SchemaMigration] = &[
    SchemaMigration {
        id: "probe/delete-one-player-links",
        applies_when: "relates probe-from @doc(\"The node the link starts at.\")",
        tql: "match $r isa probe-link; { not { $r links (probe-from: $f); }; } or { not { $r links (probe-to: $t); }; }; delete $r;",
    },
    SchemaMigration {
        id: "probe/node-mark-required",
        applies_when: PROBE_MARK_TRIGGER,
        tql: "redefine probe-node owns probe-mark @card(1);",
    },
];

async fn write(router: &mut TypeDbRouter, tql: &str) -> Result<(), String> {
    let tx = router.begin_write().await.map_err(|e| e.to_string())?;
    tx.query(tql).await.map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())
}

async fn count(router: &mut TypeDbRouter, tql: &str) -> usize {
    let tx = router.begin_read().await.expect("read tx");
    let answer = tx.query(tql).await.expect("count query");
    answer.into_rows().collect::<Vec<_>>().await.len()
}

const SEED_NODES: &str = r#"insert
    $a isa probe-node, has probe-mark "a";
    $b isa probe-node, has probe-mark "b";
    probe-link (probe-from: $a, probe-to: $b);
    probe-pair (left: $a, right: $b);"#;

const SEED_ONE_PLAYER_LINK: &str = r#"match $a isa probe-node, has probe-mark "a";
    insert probe-link (probe-from: $a);"#;

#[tokio::test]
async fn tightening_forms_probe() {
    // Forms 1–4 and 8 on conforming data: the tightened schema and its migrations
    // apply, a second apply is a no-op, and each constraint refuses a violation.
    let (mut router, name) = router();
    router
        .apply_schema(PROBE_BEFORE)
        .await
        .expect("probe schema");
    write(&mut router, SEED_NODES).await.expect("seed");
    let before = router.database_schema().await.expect("schema");
    for m in PROBE_MIGRATIONS {
        assert_eq!(
            before.matches(m.applies_when).count(),
            1,
            "{}: the trigger is not unique to its target:\n{before}",
            m.id
        );
    }
    // `define` cannot replace an explicit card; the migration's `redefine` does.
    let err = router
        .apply_schema(PROBE_AFTER)
        .await
        .expect_err("define over an explicit card is refused");
    eprintln!("define over an explicit card: {err}");
    assert!(format!("{err}").contains("DEX16"), "{err}");
    for _ in 0..2 {
        router
            .apply_schema_with_migrations(PROBE_AFTER, PROBE_MIGRATIONS)
            .await
            .expect("tightened probe schema");
    }
    let after = router.database_schema().await.expect("schema");
    eprintln!("probe export after tightening:\n{after}");
    for m in PROBE_MIGRATIONS {
        assert!(!after.contains(m.applies_when), "{} still pending", m.id);
    }
    // A repeated identical `redefine` is refused, which the trigger prevents.
    let err = {
        let tx = router.begin_schema().await.expect("schema tx");
        tx.query(PROBE_MIGRATIONS[1].tql).await.err()
    };
    eprintln!("repeated redefine: {err:?}");
    assert!(format!("{err:?}").contains("REX27"), "{err:?}");

    let refusals = [
        // Form 1: a required role.
        (SEED_ONE_PLAYER_LINK, "probe-link"),
        // Form 2: an inherited owns specialized to a required card.
        ("insert $l isa probe-leaf;", "probe-leaf"),
        // Form 3: an explicit card replaced by `redefine`.
        ("insert $n isa probe-node;", "probe-node"),
        // Form 4: ranges on the owns.
        (
            r#"insert $n isa probe-node, has probe-mark "c", has probe-weight 1.5;"#,
            "probe-weight",
        ),
        (
            r#"insert $n isa probe-node, has probe-mark "d", has probe-count -1;"#,
            "probe-count",
        ),
    ];
    for (tql, names) in refusals {
        let err = write(&mut router, tql)
            .await
            .expect_err("the tightened schema refuses the write");
        eprintln!("refused {names}: {err}");
        assert!(err.contains(names), "{err}");
    }
    // The other owner of the mark keeps its optional card.
    write(
        &mut router,
        r#"match $a isa probe-node, has probe-mark "a"; $b isa probe-node, has probe-mark "b";
           insert probe-pair (left: $a, right: $b);"#,
    )
    .await
    .expect("probe-pair's mark stays optional");
    write(
        &mut router,
        r#"insert $l isa probe-leaf, has probe-label "l";
           $n isa probe-node, has probe-mark "e", has probe-weight 1.0, has probe-count 0;"#,
    )
    .await
    .expect("conforming rows commit");
    common::drop_db_with_retry(&mut router, &name).await;
    drop(router);

    // Forms 6 and 7 on a database holding a one-player link: the tightened schema
    // alone is refused with an error naming the relation, and with the cleanup
    // ahead of it in the same schema transaction it commits.
    let (mut router, name) = self::router();
    router
        .apply_schema(PROBE_BEFORE)
        .await
        .expect("probe schema");
    write(&mut router, SEED_NODES).await.expect("seed");
    write(&mut router, SEED_ONE_PLAYER_LINK)
        .await
        .expect("a one-player link commits before tightening");
    write(
        &mut router,
        r#"match $b isa probe-node, has probe-mark "b"; insert probe-link (probe-to: $b);"#,
    )
    .await
    .expect("a link missing its other role commits before tightening");
    let err = router
        .apply_schema_with_migrations(PROBE_AFTER, &PROBE_MIGRATIONS[1..])
        .await
        .expect_err("the one-player links fail the required roles");
    eprintln!("tightening over violating data: {err}");
    assert!(format!("{err}").contains("probe-link"), "{err}");
    for _ in 0..2 {
        router
            .apply_schema_with_migrations(PROBE_AFTER, PROBE_MIGRATIONS)
            .await
            .expect("cleanup, then the tightened schema");
    }
    assert_eq!(count(&mut router, "match $r isa probe-link;").await, 1);
    common::drop_db_with_retry(&mut router, &name).await;
}

/// The shipped mem extension with extra migrations appended, for the registry probe.
struct ProbeMem {
    mem: mem_gecko::MemGecko,
    migrations: Vec<SchemaMigration>,
}

impl GeckoExtension for ProbeMem {
    fn name(&self) -> &str {
        "mem-gecko"
    }
    fn schema(&self) -> &str {
        self.mem.schema()
    }
    fn schema_migrations(&self) -> &[SchemaMigration] {
        &self.migrations
    }
    fn host_imports(&self) -> Vec<gecko_extension_api::HostImportDef> {
        vec![]
    }
    fn call_import(
        &self,
        name: &str,
        _args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        Err(format!("no import {name}"))
    }
}

#[test]
fn the_registry_accepts_the_tightening_migrations() {
    // Form 5: a `redefine` of a mem type's owns card and a one-player cleanup, as
    // mem migrations, pass the belief-edge registry.
    let mut migrations = mem_gecko::MEM_MIGRATIONS.to_vec();
    migrations.extend([
        SchemaMigration {
            id: "probe/resolves-one-player",
            applies_when: "relates record-b @card(1) @doc(\"The other record being resolved together.\")",
            tql: "match $r isa resolves; { not { $r links (record-a: $a); }; } or { not { $r links (record-b: $b); }; }; delete $r;",
        },
        SchemaMigration {
            id: "probe/belief-valid-from-required",
            applies_when: "  owns valid-from @card(0..1),\n  owns valid-to @card(0..1),\n  plays ",
            tql: "redefine belief owns valid-from @card(1);",
        },
    ]);
    let exts: Vec<Box<dyn GeckoExtension>> = vec![Box::new(ProbeMem {
        mem: mem_gecko::MemGecko::new(),
        migrations,
    })];
    mem_gecko::BeliefEdgeRegistry::from_extensions(CORE_SCHEMA, &exts)
        .expect("the registry accepts the tightening migrations");
}

/// The core and mem schemas as released before belief fields were required.
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

/// The migration that makes `belief owns valid-from` required.
fn valid_from_migration() -> &'static SchemaMigration {
    mem_gecko::MEM_MIGRATIONS
        .iter()
        .find(|m| m.tql == "redefine belief owns valid-from @card(1);")
        .expect("a migration makes belief's valid-from required")
}

/// Runs a write transaction whose raw memory items get their ownership, source-link
/// and belief fields from [`provenance::seed_provenance`] before the commit.
async fn write_seed(router: &mut TypeDbRouter, tql: &str) -> Result<(), String> {
    let tx = router.begin_write().await.map_err(|e| e.to_string())?;
    tx.query(tql).await.map_err(|e| e.to_string())?;
    gecko_engine::db::run_graph_writes(&tx, &provenance::seed_provenance())
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())
}

#[tokio::test]
async fn the_valid_from_migration_applies_once_to_an_existing_database() {
    let (mut router, name) = router();
    router
        .apply_schema(PRE_CORE)
        .await
        .expect("pre-change core");
    router
        .apply_schema(&pre_mem_schema())
        .await
        .expect("pre-change mem");
    // A belief as the writer stores it, and a similarity score just above 1 from an
    // f32 cosine.
    write_seed(
        &mut router,
        r#"insert
          $b isa belief, has concept-id "mem/bel/vf", has belief-state "asserted",
              has valid-from 2026-01-01T00:00:00, has salience 0.5,
              has base-activation 1.0, has last-access 2026-01-01T00:00:00;
          $re isa retrieval-event, has concept-id "mem/re/vf",
              has event-time 2026-01-01T00:00:00, has ingest-time 2026-01-01T00:00:00;
          surfaced (surfacer: $re, item: $b), has similarity-score 1.0000001192092896;"#,
    )
    .await
    .expect("seed the pre-change database");

    let migration = valid_from_migration();
    let before = router.database_schema().await.expect("schema");
    assert_eq!(
        before.matches(migration.applies_when).count(),
        1,
        "the valid-from trigger is not unique to belief:\n{before}"
    );
    let mem = mem_gecko::MemGecko::new();
    for _ in 0..2 {
        router
            .apply_schema_with_migrations(CORE_SCHEMA, gecko_engine::db::CORE_MIGRATIONS)
            .await
            .expect("core upgrades");
        router
            .apply_schema_with_migrations(mem.schema(), mem.schema_migrations())
            .await
            .expect("mem upgrades, and a second apply is a no-op");
    }
    let after = router.database_schema().await.expect("schema");
    for m in mem.schema_migrations() {
        assert!(!after.contains(m.applies_when), "{} left old text", m.id);
    }
    assert_eq!(
        after.matches("owns valid-from @card(1..1)").count(),
        1,
        "belief's valid-from is required:\n{after}"
    );
    assert_eq!(
        after.matches("owns valid-from @card(0..1)").count(),
        1,
        "the fixture's other owner of valid-from keeps it optional:\n{after}"
    );
    assert!(
        after.contains("validity window that opens at valid-from"),
        "belief's @doc describes the required valid-from:\n{after}"
    );

    // The upgraded database refuses a belief without valid-from…
    let err = write(
        &mut router,
        r#"insert
          $ag isa agent, has agent-id "agent-vf";
          $run isa doc-run, has run-id "run-vf";
          $b isa belief, has concept-id "mem/bel/no-vf", has belief-state "asserted",
              has salience 0.5, has base-activation 1.0, has last-access 2026-01-01T00:00:00;
          source-link (memory: $b, origin: $run), has origin-kind "test";
          ownership (owned: $b, owner: $ag), has visibility "private";"#,
    )
    .await
    .expect_err("a belief without valid-from commits after the upgrade");
    assert!(err.contains("type 'valid-from'"), "{err}");
    // …and holds the out-of-range similarity score clamped to 1.
    assert_eq!(
        count(
            &mut router,
            "match $s isa surfaced, has similarity-score $v; $v == 1.0;"
        )
        .await,
        1,
        "the similarity score above 1 is clamped"
    );
    common::drop_db_with_retry(&mut router, &name).await;
}
