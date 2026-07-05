use futures_util::StreamExt;
use std::time::{SystemTime, UNIX_EPOCH};

use cyber_gecko::CyberGecko;
use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_engine::extension::GeckoExtension;
use mem_gecko::MemGecko;

/// Generates a unique database name to avoid collisions in concurrent tests
fn unique_db_name() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("gecko_test_{}", ts)
}

#[tokio::test]
async fn test_schema_initialization() {
    let db_name = unique_db_name();
    let config = DbConfig {
        address: "localhost:1729".to_string(),
        database: db_name.clone(),
        username: "admin".to_string(),
        password: "password".to_string(),
        tls: TlsMode::Disabled,
    };

    // 1. Initialize router and create DB
    let mut db = TypeDbRouter::new(config.clone());

    // 2. Apply core schema
    let core_schema = include_str!("../../core/gecko-engine/schema/core_schema.tql");
    db.apply_schema(core_schema)
        .await
        .expect("Failed to apply core schema");

    // 3. Assemble extensions
    let extensions: Vec<Box<dyn GeckoExtension>> =
        vec![Box::new(CyberGecko::new()), Box::new(MemGecko::new())];

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

    // Check for a cyber extension type
    let answer = tx
        .query("match $t sub cyber-entity;")
        .await
        .expect("Failed to query cyber type");
    let cyber_rows: Vec<_> = answer.into_rows().collect::<Vec<_>>().await;
    assert!(
        !cyber_rows.is_empty(),
        "Cyber schema 'cyber-entity' type not found"
    );

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
