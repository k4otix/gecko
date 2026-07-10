//! Shared helpers for mem-gecko integration tests.

use gecko_engine::db::router::TypeDbRouter;

/// Deletes a throwaway test database, retrying past TypeDB's asynchronous
/// transaction-close race.
///
/// A read/write transaction closes asynchronously when its handle drops; on a
/// networked server that close can lag an immediate delete, surfacing a transient
/// `[DBD2] ... in use`. This retries for up to ~3s, then gives up — database names
/// are unique per test, so a leak on persistent failure is harmless, and cleanup
/// never fails an otherwise-passing test.
pub async fn drop_db_with_retry(router: &mut TypeDbRouter, name: &str) {
    for attempt in 0..30 {
        match router.delete_database(name).await {
            Ok(()) => return,
            Err(e) if e.to_string().contains("in use") => {
                if attempt == 29 {
                    eprintln!("drop_db: '{name}' still in use after retries; leaving it");
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(e) => panic!("drop db: {e}"),
        }
    }
}
