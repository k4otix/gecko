//! Managed TypeDB child-process orchestration — the `orchestrated` run mode (P3).
//!
//! `gecko up` (orchestrated) downloads a **hard-pinned** TypeDB server once into
//! the runtime cache, spawns it as a managed child on the configured port with a
//! data dir under the cache, waits for driver readiness, and records the child
//! PID in a pidfile. `gecko down` reads that pidfile and stops the child
//! (SIGTERM → wait → SIGKILL). The user runs `gecko up` + `gecko sync` and never
//! touches a database directly — without forcing Docker on anyone.
//!
//! Load-bearing rule (mirrors the model asset): the pinned version is a
//! **discriminator, not a moving target** ([`PINNED_TYPEDB`]) — bump it
//! deliberately, never float to `latest`. Every downloaded byte and the server's
//! data dir live under the cache root, OUTSIDE the build tree.
//!
//! The confirmed 3.12.0 launch invocation (verified against the local
//! `~/.typedb/server/typedb_server_bin --help` and a real throwaway-port spawn):
//! ```text
//! typedb_server_bin \
//!   --server.listen-address 127.0.0.1:<port> \
//!   --storage.data-directory <cache>/typedb/<ver>/data \
//!   --server.http.enabled false \
//!   --diagnostics.monitoring.enabled false \
//!   --diagnostics.reporting.metrics false \
//!   --diagnostics.reporting.errors false
//! ```
//! HTTP/monitoring are disabled so a managed child never collides with another
//! TypeDB's auxiliary ports (8000 / 4104). The readiness probe is "construct a
//! `TypeDBDriver` + `databases().all()` succeeds", retried with backoff until a
//! timeout (the driver has no dedicated health RPC — a successful listing IS the
//! health signal).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};

/// The hard-pinned TypeDB version GECKO orchestrates. A discriminator (like
/// `model_id`), NOT a moving target: bump deliberately, never float to latest.
pub const PINNED_TYPEDB: &str = "3.12.0";

/// TypeDB's public package repository — the raw-artifact download host.
const DIST_HOST: &str = "https://repo.typedb.com/public/public-release/raw/names";

/// How long `up` waits for the freshly-spawned server to become ready.
const READY_TIMEOUT: Duration = Duration::from_secs(60);
/// Delay between readiness probes.
const PROBE_INTERVAL: Duration = Duration::from_millis(500);
/// How long `down` waits for a graceful SIGTERM before escalating to SIGKILL.
const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(15);

/// What `up` did (the caller renders a message from this).
#[derive(Debug, PartialEq, Eq)]
pub enum UpOutcome {
    /// The endpoint was already serving (idempotent no-op — this is what `just up`
    /// / `setup.sh` rely on).
    AlreadyRunning,
    /// A managed child was spawned and reached readiness. Carries its PID.
    Started { pid: u32 },
}

/// What `down` did.
#[derive(Debug, PartialEq, Eq)]
pub enum DownOutcome {
    /// No managed child was running (no live pidfile) — a no-op.
    NotRunning,
    /// The managed child was stopped. Carries the PID and whether SIGKILL was
    /// needed (graceful == false).
    Stopped { pid: u32, graceful: bool },
}

// ── Pure platform / URL / path derivations (unit-tested) ────────────────────

/// Maps a Rust `target_os` / `target_arch` pair to TypeDB's dist platform token
/// (e.g. `mac-arm64`, `linux-x86_64`). Errors on an unsupported platform rather
/// than guessing an artifact that does not exist.
fn artifact_platform(os: &str, arch: &str) -> Result<String> {
    let os_tok = match os {
        "macos" => "mac",
        "linux" => "linux",
        "windows" => "windows",
        other => {
            bail!("unsupported OS for orchestrated TypeDB: {other} (use compose/external mode)")
        }
    };
    let arch_tok = match arch {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        other => {
            bail!(
                "unsupported CPU arch for orchestrated TypeDB: {other} (use compose/external mode)"
            )
        }
    };
    Ok(format!("{os_tok}-{arch_tok}"))
}

