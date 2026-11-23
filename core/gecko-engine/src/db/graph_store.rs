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
use gecko_extension_api::{
    EpistemicError, GraphReader, GraphStore, GraphValue, GraphWrite, RowCountMismatch,
    validate_read_rows,
};
use tokio::sync::{Mutex, OnceCell};
use typedb_driver::answer::QueryAnswer;
use typedb_driver::concept::Value;
use typedb_driver::given::{GivenRowEntry, GivenRows};

use crate::db::router::{GraphHandle, TypeDbRouter};

/// A [`GraphStore`] backed by a shared [`TypeDbRouter`].
///
/// Reads open independent transactions and run concurrently; writes are serialized
/// one-at-a-time through the router's writer gate and each commits atomically, so a
/// belief-tier write never overlaps another and never blocks a recall. The router
/// is locked exactly once, lazily, to establish the connection and cache a
/// [`GraphHandle`]; every query after that bypasses the lock entirely.
pub struct RouterGraphStore {
    router: Arc<Mutex<TypeDbRouter>>,
    handle: OnceCell<GraphHandle>,
}

impl RouterGraphStore {
    /// Wraps a shared router as a graph store.
    pub fn new(router: Arc<Mutex<TypeDbRouter>>) -> Self {
        Self {
            router,
            handle: OnceCell::new(),
        }
    }

    /// The cached connection handle, connecting and caching it on first use. A
    /// failed connect is not cached, so a later call retries.
    async fn handle(&self) -> Result<&GraphHandle, EpistemicError> {
        self.handle
            .get_or_try_init(|| async { self.router.lock().await.graph_handle().await })
            .await
            .map_err(store_err)
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

/// Drains the answer of write op `op` and fails unless it produced exactly
/// `expected` rows.
async fn check_row_count(
    answer: QueryAnswer,
    op: usize,
    expected: usize,
    query: &str,
) -> Result<(), EpistemicError> {
    let head: String = query.chars().take(80).collect();
    if !answer.is_row_stream() {
        return Err(EpistemicError::Storage(format!(
            "write op {op} answer is not a row stream: {head}"
        )));
    }
    let mut stream = answer.into_rows();
    let mut got = 0usize;
    while let Some(item) = stream.next().await {
        item.map_err(store_err)?;
        got += 1;
    }
    if got != expected {
        return Err(RowCountMismatch { op, expected, got }.into_error(&head));
    }
    Ok(())
}

/// Runs `ops` in order inside `tx`, which the caller owns and commits. Ops with no
/// rows are skipped; an op with no `vars` runs its query text as-is; otherwise its
/// rows are bound through a `given` stage. Any failure, including an
/// `expect_rows` mismatch, is returned so the caller can drop `tx` uncommitted
/// and roll back every op.
pub async fn run_graph_writes(
    tx: &typedb_driver::Transaction,
    ops: &[GraphWrite],
) -> Result<(), EpistemicError> {
    for (index, op) in ops.iter().enumerate() {
        if op.rows.is_empty() {
            continue; // no-op, matching the syncer's `run_rows` convention.
        }
        let answer = if op.vars.is_empty() {
            // Parameterless write: run the query text as-is.
            tx.query(&op.query).await.map_err(store_err)?
        } else {
            let mut given = GivenRows::new(op.vars.clone(), op.rows.len());
            for row in &op.rows {
                let entries: Vec<GivenRowEntry> = row.iter().map(entry).collect();
                given.push_row(entries).map_err(store_err)?;
            }
            tx.query_with_rows(&op.query, given)
                .await
                .map_err(store_err)?
        };
        if let Some(per_row) = op.expect_rows {
            check_row_count(answer, index, per_row * op.rows.len(), &op.query).await?;
        }
    }
    Ok(())
}

#[async_trait]
impl GraphStore for RouterGraphStore {
    async fn write(&self, ops: &[GraphWrite]) -> Result<(), EpistemicError> {
        let wtx = self
            .handle()
            .await?
            .begin_write()
            .await
            .map_err(store_err)?;
        let tx = wtx.transaction();
        run_graph_writes(tx, ops).await?;
        wtx.commit().await.map_err(store_err)?;
        Ok(())
    }
}

#[async_trait]
impl GraphReader for RouterGraphStore {
    async fn read(
        &self,
        query: &str,
        vars: &[String],
        row: &[GraphValue],
    ) -> Result<Vec<serde_json::Value>, EpistemicError> {
        let tx = self.handle().await?.begin_read().await.map_err(store_err)?;
        let answer = if vars.is_empty() {
            tx.query(query).await.map_err(store_err)?
        } else {
            let mut given = GivenRows::new(vars.to_vec(), 1);
            let entries: Vec<GivenRowEntry> = row.iter().map(entry).collect();
            given.push_row(entries).map_err(store_err)?;
            tx.query_with_rows(query, given).await.map_err(store_err)?
        };
        collect_documents(answer).await
    }

    async fn read_rows(
        &self,
        query: &str,
        vars: &[String],
        rows: &[Vec<GraphValue>],
    ) -> Result<Vec<serde_json::Value>, EpistemicError> {
        if !validate_read_rows(vars, rows)? {
            return Ok(Vec::new());
        }
        let tx = self.handle().await?.begin_read().await.map_err(store_err)?;
        let mut given = GivenRows::new(vars.to_vec(), rows.len());
        for row in rows {
            let entries: Vec<GivenRowEntry> = row.iter().map(entry).collect();
            given.push_row(entries).map_err(store_err)?;
        }
        let answer = tx.query_with_rows(query, given).await.map_err(store_err)?;
        collect_documents(answer).await
    }
}

/// Drains a document-stream answer into JSON values; any other answer yields none.
/// A stream error is returned, never treated as the end of the results.
async fn collect_documents(answer: QueryAnswer) -> Result<Vec<serde_json::Value>, EpistemicError> {
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
