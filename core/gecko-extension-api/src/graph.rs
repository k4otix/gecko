//! The neutral graph read/write seam.
//!
//! mem-gecko must execute TypeQL against TypeDB, but it must **not** take a hard
//! production dependency on the whole engine: mem depends only on
//! `gecko-extension-api` at build time. This module is that firewall: a small
//! capability trait whose signatures use **only neutral types** — `&str` query
//! text, typed [`GraphValue`] params, and `serde_json::Value` row-shaped results.
//! No `typedb-driver` type ever appears here.
//!
//! - **mem owns its TQL.** It builds the query strings + typed params for every
//!   belief/episode write and every persisted-function read.
//! - **The engine owns the driver.** `gecko-engine` implements this trait over its
//!   `TypeDbRouter` (a `RouterGraphStore`); `gecko-bin` injects it into the writer.
//!
//! Both reads and writes flow through this one seam.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::EpistemicError;
use crate::ids::DateTime;

/// A neutral, typed value bound to a parameterized-query variable.
///
/// Mirrors the driver's scalar value kinds **without leaking driver types** — the
/// engine impl maps each variant onto the concrete driver `Value` at the boundary.
/// Passing values out-of-band (never string-interpolated) is what keeps the seam
/// injection-safe, exactly like the syncer's `given`-stage writes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GraphValue {
    /// A string value.
    String(String),
    /// A UTC datetime value (stored naive-UTC on the wire).
    Datetime(DateTime),
    /// A double-precision float.
    Double(f64),
    /// A 64-bit signed integer (TypeDB `integer`).
    Long(i64),
    /// A boolean value.
    Boolean(bool),
}

impl From<&str> for GraphValue {
    fn from(s: &str) -> Self {
        GraphValue::String(s.to_string())
    }
}

impl From<String> for GraphValue {
    fn from(s: String) -> Self {
        GraphValue::String(s)
    }
}

/// One parameterized write: a query with named `given` variables and typed rows.
///
/// Each row supplies one set of values for `vars` (positional). A single
/// [`GraphStore::write`] call runs a whole slice of these **in one transaction**
/// and commits once — that atomicity is what lets a belief write land its entity,
/// provenance stamp, derivation and evidence together or not at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphWrite {
    /// The TypeQL write query (a `given … match? insert|put|delete|update`).
    pub query: String,
    /// The `given` variable names, in the order the rows supply them.
    pub vars: Vec<String>,
    /// One or more value rows; `vars.len()` values each. An empty `rows` is a
    /// no-op (the op is skipped), matching the syncer's `run_rows` convention.
    pub rows: Vec<Vec<GraphValue>>,
    /// The answer-row count per value row, or `None` to skip the check. The store
    /// checks only the total, `n * rows.len()` (`n` for a parameterless op), and
    /// rolls the whole transaction back on any other count.
    #[serde(default)]
    pub expect_rows: Option<usize>,
}

impl GraphWrite {
    /// A single-row write: the common case (one belief, one episode, …).
    pub fn single(query: impl Into<String>, vars: Vec<String>, row: Vec<GraphValue>) -> Self {
        Self {
            query: query.into(),
            vars,
            rows: vec![row],
            expect_rows: None,
        }
    }

    /// A parameterless write (no `given` stage).
    pub fn plain(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            vars: Vec::new(),
            rows: vec![Vec::new()],
            expect_rows: None,
        }
    }

    /// Requires the op to produce exactly `n × rows.len()` answer rows in total, so a
    /// `match` that binds nothing (or too much) fails the write instead of committing.
    ///
    /// The store checks only that total: a two-row op expecting 1 cannot tell 2 + 0
    /// from 1 + 1, so use it on single-row ops when the per-row count matters.
    pub fn expecting(self, n: usize) -> Self {
        Self {
            expect_rows: Some(n),
            ..self
        }
    }
}

/// A write op whose answer row count missed its [`GraphWrite::expect_rows`] total.
///
/// A [`GraphStore`] reports the mismatch as the `Storage` error [`Self::into_error`]
/// builds, and a caller recovers the failing op with [`Self::from_error`], so the
/// message format has one owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowCountMismatch {
    /// The index of the failing op in the slice passed to [`GraphStore::write`].
    pub op: usize,
    /// The total row count the op required.
    pub expected: usize,
    /// The row count the op produced.
    pub got: usize,
}

impl RowCountMismatch {
    const PREFIX: &'static str = "write op ";

    /// The `Storage` error for this mismatch, followed by the head of the failing query.
    pub fn into_error(self, query_head: &str) -> EpistemicError {
        let Self { op, expected, got } = self;
        EpistemicError::Storage(format!(
            "{}{op} expected {expected} row(s), matched {got}: {query_head}",
            Self::PREFIX
        ))
    }

    /// The mismatch an error built by [`Self::into_error`] carries, or `None` for any
    /// other error.
    pub fn from_error(err: &EpistemicError) -> Option<Self> {
        let EpistemicError::Storage(msg) = err else {
            return None;
        };
        let rest = msg.strip_prefix(Self::PREFIX)?;
        let (op, rest) = rest.split_once(" expected ")?;
        let (expected, rest) = rest.split_once(" row(s), matched ")?;
        let (got, _) = rest.split_once(": ")?;
        Some(Self {
            op: op.parse().ok()?,
            expected: expected.parse().ok()?,
            got: got.parse().ok()?,
        })
    }
}

/// The read-only graph capability: parameterized reads.
///
/// Host bridges receive this handle so an extension can query the graph without a
/// write path; belief writes go through the belief-edge seam instead.
#[async_trait]
pub trait GraphReader: Send + Sync {
    /// Runs a read query, returning the fetched documents as JSON values.
    ///
    /// `vars`/`row` supply an optional single `given` row of typed params
    /// (empty when the query is self-contained). The query is expected to end in a
    /// `fetch { … };` producing a document stream.
    async fn read(
        &self,
        query: &str,
        vars: &[String],
        row: &[GraphValue],
    ) -> Result<Vec<serde_json::Value>, EpistemicError>;

