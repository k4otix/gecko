//! Runtime extension selection via `gecko.toml`.
//!
//! One binary ships every compiled-in extension; the operator's `gecko.toml`
//! decides which are **activated** at runtime. This is *selection* from
//! first-party extensions, not plugin authorship — there is no dynamic loading,
//! no ABI. See [`build_extensions`].
//!
//! ```toml
//! [extensions]
//! enabled = ["cyber"]   # mem is substrate — always-on, regardless of this list
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use gecko_extension_api::GeckoExtension;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Parsed `gecko.toml`.
#[derive(Debug, Deserialize, Serialize, Default)]
pub struct GeckoConfig {
    #[serde(default)]
    pub extensions: ExtSel,
    /// The `[semantic_index]` table (plan A5.5). Absent ⇒ accelerator disabled
    /// (a bare checkout stays fully functional and drag-free).
    #[serde(default)]
    pub semantic_index: SemanticIndexConfig,
    /// The `[typedb]` table (plan P1/P3). Absent ⇒ external mode connecting to
    /// the default endpoint (current behavior).
    #[serde(default)]
    pub typedb: TypedbConfig,
    /// Optional override for the GECKO cache root — where all RUNTIME assets
    /// (the ~1.3GB model, the HNSW index, and — P3 — a downloaded TypeDB) live.
    /// This is a runtime asset location OUTSIDE the build tree; see
    /// [`cache_dir`] for the full precedence. Absent ⇒ `XDG_CACHE_HOME/gecko`
    /// (fallback `~/.cache/gecko`), overridable by the `GECKO_CACHE_DIR` env.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<String>,
}

/// The `[semantic_index]` table: the in-process HNSW retrieval accelerator (A5).
///
/// The accelerator is a **pure, rebuildable bolt-on** (invariant 8). When
/// `enabled = false` the writer runs with no index at all — recall falls back to
/// the non-vector path and no drag is added.
#[derive(Debug, Deserialize, Serialize)]
pub struct SemanticIndexConfig {
    /// Whether the semantic index is active. Default `false` (substrate-only).
    #[serde(default)]
    pub enabled: bool,
    /// Path to the persisted (file-serialized) index. The sentinel default
    /// `"gecko.hnsw"` means "derive under `cache_dir/index/gecko.hnsw`"; set an
    /// explicit (typically absolute) path to override the derived default —
    /// [`index_path`] resolves the effective location.
    #[serde(default = "default_index_path")]
    pub path: String,
    /// Which embedder to build. Only `"stub"` is compiled today; the real
    /// `bge-large-en-v1.5` impl is deferred behind a feature.
    #[serde(default = "default_embedder")]
    pub embedder: String,
    /// Which index backend. Only `"hnsw"` today; `"typedb-native"` is the future
    /// drop-in (A5.6).
    #[serde(default = "default_backend")]
    pub backend: String,
    /// Whether to record retrieval provenance (A5.7). When unset, defaults to
    /// `enabled` (on when the index is on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_retrieval_provenance: Option<bool>,
    /// The SINGLE SOURCE OF TRUTH for the embedding model, of the form
    /// `"<model>@<revision-hash>"` (A5.2). The model is a RUNTIME asset: its
    /// on-disk path is DERIVED as `cache_dir/models/<model_id>/` ([`model_path`])
    /// and is content-addressed by this id — never a build dependency, never
    /// hand-configured. Changing the revision changes the cache location.
    #[serde(default = "default_model_id")]
    pub model_id: String,
    /// Whether to automatically fetch the model if absent. Default `false` —
    /// GECKO never silently pulls ~1.3GB; the operator opts in (P2 wires the
    /// actual download).
    #[serde(default)]
    pub auto_fetch: bool,
    /// Optional explicit override for the model directory, bypassing the derived
    /// `cache_dir/models/<model_id>/` path. For air-gapped / pre-provisioned
    /// deployments that stage weights out-of-band.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_path: Option<String>,
    /// Optional mirror / base URL for model acquisition (P2/P5 consume it).
    /// `None` ⇒ the default source (HuggingFace).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_source: Option<String>,
}

