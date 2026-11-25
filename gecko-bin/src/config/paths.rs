//! Runtime-asset path resolution — extracted from `config.rs`.
//!
//! GECKO ships three artifacts with three lifecycles: the `gecko` binary (build
//! tree), the TypeDB server (its own process), and the ~1.3GB embedding model (a
//! RUNTIME asset). Load-bearing rule #1: the model is NEVER a build dependency —
//! the binary holds only a PATH, resolved at runtime here from `model_id`. All
//! runtime assets live under the cache root, OUTSIDE the repo/build tree.
//!
//! This module owns the cache-root / model-dir / index-file precedence resolvers;
//! the parent [`config`](super) module re-exports the public ones
//! ([`cache_dir`], [`model_path`], [`index_dir`], [`index_file`]).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{GeckoConfig, SemanticIndexConfig};

/// The subdirectory of the cache root that holds one index file per database when
/// no `[semantic_index] dir` is set.
const DERIVED_INDEX_SUBDIR: &str = "index";

/// Treats an empty string as unset: `Some("")` collapses to `None`, so a blank
/// config/env value falls through to the next precedence step.
fn non_empty(s: Option<&str>) -> Option<&str> {
    s.filter(|v| !v.is_empty())
}

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
    if let Some(v) = non_empty(env_override) {
        return Ok(PathBuf::from(v));
    }
    if let Some(v) = non_empty(cfg_cache) {
        return Ok(PathBuf::from(v));
    }
    if let Some(v) = non_empty(xdg_cache_home) {
        return Ok(PathBuf::from(v).join("gecko"));
    }
    if let Some(v) = non_empty(home) {
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
/// and a downloaded TypeDB) live under the returned directory.
pub fn cache_dir(cfg: &GeckoConfig) -> Result<PathBuf> {
    let dir = cache_root(cfg)?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("cache dir not creatable: {}", dir.display()))?;
    Ok(dir)
}

/// Resolves the cache root by the [`cache_dir`] precedence without creating it.
fn cache_root(cfg: &GeckoConfig) -> Result<PathBuf> {
    let env_override = std::env::var("GECKO_CACHE_DIR").ok();
    let xdg = std::env::var("XDG_CACHE_HOME").ok();
    let home = std::env::var("HOME").ok();
    resolve_cache_dir(
        env_override.as_deref(),
        cfg.cache_dir.as_deref(),
        xdg.as_deref(),
        home.as_deref(),
    )
}

/// Derives the model directory under a resolved cache root, honoring an explicit
/// `model_path` override. Pure (no I/O); [`model_path`] wraps it with cache-root
/// resolution.
#[cfg_attr(not(test), allow(dead_code))] // consumed by model fetch/load + doctor
fn derive_model_path(cache_root: &Path, sic: &SemanticIndexConfig) -> PathBuf {
    if let Some(explicit) = non_empty(sic.model_path.as_deref()) {
        return PathBuf::from(explicit);
    }
    cache_root.join("models").join(&sic.model_id)
}

/// Resolves the embedding model directory. Content-addressed as
/// `cache_dir/models/<model_id>/` unless `[semantic_index] model_path` is set,
/// which overrides (air-gapped / pre-provisioned dirs). Load-bearing rule #1:
/// this path is always OUTSIDE the build tree.
#[cfg_attr(not(test), allow(dead_code))] // consumed by model fetch/load + doctor
pub fn model_path(cfg: &GeckoConfig) -> Result<PathBuf> {
    // The explicit override needs no cache root.
    if let Some(explicit) = non_empty(cfg.semantic_index.model_path.as_deref()) {
        return Ok(PathBuf::from(explicit));
    }
    Ok(derive_model_path(&cache_dir(cfg)?, &cfg.semantic_index))
}

/// Derives the index directory under a resolved cache root: an explicit
/// `[semantic_index] dir`, or `cache_root/index`. Pure (no I/O).
fn derive_index_dir(cache_root: &Path, sic: &SemanticIndexConfig) -> PathBuf {
    match non_empty(sic.dir.as_deref()) {
        Some(explicit) => PathBuf::from(explicit),
        None => cache_root.join(DERIVED_INDEX_SUBDIR),
    }
}

