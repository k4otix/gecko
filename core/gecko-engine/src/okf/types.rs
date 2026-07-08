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

impl ScriptEngine {
    /// The canonical engine token stored in the graph and accepted in frontmatter.
    pub fn as_str(&self) -> &'static str {
        match self {
            ScriptEngine::Rhai => "rhai",
            ScriptEngine::QuickJs => "quickjs",
        }
    }

    /// Resolves a code-fence language tag (or a frontmatter `engine` value) to the
    /// engine that executes it. Returns `None` for non-executable languages
    /// (`python`, `sql`, `mermaid`, plain text, …), which are treated as
    /// documentation rather than program source.
    pub fn from_lang(lang: &str) -> Option<Self> {
        match lang.trim().to_lowercase().as_str() {
            "rhai" => Some(ScriptEngine::Rhai),
            "js" | "javascript" | "quickjs" => Some(ScriptEngine::QuickJs),
            _ => None,
        }
    }
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

    /// The concept's executable program: the fenced code blocks whose language
    /// matches the declared `engine`, concatenated in document order. `None` when
    /// the concept declares no engine or has no matching fence — such a concept is
    /// documentation, not executable. One program per concept: multiple fences are
    /// a literate-programming affordance, joined into a single program at parse
    /// time (TypeDB attribute sets are unordered, so the join must happen here).
    pub program: Option<String>,

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
///
/// GECKO is single-bundle-scoped: one bundle maps to one TypeDB database, so
/// concept IDs are pure OKF bundle-relative paths (e.g. `tables/orders`) with no
/// namespace prefix. `bundle_name`/`bundle_description` are metadata sourced from
/// an optional `bundle.json` manifest (falling back to the directory name); they
/// identify the bundle entity but are NOT used to namespace concept IDs.
#[derive(Debug, Clone)]
pub struct OkfBundle {
    pub bundle_path: String,
    pub bundle_name: String,
    pub bundle_description: Option<String>,
    pub concepts: Vec<OkfConcept>,
    pub links: Vec<OkfLink>,
    pub citations: Vec<OkfCitation>,
    pub hierarchy: Vec<HierarchyEdge>,
}
