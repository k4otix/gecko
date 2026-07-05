//! Core OKF data structures.
//!
//! Direct port of Tyke's `models.py`, extended with GECKO-specific
//! frontmatter fields (consumes, produces, engine, scopes, timeout_ms).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The scripting engine to use for executing a concept's code blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScriptEngine {
    Rhai,
    #[serde(alias = "quickjs")]
    QuickJs,
}

/// A single parsed OKF concept document.
///
/// Port of Tyke's `ParsedConcept`, extended with GECKO execution metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OkfConcept {
    /// Path-based concept identifier (e.g. "tables/orders").
    pub concept_id: String,

    /// Required OKF concept type (e.g. "Table", "Playbook").
    pub concept_type: String,

    pub title: Option<String>,
    pub description: Option<String>,
    pub resource_uri: Option<String>,
    pub tags: Vec<String>,
    pub timestamp: Option<DateTime<Utc>>,

    /// Raw markdown body after frontmatter.
    pub body: String,

    /// Extracted fenced code blocks from body.
    pub code_blocks: Vec<String>,

    /// Arbitrary extension frontmatter keys not consumed by the parser.
    pub extra_metadata: HashMap<String, String>,

    /// SHA-256 hash of the raw concept file content.
    pub file_hash: String,

    /// Original file path relative to bundle root.
    pub source_path: String,

    // -- GECKO-specific extension fields (design §3.1) --

    /// Data types this concept consumes (from frontmatter `consumes`).
    pub consumes: Vec<String>,

    /// Data types this concept produces (from frontmatter `produces`).
    pub produces: Vec<String>,

    /// Scripting engine to use for execution (from frontmatter `engine`).
    pub engine: Option<ScriptEngine>,

    /// Required permission scopes for host API access (from frontmatter `scopes`).
    pub scopes: Vec<String>,

    /// Maximum execution time in milliseconds (from frontmatter `timeout-ms`).
    pub timeout_ms: Option<u64>,
}

/// A relative markdown link from one concept to another.
///
/// Port of Tyke's `ParsedLink`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkfLink {
    pub source_id: String,
    pub target_id: String,
    pub link_text: String,
}

/// An external markdown citation link.
///
/// Port of Tyke's `ParsedCitation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkfCitation {
    pub source_id: String,
    pub target_url: String,
    pub link_text: String,
}

/// Parent-child directory hierarchy edge.
///
/// Port of Tyke's `HierarchyEdge`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HierarchyEdge {
    /// Empty string if parent is the root.
    pub parent_id: String,
    pub child_id: String,
}

/// Parsed content of a complete OKF bundle.
///
/// Port of Tyke's `BundleManifest`.
#[derive(Debug, Clone)]
pub struct OkfBundle {
    pub bundle_path: String,
    pub bundle_name: String,
    pub concepts: Vec<OkfConcept>,
    pub links: Vec<OkfLink>,
    pub citations: Vec<OkfCitation>,
    pub hierarchy: Vec<HierarchyEdge>,
}
