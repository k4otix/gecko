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
            Err(e) => {
                Err(e).with_context(|| format!("Failed to read {}", path.display()))
            }
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
        assert!(expect_err(&cfg_with(&["bogus"]))
            .contains("unknown or not-compiled extension: bogus"));
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
