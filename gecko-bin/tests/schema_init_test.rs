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
            .query("match $t sub cyber-entity;")
            .await
            .expect("Failed to query cyber type");
        let cyber_rows: Vec<_> = answer.into_rows().collect::<Vec<_>>().await;
        assert!(
            !cyber_rows.is_empty(),
            "Cyber schema 'cyber-entity' type not found"
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
