//! `gecko up`/`gecko down` run-mode routing, driven through the
//! real `gecko` binary (`CARGO_BIN_EXE_gecko`).
//!
//! These cover the paths that are SAFE to run anywhere and never spawn or
//! download a server:
//!   - `compose` / `external` mode: `up`/`down` print guidance only, no process.
//!   - `orchestrated` `down` with an empty cache: a clean no-op (never downloads).
//!   - `orchestrated` `up` against the ALREADY-RUNNING server on localhost:1729
//!     is idempotent (`AlreadyRunning`) — GUARDED so it is skipped (never triggers
//!     a 100MB download) when no server is reachable, e.g. on a bare CI runner.
//!
//! The full orchestrated download+spawn on a throwaway non-1729 port is validated
//! out-of-band — it is deliberately not baked into the per-push suite, which must
//! not pull the TypeDB dist.

use std::io::Write;
use std::net::TcpStream;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Writes a throwaway `gecko.toml` with the given `[typedb] mode` + endpoint.
fn write_config(dir: &Path, mode: &str, endpoint: &str) -> std::path::PathBuf {
    let path = dir.join("gecko.toml");
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(
        f,
        "[typedb]\nendpoint = \"{endpoint}\"\nmode = \"{mode}\"\n"
    )
    .unwrap();
    path
}

/// Runs `gecko <args...>` with an isolated cache dir; returns (stdout, success).
fn run_gecko(config: &Path, cache: &Path, args: &[&str]) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_gecko"))
        .arg("--config")
        .arg(config)
        .args(args)
        .env("GECKO_CACHE_DIR", cache)
        .output()
        .expect("failed to run gecko binary");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    (stdout, out.status.success())
}

/// True if the already-running local server on localhost:1729 is reachable.
fn server_1729_up() -> bool {
    TcpStream::connect_timeout(
        &"127.0.0.1:1729".parse().unwrap(),
        Duration::from_millis(500),
    )
    .is_ok()
}

#[test]
fn up_external_prints_guidance_and_does_not_spawn() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = write_config(tmp.path(), "external", "localhost:1729");
    let (out, ok) = run_gecko(&cfg, tmp.path(), &["up"]);
    assert!(ok, "external `up` should exit 0; got: {out}");
    assert!(
        out.contains("external"),
        "external `up` should explain the external path; got: {out}"
    );
    // No managed child ⇒ no pidfile written anywhere under the cache.
    assert!(
        !tmp.path().join("typedb").exists(),
        "external mode must not create a managed TypeDB cache tree"
    );
}

#[test]
fn up_compose_directs_to_docker_compose() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = write_config(tmp.path(), "compose", "localhost:1729");
    let (out, ok) = run_gecko(&cfg, tmp.path(), &["up"]);
    assert!(ok, "compose `up` should exit 0; got: {out}");
    assert!(
        out.contains("docker compose up"),
        "compose `up` should direct to `docker compose up`; got: {out}"
    );
}

#[test]
fn down_compose_and_external_are_message_only() {
    let tmp = tempfile::tempdir().unwrap();

    let cfg = write_config(tmp.path(), "compose", "localhost:1729");
    let (out, ok) = run_gecko(&cfg, tmp.path(), &["down"]);
    assert!(ok && out.contains("docker compose down"), "got: {out}");

    let cfg = write_config(tmp.path(), "external", "localhost:1729");
    let (out, ok) = run_gecko(&cfg, tmp.path(), &["down"]);
    assert!(ok && out.contains("external"), "got: {out}");
}

#[test]
fn down_orchestrated_empty_cache_is_noop() {
    // `down` never downloads/spawns: with an empty cache it finds no pidfile and
    // reports nothing to stop. Safe on every platform (no server needed).
    let tmp = tempfile::tempdir().unwrap();
    let cfg = write_config(tmp.path(), "orchestrated", "localhost:1729");
    let (out, ok) = run_gecko(&cfg, tmp.path(), &["down"]);
    assert!(ok, "orchestrated `down` no-op should exit 0; got: {out}");
    assert!(
        out.contains("nothing to stop") || out.contains("No managed"),
        "got: {out}"
    );
}

#[test]
fn up_orchestrated_is_idempotent_against_running_server() {
    // GUARD: only meaningful when a server is already serving 1729. Skipping when
    // it is not keeps this off the download path on bare runners.
    if !server_1729_up() {
        eprintln!("skipping: no TypeDB reachable on localhost:1729");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let cfg = write_config(tmp.path(), "orchestrated", "localhost:1729");
    let (out, ok) = run_gecko(&cfg, tmp.path(), &["up"]);
    assert!(ok, "orchestrated `up` should exit 0; got: {out}");
    assert!(
        out.contains("already running"),
        "an already-serving endpoint must be an idempotent no-op; got: {out}"
    );
    // Idempotent no-op must NOT have downloaded/spawned anything.
    assert!(
        !tmp.path()
            .join("typedb")
            .join("3.12.0")
            .join("dist")
            .exists(),
        "idempotent `up` must not fetch the dist when a server is already up"
    );
}