/// The archive extension for a platform: mac/windows ship `.zip`, linux `.tar.gz`.
fn artifact_ext(os: &str) -> &'static str {
    match os {
        "linux" => "tar.gz",
        _ => "zip",
    }
}

/// Builds the pinned dist download URL for the given platform:
/// `<host>/typedb-all-<platform>/versions/<ver>/typedb-all-<platform>-<ver>.<ext>`.
fn dist_url(version: &str, os: &str, arch: &str) -> Result<String> {
    let platform = artifact_platform(os, arch)?;
    let ext = artifact_ext(os);
    Ok(format!(
        "{DIST_HOST}/typedb-all-{platform}/versions/{version}/typedb-all-{platform}-{version}.{ext}"
    ))
}

/// The dist URL for the host platform this binary was built for.
fn host_dist_url(version: &str) -> Result<String> {
    dist_url(version, std::env::consts::OS, std::env::consts::ARCH)
}

/// `<cache>/typedb/<version>/` — the root of one pinned install.
fn version_dir(cache_root: &Path, version: &str) -> PathBuf {
    cache_root.join("typedb").join(version)
}
/// Where the extracted server distribution lives.
fn dist_dir(cache_root: &Path, version: &str) -> PathBuf {
    version_dir(cache_root, version).join("dist")
}
/// The server's RocksDB data directory (never collides with an external server's).
fn data_dir(cache_root: &Path, version: &str) -> PathBuf {
    version_dir(cache_root, version).join("data")
}
/// The pidfile recording the managed child's PID.
fn pidfile_path(cache_root: &Path, version: &str) -> PathBuf {
    version_dir(cache_root, version).join("typedb.pid")
}
/// The managed child's stdout/stderr log.
fn logfile_path(cache_root: &Path, version: &str) -> PathBuf {
    version_dir(cache_root, version).join("typedb.log")
}

/// Extracts the port from a `host:port` endpoint. Orchestrated mode binds the
/// managed child to `127.0.0.1:<port>` (local-only).
fn port_of(endpoint: &str) -> Result<u16> {
    let port = endpoint
        .rsplit_once(':')
        .map(|(_, p)| p)
        .with_context(|| format!("endpoint '{endpoint}' is missing a ':<port>'"))?;
    port.parse::<u16>()
        .with_context(|| format!("endpoint '{endpoint}' has a non-numeric port '{port}'"))
}

// ── Dist acquisition ────────────────────────────────────────────────────────

/// Locates the `typedb_server_bin` under an extracted dist dir (the archive
/// nests it under `.../server/`), searching a couple of levels deep.
fn find_server_bin(dist: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, depth: usize) -> Option<PathBuf> {
        if depth == 0 {
            return None;
        }
        let entries = fs::read_dir(dir).ok()?;
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                subdirs.push(path);
            } else if path.file_name().is_some_and(|n| n == "typedb_server_bin") {
                return Some(path);
            }
        }
        for sub in subdirs {
            if let Some(found) = walk(&sub, depth - 1) {
                return Some(found);
            }
        }
        None
    }
    walk(dist, 4)
}

