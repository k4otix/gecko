//! `gecko doctor` — the tested precondition checker (plan P7.2).
//!
//! "Is everything configured correctly?" is knowledge that must be *versioned
//! with the code* (the pinned TypeDB version, whether the model revision matches,
//! whether the endpoint answers), so it lives here as a subcommand — not as a
//! bash script that would hardcode drifting expectations.
//!
//! Each precondition is a [`CheckResult`] (`Ok | Warn | Fail` + detail + optional
//! remediation). [`run_doctor`] runs them (or a single `--check <name>`), renders
//! aligned `✓/⚠/✗` lines (or `--json`, or nothing under `--quiet`), and returns a
//! process exit code that is **FAILURE iff any check is `Fail`** — warnings do not
//! fail it. `scripts/setup.sh`'s idempotency is built on that exit code.
//!
//! Config is loaded **leniently**: doctor must run and report even when
//! `gecko.toml` is absent or unparseable (that *is* what `check_config` reports),
//! so a broken config never panics the checker.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Serialize;

use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
// `fetch::is_cached` is only reachable in the model-present probe, which is
// compiled only when the real embedder is (stub builds need no model file).
#[cfg(feature = "real-embedder")]
use gecko_semantic_index::fetch;

use crate::Cli;
use crate::config::{self, GeckoConfig, TypedbMode};
use crate::orchestrator::PINNED_TYPEDB;

/// The ordered list of check names (also the valid `--check <name>` values).
const CHECK_NAMES: [&str; 6] = [
    "config",
    "cache-dir",
    "typedb",
    "model",
    "index",
    "embedder",
];

/// Outcome of a single precondition check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Precondition satisfied.
    Ok,
    /// Not satisfied, but non-fatal (dev-fine / rebuildable / self-healing).
    Warn,
    /// A hard precondition failure — makes the exit code non-zero.
    Fail,
}

/// The result of one precondition check.
#[derive(Debug, Serialize)]
pub struct CheckResult {
    /// Stable check identifier (matches `--check <name>`).
    pub name: &'static str,
    /// Ok / Warn / Fail.
    pub status: Status,
    /// Human-readable detail of what was found.
    pub detail: String,
    /// Optional one-line remediation, printed for non-Ok results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl CheckResult {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Ok,
            detail: detail.into(),
            fix: None,
        }
    }
    fn warn(name: &'static str, detail: impl Into<String>, fix: Option<&str>) -> Self {
        Self {
            name,
            status: Status::Warn,
            detail: detail.into(),
            fix: fix.map(str::to_string),
        }
    }
    fn fail(name: &'static str, detail: impl Into<String>, fix: Option<&str>) -> Self {
        Self {
            name,
            status: Status::Fail,
            detail: detail.into(),
            fix: fix.map(str::to_string),
        }
    }
}

/// Config as doctor sees it: a best-effort [`GeckoConfig`] (the parsed file, or
/// the default when the file is absent or unparseable) plus the *reason* the file
/// could not be used — which is exactly what `check_config` reports.
pub struct LoadedConfig {
    path: PathBuf,
    cfg: GeckoConfig,
    status: ConfigState,
}

enum ConfigState {
    /// Parsed cleanly.
    Present,
    /// File absent.
    Missing,
    /// File present but could not be read/parsed (carries the reason).
    Broken(String),
}

/// Loads `gecko.toml` leniently — never errors. A missing or unparseable file
/// yields the default config so the *other* checks can still run; the reason is
/// carried in [`ConfigState`] for `check_config` to surface.
pub fn load_config_lenient(path: &Path) -> LoadedConfig {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str::<GeckoConfig>(&text) {
            Ok(cfg) => LoadedConfig {
                path: path.to_path_buf(),
                cfg,
                status: ConfigState::Present,
            },
            Err(e) => LoadedConfig {
                path: path.to_path_buf(),
                cfg: GeckoConfig::default(),
                status: ConfigState::Broken(e.to_string()),
            },
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => LoadedConfig {
            path: path.to_path_buf(),
            cfg: GeckoConfig::default(),
            status: ConfigState::Missing,
        },
        Err(e) => LoadedConfig {
            path: path.to_path_buf(),
            cfg: GeckoConfig::default(),
            status: ConfigState::Broken(e.to_string()),
        },
    }
}