fn default_index_path() -> String {
    "gecko.hnsw".to_string()
}
fn default_embedder() -> String {
    "stub".to_string()
}
fn default_backend() -> String {
    "hnsw".to_string()
}
/// The Tier-3-validated, pinned model revision — the SINGLE SOURCE OF TRUTH
/// consumed by [`default_model_id`] and the shipped `gecko init` template.
/// Kept in lockstep with `MODEL_REVISION` in
/// `.github/workflows/model-integration.yml` (which is where this SHA was
/// last validated); bumping either one without the other is a bug. A bump
/// here is caught by the `model-bump-gate` CI job (see `ci.yml`), which
/// requires a fresh Tier-3 run + the `tier3-validated` label before merge.
const PINNED_MODEL_REVISION: &str = "d4aa6901d3a41ba39fb536a557fa166f842b0e09";

fn default_model_id() -> String {
    format!("bge-large-en-v1.5@{PINNED_MODEL_REVISION}")
}

impl Default for SemanticIndexConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: default_index_path(),
            embedder: default_embedder(),
            backend: default_backend(),
            record_retrieval_provenance: None,
            model_id: default_model_id(),
            auto_fetch: false,
            model_path: None,
            model_source: None,
        }
    }
}

/// How GECKO obtains the TypeDB server (plan P1/P3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TypedbMode {
    /// GECKO downloads/manages the TypeDB server itself (P3). This is the
    /// default (best local UX — `gecko up` manages a pinned TypeDB child).
    #[default]
    Orchestrated,
    /// TypeDB is brought up via a bundled `docker compose` stack (P3).
    Compose,
    /// GECKO connects to an externally-managed TypeDB at `endpoint`.
    External,
}

/// The `[typedb]` table: how to reach (and, from P3, how to run) the TypeDB
/// server. In P1 only `endpoint` affects behavior; `mode` is plumbed but not yet
/// acted on (main.rs still connects to `endpoint` regardless of mode).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TypedbConfig {
    /// The TypeDB server address. Default `"localhost:1729"`. The CLI
    /// `--address` flag overrides this when passed.
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    /// The TypeDB run mode. **Default `Orchestrated`** (P3): `gecko up` manages a
    /// pinned TypeDB child process. Set `external` to connect to a server you run,
    /// or `compose` for the Docker stack.
    #[serde(default)]
    pub mode: TypedbMode,
}

fn default_endpoint() -> String {
    "localhost:1729".to_string()
}

impl Default for TypedbConfig {
    fn default() -> Self {
        Self {
            endpoint: default_endpoint(),
            mode: TypedbMode::default(),
        }
    }
}

impl SemanticIndexConfig {
    /// Resolves whether retrieval-provenance recording is on: the explicit setting
    /// if given, else defaults to `enabled` (A5.7: default on when the index is on).
    pub fn record_provenance(&self) -> bool {
        self.record_retrieval_provenance.unwrap_or(self.enabled)
    }
}

/// The `[extensions]` table: the operator's runtime selection.
#[derive(Debug, Deserialize, Serialize, Default)]
pub struct ExtSel {
    /// Short names of extensions to activate (e.g. `"cyber"`). The `mem`
    /// substrate is always-on and need not be listed; unknown names error.
    #[serde(default)]
    pub enabled: Vec<String>,
}

impl GeckoConfig {
    /// Load and parse `gecko.toml` at `path`.
    ///
    /// A missing file is not an error: it yields the default (substrate-only)
    /// configuration, so a bare checkout runs `mem` alone. Ship a `gecko.toml`
    /// to activate domain extensions.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let cfg: GeckoConfig = toml::from_str(&text)
                    .with_context(|| format!("Failed to parse {}", path.display()))?;
                Ok(cfg)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                warn!(
                    path = %path.display(),
                    "No gecko.toml found — running with the mem substrate only"
                );
                Ok(GeckoConfig::default())
            }
            Err(e) => Err(e).with_context(|| format!("Failed to read {}", path.display())),
        }
    }
}