/// Ensures the pinned server binary is present under the cache, downloading and
/// extracting the platform dist on first run. Returns the server-binary path.
/// Idempotent: a subsequent call with the bin already extracted is a no-op.
fn ensure_present(cache_root: &Path, version: &str) -> Result<PathBuf> {
    let dist = dist_dir(cache_root, version);
    if let Some(bin) = find_server_bin(&dist) {
        return Ok(bin);
    }

    let url = host_dist_url(version)?;
    let ext = artifact_ext(std::env::consts::OS);
    let vdir = version_dir(cache_root, version);
    fs::create_dir_all(&vdir)
        .with_context(|| format!("cannot create TypeDB cache dir {}", vdir.display()))?;
    let archive = vdir.join(format!("typedb-all.{ext}"));

    println!(
        "Downloading pinned TypeDB {version} → {}",
        archive.display()
    );
    println!("  Source: {url}");
    download(&url, &archive)
        .with_context(|| format!("failed to download TypeDB dist from {url}"))?;

    fs::create_dir_all(&dist)
        .with_context(|| format!("cannot create dist dir {}", dist.display()))?;
    extract(&archive, &dist, ext)
        .with_context(|| format!("failed to extract {}", archive.display()))?;
    // The archive is no longer needed once extracted.
    let _ = fs::remove_file(&archive);

    let bin = find_server_bin(&dist).with_context(|| {
        format!(
            "extracted TypeDB dist but no `typedb_server_bin` found under {}",
            dist.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&bin)?.permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(&bin, perms);
    }
    Ok(bin)
}

/// Streams a URL to a file with a blocking HTTP client (matches the model
/// fetcher's `ureq` dependency).
fn download(url: &str, dest: &Path) -> Result<()> {
    let resp = ureq::get(url)
        .call()
        .with_context(|| format!("HTTP GET {url} failed"))?;
    let mut reader = resp.into_reader();
    let mut file =
        fs::File::create(dest).with_context(|| format!("cannot create {}", dest.display()))?;
    std::io::copy(&mut reader, &mut file).context("write of downloaded dist failed")?;
    Ok(())
}

/// Extracts a `.tar.gz` or `.zip` archive into `dest`.
fn extract(archive: &Path, dest: &Path, ext: &str) -> Result<()> {
    let file = fs::File::open(archive)
        .with_context(|| format!("cannot open archive {}", archive.display()))?;
    if ext == "tar.gz" {
        let gz = flate2::read::GzDecoder::new(file);
        let mut ar = tar::Archive::new(gz);
        ar.unpack(dest).context("tar.gz extraction failed")?;
    } else {
        let mut zip = zip::ZipArchive::new(file).context("opening zip failed")?;
        zip.extract(dest).context("zip extraction failed")?;
    }
    Ok(())
}

// ── Process lifecycle ───────────────────────────────────────────────────────

/// Spawns the server as a detached child (stdout/stderr → the logfile). Returns
/// its PID. The child intentionally OUTLIVES this process: `gecko up` exits, the
/// orphaned server keeps serving, and `gecko down` finds it via the pidfile.
fn spawn_server(server_bin: &Path, port: u16, data: &Path, log: &Path) -> Result<u32> {
    let log_out = fs::File::create(log)
        .with_context(|| format!("cannot create server log {}", log.display()))?;
    let log_err = log_out
        .try_clone()
        .context("cannot duplicate server log handle")?;

    let child = Command::new(server_bin)
        .arg("--server.listen-address")
        .arg(format!("127.0.0.1:{port}"))
        .arg("--storage.data-directory")
        .arg(data)
        // Disable the auxiliary HTTP + monitoring ports so a managed child never
        // collides with another TypeDB's 8000 / 4104.
        .arg("--server.http.enabled")
        .arg("false")
        .arg("--diagnostics.monitoring.enabled")
        .arg("false")
        .arg("--diagnostics.reporting.metrics")
        .arg("false")
        .arg("--diagnostics.reporting.errors")
        .arg("false")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err))
        .spawn()
        .with_context(|| format!("failed to spawn {}", server_bin.display()))?;

    Ok(child.id())
}

/// One readiness probe: connect the driver and list databases. A successful
/// listing is the health signal (the driver exposes no dedicated health RPC).
async fn probe_once(endpoint: &str, username: &str, password: &str) -> bool {
    let config = DbConfig {
        address: endpoint.to_string(),
        database: "gecko".to_string(),
        username: username.to_string(),
        password: password.to_string(),
        tls: TlsMode::Disabled,
    };
    let mut router = TypeDbRouter::new(config);
    router.list_databases().await.is_ok()
}

