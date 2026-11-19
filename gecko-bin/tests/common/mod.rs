//! Shared helpers for gecko-bin integration tests.
//!
//! The central piece is [`TestDb`], a uniquely-named database that deletes itself
//! on drop — even if the test panics — so repeated test runs never leave
//! databases behind on the server.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};

/// A uniquely-named test database whose `Drop` deletes it from the server.
///
/// Declare it **before** any `TypeDbRouter` used in the test so it is dropped
/// *last* (after those routers close their connections).
pub struct TestDb {
    pub config: DbConfig,
    pub name: String,
}

impl TestDb {
    /// Creates a config for a fresh, uniquely-named database. The name combines a
    /// nanosecond timestamp, the process id, and a process-wide monotonic counter.
    /// The timestamp is in nanosecond *units*, not nanosecond *resolution*, so it
    /// cannot alone guarantee uniqueness: the counter separates two `new` calls in
    /// one clock tick within a process (threaded `cargo test`), and the pid
    /// separates two tests that sample the same tick in different processes (CI's
    /// nextest runs one process per test, where the counter resets to zero).
    pub fn new(prefix: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let pid = std::process::id();
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let name = format!("{prefix}_{ts}_{pid}_{seq}");
        let config = DbConfig {
            address: "localhost:1729".to_string(),
            database: name.clone(),
            username: "admin".to_string(),
            password: "password".to_string(),
            tls: TlsMode::Disabled,
        };
        Self { config, name }
    }

    /// A router pointed at this test database.
    pub fn router(&self) -> TypeDbRouter {
        TypeDbRouter::new(self.config.clone())
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let config = self.config.clone();
        let name = self.name.clone();
        // Run the deletion on a fresh runtime in a separate thread: this Drop may
        // fire inside a `#[tokio::test]` runtime, where `block_on` would panic
        // with "cannot start a runtime from within a runtime".
        let _ = std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Runtime::new() {
                rt.block_on(async move {
                    let mut db = TypeDbRouter::new(config);
                    let _ = db.delete_database(&name).await;
                });
            }
        })
        .join();
    }
}
