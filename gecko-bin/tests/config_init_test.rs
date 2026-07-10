//! `gecko init` (config bootstrap): writes a default gecko.toml if absent
//! and NEVER clobbers an existing one. Drives the real binary — no TypeDB needed
//! (config init runs before any connection is opened).

use std::process::Command;

fn gecko() -> Command {
    Command::new(env!("CARGO_BIN_EXE_gecko"))
}

#[test]
fn init_writes_default_config_then_is_a_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("gecko.toml");

    // 1. Absent → writes a valid default config.
    let status = gecko()
        .arg("--config")
        .arg(&cfg_path)
        .arg("init")
        .status()
        .expect("failed to run gecko init");
    assert!(status.success(), "gecko init should succeed");
    assert!(cfg_path.exists(), "gecko init should create gecko.toml");

    let written = std::fs::read_to_string(&cfg_path).unwrap();
    // It must parse as valid TOML with the expected tables present.
    let parsed: toml::Value = toml::from_str(&written).expect("default config must be valid TOML");
    assert!(parsed.get("extensions").is_some());
    assert!(parsed.get("semantic_index").is_some());
    assert!(parsed.get("typedb").is_some());
    assert_eq!(
        parsed["typedb"]["mode"].as_str(),
        Some("orchestrated"),
        "the default run mode is orchestrated"
    );

    // 2. Present → idempotent no-op: existing content is preserved byte-for-byte.
    let sentinel = "# hand-edited — must survive\n[extensions]\nenabled = [\"cyber\"]\n";
    std::fs::write(&cfg_path, sentinel).unwrap();
    let status = gecko()
        .arg("--config")
        .arg(&cfg_path)
        .arg("init")
        .status()
        .expect("failed to re-run gecko init");
    assert!(status.success(), "re-running gecko init should succeed");
    assert_eq!(
        std::fs::read_to_string(&cfg_path).unwrap(),
        sentinel,
        "gecko init must NEVER clobber an existing config"
    );
}
