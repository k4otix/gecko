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

/// The neutral graph capability: parameterized reads + atomic batched writes.
///
/// This is the **only** production coupling mem-gecko has to a graph backend. The
/// engine implements it over the real driver; tests implement it in-memory.
#[async_trait]
pub trait GraphStore: Send + Sync {
    /// Runs a sequence of parameterized writes in **one** transaction and commits.
    ///
    /// Ops execute in order; any error rolls the whole transaction back (nothing
    /// is committed). Ops with empty `rows` are skipped. An op whose answer row
    /// count differs from its [`GraphWrite::expect_rows`] total is an error built by
    /// [`RowCountMismatch::into_error`].
    async fn write(&self, ops: &[GraphWrite]) -> Result<(), EpistemicError>;

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
}
