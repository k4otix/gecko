// Verifies core + mem schema initialization on every push, including the
// candle-free Tier-2 command (`cargo test --test '*' --no-default-features`).
// Only the `cyber-entity` assertion is gated behind the `cyber` feature, since
// that type only exists when the `cyber` extension is compiled in.
use futures_util::StreamExt;

#[cfg(feature = "cyber")]
use cyber_gecko::CyberGecko;
use gecko_engine::extension::GeckoExtension;
use mem_gecko::MemGecko;

mod common;
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

    let test_db = TestDb::new("gecko_test_cyber_migration");
    let mut db = test_db.router();
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
         (evidence-component: $c, evidence-source: $s) isa evidence-of; \
         (technique: $t, required: $c) isa requires-evidence; \
         (covered-source: $s, covered-component: $c, coverage-scope: $p) isa sensor-coverage;",
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

/// The schemas as released before the schema tightening.
const PRE_CORE: &str = include_str!("fixtures/schemas_pre_tightening/core_schema.tql");
const PRE_MEM_TYPES: &str = include_str!("fixtures/schemas_pre_tightening/mem_types.tql");
const PRE_MEM_FUNCTIONS: &str = include_str!("fixtures/schemas_pre_tightening/mem_functions.tql");

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
/// `redefine` of `belief`'s explicit valid-from card.
fn tightened_mem_migrations() -> Vec<gecko_extension_api::SchemaMigration> {
    let mut migrations = mem_gecko::MEM_MIGRATIONS.to_vec();
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
         (container: $b, member: $c) isa containment; \
         (container: $b) isa containment; \
         (owning-population: $p, member: $c) isa population-member; \
         (owning-population: $p) isa population-member;",
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
    let err = db
        .apply_schema_with_migrations(&core, CORE_MIGRATIONS)
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

    // mem's shipped cleanups for relations this probe leaves untightened stay
    // pending; every other migration has run.
    let after = db.database_schema().await.expect("schema");
    let untightened = |m: &gecko_extension_api::SchemaMigration| {
        m.id.starts_with("mem/")
            && m.tql.starts_with("match ")
            && !m.tql.starts_with("match $r isa population-member;")
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
    const PRE_CYBER: &str = include_str!("fixtures/schemas_pre_tightening/cyber_schema.tql");
    const PRE_CYBER_FUNCTIONS: &str =
        include_str!("fixtures/schemas_pre_tightening/cyber_functions.tql");

    let pre_cyber = format!("{PRE_CYBER}\n{PRE_CYBER_FUNCTIONS}");
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
    let mut cyber_migrations = cyber_gecko::migrations::CYBER_MIGRATIONS.to_vec();
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
         (user: $g, used: $t) isa uses; \
         (used: $t) isa uses;",
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
        .apply_schema_with_migrations(&cyber, &cyber_gecko::migrations::CYBER_MIGRATIONS)
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
