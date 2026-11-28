//! The OKF data structures extensions and the engine share.
//!
//! Data models for Open Knowledge Format (OKF) concepts, bundles, and relations, including
//! GECKO-specific frontmatter keys (`engine`, `scopes`, `timeout-ms`).
//! An extension that imports content (such as a STIX bundle) builds these types
//! directly; the engine parses, syncs and executes them.

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
    /// engine that executes it. Returns `None` for any other language (`python`,
    /// `sql`, `mermaid`, plain text, …): a fence in one is documentation, and an
    /// `engine` naming one is refused by the parser.
    pub fn from_lang(lang: &str) -> Option<Self> {
        let lang = lang.trim().to_lowercase();
        SCRIPT_ENGINE_LANGS
            .contains(&lang.as_str())
            .then_some(ScriptEngine::QuickJs)
    }
}

/// The spellings [`ScriptEngine::from_lang`] accepts, for both an `engine` value and a
/// code fence's language, compared after trimming and lowercasing.
pub const SCRIPT_ENGINE_LANGS: [&str; 3] = ["js", "javascript", "quickjs"];

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
    /// the concept declares no engine — such a concept is documentation, not
    /// executable. The parser refuses a declared engine with no non-blank matching
    /// fence, so a parsed concept with an engine always has a program. One program
    /// per concept: multiple fences are a literate-programming affordance, joined
    /// into a single program at parse time (TypeDB attribute sets are unordered, so
    /// the join must happen here).
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
    /// Scripting engine to use for execution (from frontmatter `engine`).
    pub engine: Option<ScriptEngine>,

    /// Required permission scopes for host API access (from frontmatter `scopes`).
    pub scopes: Vec<String>,

    /// Maximum execution time in milliseconds (from frontmatter `timeout-ms`), from 1 to
    /// the engine's ceiling (`gecko_engine::sandbox::wasm_executor::MAX_TIMEOUT_MS`).
    pub timeout_ms: Option<u64>,

    /// The OKF v0.2 provenance, trust and lifecycle facts the file states (`generated.by`,
    /// `verified`, `status`, `stale_after`); `generated.at` fills [`Self::timestamp`].
    #[serde(default)]
    pub provenance: OkfProvenance,
}

/// A concept's OKF v0.2 trust tier, derived from its `verified` actors (OKF §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrustTier {
    Unverified,
    MachineConfirmed,
    HumanReviewed,
}

impl TrustTier {
    /// The tier's OKF name, as stored in the graph.
    pub fn as_str(self) -> &'static str {
        match self {
            TrustTier::Unverified => "unverified",
            TrustTier::MachineConfirmed => "machine-confirmed",
            TrustTier::HumanReviewed => "human-reviewed",
        }
    }
}

/// One OKF v0.2 `verified` event: who confirmed the concept, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verification {
    pub by: String,
    pub at: DateTime<Utc>,
}

/// The OKF v0.2 provenance, trust and lifecycle facts of a concept. Every field is
/// empty for a concept that states none of them, including every extension-built one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OkfProvenance {
    /// The `generated.by` actor, as written.
    pub generated_by: Option<String>,
    /// The tier derived from `verified`; `None` when `verified` is absent or holds no
    /// valid event.
    pub trust_tier: Option<TrustTier>,
    /// The `status`, as written; absence means `stable`.
    pub status: Option<String>,
    /// The instant from which the concept is stale.
    pub stale_after: Option<DateTime<Utc>>,
    /// The valid `verified` events, without exact duplicates, in file order.
    pub verifications: Vec<Verification>,
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
/// By default one bundle maps to one TypeDB database; bundles may share a database
/// with `--database`. Concept IDs are pure OKF bundle-relative paths (e.g.
/// `tables/orders`) with no namespace prefix, and must not collide across the
/// bundles of one database. `bundle_name`/`bundle_description` are metadata
/// sourced from an optional `bundle.json` manifest (falling back to the directory
/// name); they identify the bundle entity but are NOT used to namespace concept
/// IDs.
#[derive(Debug, Clone)]
pub struct OkfBundle {
    pub bundle_path: String,
    pub bundle_name: String,
    pub bundle_description: Option<String>,
    pub concepts: Vec<OkfConcept>,
    pub links: Vec<OkfLink>,
    pub citations: Vec<OkfCitation>,
    pub hierarchy: Vec<HierarchyEdge>,
    /// The `okf_version` the bundle-root `index.md` declares, as written.
    pub okf_version: Option<String>,
    /// Warnings about what the parser skipped or could not read: the root index's
    /// `okf_version`, then each skipped symlink in walk order, then each Markdown file
    /// that is not a concept and each unreadable concept value, in file order. A
    /// concept's value stays in its `metadata-json` as written; an unreadable
    /// `okf_version` is not recorded.
    pub warnings: Vec<String>,
}
