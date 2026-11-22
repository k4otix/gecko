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

/// An extension carrying the cyber schema as of the frozen fixture and the
/// detection-model migrations, as the belief-edge registry reads an extension.
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
        &cyber_gecko::migrations::DETECTION_MODEL_MIGRATIONS
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
    let migrations = &cyber_gecko::migrations::DETECTION_MODEL_MIGRATIONS;

    // The registry accepts the migrations: none names a core or mem type.
    let exts: Vec<Box<dyn GeckoExtension>> = vec![
        Box::new(MemGecko::new()),
        Box::new(FrozenCyber { schema: FROZEN }),
    ];
    mem_gecko::BeliefEdgeRegistry::from_extensions(CORE_SCHEMA, &exts)
        .expect("the registry accepts the detection-model migrations");

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

    let tx = db.begin_write().await.expect("begin write");
    for q in ["insert $s isa data-source, has concept-id 'ds-1'; \
         $c isa data-component, has concept-id 'dc-1'; \
         $t isa attack-pattern, has concept-id 'tp-1'; \
         $k isa sensor, has concept-id 'sn-1'; \
         $p isa population, has spec-hash 'pop-1'; \
         (evidence-component: $c, evidence-source: $s) isa evidence-of; \
         (technique: $t, required: $c) isa requires-evidence; \
         (covering-sensor: $k, covered-source: $s, covered-component: $c, coverage-scope: $p) isa sensor-coverage;"]
    {
        tx.query(q).await.expect("seed instances");
    }
    tx.commit().await.expect("commit seed");

    // The migrations run ahead of the schema they accompany. The replacement
    // schema is a no-op, since the post-migration cyber schema is not yet written.
    let after_schema = "define attribute probe-noop value string;";
    for _ in 0..2 {
        db.apply_schema_with_migrations(after_schema, migrations)
            .await
            .expect("detection-model migrations apply");
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
}