    /// Runs one read query over several `given` rows, returning the documents from
    /// every row.
    ///
    /// No order is promised across or within rows, so a caller identifies a
    /// document's row by fetching an attribute the row matched. Rows should be
    /// distinct. `limit`, `sort` and `reduce` apply across all rows, so a batched
    /// query must not use them per row.
    ///
    /// An empty `vars` or a row whose length differs from `vars.len()` is
    /// [`EpistemicError::InvalidInput`] (use [`read`](Self::read) for a query with no
    /// `given`); empty `rows` returns no documents without running a query. The
    /// default implementation calls `read` once per row; a store overrides it to
    /// send one query.
    async fn read_rows(
        &self,
        query: &str,
        vars: &[String],
        rows: &[Vec<GraphValue>],
    ) -> Result<Vec<serde_json::Value>, EpistemicError> {
        validate_read_rows(vars, rows)?;
        let mut out = Vec::new();
        for row in rows {
            out.extend(self.read(query, vars, row).await?);
        }
        Ok(out)
    }
}

/// Refuses arguments [`GraphReader::read_rows`] cannot run: an empty `vars`, or a
/// row whose length differs from `vars.len()`.
///
/// Every implementation calls this first so they all refuse the same input.
pub fn validate_read_rows(vars: &[String], rows: &[Vec<GraphValue>]) -> Result<(), EpistemicError> {
    if vars.is_empty() {
        return Err(EpistemicError::InvalidInput(
            "read_rows needs at least one given variable; use read for a query with none".into(),
        ));
    }
    if let Some(index) = rows.iter().position(|row| row.len() != vars.len()) {
        return Err(EpistemicError::InvalidInput(format!(
            "read_rows row {index} has {} value(s) for {} variable(s)",
            rows[index].len(),
            vars.len()
        )));
    }
    Ok(())
}

/// The neutral graph capability: parameterized reads + atomic batched writes.
///
/// This is the **only** production coupling mem-gecko has to a graph backend. The
/// engine implements it over the real driver; tests implement it in-memory.
#[async_trait]
pub trait GraphStore: GraphReader {
    /// Runs a sequence of parameterized writes in **one** transaction and commits.
    ///
    /// Ops execute in order; any error rolls the whole transaction back (nothing
    /// is committed). Ops with empty `rows` are skipped. An op whose answer row
    /// count differs from its [`GraphWrite::expect_rows`] total is an error built by
    /// [`RowCountMismatch::into_error`].
    async fn write(&self, ops: &[GraphWrite]) -> Result<(), EpistemicError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_count_mismatch_round_trips_through_its_error() {
        let m = RowCountMismatch {
            op: 7,
            expected: 1,
            got: 0,
        };
        let err = m.clone().into_error("given $b: string; match $x isa y;");
        let EpistemicError::Storage(msg) = &err else {
            panic!("expected Storage, got {err:?}");
        };
        assert_eq!(
            msg,
            "write op 7 expected 1 row(s), matched 0: given $b: string; match $x isa y;"
        );
        assert_eq!(RowCountMismatch::from_error(&err), Some(m));
    }

    #[test]
    fn row_count_mismatch_ignores_other_errors() {
        for err in [
            EpistemicError::Storage("write answer is not a row stream: q".into()),
            EpistemicError::Storage("write op x expected 1 row(s), matched 0: q".into()),
            EpistemicError::InvalidInput("write op 1 expected 1 row(s), matched 0: q".into()),
        ] {
            assert_eq!(RowCountMismatch::from_error(&err), None, "{err:?}");
        }
    }

    /// A reader that answers each single-row `read` with one document echoing the
    /// row, and counts its calls.
    struct EchoReader {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl GraphReader for EchoReader {
        async fn read(
            &self,
            _query: &str,
            _vars: &[String],
            row: &[GraphValue],
        ) -> Result<Vec<serde_json::Value>, EpistemicError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(vec![serde_json::json!({ "row": row })])
        }
    }

    fn echo() -> EchoReader {
        EchoReader {
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn vars() -> Vec<String> {
        vec!["a".to_string()]
    }

    #[tokio::test]
    async fn default_read_rows_reads_each_row_and_concatenates() {
        let reader = echo();
        let rows = vec![vec![GraphValue::from("x")], vec![GraphValue::from("y")]];
        let docs = reader.read_rows("q", &vars(), &rows).await.unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0]["row"][0]["String"], "x");
        assert_eq!(docs[1]["row"][0]["String"], "y");
    }

    #[tokio::test]
    async fn default_read_rows_with_no_rows_reads_nothing() {
        let reader = echo();
        let docs = reader.read_rows("q", &vars(), &[]).await.unwrap();
        assert!(docs.is_empty());
        assert_eq!(reader.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn read_rows_refuses_a_row_of_the_wrong_length() {
        let reader = echo();
        let rows = vec![
            vec![GraphValue::from("x")],
            vec![GraphValue::from("y"), GraphValue::from("z")],
        ];
        let err = reader.read_rows("q", &vars(), &rows).await.unwrap_err();
        assert!(matches!(err, EpistemicError::InvalidInput(_)), "{err:?}");
        assert_eq!(reader.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn read_rows_refuses_empty_vars() {
        let reader = echo();
        let err = reader.read_rows("q", &[], &[Vec::new()]).await.unwrap_err();
        assert!(matches!(err, EpistemicError::InvalidInput(_)), "{err:?}");
        assert_eq!(reader.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}