// ── individual checks ───────────────────────────────────────────────────────

/// `config`: FAIL if `gecko.toml` is missing or won't parse.
fn check_config(loaded: &LoadedConfig) -> CheckResult {
    let p = loaded.path.display();
    match &loaded.status {
        ConfigState::Present => CheckResult::ok("config", format!("{p} present and valid")),
        ConfigState::Missing => {
            CheckResult::fail("config", format!("{p} not found"), Some("gecko init"))
        }
        ConfigState::Broken(why) => CheckResult::fail(
            "config",
            format!("{p} could not be parsed: {why}"),
            Some("gecko init"),
        ),
    }
}

/// `cache-dir`: FAIL if the cache root is unresolvable or not writable.
fn check_cache_dir(cfg: &GeckoConfig) -> CheckResult {
    match config::cache_dir(cfg) {
        Ok(dir) => {
            // Prove writability rather than assume it: the resolver created the
            // directory, but the mount could still be read-only.
            let probe = dir.join(".gecko-doctor-write-probe");
            match std::fs::write(&probe, b"ok") {
                Ok(()) => {
                    let _ = std::fs::remove_file(&probe);
                    CheckResult::ok("cache-dir", format!("{} (writable)", dir.display()))
                }
                Err(e) => CheckResult::fail(
                    "cache-dir",
                    format!("{} is not writable: {e}", dir.display()),
                    Some("check GECKO_CACHE_DIR (or [cache_dir] in gecko.toml)"),
                ),
            }
        }
        Err(e) => CheckResult::fail(
            "cache-dir",
            format!("cache root unresolvable: {e:#}"),
            Some("check GECKO_CACHE_DIR (or [cache_dir] in gecko.toml)"),
        ),
    }
}

/// Compares a live server version against the pinned one.
/// Equal ⇒ Ok; same major but strictly newer ⇒ Warn (compatible); else ⇒ Fail.
fn version_status(server: &str, pinned: &str) -> Status {
    if server == pinned {
        return Status::Ok;
    }
    match (parse_semver(server), parse_semver(pinned)) {
        (Some(s), Some(p)) if s.0 == p.0 && s > p => Status::Warn,
        _ => Status::Fail,
    }
}

/// Lenient `major.minor.patch` parse: numeric prefix of each dotted component,
/// tolerating suffixes like `-rc1`. `None` only if the major component is absent.
fn parse_semver(v: &str) -> Option<(u32, u32, u32)> {
    let num = |part: Option<&str>| -> u32 {
        part.unwrap_or("0")
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0)
    };
    let mut it = v.trim().split('.');
    let first = it.next()?;
    let major: u32 = first
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()?;
    Some((major, num(it.next()), num(it.next())))
}

/// `typedb`: FAIL if the endpoint is unreachable (orchestrated/compose mode) or
/// the version differs from pinned; WARN if newer-but-compatible, or if external
/// mode is simply not up yet (gecko manages nothing there).
async fn check_typedb(
    address: &str,
    username: &str,
    password: &str,
    tls: TlsMode,
    mode: TypedbMode,
) -> CheckResult {
    let dbcfg = DbConfig {
        address: address.to_string(),
        database: "gecko".to_string(),
        username: username.to_string(),
        password: password.to_string(),
        tls,
    };
    let mut router = TypeDbRouter::new(dbcfg);
    match router.server_version().await {
        Ok(version) => match version_status(&version, PINNED_TYPEDB) {
            Status::Ok => CheckResult::ok(
                "typedb",
                format!("reachable at {address}, version {version} (pinned)"),
            ),
            Status::Warn => CheckResult::warn(
                "typedb",
                format!(
                    "reachable at {address}, version {version} is newer than pinned {PINNED_TYPEDB} (compatible)"
                ),
                None,
            ),
            Status::Fail => CheckResult::fail(
                "typedb",
                format!("reachable at {address}, but version {version} ≠ pinned {PINNED_TYPEDB}"),
                Some(
                    "install/run the pinned TypeDB (gecko up), or align [typedb] with your server",
                ),
            ),
        },
        Err(e) => match mode {
            // External mode: gecko manages nothing, so an unreachable endpoint is
            // "your server isn't up yet", not a gecko misconfiguration — a warning.
            TypedbMode::External => CheckResult::warn(
                "typedb",
                format!("external mode: no TypeDB reachable at {address} ({e})"),
                Some("start your TypeDB and point [typedb] endpoint at it"),
            ),
            TypedbMode::Orchestrated => CheckResult::fail(
                "typedb",
                format!("orchestrated mode: no TypeDB reachable at {address} ({e})"),
                Some("gecko up"),
            ),
            TypedbMode::Compose => CheckResult::fail(
                "typedb",
                format!("compose mode: no TypeDB reachable at {address} ({e})"),
                Some("docker compose up -d"),
            ),
        },
    }
}