/// Retries the readiness probe until it succeeds or `timeout` elapses.
async fn wait_ready(
    endpoint: &str,
    username: &str,
    password: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if probe_once(endpoint, username, password).await {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "TypeDB did not become ready at {endpoint} within {}s",
                timeout.as_secs()
            );
        }
        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

#[cfg(unix)]
fn signal(pid: u32, sig: i32) -> bool {
    // SAFETY: `kill(2)` with a valid pid is a simple syscall; sig 0 only checks
    // existence. A stale/reused pid at worst returns an error we treat as false.
    unsafe { libc::kill(pid as libc::pid_t, sig) == 0 }
}

/// Whether a process with `pid` currently exists (signal 0 is the POSIX
/// existence check).
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    signal(pid, 0)
}
#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    false
}

/// The command-name fragment the identity guard looks for in a live PID's
/// command line before treating it as "the TypeDB server we spawned".
const TYPEDB_SERVER_BIN_NAME: &str = "typedb_server_bin";

/// Identity guard for `down()`: a PID existing is NOT enough evidence that it
/// is the TypeDB child this orchestrator spawned — PIDs get recycled, and a
/// long-lived pidfile could now point at an unrelated process. Shells out to
/// `ps -p <pid> -o command=` (portable across macOS + Linux, unlike
/// `/proc/<pid>/cmdline`) and checks the command string for the known server
/// binary name. Any ambiguity (ps missing, empty output, pid raced away)
/// resolves to `false` — the caller must then treat the pidfile as stale
/// rather than ever signal a process it cannot positively identify.
#[cfg(unix)]
fn pid_is_typedb_server(pid: u32) -> bool {
    let output = Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .arg("-o")
        .arg("command=")
        .output();
    match output {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).contains(TYPEDB_SERVER_BIN_NAME)
        }
        _ => false,
    }
}
#[cfg(not(unix))]
fn pid_is_typedb_server(_pid: u32) -> bool {
    false
}

fn write_pidfile(path: &Path, pid: u32) -> Result<()> {
    fs::write(path, pid.to_string())
        .with_context(|| format!("cannot write pidfile {}", path.display()))
}

/// Reads a PID from a pidfile, returning `None` if absent or unparseable.
fn read_pidfile(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse::<u32>().ok()
}

// ── Public entry points ─────────────────────────────────────────────────────

/// `gecko up` in orchestrated mode. Idempotent: if the endpoint is already
/// serving, no-op; otherwise ensure the pinned server is present, spawn it, wait
/// for readiness, and record its PID.
pub async fn up(
    cache_root: &Path,
    endpoint: &str,
    username: &str,
    password: &str,
) -> Result<UpOutcome> {
    // Idempotency first: if anything is already serving the endpoint (a prior
    // `gecko up`, or an externally-run server on this port), do nothing. This is
    // what `just up` / `setup.sh` rely on, and it means `gecko up` never disturbs
    // a server it did not start.
    if probe_once(endpoint, username, password).await {
        return Ok(UpOutcome::AlreadyRunning);
    }

    let port = port_of(endpoint)?;
    let server_bin = ensure_present(cache_root, PINNED_TYPEDB)?;
    let data = data_dir(cache_root, PINNED_TYPEDB);
    fs::create_dir_all(&data)
        .with_context(|| format!("cannot create data dir {}", data.display()))?;
    let log = logfile_path(cache_root, PINNED_TYPEDB);

    let pid = spawn_server(&server_bin, port, &data, &log)?;
    write_pidfile(&pidfile_path(cache_root, PINNED_TYPEDB), pid)?;

    match wait_ready(endpoint, username, password, READY_TIMEOUT).await {
        Ok(()) => Ok(UpOutcome::Started { pid }),
        Err(e) => {
            let context = cleanup_after_ready_timeout(cache_root, pid, &log);
            Err(e.context(context))
        }
    }
}

