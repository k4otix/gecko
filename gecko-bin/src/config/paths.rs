//! Runtime-asset path resolution (plan P1) — extracted from `config.rs`.
//!
//! GECKO ships three artifacts with three lifecycles: the `gecko` binary (build
//! tree), the TypeDB server (its own process), and the ~1.3GB embedding model (a
//! RUNTIME asset). Load-bearing rule #1: the model is NEVER a build dependency —
//! the binary holds only a PATH, resolved at runtime here from `model_id`. All
//! runtime assets live under the cache root, OUTSIDE the repo/build tree.
//!
//! This module owns the cache-root / model-dir / index-path precedence resolvers;
//! the parent [`config`](super) module re-exports the public ones
//! ([`cache_dir`], [`model_path`], [`index_path`]).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{GeckoConfig, SemanticIndexConfig};

/// The basename of the derived index file under `cache_dir/index/` when no
/// explicit `[semantic_index] path` override is set.
const DERIVED_INDEX_FILENAME: &str = "gecko.hnsw";

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

/// Derives the index path under a resolved cache root. An absent `path` (`None`)
/// derives under `cache_dir/index/gecko.hnsw`; a `Some(path)` is used verbatim as
/// an explicit override. Pure (no I/O); [`index_path`] wraps it.
fn derive_index_path(cache_root: &Path, sic: &SemanticIndexConfig) -> PathBuf {
    match sic.path.as_deref().filter(|s| !s.is_empty()) {
        Some(explicit) => PathBuf::from(explicit),
        None => cache_root.join("index").join(DERIVED_INDEX_FILENAME),
    }
}

/// Resolves the effective HNSW index path. Defaults to
/// `cache_dir/index/gecko.hnsw`; an explicit `[semantic_index] path` overrides
/// (any `Some(path)`, including a literal relative path, wins over the default).
pub fn index_path(cfg: &GeckoConfig) -> Result<PathBuf> {
    // An explicit override needs no cache root.
    if let Some(explicit) = cfg.semantic_index.path.as_deref().filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(explicit));
    }
    Ok(derive_index_path(&cache_dir(cfg)?, &cfg.semantic_index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GeckoConfig, SemanticIndexConfig};

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
        // Absent path derives under the cache root.
        let sic = SemanticIndexConfig::default();
        assert_eq!(
            derive_index_path(Path::new("/cache/gecko"), &sic),
            PathBuf::from("/cache/gecko/index/gecko.hnsw")
        );
        // Explicit path wins over the derived default (keeps A5 temp-path tests working).
        let sic = SemanticIndexConfig {
            path: Some("/tmp/test-abc.hnsw".to_string()),
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
        // Option<String> now expresses what the old sentinel could not: a literal
        // relative `gecko.hnsw` in the working dir (not the derived cache path).
        let sic = SemanticIndexConfig {
            path: Some("gecko.hnsw".to_string()),
            ..Default::default()
        };
        assert_eq!(
            derive_index_path(Path::new("/cache/gecko"), &sic),
            PathBuf::from("gecko.hnsw")
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
}
