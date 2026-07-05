//! Core OKF data structures.
//!
//! Core data models representing Open Knowledge Format (OKF) concepts, bundles, and relations.
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
/// Represents a parsed OKF document with its metadata and extracted content.
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
/// Represents an internal markdown link between two concepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkfLink {
    pub source_id: String,
    pub target_id: String,
    pub link_text: String,
}

/// An external markdown citation link.
///
/// Represents an external reference or citation found in a concept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkfCitation {
    pub source_id: String,
    pub target_url: String,
    pub link_text: String,
}

/// Parent-child directory hierarchy edge.
///
/// Represents a hierarchical directory structure relationship.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HierarchyEdge {
    /// Empty string if parent is the root.
    pub parent_id: String,
    pub child_id: String,
}

/// Parsed content of a complete OKF bundle.
///
/// Represents a collection of OKF concepts, typically parsed from a directory.
#[derive(Debug, Clone)]
pub struct OkfBundle {
    pub bundle_path: String,
    pub bundle_name: String,
    pub concepts: Vec<OkfConcept>,
    pub links: Vec<OkfLink>,
    pub citations: Vec<OkfCitation>,
    pub hierarchy: Vec<HierarchyEdge>,
}