/// Called when a just-spawned child never reaches readiness: never leave it
/// orphaned. Stops it via the same path `down()` uses and, only once the stop
/// actually ran, removes the pidfile (so a stop failure does not erase the
/// only record of a still-running process). Returns the context string to
/// attach to the readiness error.
fn cleanup_after_ready_timeout(cache_root: &Path, pid: u32, log: &Path) -> String {
    if stop_process(pid).is_ok() {
        let _ = fs::remove_file(&pidfile_path(cache_root, PINNED_TYPEDB));
        format!(
            "spawned TypeDB (pid {pid}) never became ready within the timeout; \
             it has been stopped so it is not left orphaned. See the log at {}",
            log.display()
        )
    } else {
        format!(
            "spawned TypeDB (pid {pid}) never became ready within the timeout, \
             AND it could not be stopped automatically — you may need to stop \
             it manually. See the log at {}",
            log.display()
        )
    }
}

/// `gecko down` in orchestrated mode. Stops the managed child (SIGTERM → wait →
/// SIGKILL) and removes the pidfile. No-op if nothing is running.
pub fn down(cache_root: &Path) -> Result<DownOutcome> {
    let pidfile = pidfile_path(cache_root, PINNED_TYPEDB);
    let Some(pid) = read_pidfile(&pidfile) else {
        return Ok(DownOutcome::NotRunning);
    };
    if !process_alive(pid) {
        // Stale pidfile (server already gone) — clean it up.
        let _ = fs::remove_file(&pidfile);
        return Ok(DownOutcome::NotRunning);
    }
    if !pid_is_typedb_server(pid) {
        // The PID exists but does not look like the TypeDB server we spawned
        // — almost certainly a recycled PID now held by an unrelated process.
        // Never signal it: treat the pidfile as stale instead (same cleanup
        // as the dead-PID path above).
        let _ = fs::remove_file(&pidfile);
        return Ok(DownOutcome::NotRunning);
    }

    stop_process(pid)?;
    let _ = fs::remove_file(&pidfile);
    Ok(DownOutcome::Stopped {
        pid,
        graceful: process_stopped_gracefully(pid),
    })
}

