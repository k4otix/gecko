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

use std::path::Path;

use anyhow::{Context, Result};
use gecko_extension_api::GeckoExtension;
use serde::Deserialize;
use tracing::{info, warn};

/// Parsed `gecko.toml`.
#[derive(Debug, Deserialize, Default)]
pub struct GeckoConfig {
    #[serde(default)]
    pub extensions: ExtSel,
    /// The `[semantic_index]` table (plan A5.5). Absent ⇒ accelerator disabled
    /// (a bare checkout stays fully functional and drag-free).
    #[serde(default)]
    pub semantic_index: SemanticIndexConfig,
}

/// The `[semantic_index]` table: the in-process HNSW retrieval accelerator (A5).
///
/// The accelerator is a **pure, rebuildable bolt-on** (invariant 8). When
/// `enabled = false` the writer runs with no index at all — recall falls back to
/// the non-vector path and no drag is added.
#[derive(Debug, Deserialize)]
pub struct SemanticIndexConfig {
    /// Whether the semantic index is active. Default `false` (substrate-only).
    #[serde(default)]
    pub enabled: bool,
    /// Path to the persisted (file-serialized) index. Default `"gecko.hnsw"`.
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
    #[serde(default)]
    pub record_retrieval_provenance: Option<bool>,
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

impl Default for SemanticIndexConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: default_index_path(),
            embedder: default_embedder(),
            backend: default_backend(),
            record_retrieval_provenance: None,
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
#[derive(Debug, Deserialize, Default)]
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
pub fn build_extensions(cfg: &GeckoConfig) -> Result<Vec<Box<dyn GeckoExtension>>> {
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
}
