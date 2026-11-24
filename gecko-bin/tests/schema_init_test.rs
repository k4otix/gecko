// Verifies core + mem schema initialization on every push, including the
// candle-free Tier-2 command (`cargo test --test '*' --no-default-features`).
// Only the `cyber-entity` assertion is gated behind the `cyber` feature, since
// that type only exists when the `cyber` extension is compiled in.
use futures_util::StreamExt;

#[cfg(feature = "cyber")]
use cyber_gecko::CyberGecko;
use gecko_engine::extension::GeckoExtension;
use mem_gecko::MemGecko;

#[cfg(feature = "cyber")]
#[path = "common/attack.rs"]
mod attack;
mod common;
#[cfg(feature = "cyber")]
#[path = "common/stix_objects.rs"]
mod stix_objects;
use common::TestDb;

#[tokio::test]
async fn test_schema_initialization() {
    // Self-cleaning database: dropped from the server when `test_db` goes out of
    // scope (declared first so it drops last, after `db` closes).
    let test_db = TestDb::new("gecko_test_schema");

    // 1. Initialize router and create DB
    let mut db = test_db.router();

    // 2. Apply core schema
    let core_schema = include_str!("../../core/gecko-engine/schema/core_schema.tql");
    db.apply_schema(core_schema)
        .await
        .expect("Failed to apply core schema");

    // 3. Assemble extensions
    #[allow(unused_mut)]
    let mut extensions: Vec<Box<dyn GeckoExtension>> = vec![Box::new(MemGecko::new())];
    #[cfg(feature = "cyber")]
    extensions.push(Box::new(CyberGecko::new()));

    // 4. Apply extension schemas
    for ext in extensions {
        let schema = ext.schema();
        if !schema.trim().is_empty() {
            db.apply_schema(schema)
                .await
                .unwrap_or_else(|e| panic!("Failed to apply {} schema: {}", ext.name(), e));
        }
    }

    // 5. Verify schemas were applied by querying the defined types
    let tx = db
        .begin_read()
        .await
        .expect("Failed to begin read transaction");

    // Check for a core type
    let answer = tx
        .query("match $t sub concept;")
        .await
        .expect("Failed to query core type");
    assert!(answer.is_row_stream());
    let rows: Vec<_> = answer.into_rows().collect::<Vec<_>>().await;
    assert!(!rows.is_empty(), "Core schema 'concept' type not found");

    // Check for a cyber extension type (only when the `cyber` feature compiled
    // the extension in and its schema was applied above).
    #[cfg(feature = "cyber")]
    {
        let answer = tx
            .query("match $t sub cyber-object;")
            .await
            .expect("Failed to query cyber type");
        let cyber_rows: Vec<_> = answer.into_rows().collect::<Vec<_>>().await;
        assert!(
            !cyber_rows.is_empty(),
            "Cyber schema 'cyber-object' type not found"
        );
    }

    // Check for a mem extension type
    let answer = tx
        .query("match $t sub execution-episode;")
        .await
        .expect("Failed to query mem type");
    let mem_rows: Vec<_> = answer.into_rows().collect::<Vec<_>>().await;
    assert!(
        !mem_rows.is_empty(),
        "Mem schema 'execution-episode' type not found"
    );
}

const OLD_BUNDLE_DOC: &str = "entity bundle @doc(\"The single OKF documentation bundle ingested into this database, keyed by its declared name; bundle-path is machine-local provenance, not identity.\")";
const NEW_BUNDLE_SENTENCE: &str = "An OKF documentation bundle synced into this database, keyed by its declared name and containing its concepts; several bundles may share a database, and bundle-path is machine-local provenance, not identity.";

#[tokio::test]
async fn previous_core_schema_upgrades_to_the_shipped_schema() {
    use gecko_engine::db::{CORE_MIGRATIONS, CORE_SCHEMA};

    let test_db = TestDb::new("gecko_test_core_migration");
    let mut db = test_db.router();

    let new_statement = format!("entity bundle @doc(\"{NEW_BUNDLE_SENTENCE}\")");
    let previous = CORE_SCHEMA.replace(&new_statement, OLD_BUNDLE_DOC);
    assert_ne!(
        previous, CORE_SCHEMA,
        "the previous schema must differ from the shipped one"
    );
    db.apply_schema(&previous)
        .await
        .expect("previous core schema applies");

    let err = db
        .apply_schema(CORE_SCHEMA)
        .await
        .expect_err("a changed annotation needs a migration");
    assert!(err.to_string().contains("DEX15"), "unexpected error: {err}");

    for _ in 0..2 {
        db.apply_schema_with_migrations(CORE_SCHEMA, CORE_MIGRATIONS)
            .await
            .expect("core schema applies with its migrations");
    }

    let schema = db.database_schema().await.expect("schema");
    assert!(
        !CORE_MIGRATIONS
            .iter()
            .any(|m| schema.contains(m.applies_when)),
        "a migration's replaced text is still in the database"
    );
    assert!(schema.contains(NEW_BUNDLE_SENTENCE));
}

/// An extension carrying the cyber schema as of the frozen fixture and the cyber
/// migrations, as the belief-edge registry reads an extension.
#[cfg(feature = "cyber")]
struct FrozenCyber {
    schema: &'static str,
}