/// SIGTERM the process, wait up to the graceful timeout, then SIGKILL. Returns
/// once the process is gone (or a best-effort SIGKILL was sent).
#[cfg(unix)]
fn stop_process(pid: u32) -> Result<()> {
    signal(pid, libc::SIGTERM);
    let deadline = Instant::now() + GRACEFUL_STOP_TIMEOUT;
    while process_alive(pid) {
        if Instant::now() >= deadline {
            signal(pid, libc::SIGKILL);
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}
#[cfg(not(unix))]
fn stop_process(_pid: u32) -> Result<()> {
    bail!("orchestrated mode is only supported on Unix; use compose or external mode")
}

/// Records (for the [`DownOutcome`]) whether the process exited before the
/// graceful deadline. Called after [`stop_process`] returns, so a still-alive
/// pid means SIGKILL was used.
fn process_stopped_gracefully(pid: u32) -> bool {
    // Give a moment for a SIGKILL'd process to be reaped, then check.
    for _ in 0..10 {
        if !process_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_platform_maps_known_targets() {
        assert_eq!(artifact_platform("macos", "aarch64").unwrap(), "mac-arm64");
        assert_eq!(artifact_platform("macos", "x86_64").unwrap(), "mac-x86_64");
        assert_eq!(
            artifact_platform("linux", "x86_64").unwrap(),
            "linux-x86_64"
        );
        assert_eq!(
            artifact_platform("linux", "aarch64").unwrap(),
            "linux-arm64"
        );
        assert!(artifact_platform("freebsd", "x86_64").is_err());
        assert!(artifact_platform("linux", "riscv64").is_err());
    }

    #[test]
    fn artifact_ext_is_zip_for_mac_targz_for_linux() {
        assert_eq!(artifact_ext("macos"), "zip");
        assert_eq!(artifact_ext("windows"), "zip");
        assert_eq!(artifact_ext("linux"), "tar.gz");
    }

    #[test]
    fn dist_url_matches_the_pinned_scheme() {
        // The exact scheme verified live (HEAD 200) against repo.typedb.com.
        assert_eq!(
            dist_url("3.12.0", "macos", "aarch64").unwrap(),
            "https://repo.typedb.com/public/public-release/raw/names/\
             typedb-all-mac-arm64/versions/3.12.0/typedb-all-mac-arm64-3.12.0.zip"
        );
        assert_eq!(
            dist_url("3.12.0", "linux", "x86_64").unwrap(),
            "https://repo.typedb.com/public/public-release/raw/names/\
             typedb-all-linux-x86_64/versions/3.12.0/typedb-all-linux-x86_64-3.12.0.tar.gz"
        );
    }

    #[test]
    fn pinned_version_is_3_12_0() {
        // The pin is a hard discriminator — this test guards against an
        // accidental float.
        assert_eq!(PINNED_TYPEDB, "3.12.0");
    }

    #[test]
    fn path_derivations_live_under_the_cache_root() {
        let cache = Path::new("/cache/gecko");
        assert_eq!(
            version_dir(cache, "3.12.0"),
            PathBuf::from("/cache/gecko/typedb/3.12.0")
        );
        assert_eq!(
            data_dir(cache, "3.12.0"),
            PathBuf::from("/cache/gecko/typedb/3.12.0/data")
        );
        assert_eq!(
            dist_dir(cache, "3.12.0"),
            PathBuf::from("/cache/gecko/typedb/3.12.0/dist")
        );
        assert_eq!(
            pidfile_path(cache, "3.12.0"),
            PathBuf::from("/cache/gecko/typedb/3.12.0/typedb.pid")
        );
    }

    #[test]
    fn port_of_parses_host_port() {
        assert_eq!(port_of("localhost:1729").unwrap(), 1729);
        assert_eq!(port_of("127.0.0.1:17298").unwrap(), 17298);
        assert!(port_of("localhost").is_err());
        assert!(port_of("localhost:notaport").is_err());
    }

    #[test]
    fn pidfile_roundtrips_and_missing_reads_none() {
        let tmp = tempfile::tempdir().unwrap();
        let pf = tmp.path().join("typedb.pid");
        assert_eq!(read_pidfile(&pf), None);
        write_pidfile(&pf, 424242).unwrap();
        assert_eq!(read_pidfile(&pf), Some(424242));
        // Own PID is alive; an implausible PID is not.
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(2_000_000_000));
    }

    #[test]
    fn down_is_noop_without_pidfile() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(down(tmp.path()).unwrap(), DownOutcome::NotRunning);
    }

    #[test]
    fn down_cleans_a_stale_pidfile() {
        let tmp = tempfile::tempdir().unwrap();
        let vdir = version_dir(tmp.path(), PINNED_TYPEDB);
        fs::create_dir_all(&vdir).unwrap();
        let pf = pidfile_path(tmp.path(), PINNED_TYPEDB);
        // A dead PID → down is a no-op AND removes the stale file.
        write_pidfile(&pf, 2_000_000_000).unwrap();
        assert_eq!(down(tmp.path()).unwrap(), DownOutcome::NotRunning);
        assert!(!pf.exists(), "stale pidfile should be cleaned up");
    }

    // ── FIX 1: PID-identity guard ────────────────────────────────────────────

    #[test]
    #[cfg(unix)]
    fn down_treats_a_live_non_typedb_pid_as_stale_and_never_signals_it() {
        // A pidfile can outlive the process it named if the PID gets recycled.
        // Point it at OUR OWN test process (definitely alive, definitely not
        // `typedb_server_bin`) and confirm `down()` refuses to signal it: it
        // must clean the pidfile up as stale instead of sending SIGTERM/KILL.
        let tmp = tempfile::tempdir().unwrap();
        let vdir = version_dir(tmp.path(), PINNED_TYPEDB);
        fs::create_dir_all(&vdir).unwrap();
        let pf = pidfile_path(tmp.path(), PINNED_TYPEDB);
        let own_pid = std::process::id();
        write_pidfile(&pf, own_pid).unwrap();

        assert!(!pid_is_typedb_server(own_pid));
        assert_eq!(down(tmp.path()).unwrap(), DownOutcome::NotRunning);
        assert!(
            !pf.exists(),
            "pidfile for a recycled/foreign PID should be cleaned up as stale"
        );
        // The real proof: we are still here to make this assertion — `down()`
        // did not signal us.
        assert!(
            process_alive(own_pid),
            "down() must never signal a PID that is not the TypeDB server"
        );
    }

    // ── FIX 2: dead-port readiness-timeout test ──────────────────────────────

    #[tokio::test]
    async fn wait_ready_times_out_quickly_against_a_dead_port() {
        // Bind an ephemeral port then drop the listener: the OS hands back a
        // port with nothing listening on it, without ever touching 1729 or any
        // other real endpoint.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let endpoint = format!("127.0.0.1:{port}");

        let start = Instant::now();
        // Low, test-only timeout (the injectable `timeout` param on
        // `wait_ready`) so this runs in a couple of seconds, not 60s.
        let result = wait_ready(&endpoint, "user", "pass", Duration::from_secs(2)).await;
        let elapsed = start.elapsed();

        let err = result.expect_err("probing a dead port must fail, never hang or succeed");
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("ready") || msg.contains("timeout"),
            "error should mention readiness/timeout; got: {msg}"
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "the timeout must be short and bounded; took {elapsed:?}"
        );
    }

    // ── FIX 3: don't orphan the child on readiness timeout ───────────────────

    #[test]
    #[cfg(unix)]
    fn ready_timeout_cleanup_stops_the_child_and_clears_the_pidfile() {
        // Exercises the up()-timeout path (`cleanup_after_ready_timeout`)
        // without needing a real TypeDB binary or network: spawn a long-lived
        // child directly, pretend it never became ready, and confirm the
        // cleanup helper stops it (not left orphaned) and removes the pidfile.
        let tmp = tempfile::tempdir().unwrap();
        let vdir = version_dir(tmp.path(), PINNED_TYPEDB);
        fs::create_dir_all(&vdir).unwrap();
        let pf = pidfile_path(tmp.path(), PINNED_TYPEDB);
        let log = logfile_path(tmp.path(), PINNED_TYPEDB);

        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("failed to spawn `sleep 30` for the test");
        let pid = child.id();
        write_pidfile(&pf, pid).unwrap();
        assert!(process_alive(pid), "test child should start out alive");

        // A `std::process::Child` that nobody `.wait()`s on becomes a zombie
        // once it exits — and `kill(pid, 0)` still reports a zombie as
        // "alive". In production that's harmless (the short-lived `gecko`
        // process exits right after, and the zombie gets reaped/reparented),
        // but this test process is long-lived, so reap concurrently on a
        // background thread — exactly what lets `process_alive` observe the
        // child as truly gone once `stop_process`'s SIGTERM lands.
        let reaper = std::thread::spawn(move || {
            let _ = child.wait();
        });

        let context = cleanup_after_ready_timeout(tmp.path(), pid, &log);
        assert!(context.contains("stopped"), "got: {context}");

        reaper.join().expect("reaper thread panicked");
        assert!(
            !process_alive(pid),
            "a never-ready spawn must be stopped, not left orphaned"
        );
        assert!(
            !pf.exists(),
            "the pidfile should be cleared once the child is stopped"
        );
    }
}
