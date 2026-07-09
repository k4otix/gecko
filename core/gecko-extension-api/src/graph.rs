//! The neutral graph read/write seam (the A3 generalization of A1's
//! `BeliefCommitter`).
//!
//! mem-gecko must execute TypeQL against TypeDB, but it must **not** take a hard
//! production dependency on the whole engine (A0's contract: mem depends only on
//! `gecko-extension-api` at build time). This module is that firewall: a small
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
}

impl GraphWrite {
    /// A single-row write: the common case (one belief, one episode, …).
    pub fn single(query: impl Into<String>, vars: Vec<String>, row: Vec<GraphValue>) -> Self {
        Self {
            query: query.into(),
            vars,
            rows: vec![row],
        }
    }

    /// A parameterless write (no `given` stage).
    pub fn plain(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            vars: Vec::new(),
            rows: vec![Vec::new()],
        }
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
    /// is committed). Ops with empty `rows` are skipped.
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