// ── Runtime asset layout (plan P1) ──────────────────────────────────────────
//
// GECKO ships three artifacts with three lifecycles: the `gecko` binary (build
// tree), the TypeDB server (its own process), and the ~1.3GB embedding model (a
// RUNTIME asset). Load-bearing rule #1: the model is NEVER a build dependency —
// the binary holds only a PATH, resolved at runtime here from `model_id`. All
// runtime assets live under the cache root, OUTSIDE the repo/build tree.

/// Pure precedence resolver for the cache root, factored out for testing. Order:
/// `GECKO_CACHE_DIR` env > `gecko.toml` `cache_dir` > `XDG_CACHE_HOME/gecko` >
/// `<home>/.cache/gecko`. Does no I/O and reads no environment itself — callers
/// pass the resolved inputs.
fn resolve_cache_dir(
    env_override: Option<&str>,
    cfg_cache: Option<&str>,
    xdg_cache_home: Option<&str>,
    home: Option<&str>,
) -> Result<PathBuf> {
    if let Some(v) = env_override.filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(v));
    }
    if let Some(v) = cfg_cache.filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(v));
    }
    if let Some(v) = xdg_cache_home.filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(v).join("gecko"));
    }
    if let Some(v) = home.filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(v).join(".cache").join("gecko"));
    }
    anyhow::bail!(
        "cannot resolve GECKO cache dir: set GECKO_CACHE_DIR, or `cache_dir` in \
         gecko.toml, or ensure HOME/XDG_CACHE_HOME is set"
    )
}

/// Resolves the GECKO cache root and ensures it exists and is writable.
///
/// Precedence: `GECKO_CACHE_DIR` env > `gecko.toml` `cache_dir` >
/// `XDG_CACHE_HOME/gecko` > `~/.cache/gecko`. All runtime assets (models, index,
/// and — P3 — a downloaded TypeDB) live under the returned directory.
pub fn cache_dir(cfg: &GeckoConfig) -> Result<PathBuf> {
    let env_override = std::env::var("GECKO_CACHE_DIR").ok();
    let xdg = std::env::var("XDG_CACHE_HOME").ok();
    let home = std::env::var("HOME").ok();
    let dir = resolve_cache_dir(
        env_override.as_deref(),
        cfg.cache_dir.as_deref(),
        xdg.as_deref(),
        home.as_deref(),
    )?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("cache dir not creatable: {}", dir.display()))?;
    Ok(dir)
}

/// Derives the model directory under a resolved cache root, honoring an explicit
/// `model_path` override. Pure (no I/O); [`model_path`] wraps it with cache-root
/// resolution.
#[cfg_attr(not(test), allow(dead_code))] // consumed by P2 (model fetch/load) + doctor
fn derive_model_path(cache_root: &Path, sic: &SemanticIndexConfig) -> PathBuf {
    if let Some(explicit) = sic.model_path.as_deref().filter(|s| !s.is_empty()) {
        return PathBuf::from(explicit);
    }
    cache_root.join("models").join(&sic.model_id)
}

/// Resolves the embedding model directory. Content-addressed as
/// `cache_dir/models/<model_id>/` unless `[semantic_index] model_path` is set,
/// which overrides (air-gapped / pre-provisioned dirs). Load-bearing rule #1:
/// this path is always OUTSIDE the build tree.
#[cfg_attr(not(test), allow(dead_code))] // consumed by P2 (model fetch/load) + doctor
pub fn model_path(cfg: &GeckoConfig) -> Result<PathBuf> {
    // The explicit override needs no cache root.
    if let Some(explicit) = cfg
        .semantic_index
        .model_path
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        return Ok(PathBuf::from(explicit));
    }
    Ok(derive_model_path(&cache_dir(cfg)?, &cfg.semantic_index))
}

/// Derives the index path under a resolved cache root. The sentinel default
/// `"gecko.hnsw"` derives under `cache_dir/index/gecko.hnsw`; any other value is
/// an explicit override used verbatim. Pure (no I/O); [`index_path`] wraps it.
fn derive_index_path(cache_root: &Path, sic: &SemanticIndexConfig) -> PathBuf {
    if sic.path == default_index_path() {
        return cache_root.join("index").join("gecko.hnsw");
    }
    PathBuf::from(&sic.path)
}