/// `model`: FAIL only when the model is genuinely required and missing — semantic
/// index enabled, a real embedder compiled and selected, the model absent, and
/// `auto_fetch=false`. WARN when the index is disabled, this is a stub build, the
/// stub embedder is selected, or `auto_fetch` will pull it on first use.
fn check_model(cfg: &GeckoConfig) -> CheckResult {
    let sic = &cfg.semantic_index;
    if !sic.enabled {
        return CheckResult::warn(
            "model",
            "semantic_index disabled — no embedding model required",
            None,
        );
    }

    #[cfg(not(feature = "real-embedder"))]
    {
        let _ = sic;
        CheckResult::warn(
            "model",
            "stub build (real-embedder not compiled) — recall uses the deterministic stub; no model file needed",
            Some("build --features real-embedder for real embedding"),
        )
    }

    #[cfg(feature = "real-embedder")]
    {
        if sic.embedder == "stub" {
            return CheckResult::warn(
                "model",
                "embedder = \"stub\" — deterministic hash embedder; no model file needed",
                None,
            );
        }
        if sic.model_id.contains("<revision>") {
            return CheckResult::fail(
                "model",
                format!(
                    "model_id '{}' has an unresolved '<revision>' placeholder",
                    sic.model_id
                ),
                Some("pin a real revision in gecko.toml, then run gecko model fetch"),
            );
        }
        let dir = match config::model_path(cfg) {
            Ok(d) => d,
            Err(e) => {
                return CheckResult::fail(
                    "model",
                    format!("cannot resolve the model directory: {e:#}"),
                    Some("check GECKO_CACHE_DIR / [semantic_index] model_path"),
                );
            }
        };
        if fetch::is_cached(&dir) {
            CheckResult::ok("model", format!("staged at {}", dir.display()))
        } else if sic.auto_fetch {
            CheckResult::warn(
                "model",
                format!(
                    "not staged at {} — auto_fetch=true will pull it on first use",
                    dir.display()
                ),
                Some("gecko model fetch (to pre-stage now)"),
            )
        } else {
            CheckResult::fail(
                "model",
                format!(
                    "embedding model not staged at {} (auto_fetch=false)",
                    dir.display()
                ),
                Some("gecko model fetch"),
            )
        }
    }
}

/// `index`: never FAILs (the index is derived and rebuilt from the graph). WARN
/// if the semantic index is enabled but the file is absent (rebuilt next run).
fn check_index(cfg: &GeckoConfig) -> CheckResult {
    if !cfg.semantic_index.enabled {
        return CheckResult::ok("index", "semantic_index disabled — no index expected");
    }
    match config::index_path(cfg) {
        Ok(path) if path.exists() => {
            CheckResult::ok("index", format!("present at {}", path.display()))
        }
        Ok(path) => CheckResult::warn(
            "index",
            format!(
                "index file absent at {} — rebuilt from the graph on next run",
                path.display()
            ),
            None,
        ),
        Err(e) => CheckResult::warn(
            "index",
            format!("cannot resolve the index path: {e:#} — rebuilt from the graph on next run"),
            None,
        ),
    }
}

/// `embedder`: WARN on a stub build (fine for dev), Ok when the real embedder is
/// compiled in. Never FAILs.
fn check_embedder() -> CheckResult {
    #[cfg(feature = "real-embedder")]
    {
        CheckResult::ok(
            "embedder",
            "real-embedder compiled (candle-backed CandleEmbedder available)",
        )
    }
    #[cfg(not(feature = "real-embedder"))]
    {
        CheckResult::warn(
            "embedder",
            "stub build — the candle embedder is not compiled (fine for dev)",
            Some("build --features real-embedder for real embedding"),
        )
    }
}

