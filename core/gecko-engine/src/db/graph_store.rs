//! `RouterGraphStore`: the engine-side implementation of the neutral
//! [`GraphStore`] seam over [`TypeDbRouter`].
//!
//! This is the *other half* of mem-gecko's read/write firewall. mem builds
//! injection-safe query text + typed [`GraphValue`] params and never sees a driver
//! type; this adapter maps those onto the concrete `typedb-driver` `Value`/`given`
//! machinery, runs a whole write batch in one transaction (atomic), and streams
//! read documents back as JSON. `gecko-bin` constructs one of these and injects it
//! into `MemWriter`, so mem keeps its only non-dev dependency as
//! `gecko-extension-api`.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use gecko_extension_api::{EpistemicError, GraphStore, GraphValue, GraphWrite};
use tokio::sync::Mutex;
use typedb_driver::concept::Value;
use typedb_driver::given::{GivenRowEntry, GivenRows};

use crate::db::router::TypeDbRouter;

/// A [`GraphStore`] backed by a shared [`TypeDbRouter`].
///
/// The router is behind an async mutex because its transaction accessors take
/// `&mut self`; belief-tier writes are sparse (invariant 4) so serialising them
/// through one connection is not a bottleneck. This also means [`Self::read`]
/// is serialized behind the same mutex as every write — concurrent recalls queue
/// up one at a time on this router rather than running in parallel read
/// transactions. That is intentional here (recall volume is low) but is worth
/// knowing before reusing this adapter somewhere read-heavy.
pub struct RouterGraphStore {
    router: Arc<Mutex<TypeDbRouter>>,
}

impl RouterGraphStore {
    /// Wraps a shared router as a graph store.
    pub fn new(router: Arc<Mutex<TypeDbRouter>>) -> Self {
        Self { router }
    }
}

/// Maps a neutral [`GraphValue`] onto the driver's `given`-row value.
fn entry(v: &GraphValue) -> GivenRowEntry {
    GivenRowEntry::Value(match v {
        GraphValue::String(s) => Value::String(s.clone()),
        GraphValue::Datetime(dt) => Value::Datetime(dt.naive_utc()),
        GraphValue::Double(d) => Value::Double(*d),
        GraphValue::Long(i) => Value::Integer(*i),
        GraphValue::Boolean(b) => Value::Boolean(*b),
    })
}

fn store_err<E: std::fmt::Display>(e: E) -> EpistemicError {
    EpistemicError::Storage(e.to_string())
}

#[async_trait]
impl GraphStore for RouterGraphStore {
    async fn write(&self, ops: &[GraphWrite]) -> Result<(), EpistemicError> {
        let mut router = self.router.lock().await;
        let tx = router.begin_write().await.map_err(store_err)?;
        for op in ops {
            if op.rows.is_empty() {
                continue; // no-op, matching the syncer's `run_rows` convention.
            }
            if op.vars.is_empty() {
                // Parameterless write: run the query text as-is.
                tx.query(&op.query).await.map_err(store_err)?;
                continue;
            }
            let mut given = GivenRows::new(op.vars.clone(), op.rows.len());
            for row in &op.rows {
                let entries: Vec<GivenRowEntry> = row.iter().map(entry).collect();
                given.push_row(entries).map_err(store_err)?;
            }
            tx.query_with_rows(&op.query, given)
                .await
                .map_err(store_err)?;
        }
        tx.commit().await.map_err(store_err)?;
        Ok(())
    }

    async fn read(
        &self,
        query: &str,
        vars: &[String],
        row: &[GraphValue],
    ) -> Result<Vec<serde_json::Value>, EpistemicError> {
        let mut router = self.router.lock().await;
        let tx = router.begin_read().await.map_err(store_err)?;
        let answer = if vars.is_empty() {
            tx.query(query).await.map_err(store_err)?
        } else {
            let mut given = GivenRows::new(vars.to_vec(), 1);
            let entries: Vec<GivenRowEntry> = row.iter().map(entry).collect();
            given.push_row(entries).map_err(store_err)?;
            tx.query_with_rows(query, given).await.map_err(store_err)?
        };

        let mut out = Vec::new();
        if answer.is_document_stream() {
            let mut stream = answer.into_documents();
            while let Some(item) = stream.next().await {
                let doc = item.map_err(store_err)?;
                let json: serde_json::Value =
                    serde_json::from_str(&doc.into_json().to_string()).map_err(store_err)?;
                out.push(json);
            }
        }
        Ok(out)
    }
}