/// Resolves the effective HNSW index path. Defaults to
/// `cache_dir/index/gecko.hnsw`; an explicit `[semantic_index] path` overrides
/// (an absolute/explicit path wins over the derived default).
pub fn index_path(cfg: &GeckoConfig) -> Result<PathBuf> {
    // An explicit override needs no cache root.
    if cfg.semantic_index.path != default_index_path() {
        return Ok(PathBuf::from(&cfg.semantic_index.path));
    }
    Ok(derive_index_path(&cache_dir(cfg)?, &cfg.semantic_index))
}

/// The documented default `gecko.toml` written by `gecko init`. Reflects the
/// struct defaults above so it round-trips through [`GeckoConfig::load`].
pub fn default_gecko_toml() -> &'static str {
    r#"# GECKO runtime configuration.
#
# One binary ships every compiled-in extension; this file decides which are
# activated at runtime — no rebuild required. Registration is the gate: only
# activated extensions get their host functions loaded and write-paths opened.

[extensions]
# Domain extensions to activate. The `mem` substrate is always-on and does not
# need to be listed. Unknown or not-compiled names cause startup to fail.
enabled = []

# The GECKO cache root — where all RUNTIME assets live (the ~1.3GB embedding
# model, the HNSW index, and — P3 — a downloaded TypeDB). Kept OUTSIDE the repo.
# Precedence: GECKO_CACHE_DIR env > this key > XDG_CACHE_HOME/gecko > ~/.cache/gecko.
# cache_dir = "/path/to/cache"

# In-process semantic retrieval accelerator (plan A5). A PURE, rebuildable bolt-on
# (invariant 8) — never a source of truth, deletable when TypeDB ships native
# vector search. When `enabled = false` (or this table is absent) the substrate
# runs with no index: recall falls back to the non-vector path, drag-free.
[semantic_index]
enabled  = false
# The persisted index path. "gecko.hnsw" (the default) derives under
# cache_dir/index/gecko.hnsw; set an explicit path to override.
path     = "gecko.hnsw"
embedder = "stub"          # deterministic hash stub; future: "bge-large-en-v1.5", "api:<provider>"
backend  = "hnsw"          # future drop-in: "typedb-native"
# The SINGLE SOURCE OF TRUTH for the model. The model directory is DERIVED as
# cache_dir/models/<model_id>/ (content-addressed) — never hand-configured.
model_id = "bge-large-en-v1.5@d4aa6901d3a41ba39fb536a557fa166f842b0e09"
# Never silently pull ~1.3GB; the operator opts in (P2 wires the download).
auto_fetch = false
# model_path = "/pre/provisioned/model/dir"   # optional override (air-gapped)
# model_source = "https://mirror.example/models"  # optional mirror; default: HuggingFace

# How to reach (and how to run) the TypeDB server.
[typedb]
endpoint = "localhost:1729"
# Run mode: "orchestrated" | "compose" | "external".
#   orchestrated (default): `gecko up` downloads a PINNED TypeDB once into the
#                cache and runs it as a managed child process; `gecko down` stops
#                it. No database install, no Docker required.
#   compose:     bring TypeDB up yourself via `docker compose up -d` (see the
#                repo's docker-compose.yml).
#   external:    point `endpoint` at a TypeDB you run yourself; gecko connects but
#                never manages a process.
mode = "orchestrated"
"#
}