// ── driver ──────────────────────────────────────────────────────────────────

/// Runs the requested checks and returns the process exit code (FAILURE iff any
/// check is `Fail`). `only` restricts to a single named check; `json` serializes;
/// `quiet` suppresses all normal output (only the exit code is meaningful).
pub async fn run_doctor(cli: &Cli, only: Option<&str>, json: bool, quiet: bool) -> ExitCode {
    if let Some(name) = only
        && !CHECK_NAMES.contains(&name)
    {
        if !quiet {
            eprintln!("unknown check '{name}' (valid: {})", CHECK_NAMES.join(", "));
        }
        return ExitCode::FAILURE;
    }

    let loaded = load_config_lenient(&cli.config);
    let cfg = &loaded.cfg;
    let want = |name: &str| only.is_none_or(|o| o == name);

    // Only run the checks that were asked for — importantly, `--check model` (used
    // by setup.sh) must NOT connect to TypeDB, and `--check config` must not do
    // any network I/O. So we run conditionally rather than run-all-then-filter.
    let mut checks: Vec<CheckResult> = Vec::new();
    if want("config") {
        checks.push(check_config(&loaded));
    }
    if want("cache-dir") {
        checks.push(check_cache_dir(cfg));
    }
    if want("typedb") {
        let tls = if cli.tls {
            TlsMode::Enabled {
                ca_cert: cli.ca_cert.clone(),
            }
        } else {
            TlsMode::Disabled
        };
        let address = cli
            .address
            .clone()
            .unwrap_or_else(|| cfg.typedb.endpoint.clone());
        checks
            .push(check_typedb(&address, &cli.username, &cli.password, tls, cfg.typedb.mode).await);
    }
    if want("model") {
        checks.push(check_model(cfg));
    }
    if want("index") {
        checks.push(check_index(cfg));
    }
    if want("embedder") {
        checks.push(check_embedder());
    }

    render(&checks, json, quiet);

    if checks.iter().any(|c| c.status == Status::Fail) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Renders the results: `--json` serializes the array; `--quiet` prints nothing;
/// otherwise aligned `✓/⚠/✗ <name>  <detail>` with a remediation line for non-Ok.
fn render(checks: &[CheckResult], json: bool, quiet: bool) {
    if quiet {
        return;
    }
    if json {
        match serde_json::to_string_pretty(checks) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("failed to serialize doctor results: {e}"),
        }
        return;
    }
    let width = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    for c in checks {
        let sym = match c.status {
            Status::Ok => "✓",
            Status::Warn => "⚠",
            Status::Fail => "✗",
        };
        println!("{sym} {:<width$}  {}", c.name, c.detail, width = width);
        if c.status != Status::Ok
            && let Some(fix) = &c.fix
        {
            println!("  {:<width$}  → fix: {fix}", "", width = width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SemanticIndexConfig;

    #[test]
    fn version_status_equal_is_ok() {
        assert_eq!(version_status("3.12.0", "3.12.0"), Status::Ok);
    }

    #[test]
    fn version_status_newer_same_major_is_warn() {
        assert_eq!(version_status("3.13.0", "3.12.0"), Status::Warn);
        assert_eq!(version_status("3.12.1", "3.12.0"), Status::Warn);
    }

    #[test]
    fn version_status_older_or_different_major_is_fail() {
        assert_eq!(version_status("3.11.5", "3.12.0"), Status::Fail);
        assert_eq!(version_status("4.0.0", "3.12.0"), Status::Fail);
        assert_eq!(version_status("2.99.99", "3.12.0"), Status::Fail);
        assert_eq!(version_status("garbage", "3.12.0"), Status::Fail);
    }

    #[test]
    fn parse_semver_tolerates_suffixes_and_short_forms() {
        assert_eq!(parse_semver("3.12.0"), Some((3, 12, 0)));
        assert_eq!(parse_semver("3.12"), Some((3, 12, 0)));
        assert_eq!(parse_semver("3"), Some((3, 0, 0)));
        assert_eq!(parse_semver("3.12.0-rc1"), Some((3, 12, 0)));
        assert_eq!(parse_semver("nope"), None);
    }

    #[test]
    fn check_config_present_missing_broken() {
        let tmp = tempfile::tempdir().unwrap();

        // Missing file → Fail with the `gecko init` remedy.
        let missing = tmp.path().join("absent.toml");
        let r = check_config(&load_config_lenient(&missing));
        assert_eq!(r.status, Status::Fail);
        assert_eq!(r.fix.as_deref(), Some("gecko init"));

        // Present + valid → Ok.
        let good = tmp.path().join("good.toml");
        std::fs::write(&good, "[extensions]\nenabled = []\n").unwrap();
        assert_eq!(check_config(&load_config_lenient(&good)).status, Status::Ok);

        // Present + unparseable → Fail (lenient load must NOT panic).
        let bad = tmp.path().join("bad.toml");
        std::fs::write(&bad, "this is = = not toml [[[").unwrap();
        let r = check_config(&load_config_lenient(&bad));
        assert_eq!(r.status, Status::Fail);
        assert!(r.detail.contains("could not be parsed"));
    }

    #[test]
    fn check_cache_dir_ok_for_writable_root() {
        // Point the cache root (via config) at a fresh temp dir — resolvable and
        // writable → Ok. (Uses the config `cache_dir` key, no env mutation.)
        let tmp = tempfile::tempdir().unwrap();
        let cfg = GeckoConfig {
            cache_dir: Some(tmp.path().join("cache").to_string_lossy().into_owned()),
            ..Default::default()
        };
        assert_eq!(check_cache_dir(&cfg).status, Status::Ok);
    }

    #[test]
    fn check_model_warns_when_index_disabled() {
        let cfg = GeckoConfig::default(); // semantic_index disabled
        let r = check_model(&cfg);
        assert_eq!(r.status, Status::Warn);
    }

    #[test]
    fn check_index_ok_when_disabled_and_warn_when_missing() {
        let cfg = GeckoConfig::default();
        assert_eq!(check_index(&cfg).status, Status::Ok);

        let tmp = tempfile::tempdir().unwrap();
        let cfg = GeckoConfig {
            semantic_index: SemanticIndexConfig {
                enabled: true,
                path: tmp.path().join("nope.hnsw").to_string_lossy().into_owned(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(check_index(&cfg).status, Status::Warn);
    }

    #[test]
    fn check_embedder_matches_build_flavor() {
        let r = check_embedder();
        assert_ne!(r.status, Status::Fail); // never fails
        #[cfg(feature = "real-embedder")]
        assert_eq!(r.status, Status::Ok);
        #[cfg(not(feature = "real-embedder"))]
        assert_eq!(r.status, Status::Warn);
    }

    #[cfg(feature = "real-embedder")]
    #[test]
    fn check_model_fails_when_required_and_absent() {
        // Enabled + real embedder selected + real revision pinned + not staged +
        // auto_fetch=false ⇒ the one hard Fail case.
        let tmp = tempfile::tempdir().unwrap();
        let cfg = GeckoConfig {
            cache_dir: Some(tmp.path().to_string_lossy().into_owned()),
            semantic_index: SemanticIndexConfig {
                enabled: true,
                embedder: "bge-large-en-v1.5".to_string(),
                model_id: "bge-large-en-v1.5@deadbeef".to_string(),
                auto_fetch: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let r = check_model(&cfg);
        assert_eq!(r.status, Status::Fail);
        assert_eq!(r.fix.as_deref(), Some("gecko model fetch"));
    }

    /// The `typedb` Fail path against a dead endpoint — hermetic (binds then
    /// drops an ephemeral port, never touching 1729). Orchestrated mode ⇒ Fail
    /// with the `gecko up` remedy; external mode ⇒ Warn (server is the user's).
    #[tokio::test]
    async fn check_typedb_dead_endpoint_fail_orchestrated_warn_external() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let addr = format!("127.0.0.1:{port}");

        let orch = check_typedb(
            &addr,
            "admin",
            "password",
            TlsMode::Disabled,
            TypedbMode::Orchestrated,
        )
        .await;
        assert_eq!(orch.status, Status::Fail);
        assert_eq!(orch.fix.as_deref(), Some("gecko up"));

        let ext = check_typedb(
            &addr,
            "admin",
            "password",
            TlsMode::Disabled,
            TypedbMode::External,
        )
        .await;
        assert_eq!(ext.status, Status::Warn);
    }
}