#[cfg(feature = "cyber")]
impl GeckoExtension for FrozenCyber {
    fn name(&self) -> &str {
        "cyber-gecko"
    }
    fn schema(&self) -> &str {
        self.schema
    }
    fn schema_migrations(&self) -> &[gecko_extension_api::SchemaMigration] {
        &cyber_gecko::migrations::CYBER_MIGRATIONS
    }
    fn belief_edges(&self) -> &[gecko_extension_api::BeliefEdgeDecl] {
        &cyber_gecko::CYBER_BELIEF_EDGES
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

#[cfg(feature = "cyber")]
#[tokio::test]
async fn old_cyber_detection_types_migrate_away() {
    use gecko_engine::db::{CORE_MIGRATIONS, CORE_SCHEMA};
    use gecko_extension_api::GeckoExtension as _;

    const FROZEN: &str = include_str!("fixtures/cyber_schema_1cd621d.tql");
    let migrations = &cyber_gecko::migrations::CYBER_MIGRATIONS;

    // The registry accepts the migrations: none names a core or mem type.
    let exts: Vec<Box<dyn GeckoExtension>> = vec![
        Box::new(MemGecko::new()),
        Box::new(FrozenCyber { schema: FROZEN }),
    ];
    mem_gecko::BeliefEdgeRegistry::from_extensions(CORE_SCHEMA, &exts)
        .expect("the registry accepts the cyber migrations");

    // The frozen cyber schema has `population` play a role, so the database starts
    // on the pre-tightening core and mem, which declare it, and takes the shipped
    // core and mem with their migrations before the frozen cyber schema.
    let test_db = TestDb::new("gecko_test_cyber_migration");
    let mut db = test_db.router();
    db.apply_schema(PRE_CORE)
        .await
        .expect("pre-tightening core");
    db.apply_schema(&pre_mem_schema())
        .await
        .expect("pre-tightening mem");
    db.apply_schema_with_migrations(CORE_SCHEMA, CORE_MIGRATIONS)
        .await
        .expect("core schema");
    let mem = MemGecko::new();
    db.apply_schema_with_migrations(mem.schema(), mem.schema_migrations())
        .await
        .expect("mem schema");
    db.apply_schema(FROZEN).await.expect("frozen cyber schema");

    let before = db.database_schema().await.expect("schema");
    for gone in [
        "relation evidence-of",
        "relation requires-evidence",
        "relation sensor-coverage",
        "fun blinded-detections",
    ] {
        assert!(before.contains(gone), "frozen schema lacks {gone}");
    }

    // The seeded coverage names no sensor: the shipped schema requires a sensor to
    // own a platform, which the frozen schema's sensor cannot.
    let tx = db.begin_write().await.expect("begin write");
    tx.query(
        "insert $s isa data-source, has concept-id 'ds-1'; \
         $c isa data-component, has concept-id 'dc-1'; \
         $t isa attack-pattern, has concept-id 'tp-1'; \
         $p isa population, has spec-hash 'pop-1'; \
         evidence-of (evidence-component: $c, evidence-source: $s); \
         requires-evidence (technique: $t, required: $c); \
         sensor-coverage (covered-source: $s, covered-component: $c, coverage-scope: $p);",
    )
    .await
    .expect("seed instances");
    tx.commit().await.expect("commit seed");

    // A plain `define` of the shipped schema is refused over the changed `@doc`s.
    let shipped = CyberGecko::new();
    let err = db
        .apply_schema(shipped.schema())
        .await
        .expect_err("a changed annotation needs a migration");
    assert!(err.to_string().contains("DEX16"), "unexpected error: {err}");

    // The migrations run ahead of the shipped schema, and a second run is a no-op.
    for _ in 0..2 {
        db.apply_schema_with_migrations(shipped.schema(), migrations)
            .await
            .expect("cyber migrations apply");
    }

    let after = db.database_schema().await.expect("schema");
    for gone in [
        "evidence-of",
        "requires-evidence",
        "sensor-coverage",
        "blinded-detections",
        "coverage-scope",
    ] {
        assert!(!after.contains(gone), "{gone} survived the migration");
    }
    assert!(after.contains("entity data-source"));
    assert!(after.contains("entity population"));
    assert!(
        !migrations.iter().any(|m| after.contains(m.applies_when)),
        "a migration's replaced text is still in the database"
    );
    for doc in [
        "imported for reference since ATT&CK deprecates every data source.",
        "an analytic reads through a log source",
        "A deployed telemetry sensor from a sensor file",
    ] {
        assert!(
            after.contains(doc),
            "the shipped @doc {doc:?} is not installed"
        );
    }
    assert!(after.contains("fun claim-platforms"));
}

/// The shipped `@doc` of `undetected-techniques()`, and the one it replaced.
#[cfg(feature = "cyber")]
const LIVE_GAPS_DOC: &str =
    "The coverage-gap list: live ATT&CK techniques with no asserted detection claim against them.";
#[cfg(feature = "cyber")]
const OLD_GAPS_DOC: &str =
    "The coverage-gap list: ATT&CK techniques with no asserted detection claim against them.";

#[cfg(feature = "cyber")]
#[tokio::test]
async fn a_changed_function_doc_reaches_an_existing_database() {
    use gecko_engine::db::{CORE_MIGRATIONS, CORE_SCHEMA};

    let shipped = CyberGecko::new();
    let previous = shipped.schema().replace(LIVE_GAPS_DOC, OLD_GAPS_DOC);
    assert_ne!(
        previous,
        shipped.schema(),
        "the shipped schema carries the live gaps @doc"
    );

    let test_db = TestDb::new("gecko_test_function_doc_redefine");
    let mut db = test_db.router();
    db.apply_schema_with_migrations(CORE_SCHEMA, CORE_MIGRATIONS)
        .await
        .expect("core schema");
    let mem = MemGecko::new();
    db.apply_schema_with_migrations(mem.schema(), mem.schema_migrations())
        .await
        .expect("mem schema");
    db.apply_schema(&previous)
        .await
        .expect("the previous cyber schema applies");
    assert!(
        db.database_schema()
            .await
            .expect("schema")
            .contains(OLD_GAPS_DOC)
    );

    for _ in 0..2 {
        db.apply_schema_with_migrations(shipped.schema(), shipped.schema_migrations())
            .await
            .expect("the shipped cyber schema redefines the function");
    }
    let after = db.database_schema().await.expect("schema");
    assert!(
        after.contains(LIVE_GAPS_DOC),
        "the new @doc is not installed"
    );
    assert!(!after.contains(OLD_GAPS_DOC), "the old @doc survived");
}

/// The schemas as released before the schema tightening.
const PRE_CORE: &str = include_str!("fixtures/schemas_pre_tightening/core_schema.tql");
const PRE_MEM_TYPES: &str = include_str!("fixtures/schemas_pre_tightening/mem_types.tql");
const PRE_MEM_FUNCTIONS: &str = include_str!("fixtures/schemas_pre_tightening/mem_functions.tql");

#[cfg(feature = "cyber")]
const PRE_CYBER: &str = include_str!("fixtures/schemas_pre_tightening/cyber_schema.tql");
#[cfg(feature = "cyber")]
const PRE_CYBER_FUNCTIONS: &str =
    include_str!("fixtures/schemas_pre_tightening/cyber_functions.tql");

/// The cyber schema and functions as released before the schema tightening.
#[cfg(feature = "cyber")]
fn pre_cyber_schema() -> String {
    format!("{PRE_CYBER}\n{PRE_CYBER_FUNCTIONS}")
}

/// The cyber types and relations added after the pre-tightening release, as the
/// shipped schema spells them.
#[cfg(feature = "cyber")]
const NEW_CYBER_RELATIONS: &str = r#"define
  attribute retired-attack-id value string;
  attribute tactic-shortname value string;

  cyber-object owns retired-attack-id @card(0..1);

  entity tactic sub cyber-object @doc("An ATT&CK tactic (STIX x-mitre-tactic): the adversary goal a technique serves, matched to techniques by its shortname."),
      owns tactic-shortname @card(1),
      plays technique-tactic:tactic;

  relation subtechnique-of @doc("An ATT&CK 'subtechnique-of' relationship: a sub-technique refines a parent technique, and a sub-technique has one parent."),
      relates subtechnique @card(1) @doc("The sub-technique that refines the parent."),
      relates parent @card(1) @doc("The technique the sub-technique refines.");

  relation revoked-by @doc("An ATT&CK 'revoked-by' relationship: a revoked object points to the object that replaced it, and a revoked object has one replacement."),
      relates revoked @card(1) @doc("The revoked object."),
      relates replacement @card(1) @doc("The object that replaced the revoked one.");

  relation technique-tactic @doc("Ties a technique to a tactic whose shortname is one of its kill-chain phases."),
      relates technique @card(1) @doc("The technique that serves the tactic."),
      relates tactic @card(1) @doc("The tactic the technique serves.");

  attack-pattern plays technique-tactic:technique,
      plays subtechnique-of:subtechnique @card(0..1),
      plays subtechnique-of:parent,
      plays revoked-by:revoked @card(0..1),
      plays revoked-by:replacement;
  intrusion-set plays revoked-by:revoked @card(0..1),
      plays revoked-by:replacement;
  malware plays revoked-by:revoked @card(0..1),
      plays revoked-by:replacement;
  tool plays revoked-by:revoked @card(0..1),
      plays revoked-by:replacement;
"#;

/// The mem types and functions as one `define`, composed as mem-gecko composes them.
fn pre_mem_schema() -> String {
    let functions = PRE_MEM_FUNCTIONS
        .trim_start()
        .strip_prefix("define")
        .expect("the functions fragment starts with define");
    format!("{}\n{functions}", PRE_MEM_TYPES.trim_end())
}

/// `text` with each `(from, to)` replacement made, each `from` occurring exactly once.
fn tightened(text: &str, edits: &[(&str, &str)]) -> String {
    let mut out = text.to_string();
    for (from, to) in edits {
        assert_eq!(out.matches(from).count(), 1, "{from:?} is not unique");
        out = out.replace(from, to);
    }
    out
}

async fn count_rows(db: &mut gecko_engine::db::router::TypeDbRouter, tql: &str) -> usize {
    let tx = db.begin_read().await.expect("read tx");
    let answer = tx.query(tql).await.expect("count query");
    answer.into_rows().collect::<Vec<_>>().await.len()
}

async fn write_rows(db: &mut gecko_engine::db::router::TypeDbRouter, tql: &str) {
    let tx = db.begin_write().await.expect("write tx");
    tx.query(tql).await.expect("write query");
    tx.commit().await.expect("commit");
}

/// An extension with the given name, schema, migrations and belief edges, as the
/// belief-edge registry reads an extension.
struct ProbeExtension {
    name: &'static str,
    schema: String,
    migrations: Vec<gecko_extension_api::SchemaMigration>,
    belief_edges: &'static [gecko_extension_api::BeliefEdgeDecl],
}

impl GeckoExtension for ProbeExtension {
    fn name(&self) -> &str {
        self.name
    }
    fn schema(&self) -> &str {
        &self.schema
    }
    fn schema_migrations(&self) -> &[gecko_extension_api::SchemaMigration] {
        &self.migrations
    }
    fn belief_edges(&self) -> &[gecko_extension_api::BeliefEdgeDecl] {
        self.belief_edges
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

/// The tightening's mem fragment: required `population-member` roles and a
/// required `belief` valid-from, carried by a standalone statement.
fn tightened_mem() -> String {
    tightened(
        &pre_mem_schema(),
        &[
            (
                "relates owning-population @doc(",
                "relates owning-population @card(1) @doc(",
            ),
            (
                "relates member @doc(\"A concept the population's",
                "relates member @card(1) @doc(\"A concept the population's",
            ),
            (
                "      owns valid-from @card(0..1),\n      owns valid-to @card(0..1),\n      owns retrieval-provenance",
                "      owns valid-to @card(0..1),\n      owns retrieval-provenance",
            ),
            (
                "  entity playbook sub memory-item",
                "  belief owns valid-from @card(1);\n\n  entity playbook sub memory-item",
            ),
        ],
    )
}

/// mem's migrations ahead of the tightened fragment: a one-player cleanup, then the
/// `redefine` of `belief`'s explicit valid-from card. The shipped belief-field
/// migrations are left out: this probe carries its own valid-from `redefine`, and
/// its fragment keeps belief's old `@doc` and an unranged similarity score.
fn tightened_mem_migrations() -> Vec<gecko_extension_api::SchemaMigration> {
    let mut migrations: Vec<_> = mem_gecko::MEM_MIGRATIONS
        .iter()
        .copied()
        .filter(|m| {
            !matches!(
                m.id,
                "mem/belief-required-window-doc"
                    | "mem/belief-valid-from-required"
                    | "mem/similarity-score-clamp-high"
                    | "mem/similarity-score-clamp-low"
            )
        })
        .collect();
    migrations.extend([
        gecko_extension_api::SchemaMigration {
            id: "probe/population-member-one-player",
            applies_when: "relates member @doc(\"A concept the population's spec currently matches.\")",
            tql: "match $r isa population-member; { not { $r links (owning-population: $p); }; } or { not { $r links (member: $m); }; }; delete $r;",
        },
        gecko_extension_api::SchemaMigration {
            id: "probe/belief-valid-from-required",
            applies_when: "  owns valid-from @card(0..1),\n  owns valid-to @card(0..1),\n  plays ",
            tql: "redefine belief owns valid-from @card(1);",
        },
    ]);
    migrations
}

#[tokio::test]
async fn tightening_forms_apply_to_the_pre_tightening_core_and_mem() {
    use gecko_engine::db::CORE_MIGRATIONS;

    let core = tightened(
        PRE_CORE,
        &[
            (
                "relates container @doc(",
                "relates container @card(1) @doc(",
            ),
            (
                "relates member @doc(\"A concept belonging",
                "relates member @card(1) @doc(\"A concept belonging",
            ),
        ],
    );
    let mut core_migrations = CORE_MIGRATIONS.to_vec();
    core_migrations.push(gecko_extension_api::SchemaMigration {
        id: "probe/containment-one-player",
        applies_when: "relates container @doc(\"The bundle that contains the members.\")",
        tql: "match $r isa containment; { not { $r links (container: $c); }; } or { not { $r links (member: $m); }; }; delete $r;",
    });
    let mem = tightened_mem();
    let mem_migrations = tightened_mem_migrations();

    // The registry accepts mem's cleanup and `redefine` entries.
    let exts: Vec<Box<dyn GeckoExtension>> = vec![Box::new(ProbeExtension {
        name: "mem-gecko",
        schema: mem.clone(),
        migrations: mem_migrations.clone(),
        belief_edges: &[],
    })];
    mem_gecko::BeliefEdgeRegistry::from_extensions(&core, &exts)
        .expect("the registry accepts mem's tightening migrations");

    let test_db = TestDb::new("gecko_test_tightening_core_mem");
    let mut db = test_db.router();
    db.apply_schema(PRE_CORE)
        .await
        .expect("pre-tightening core");
    db.apply_schema(&pre_mem_schema())
        .await
        .expect("pre-tightening mem");
    write_rows(
        &mut db,
        "insert $b isa bundle, has bundle-name 'probe-b'; \
         $c isa concept, has concept-id 'probe/c'; \
         $p isa population, has spec-hash 'probe-pop'; \
         containment (container: $b, member: $c); \
         containment (container: $b); \
         population-member (owning-population: $p, member: $c); \
         population-member (owning-population: $p);",
    )
    .await;

    let before = db.database_schema().await.expect("schema");
    for m in core_migrations.iter().chain(&mem_migrations) {
        assert!(
            before.matches(m.applies_when).count() <= 1,
            "{}: the trigger is not unique",
            m.id
        );
    }
    for m in [&core_migrations, &mem_migrations]
        .into_iter()
        .flat_map(|ms| ms.iter().filter(|m| m.id.starts_with("probe/")))
    {
        assert_eq!(before.matches(m.applies_when).count(), 1, "{}", m.id);
    }

    // Without its cleanup, each tightened fragment is refused over the one-player row.
    let core_without_cleanup: Vec<_> = core_migrations
        .iter()
        .copied()
        .filter(|m| !m.tql.starts_with("match $r isa containment;"))
        .collect();
    let err = db
        .apply_schema_with_migrations(&core, &core_without_cleanup)
        .await
        .expect_err("a one-player containment fails the required roles");
    assert!(err.to_string().contains("'containment'"), "{err}");
    for _ in 0..2 {
        db.apply_schema_with_migrations(&core, &core_migrations)
            .await
            .expect("core cleanup, then the tightened core");
    }
    let without_cleanup: Vec<_> = mem_migrations
        .iter()
        .copied()
        .filter(|m| !m.tql.starts_with("match $r isa population-member;"))
        .collect();
    let err = db
        .apply_schema_with_migrations(&mem, &without_cleanup)
        .await
        .expect_err("a one-player population-member fails the required roles");
    assert!(err.to_string().contains("'population-member'"), "{err}");
    for _ in 0..2 {
        db.apply_schema_with_migrations(&mem, &mem_migrations)
            .await
            .expect("mem cleanup and redefine, then the tightened mem");
    }

    // The shipped cleanups for relations this probe leaves untightened stay
    // pending; every other migration has run.
    let after = db.database_schema().await.expect("schema");
    let untightened = |m: &gecko_extension_api::SchemaMigration| {
        (m.id.starts_with("mem/") || m.id.starts_with("core/"))
            && m.tql.starts_with("match ")
            && !m.tql.starts_with("match $r isa population-member;")
            && !m.tql.starts_with("match $r isa containment;")
    };
    for m in core_migrations
        .iter()
        .chain(&mem_migrations)
        .filter(|m| !untightened(m))
    {
        assert!(!after.contains(m.applies_when), "{} still pending", m.id);
    }
    assert!(
        after.contains(
            "relates container @card(1..1) @doc(\"The bundle that contains the members.\")"
        )
    );
    assert!(after.contains(
        "relates member @card(1..1) @doc(\"A concept the population's spec currently matches.\")"
    ));
    assert_eq!(after.matches("owns valid-from @card(1..1)").count(), 1);
    assert_eq!(after.matches("owns valid-from @card(0..1)").count(), 1);
    assert_eq!(count_rows(&mut db, "match $r isa containment;").await, 1);
    assert_eq!(
        count_rows(&mut db, "match $r isa population-member;").await,
        1
    );
}

#[cfg(feature = "cyber")]
#[tokio::test]
async fn tightening_forms_apply_to_the_pre_tightening_cyber() {
    let pre_cyber = pre_cyber_schema();
    let cyber = tightened(
        &pre_cyber,
        &[
            ("relates user @doc(", "relates user @card(1) @doc("),
            ("relates used @doc(", "relates used @card(1) @doc("),
            (
                "relates detecting-logic @doc(",
                "relates detecting-logic @card(1) @doc(",
            ),
            (
                "relates detected-technique @doc(",
                "relates detected-technique @card(1) @doc(",
            ),
            (
                "relates detection-claim @doc(",
                "relates detection-claim @card(1) @doc(",
            ),
        ],
    );
    // The shipped cleanups are left out: the probe tightens only `uses` and
    // `technique-detection`, with cleanups of its own.
    let shipped: Vec<_> = cyber_gecko::migrations::CYBER_MIGRATIONS
        .iter()
        .filter(|m| !m.id.ends_with("-required-roles"))
        .cloned()
        .collect();
    let mut cyber_migrations = shipped.clone();
    cyber_migrations.extend([
        gecko_extension_api::SchemaMigration {
            id: "probe/uses-one-player",
            applies_when: "relates used @doc(\"The technique, malware, or tool being used.\")",
            tql: "match $r isa uses; { not { $r links (user: $u); }; } or { not { $r links (used: $d); }; }; delete $r;",
        },
        gecko_extension_api::SchemaMigration {
            id: "probe/technique-detection-one-player",
            applies_when: "relates detecting-logic @doc(\"The concept holding the detection logic.\")",
            tql: "match $r isa technique-detection; { not { $r links (detecting-logic: $l); }; } or { not { $r links (detected-technique: $t); }; } or { not { $r links (detection-claim: $c); }; }; delete $r;",
        },
    ]);

    // The registry accepts the cleanups through cyber's migrations, one of them on a
    // belief edge cyber owns.
    let exts: Vec<Box<dyn GeckoExtension>> = vec![
        Box::new(ProbeExtension {
            name: "mem-gecko",
            schema: tightened_mem(),
            migrations: tightened_mem_migrations(),
            belief_edges: &[],
        }),
        Box::new(ProbeExtension {
            name: "cyber-gecko",
            schema: cyber.clone(),
            migrations: cyber_migrations.clone(),
            belief_edges: &cyber_gecko::CYBER_BELIEF_EDGES,
        }),
    ];
    mem_gecko::BeliefEdgeRegistry::from_extensions(PRE_CORE, &exts)
        .expect("the registry accepts cyber's tightening migrations");

    let test_db = TestDb::new("gecko_test_tightening_cyber");
    let mut db = test_db.router();
    db.apply_schema(PRE_CORE)
        .await
        .expect("pre-tightening core");
    db.apply_schema(&pre_mem_schema())
        .await
        .expect("pre-tightening mem");
    db.apply_schema(&pre_cyber)
        .await
        .expect("pre-tightening cyber");
    write_rows(
        &mut db,
        "insert $g isa intrusion-set, has concept-id 'probe-g'; \
         $t isa attack-pattern, has concept-id 'probe-t'; \
         uses (user: $g, used: $t); \
         uses (used: $t);",
    )
    .await;
    let before = db.database_schema().await.expect("schema");
    for m in cyber_migrations
        .iter()
        .filter(|m| m.id.starts_with("probe/"))
    {
        assert_eq!(before.matches(m.applies_when).count(), 1, "{}", m.id);
    }

    let err = db
        .apply_schema_with_migrations(&cyber, &shipped)
        .await
        .expect_err("a one-player uses fails the required roles");
    assert!(err.to_string().contains("'uses'"), "{err}");
    for _ in 0..2 {
        db.apply_schema_with_migrations(&cyber, &cyber_migrations)
            .await
            .expect("cyber cleanups, then the tightened cyber");
    }
    let after = db.database_schema().await.expect("schema");
    for m in &cyber_migrations {
        assert!(!after.contains(m.applies_when), "{} still pending", m.id);
    }
    assert!(after.contains(
        "relates used @card(1..1) @doc(\"The technique, malware, or tool being used.\")"
    ));
    assert_eq!(count_rows(&mut db, "match $r isa uses;").await, 1);
}

#[tokio::test]
async fn a_database_holding_a_one_player_core_relation_upgrades() {
    use gecko_engine::db::{CORE_MIGRATIONS, CORE_SCHEMA};

    let test_db = TestDb::new("gecko_test_core_one_player_upgrade");
    let mut db = test_db.router();
    db.apply_schema(PRE_CORE)
        .await
        .expect("pre-tightening core");
    write_rows(
        &mut db,
        "insert $b isa bundle, has bundle-name 'up-b'; \
         $c isa concept, has concept-id 'up/c'; \
         $d isa concept, has concept-id 'up/d'; \
         $e isa external-resource, has target-url 'https://example.com/up'; \
         containment (container: $b, member: $c); \
         containment (container: $b); \
         containment (member: $d); \
         hierarchy (parent: $c, child: $d); \
         hierarchy (parent: $c); \
         hierarchy (child: $d); \
         concept-link (source: $c, target: $d); \
         concept-link (source: $c); \
         concept-link (target: $d); \
         citation (citing-concept: $c, cited-resource: $e); \
         citation (citing-concept: $c); \
         citation (cited-resource: $e);",
    )
    .await;

    // Each cleanup's trigger is in the pre-tightening export exactly once.
    let before = db.database_schema().await.expect("schema");
    let cleanups: Vec<_> = CORE_MIGRATIONS
        .iter()
        .filter(|m| m.tql.starts_with("match "))
        .collect();
    assert_eq!(cleanups.len(), 4, "one cleanup per core relation");
    for m in &cleanups {
        assert_eq!(before.matches(m.applies_when).count(), 1, "{}", m.id);
    }

    for _ in 0..2 {
        db.apply_schema_with_migrations(CORE_SCHEMA, CORE_MIGRATIONS)
            .await
            .expect("core cleanups, then the shipped core");
    }

    let after = db.database_schema().await.expect("schema");
    for m in CORE_MIGRATIONS {
        assert!(!after.contains(m.applies_when), "{} still pending", m.id);
    }
    // `citation` carries its own cards on the roles it specializes, beside the
    // ones it inherits through them from `okf-link`.
    for role in [
        "relates source @card(1..1)",
        "relates target @card(1..1)",
        "relates citing-concept as source @card(1..1)",
        "relates cited-resource as target @card(1..1)",
    ] {
        assert!(after.contains(role), "the export lacks {role:?}");
    }
    for (relation, full) in [
        ("containment", "links (container: $x, member: $y)"),
        ("hierarchy", "links (parent: $x, child: $y)"),
        ("concept-link", "links (source: $x, target: $y)"),
        ("citation", "links (citing-concept: $x, cited-resource: $y)"),
    ] {
        assert_eq!(
            count_rows(&mut db, &format!("match $r isa {relation};")).await,
            1,
            "only the fully played {relation} survives"
        );
        assert_eq!(
            count_rows(&mut db, &format!("match $r isa {relation}, {full};")).await,
            1,
            "the surviving {relation} is the fully played one"
        );
    }
}

/// Whether a shipped migration is one the schema tightening adds: a one-player
/// cleanup ahead of a required role, belief's required fields, or the similarity
/// clamp ahead of its range.
#[cfg(feature = "cyber")]
fn added_by_the_tightening(id: &str) -> bool {
    id.ends_with("-required-roles")
        || matches!(
            id,
            "mem/belief-required-window-doc"
                | "mem/belief-valid-from-required"
                | "mem/similarity-score-clamp-high"
                | "mem/similarity-score-clamp-low"
        )
}

/// Runs `writes` in one write transaction and returns the error that refuses them,
/// at the query or at the commit.
#[cfg(feature = "cyber")]
async fn refusal(
    db: &mut gecko_engine::db::router::TypeDbRouter,
    writes: &[gecko_extension_api::GraphWrite],
) -> String {
    let tx = db.begin_write().await.expect("write tx");
    if let Err(e) = gecko_engine::db::run_graph_writes(&tx, writes).await {
        return e.to_string();
    }
    tx.commit()
        .await
        .expect_err("the write commits over the tightened schema")
        .to_string()
}

/// The ATT&CK fixture with a malware that uses its technique and an indicator that
/// indicates the malware.
#[cfg(feature = "cyber")]
fn attack_with_malware() -> String {
    const MALWARE: &str = "malware--b0000000-0000-4000-8000-000000000001";
    const INDICATOR: &str = "indicator--c0000000-0000-4000-8000-000000000001";
    let mut bundle: serde_json::Value =
        serde_json::from_str(&attack::fixture_json()).expect("the fixture is JSON");
    let objects = bundle["objects"].as_array_mut().expect("objects");
    objects.extend([
        serde_json::json!({ "type": "malware", "id": MALWARE, "name": "ExampleRAT" }),
        serde_json::json!({
            "type": "indicator",
            "id": INDICATOR,
            "name": "Bad IP",
            "pattern": "[ipv4-addr:value = '1.1.1.1']",
        }),
        serde_json::json!({
            "type": "relationship",
            "id": "relationship--e0000000-0000-4000-8000-000000000002",
            "relationship_type": "uses",
            "source_ref": MALWARE,
            "target_ref": attack::TECHNIQUE_STIX_ID,
        }),
        serde_json::json!({
            "type": "relationship",
            "id": "relationship--e0000000-0000-4000-8000-000000000003",
            "relationship_type": "indicates",
            "source_ref": INDICATOR,
            "target_ref": MALWARE,
        }),
    ]);
    bundle.to_string()
}

/// The schema parts an upgraded pre-tightening database keeps that a fresh database
/// does not have. Each is inert: nothing shipped references it. A `fun <name>` entry
/// names a function whose text is the pre-tightening fixture's own; any other entry
/// is a `schema_parts` part verbatim.
#[cfg(feature = "cyber")]
const CUT_LEFTOVERS: &[&str] = &[
    "fun blast-radius-at",
    "fun population-members",
    "fun population-members-at",
    "attribute computed-at | value datetime",
    "attribute spec-dialect | value string",
    "attribute spec-hash | value string",
    "attribute spec-text | value string",
    "entity belief | plays scoped:scoped-belief",
    "entity concept | plays population-member:member",
    "entity population @doc(\"A named cohort defined by a stored TypeDB match predicate (spec-text); the source-of-truth for a set of concepts, deduplicated by spec-hash.\") | owns spec-dialect @card(0..1)",
    "entity population @doc(\"A named cohort defined by a stored TypeDB match predicate (spec-text); the source-of-truth for a set of concepts, deduplicated by spec-hash.\") | owns spec-text @card(0..1)",
    "entity population @doc(\"A named cohort defined by a stored TypeDB match predicate (spec-text); the source-of-truth for a set of concepts, deduplicated by spec-hash.\") | owns spec-hash @key",
    "entity population @doc(\"A named cohort defined by a stored TypeDB match predicate (spec-text); the source-of-truth for a set of concepts, deduplicated by spec-hash.\") | plays scoped:population",
    "entity population @doc(\"A named cohort defined by a stored TypeDB match predicate (spec-text); the source-of-truth for a set of concepts, deduplicated by spec-hash.\") | plays population-member:owning-population",
    "relation scoped @doc(\"Scopes a belief to the population it is about.\") | relates scoped-belief @doc(\"The belief scoped to the population.\")",
    "relation scoped @doc(\"Scopes a belief to the population it is about.\") | relates population @doc(\"The population the belief concerns.\")",
    "relation population-member @doc(\"Materialized cache of a population's membership: the concepts the spec currently evaluates to, over the window it held and with a staleness marker.\") | relates owning-population @doc(\"The population the membership belongs to.\")",
    "relation population-member @doc(\"Materialized cache of a population's membership: the concepts the spec currently evaluates to, over the window it held and with a staleness marker.\") | relates member @doc(\"A concept the population's spec currently matches.\")",
    "relation population-member @doc(\"Materialized cache of a population's membership: the concepts the spec currently evaluates to, over the window it held and with a staleness marker.\") | owns valid-from @card(0..1)",
    "relation population-member @doc(\"Materialized cache of a population's membership: the concepts the spec currently evaluates to, over the window it held and with a staleness marker.\") | owns valid-to @card(0..1)",
    "relation population-member @doc(\"Materialized cache of a population's membership: the concepts the spec currently evaluates to, over the window it held and with a staleness marker.\") | owns computed-at @card(0..1)",
];

/// [`CUT_LEFTOVERS`] as `schema_parts` parts, each function read from the
/// pre-tightening fixture.
#[cfg(feature = "cyber")]
fn cut_leftover_parts() -> std::collections::BTreeSet<String> {
    CUT_LEFTOVERS
        .iter()
        .map(|entry| match entry.strip_prefix("fun ") {
            Some(name) => {
                let start = PRE_MEM_FUNCTIONS
                    .find(&format!("  fun {name}("))
                    .unwrap_or_else(|| panic!("the fixture declares no function {name}"));
                let text = &PRE_MEM_FUNCTIONS[start..];
                text[..text.find("\n\n").unwrap_or(text.len())]
                    .trim()
                    .to_string()
            }
            None => entry.to_string(),
        })
        .collect()
}

/// A database schema export as the set of its `(type head, capability)` pairs and
/// the set of its function texts. An upgrade appends a newly defined type, capability
/// or function after the existing ones, where a fresh database lists it in definition
/// order; that order carries no meaning. A type with no capability contributes its
/// head alone.
#[cfg(feature = "cyber")]
fn schema_parts(schema: &str) -> std::collections::BTreeSet<String> {
    let mut chunks = schema.split("\nfun ");
    let types_text = chunks.next().unwrap_or_default();
    let mut parts = std::collections::BTreeSet::new();
    for decl in types_text.trim_start_matches("define").split(";\n") {
        let decl = decl.trim().trim_end_matches(';');
        let mut lines = decl.split(",\n").map(str::trim);
        let head = lines.next().unwrap_or_default();
        let mut capabilities = lines.peekable();
        if capabilities.peek().is_none() {
            parts.insert(head.to_string());
        }
        for capability in capabilities {
            parts.insert(format!("{head} | {capability}"));
        }
    }
    parts.extend(chunks.map(|f| format!("fun {}", f.trim_end())));
    parts
}

/// A database on the pre-tightening schemas, populated by every current writer path
/// plus the one-player rows an earlier release's GC could leave, takes the shipped schemas.
#[cfg(feature = "cyber")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tightened_schema_applies_to_a_populated_database() {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use gecko_engine::db::{CORE_MIGRATIONS, CORE_SCHEMA, RouterGraphStore};
    use gecko_engine::okf::parser::parse_bundle;
    use gecko_engine::syncer::bundle::sync_bundle;
    use gecko_extension_api::{
        ActorId, BeliefDraft, BeliefEdge, ConceptId, ContextBudget, DerivationMethod, Embedder,
        EpisodeDraft, EpistemicWriter, GraphValue, GraphWrite, Outcome, ProvenanceSource,
        RecallQuery, RunContext, Visibility,
    };
    use gecko_semantic_index::{HnswIndex, StubEmbedder};
    use mem_gecko::{
        BELIEF_TIER_ROOT, BeliefEdgeRegistry, ConsolidationDaemon, MemConsolidationDaemon,
        MemWriter,
    };

    const LOGIC: &str = "notes/a";
    let test_db = TestDb::new("gecko_test_tightened_populated");
    let mut db = test_db.router();
    db.apply_schema(PRE_CORE)
        .await
        .expect("pre-tightening core");
    db.apply_schema(&pre_mem_schema())
        .await
        .expect("pre-tightening mem");
    db.apply_schema(&pre_cyber_schema())
        .await
        .expect("pre-tightening cyber");
    // The ATT&CK post-sync sweep reads every typed relation, and the adapter
    // writes retired-attack-id; the types the pre-tightening release never
    // declared are defined as shipped.
    db.apply_schema(NEW_CYBER_RELATIONS)
        .await
        .expect("the relations declared with required roles");

    // An OKF bundle with a concept link, a citation and a hierarchy.
    let okf = tempfile::TempDir::new().expect("temp dir");
    std::fs::create_dir_all(okf.path().join("notes/deep")).expect("okf dirs");
    std::fs::write(
        okf.path().join("bundle.json"),
        serde_json::json!({ "name": "upgrade-notes" }).to_string(),
    )
    .expect("bundle.json");
    for (path, body) in [
        (
            "notes/a.md",
            "See [b](b.md) and [x](https://example.com/upgrade).",
        ),
        ("notes/b.md", "Linked."),
        ("notes/deep/c.md", "Nested."),
    ] {
        std::fs::write(
            okf.path().join(path),
            format!("---\ntype: Note\ntitle: {path}\n---\n\n{body}\n"),
        )
        .expect("write a concept");
    }
    let notes = parse_bundle(okf.path()).expect("parse the OKF bundle");
    let r = sync_bundle(&mut db, &notes, &[], BELIEF_TIER_ROOT)
        .await
        .expect("sync the OKF bundle");
    assert!(
        r.links_attempted >= 1 && r.citations_attempted >= 1,
        "{r:?}"
    );

    // ATT&CK with its post-sync writes, then a sensor file.
    let (attack_okf, rels) = cyber_gecko::stix::to_okf(&attack_with_malware()).expect("converts");
    sync_bundle(
        &mut db,
        &attack_okf,
        &cyber_gecko::stix::cyber_post_sync(&rels),
        BELIEF_TIER_ROOT,
    )
    .await
    .expect("sync ATT&CK");
    let sensors = cyber_gecko::sensors::parse(
        "platforms: [Windows]\nsensors:\n  - name: windows-edr\n    platforms: [Windows]\n    \
         log_sources: [WinEventLog:Sysmon]\n    components: [Process Creation]\n",
    )
    .expect("the sensor file parses");
    sync_bundle(
        &mut db,
        &cyber_gecko::sensors::to_okf(&sensors, cyber_gecko::sensors::DEFAULT_BUNDLE_NAME),
        &[],
        BELIEF_TIER_ROOT,
    )
    .await
    .expect("sync the sensor file");

    // A GC by an earlier release deleted a concept's containment and then the
    // entity, so the malware's `uses` and `indicates` are left with one player each.
    {
        let tx = db.begin_write().await.expect("write tx");
        tx.query("match $m isa malware; $r isa containment, links (member: $m); delete $r;")
            .await
            .expect("delete the malware's containment");
        tx.query("match $m isa malware; delete $m;")
            .await
            .expect("delete the malware");
        tx.commit().await.expect("commit the earlier release's GC");
    }
    assert_eq!(
        count_rows(&mut db, "match $r isa uses; not { $r links (user: $u); };").await,
        1,
        "the earlier release's GC leaves a one-player uses"
    );

    // Every MemWriter write path the reconciliation rig drives.
    let store = Arc::new(RouterGraphStore::new(Arc::new(tokio::sync::Mutex::new(
        test_db.router(),
    ))));
    let embedder = Arc::new(StubEmbedder::new(384));
    let index_dir = tempfile::TempDir::new().expect("index dir");
    let index = HnswIndex::open(
        index_dir.path().join("upgrade.hnsw"),
        embedder.model_id(),
        384,
    )
    .expect("open the index");
    let registry = BeliefEdgeRegistry::from_extensions(
        CORE_SCHEMA,
        &[mem_gecko::extension(), cyber_gecko::extension()],
    )
    .expect("belief-edge registry");
    let writer = Arc::new(
        MemWriter::new(store, Some(embedder), Some(Arc::new(index)))
            .with_retrieval_provenance(true)
            .with_belief_edges(registry),
    );
    let ctx = RunContext::new(
        ActorId::new("system"),
        ProvenanceSource::ExecutableDoc {
            concept_id: ConceptId::new(LOGIC),
        },
        chrono::Utc::now(),
    );
    let draft = |text: &str, confidence: Option<f64>| BeliefDraft {
        text: text.to_string(),
        owner: ctx.actor.clone(),
        visibility: Visibility::Private,
        confidence,
        entrenchment: None,
    };
    let episode = || EpisodeDraft {
        text: "smb lateral movement observed".to_string(),
        event_time: ctx.occurred_at,
        ingest_time: ctx.occurred_at,
    };
    writer.observe(&ctx, episode()).await.expect("observe");
    let asserted = writer
        .assert_belief(
            &ctx,
            draft("host is not compromised", None),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .expect("assert");
    let chunks = writer
        .clone()
        .as_epistemic_reader()
        .expect("mem writer is a reader")
        .recall(
            &ctx,
            RecallQuery {
                text: "host is not compromised".to_string(),
                as_of: None,
            },
            ContextBudget {
                max_chunks: 5,
                max_tokens: None,
            },
        )
        .await
        .expect("recall");
    assert!(!chunks.is_empty(), "recall surfaced nothing");
    let recalled: Vec<_> = chunks.iter().map(|c| c.id.clone()).collect();
    let derived = writer
        .assert_belief(
            &ctx,
            draft("host is compromised via smb", Some(0.6)),
            &recalled,
            DerivationMethod::LlmSynthesis,
        )
        .await
        .expect("derive from the recall");
    let superseding = writer
        .supersede(
            &ctx,
            derived,
            draft("host is compromised via smb and rdp", None),
            "revised",
        )
        .await
        .expect("supersede");
    writer
        .contest(&ctx, &[superseding.clone(), asserted.clone()])
        .await
        .expect("contest");
    writer
        .rests_on(&ctx, asserted.clone(), std::slice::from_ref(&superseding))
        .await
        .expect("rests_on");
    writer
        .record_prediction(&ctx, asserted.clone(), 0.7)
        .await
        .expect("record_prediction");
    writer
        .resolve_prediction(&ctx, asserted.clone(), Outcome::Confirmed)
        .await
        .expect("resolve_prediction");
    let pair = [
        writer.observe(&ctx, episode()).await.expect("observe"),
        writer.observe(&ctx, episode()).await.expect("observe"),
    ];
    let (keeper, loser) = if pair[0].0 < pair[1].0 {
        (&pair[0], &pair[1])
    } else {
        (&pair[1], &pair[0])
    };
    writer
        .dedup_episodes(&ctx, keeper, loser)
        .await
        .expect("dedup_episodes");
    writer.observe(&ctx, episode()).await.expect("observe");
    let report = MemConsolidationDaemon::new(writer.clone())
        .tick(chrono::Utc::now())
        .await
        .expect("daemon tick");
    assert!(report.episodes_deduped >= 1, "{report:?}");

    // cyber-gecko's two belief-edge writes.
    let edges = writer
        .edge_writer_for("cyber-gecko")
        .expect("cyber-gecko edge writer");
    edges
        .assert_belief_with_edges(
            &ctx,
            draft("notes/a detects T1059", Some(0.9)),
            &[],
            DerivationMethod::ExternalTool,
            &[BeliefEdge {
                relation: "technique-detection".into(),
                players: vec![
                    ("detecting-logic".into(), GraphValue::from(LOGIC)),
                    (
                        "detected-technique".into(),
                        GraphValue::from(attack::TECHNIQUE_ATTACK_ID),
                    ),
                ],
                attrs: vec![],
            }],
        )
        .await
        .expect("claim");
    edges
        .assert_belief_with_edges(
            &ctx,
            draft("disposition of notes/a: true-positive", None),
            &[],
            DerivationMethod::HumanAssertion,
            &[BeliefEdge {
                relation: "alert-disposition".into(),
                players: vec![("dispositioned-logic".into(), GraphValue::from(LOGIC))],
                attrs: vec![(
                    "detection-verdict".into(),
                    GraphValue::from("true-positive"),
                )],
            }],
        )
        .await
        .expect("disposition");

    // A one-player concept link a database synced by an earlier release can hold,
    // which today's GC deletes with its concept.
    let seed = [GraphWrite::single(
        "given $id: string; \
         match $c isa concept, has concept-id $cid; $cid == $id; \
         insert concept-link (source: $c);",
        vec!["id".into()],
        vec![GraphValue::from(LOGIC)],
    )
    .expecting(1)];
    {
        let tx = db.begin_write().await.expect("write tx");
        gecko_engine::db::run_graph_writes(&tx, &seed)
            .await
            .expect("seed one-player rows");
        tx.commit().await.expect("commit the one-player rows");
    }

    // Every trigger the tightening adds is in the populated database's export once.
    let migrations: Vec<(&str, &[gecko_extension_api::SchemaMigration])> = vec![
        ("core", CORE_MIGRATIONS),
        ("mem", mem_gecko::MEM_MIGRATIONS),
        ("cyber", &cyber_gecko::migrations::CYBER_MIGRATIONS),
    ];
    let before = db.database_schema().await.expect("schema");
    let mut added = BTreeMap::new();
    for (owner, list) in &migrations {
        for m in list.iter().filter(|m| added_by_the_tightening(m.id)) {
            assert_eq!(
                before.matches(m.applies_when).count(),
                1,
                "{}: the trigger is not in the pre-tightening export once",
                m.id
            );
            *added.entry(*owner).or_insert(0) += 1;
        }
    }
    assert_eq!(
        added,
        BTreeMap::from([("core", 4), ("mem", 20), ("cyber", 9)]),
        "the triggers checked"
    );

    // The shipped schemas with their migrations, as `schema-init` applies them.
    let extensions = [mem_gecko::extension(), cyber_gecko::extension()];
    BeliefEdgeRegistry::from_extensions(CORE_SCHEMA, &extensions)
        .expect("the registry accepts the shipped schemas");
    for _ in 0..2 {
        db.apply_schema_with_migrations(CORE_SCHEMA, CORE_MIGRATIONS)
            .await
            .expect("the shipped core applies to the populated database");
        for ext in &extensions {
            db.apply_schema_with_migrations(ext.schema(), ext.schema_migrations())
                .await
                .unwrap_or_else(|e| panic!("the shipped {} applies: {e}", ext.name()));
        }
    }
    let after = db.database_schema().await.expect("schema");
    for (_, list) in &migrations {
        for m in list.iter() {
            assert!(!after.contains(m.applies_when), "{} still pending", m.id);
        }
    }

    // The upgraded schema is the one a fresh database gets.
    let fresh_db = TestDb::new("gecko_test_tightened_fresh");
    attack::sync_fixture(&fresh_db).await;
    let fresh = fresh_db.router().database_schema().await.expect("schema");
    let (after_parts, fresh_parts) = (schema_parts(&after), schema_parts(&fresh));
    let missing: Vec<&String> = fresh_parts.difference(&after_parts).collect();
    assert!(
        missing.is_empty(),
        "the upgraded schema lacks parts a fresh one has: {missing:#?}"
    );
    let leftovers: std::collections::BTreeSet<String> =
        after_parts.difference(&fresh_parts).cloned().collect();
    assert_eq!(
        leftovers,
        cut_leftover_parts(),
        "the upgraded schema keeps exactly the named leftovers beyond a fresh one"
    );

    // core's required roles: the one-player link is gone, the synced link stays.
    assert_eq!(
        count_rows(&mut db, "match $r isa concept-link;").await,
        1,
        "only the synced concept link survives"
    );
    assert_eq!(
        count_rows(
            &mut db,
            "match $r isa concept-link, links (source: $s, target: $t);"
        )
        .await,
        1
    );
    let err = refusal(&mut db, &[seed[0].clone()]).await;
    assert!(err.contains("concept-link"), "{err}");

    // cyber's required roles: the one-player SROs are gone, and another is refused.
    for (relation, full) in [
        ("uses", "links (user: $x, used: $y)"),
        ("indicates", "links (indicator-source: $x, indicated: $y)"),
    ] {
        assert_eq!(
            count_rows(&mut db, &format!("match $r isa {relation};")).await,
            count_rows(&mut db, &format!("match $r isa {relation}, {full};")).await,
            "a one-player {relation} survived"
        );
    }
    let err = refusal(
        &mut db,
        &[GraphWrite::single(
            "given $id: string; \
             match $t isa attack-pattern, has attack-id $a; $a == $id; \
             insert uses (used: $t);",
            vec!["id".into()],
            vec![GraphValue::from(attack::TECHNIQUE_ATTACK_ID)],
        )],
    )
    .await;
    assert!(err.contains("uses"), "{err}");

    // belief's required fields and the score range hold on every written row.
    assert!(count_rows(&mut db, "match $b isa belief;").await >= 5);
    for attr in ["valid-from", "salience", "base-activation", "last-access"] {
        assert_eq!(
            count_rows(
                &mut db,
                &format!("match $b isa belief; not {{ $b has {attr} $v; }};")
            )
            .await,
            0,
            "a belief lacks {attr}"
        );
    }
    assert!(count_rows(&mut db, "match $r isa surfaced, has similarity-score $s;").await >= 1);
    let err = refusal(
        &mut db,
        &[GraphWrite::single(
            "given $id: string; \
             match $b isa belief, has concept-id $cid, has valid-from $v; $cid == $id; \
             delete has $v of $b;",
            vec!["id".into()],
            vec![GraphValue::from(asserted.0.as_str())],
        )
        .expecting(1)],
    )
    .await;
    assert!(err.contains("valid-from"), "{err}");
}