/// Resolves the directory that holds one index file per database, creating
/// nothing. Refuses a config that still sets `[semantic_index] path`, and a `dir`
/// that exists but is not a directory.
pub fn index_dir(cfg: &GeckoConfig) -> Result<PathBuf> {
    let sic = &cfg.semantic_index;
    if sic.path.is_some() {
        anyhow::bail!(
            "`[semantic_index] path` is no longer read: the index is one file per database. \
             Set `[semantic_index] dir` to a directory instead, or remove `path` to use the \
             cache directory."
        );
    }
    let dir = match non_empty(sic.dir.as_deref()) {
        Some(explicit) => PathBuf::from(explicit),
        None => derive_index_dir(&cache_root(cfg)?, sic),
    };
    if dir.exists() && !dir.is_dir() {
        anyhow::bail!(
            "`[semantic_index] dir` is {}, which is not a directory; point `dir` at a directory",
            dir.display()
        );
    }
    Ok(dir)
}

/// Resolves `database`'s index file, `<index dir>/<encoded name>.hnsw`, creating
/// nothing.
pub fn index_file(cfg: &GeckoConfig, database: &str) -> Result<PathBuf> {
    Ok(index_dir(cfg)?.join(format!("{}.hnsw", encode_database_name(database)?)))
}

/// Encodes a database name as a filename stem: `[A-Za-z0-9_-]` are kept and every
/// other UTF-8 byte becomes `%XX`, so distinct names never share a file and no name
/// escapes the index directory. An empty name is refused.
fn encode_database_name(name: &str) -> Result<String> {
    if name.is_empty() {
        anyhow::bail!("cannot name an index file for an empty database name");
    }
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    Ok(out)
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

    fn with_dir(dir: &str) -> GeckoConfig {
        GeckoConfig {
            semantic_index: SemanticIndexConfig {
                enabled: true,
                dir: Some(dir.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn index_file_defaults_to_cache_index_dir() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var("GECKO_CACHE_DIR").ok();
        // SAFETY: serialized by ENV_LOCK; restored below.
        unsafe { std::env::set_var("GECKO_CACHE_DIR", tmp.path()) };
        let cfg = GeckoConfig {
            semantic_index: SemanticIndexConfig {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = index_file(&cfg, "alpha");
        match prev {
            Some(v) => unsafe { std::env::set_var("GECKO_CACHE_DIR", v) },
            None => unsafe { std::env::remove_var("GECKO_CACHE_DIR") },
        }
        assert_eq!(
            resolved.unwrap(),
            tmp.path().join("index").join("alpha.hnsw")
        );
        assert!(
            !tmp.path().join("index").exists(),
            "resolving creates nothing"
        );
    }

    #[test]
    fn explicit_dir_is_used() {
        assert_eq!(
            index_file(&with_dir("/x/idx"), "alpha").unwrap(),
            PathBuf::from("/x/idx/alpha.hnsw")
        );
    }

    #[test]
    fn encoded_names_stay_in_dir_and_are_injective() {
        let cfg = with_dir("/x/idx");
        let dir = index_dir(&cfg).unwrap();
        let names = ["a/b", "..", "données", "a%2Fb", "A-b_9"];
        let files: Vec<PathBuf> = names.iter().map(|n| index_file(&cfg, n).unwrap()).collect();
        for f in &files {
            assert_eq!(
                f.parent(),
                Some(dir.as_path()),
                "{f:?} stays in the index dir"
            );
        }
        let distinct: std::collections::HashSet<&PathBuf> = files.iter().collect();
        assert_eq!(distinct.len(), names.len(), "no two names share a file");
        assert_eq!(encode_database_name("A-b_9").unwrap(), "A-b_9");
        assert_eq!(encode_database_name("a/b").unwrap(), "a%2Fb");
        assert_eq!(encode_database_name("a%2Fb").unwrap(), "a%252Fb");
        assert_eq!(encode_database_name("é").unwrap(), "%C3%A9");
    }

    #[test]
    fn empty_database_name_is_refused() {
        assert!(index_file(&with_dir("/x/idx"), "").is_err());
    }

    #[test]
    fn a_stale_path_key_is_refused_naming_dir() {
        let cfg = GeckoConfig {
            semantic_index: SemanticIndexConfig {
                enabled: true,
                path: Some(toml::Value::String("gecko.hnsw".into())),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = format!("{:#}", index_dir(&cfg).unwrap_err());
        assert!(err.contains("[semantic_index] path"), "{err}");
        assert!(err.contains("[semantic_index] dir"), "{err}");
    }

    #[test]
    fn a_dir_that_is_a_file_is_refused() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let err = format!(
            "{:#}",
            index_dir(&with_dir(tmp.path().to_str().unwrap())).unwrap_err()
        );
        assert!(err.contains("not a directory"), "{err}");
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
        let index = derive_index_dir(&cache_root, &sic);
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
