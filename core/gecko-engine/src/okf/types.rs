//! Core OKF data structures.
//!
//! Data models for Open Knowledge Format (OKF) concepts, bundles, and relations, including
//! GECKO-specific frontmatter fields (consumes, produces, engine, scopes, timeout_ms).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The scripting engine to use for executing a concept's program.
///
/// GECKO runs all synced code as untrusted inside one WebAssembly sandbox boundary.
/// QuickJS (JavaScript, via a WASM guest) is the only supported engine; this stays an
/// enum so additional WASM-guest engines can be added later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScriptEngine {
    QuickJs,
}

impl ScriptEngine {
    /// The canonical engine token stored in the graph and accepted in frontmatter.
    pub fn as_str(&self) -> &'static str {
        match self {
            ScriptEngine::QuickJs => "quickjs",
        }
    }

    /// Resolves a code-fence language tag (or a frontmatter `engine` value) to the
    /// engine that executes it. Returns `None` for non-executable languages
    /// (`python`, `sql`, `mermaid`, plain text, …), which are treated as
    /// documentation rather than program source.
    pub fn from_lang(lang: &str) -> Option<Self> {
        match lang.trim().to_lowercase().as_str() {
            "js" | "javascript" | "quickjs" => Some(ScriptEngine::QuickJs),
            _ => None,
        }
    }
}

/// A single parsed OKF concept document, with its metadata and extracted content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OkfConcept {
    /// Path-based concept identifier (e.g. "tables/orders").
    pub concept_id: String,

    /// Required OKF concept type (e.g. "Table", "Playbook").
    pub concept_type: String,

    /// Optional specific sub-type to insert as, instead of the generic `concept`.
    #[serde(default)]
    pub type_hint: Option<String>,

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

    /// Schema-typed attributes an extension attaches beyond the core OKF fields.
    /// Each is persisted as a real typed `owns` on the concept's entity (see
    /// [`TypedAttribute`]), not folded into the `metadata-json` blob.
    #[serde(default)]
    pub typed_attributes: Vec<TypedAttribute>,

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

/// A single schema-typed attribute an extension attaches to a concept. The
/// `name` is a fixed schema label defined by the extension's own schema fragment
/// (validated as a TypeQL identifier before use); the value is written through
/// the parameterized `given` stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedAttribute {
    pub name: String,
    pub value: TypedAttrValue,
}

/// The value of a [`TypedAttribute`], covering the TypeDB value types extensions
/// currently attach.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedAttrValue {
    String(String),
    Bool(bool),
    Datetime(DateTime<Utc>),
}

/// A relative markdown link between two concepts within the bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkfLink {
    pub source_id: String,
    pub target_id: String,
    pub link_text: String,
}

/// An external citation link (e.g. a URL) found in a concept's body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkfCitation {
    pub source_id: String,
    pub target_url: String,
    pub link_text: String,
}

/// A parent-child directory hierarchy edge.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HierarchyEdge {
    /// Empty string if parent is the root.
    pub parent_id: String,
    pub child_id: String,
}

/// The parsed content of a complete OKF bundle: its concepts, links, citations, and
/// directory hierarchy.
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