/// Resolve the config into the ordered list of extensions to register.
///
/// **Registration order = schema-application order.** The `mem` substrate is
/// registered first, unconditionally (forced-on even if absent from the config),
/// so its types are defined before any domain extension that subtypes them.
///
/// **Registration is the gate:** only extensions returned here get their host
/// functions loaded and their write-paths opened. A `cyber` entry that names a
/// not-compiled extension is a hard error, so the `#[cfg]` arms below and the
/// operator's config can never silently disagree.
// `vec_init_then_push`: mem is pushed (not folded into `vec![..]`) because the
// later domain-extension pushes are `#[cfg]`-gated — under `--no-default-features`
// (no `cyber`) nothing else pushes, so a `vec![..]` init would leave `mut` unused.
#[allow(clippy::vec_init_then_push)]
pub fn build_extensions(cfg: &GeckoConfig) -> Result<Vec<Box<dyn GeckoExtension>>> {
    // Initialize-then-push (not `vec![..]`): the later domain-extension pushes are
    // `#[cfg]`-gated, so under `--no-default-features` (no `cyber`) nothing else
    // pushes — folding mem into `vec![..]` would then leave `mut` unused. Keeping
    // the push also keeps mem's registration next to its explanatory log line.
    let mut extensions: Vec<Box<dyn GeckoExtension>> = Vec::new();

    // Substrate — always-on, always first (mem types must exist before any
    // domain extension can subtype them).
    extensions.push(mem_gecko::extension());
    info!(extension = "mem-gecko", "Registered substrate (forced-on)");

    for name in &cfg.extensions.enabled {
        match name.as_str() {
            // Already forced-on; listing it explicitly is a harmless no-op.
            "mem" => {}

            #[cfg(feature = "cyber")]
            "cyber" => {
                extensions.push(cyber_gecko::extension());
                info!(extension = "cyber-gecko", "Registered from gecko.toml");
            }
            #[cfg(not(feature = "cyber"))]
            "cyber" => anyhow::bail!(
                "extension 'cyber' is enabled in gecko.toml but was not compiled \
                 into this binary (this is a --no-default-features build); rebuild \
                 with the 'cyber' feature or remove it from gecko.toml"
            ),

            other => anyhow::bail!("unknown or not-compiled extension: {other}"),
        }
    }

    Ok(extensions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(enabled: &[&str]) -> GeckoConfig {
        GeckoConfig {
            extensions: ExtSel {
                enabled: enabled.iter().map(|s| s.to_string()).collect(),
            },
            ..Default::default()
        }
    }

    fn names(exts: &[Box<dyn GeckoExtension>]) -> Vec<&str> {
        exts.iter().map(|e| e.name()).collect()
    }

    #[test]
    fn parses_enabled_list() {
        let cfg: GeckoConfig = toml::from_str("[extensions]\nenabled = [\"cyber\"]\n").unwrap();
        assert_eq!(cfg.extensions.enabled, vec!["cyber".to_string()]);
    }

    #[test]
    fn semantic_index_defaults_to_disabled_and_drag_free() {
        // No [semantic_index] table ⇒ accelerator off (bare checkout stays drag-free).
        let cfg = GeckoConfig::default();
        assert!(!cfg.semantic_index.enabled);
        assert_eq!(cfg.semantic_index.path, "gecko.hnsw");
        assert_eq!(cfg.semantic_index.embedder, "stub");
        assert_eq!(cfg.semantic_index.backend, "hnsw");
        // record-provenance defaults to `enabled` (off here).
        assert!(!cfg.semantic_index.record_provenance());
    }

    #[test]
    fn parses_semantic_index_table() {
        let cfg: GeckoConfig = toml::from_str(
            "[semantic_index]\nenabled = true\npath = \"x.hnsw\"\nembedder = \"stub\"\nbackend = \"hnsw\"\n",
        )
        .unwrap();
        assert!(cfg.semantic_index.enabled);
        assert_eq!(cfg.semantic_index.path, "x.hnsw");
        // Unset record flag defaults to `enabled` (true here).
        assert!(cfg.semantic_index.record_provenance());
    }

    #[test]
    fn record_provenance_can_be_overridden() {
        let cfg: GeckoConfig = toml::from_str(
            "[semantic_index]\nenabled = true\nrecord_retrieval_provenance = false\n",
        )
        .unwrap();
        assert!(cfg.semantic_index.enabled);
        assert!(!cfg.semantic_index.record_provenance());
    }

    #[test]
    fn empty_config_is_substrate_only() {
        let cfg = GeckoConfig::default();
        assert_eq!(names(&build_extensions(&cfg).unwrap()), vec!["mem-gecko"]);
    }

    #[cfg(feature = "cyber")]
    #[test]
    fn mem_is_forced_on_and_first() {
        // Even listing only cyber, mem is registered first (schema subtyping order).
        let exts = build_extensions(&cfg_with(&["cyber"])).unwrap();
        assert_eq!(exts[0].name(), "mem-gecko");
    }

    #[test]
    fn listing_mem_is_a_harmless_noop() {
        let exts = build_extensions(&cfg_with(&["mem"])).unwrap();
        assert_eq!(names(&exts), vec!["mem-gecko"]);
    }

    /// Regression test for the P5 review fix: `model-integration.yml`'s
    /// "Configure model mirror" step used to `>> gecko.toml` a second
    /// `[semantic_index]` table onto the ALREADY-committed `gecko.toml`
    /// (which ships its own `[semantic_index]` table), producing a duplicate
    /// TOML table that fails to parse. The fix writes a standalone config
    /// (never appended to the committed file) — this is the exact template
    /// the workflow generates, asserting it parses as a single, complete,
    /// valid document with the mirror wired up via `model_source`.
    #[test]
    fn ci_mirror_config_template_parses_without_duplicate_table() {
        let rendered = concat!(
            "[extensions]\n",
            "enabled = [\"cyber\"]\n",
            "\n",
            "[typedb]\n",
            "endpoint = \"localhost:1729\"\n",
            "mode = \"external\"\n",
            "\n",
            "[semantic_index]\n",
            "enabled = true\n",
            "path = \"gecko.hnsw\"\n",
            "embedder = \"stub\"\n",
            "backend = \"hnsw\"\n",
            "model_id = \"bge-large-en-v1.5@d4aa6901d3a41ba39fb536a557fa166f842b0e09\"\n",
            "model_source = \"https://mirror.example/models\"\n",
        );
        let cfg: GeckoConfig = toml::from_str(rendered)
            .expect("standalone mirror config must parse cleanly (no duplicate table)");
        assert_eq!(cfg.extensions.enabled, vec!["cyber".to_string()]);
        assert_eq!(cfg.typedb.endpoint, "localhost:1729");
        assert_eq!(cfg.typedb.mode, TypedbMode::External);
        assert!(cfg.semantic_index.enabled);
        assert_eq!(
            cfg.semantic_index.model_id,
            "bge-large-en-v1.5@d4aa6901d3a41ba39fb536a557fa166f842b0e09"
        );
        assert_eq!(
            cfg.semantic_index.model_source.as_deref(),
            Some("https://mirror.example/models")
        );
    }

    fn expect_err(cfg: &GeckoConfig) -> String {
        match build_extensions(cfg) {
            Ok(_) => panic!("expected build_extensions to error"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn unknown_extension_errors() {
        assert!(
            expect_err(&cfg_with(&["bogus"])).contains("unknown or not-compiled extension: bogus")
        );
    }

    #[cfg(feature = "cyber")]
    #[test]
    fn cyber_registered_after_mem_when_compiled() {
        let exts = build_extensions(&cfg_with(&["cyber"])).unwrap();
        assert_eq!(names(&exts), vec!["mem-gecko", "cyber-gecko"]);
    }

    #[cfg(not(feature = "cyber"))]
    #[test]
    fn cyber_rejected_when_not_compiled() {
        assert!(expect_err(&cfg_with(&["cyber"])).contains("not compiled"));
    }

    // ── P1: cache layout + config schema ────────────────────────────────────

    /// Serializes the handful of tests that mutate process-global env vars.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn cache_dir_precedence_env_over_config_over_xdg_over_home() {
        // env override wins over everything
        assert_eq!(
            resolve_cache_dir(Some("/e"), Some("/c"), Some("/x"), Some("/h")).unwrap(),
            PathBuf::from("/e")
        );
        // config wins over xdg/home
        assert_eq!(
            resolve_cache_dir(None, Some("/c"), Some("/x"), Some("/h")).unwrap(),
            PathBuf::from("/c")
        );
        // xdg (gets /gecko suffix) wins over home
        assert_eq!(
            resolve_cache_dir(None, None, Some("/x"), Some("/h")).unwrap(),
            PathBuf::from("/x/gecko")
        );
        // home fallback → ~/.cache/gecko
        assert_eq!(
            resolve_cache_dir(None, None, None, Some("/h")).unwrap(),
            PathBuf::from("/h/.cache/gecko")
        );
        // empty strings are treated as unset
        assert_eq!(
            resolve_cache_dir(Some(""), Some(""), Some(""), Some("/h")).unwrap(),
            PathBuf::from("/h/.cache/gecko")
        );
        // nothing resolvable → error
        assert!(resolve_cache_dir(None, None, None, None).is_err());
    }

    #[test]
    fn cache_dir_env_override_is_created_and_writable() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("nested/cache");
        let prev = std::env::var("GECKO_CACHE_DIR").ok();
        // SAFETY: serialized by ENV_LOCK; restored below.
        unsafe { std::env::set_var("GECKO_CACHE_DIR", &target) };
        let resolved = cache_dir(&GeckoConfig::default()).unwrap();
        assert_eq!(resolved, target);
        assert!(resolved.is_dir(), "cache dir should be created");
        match prev {
            Some(v) => unsafe { std::env::set_var("GECKO_CACHE_DIR", v) },
            None => unsafe { std::env::remove_var("GECKO_CACHE_DIR") },
        }
    }

    #[test]
    fn model_path_derives_from_cache_and_model_id() {
        let sic = SemanticIndexConfig {
            model_id: "bge-large-en-v1.5@abc123".to_string(),
            ..Default::default()
        };
        assert_eq!(
            derive_model_path(Path::new("/cache/gecko"), &sic),
            PathBuf::from("/cache/gecko/models/bge-large-en-v1.5@abc123")
        );
    }

    #[test]
    fn model_path_explicit_override_wins() {
        let sic = SemanticIndexConfig {
            model_id: "bge-large-en-v1.5@abc123".to_string(),
            model_path: Some("/opt/preprovisioned/model".to_string()),
            ..Default::default()
        };
        assert_eq!(
            derive_model_path(Path::new("/cache/gecko"), &sic),
            PathBuf::from("/opt/preprovisioned/model")
        );
        // The public resolver also honors the override without touching the cache root.
        let cfg = GeckoConfig {
            semantic_index: sic,
            ..Default::default()
        };
        assert_eq!(
            model_path(&cfg).unwrap(),
            PathBuf::from("/opt/preprovisioned/model")
        );
    }

    #[test]
    fn index_path_defaults_derive_but_explicit_overrides() {
        // Sentinel default derives under the cache root.
        let sic = SemanticIndexConfig::default();
        assert_eq!(
            derive_index_path(Path::new("/cache/gecko"), &sic),
            PathBuf::from("/cache/gecko/index/gecko.hnsw")
        );
        // Explicit path wins over the derived default (keeps A5 temp-path tests working).
        let sic = SemanticIndexConfig {
            path: "/tmp/test-abc.hnsw".to_string(),
            ..Default::default()
        };
        assert_eq!(
            derive_index_path(Path::new("/cache/gecko"), &sic),
            PathBuf::from("/tmp/test-abc.hnsw")
        );
        let cfg = GeckoConfig {
            semantic_index: sic,
            ..Default::default()
        };
        assert_eq!(
            index_path(&cfg).unwrap(),
            PathBuf::from("/tmp/test-abc.hnsw")
        );
    }

    /// Load-bearing rule #1, encoded structurally: no runtime asset (model or
    /// index) can ever resolve inside the repo or `target/`.
    #[test]
    fn runtime_assets_never_resolve_inside_repo_or_target() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root");
        // A representative cache root outside the repo (as ~/.cache/gecko would be).
        let cache_root = resolve_cache_dir(None, None, None, Some("/home/gecko-user")).unwrap();
        assert!(
            !cache_root.starts_with(repo_root),
            "cache root must be outside the repo"
        );
        let sic = SemanticIndexConfig::default();
        let model = derive_model_path(&cache_root, &sic);
        let index = derive_index_path(&cache_root, &sic);
        for p in [&model, &index] {
            assert!(
                !p.starts_with(repo_root),
                "{p:?} must not be under the repo"
            );
            assert!(
                !p.components().any(|c| c.as_os_str() == "target"),
                "{p:?} must not be under target/"
            );
        }
    }

    #[test]
    fn typedb_defaults_to_orchestrated_endpoint() {
        // P3: orchestrated is the default local-UX mode; endpoint unchanged.
        let cfg = GeckoConfig::default();
        assert_eq!(cfg.typedb.endpoint, "localhost:1729");
        assert_eq!(cfg.typedb.mode, TypedbMode::Orchestrated);
    }

    #[test]
    fn typedb_mode_external_still_parses() {
        // External remains selectable for bring-your-own deployments.
        let cfg: GeckoConfig = toml::from_str("[typedb]\nmode = \"external\"\n").unwrap();
        assert_eq!(cfg.typedb.mode, TypedbMode::External);
    }

    #[test]
    fn typedb_mode_serde_rename() {
        let cfg: GeckoConfig = toml::from_str("[typedb]\nmode = \"orchestrated\"\n").unwrap();
        assert_eq!(cfg.typedb.mode, TypedbMode::Orchestrated);
        let cfg: GeckoConfig = toml::from_str("[typedb]\nmode = \"compose\"\n").unwrap();
        assert_eq!(cfg.typedb.mode, TypedbMode::Compose);
        // Serializes back to the lowercase rename.
        let s = toml::to_string(&TypedbConfig {
            endpoint: "localhost:1729".to_string(),
            mode: TypedbMode::Orchestrated,
        })
        .unwrap();
        assert!(s.contains("mode = \"orchestrated\""), "got: {s}");
    }

    #[test]
    fn semantic_index_new_fields_default() {
        let sic = SemanticIndexConfig::default();
        assert_eq!(
            sic.model_id,
            "bge-large-en-v1.5@d4aa6901d3a41ba39fb536a557fa166f842b0e09"
        );
        assert!(!sic.auto_fetch, "never silently pull 1.3GB");
        assert!(sic.model_path.is_none());
        assert!(sic.model_source.is_none());
    }

    #[test]
    fn default_gecko_toml_parses_to_valid_config() {
        let cfg: GeckoConfig = toml::from_str(default_gecko_toml()).unwrap();
        assert!(cfg.extensions.enabled.is_empty());
        assert!(!cfg.semantic_index.enabled);
        assert_eq!(
            cfg.semantic_index.model_id,
            "bge-large-en-v1.5@d4aa6901d3a41ba39fb536a557fa166f842b0e09"
        );
        assert!(!cfg.semantic_index.auto_fetch);
        assert_eq!(cfg.typedb.endpoint, "localhost:1729");
        assert_eq!(cfg.typedb.mode, TypedbMode::Orchestrated);
    }

    /// Regression guard for the documented first-run golden path: a shipped
    /// default `model_id` containing the literal `<revision>` placeholder
    /// makes `gecko model fetch` (and `gecko doctor`'s model check) hard-fail
    /// against the exact config `gecko init` writes — see FIX 1a of the
    /// packaging review. Pin both the absence of the placeholder and the
    /// exact Tier-3-validated SHA so a future edit can't silently
    /// reintroduce the broken golden path.
    #[test]
    fn default_model_id_has_no_unresolved_revision_placeholder() {
        let id = default_model_id();
        assert!(
            !id.contains("<revision>"),
            "default_model_id() must not ship the unresolved placeholder: {id}"
        );
        assert_eq!(
            id, "bge-large-en-v1.5@d4aa6901d3a41ba39fb536a557fa166f842b0e09",
            "must match the Tier-3-pinned MODEL_REVISION in \
             .github/workflows/model-integration.yml"
        );
        // The `gecko init` template must carry the same pinned SHA, not the
        // placeholder — it's what a fresh user's gecko.toml actually ships.
        assert!(
            default_gecko_toml().contains(&format!("model_id = \"{id}\"")),
            "gecko init template must embed the same pinned model_id"
        );
    }
}
