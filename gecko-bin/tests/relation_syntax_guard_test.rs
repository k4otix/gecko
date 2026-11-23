//! Every query and schema source in the workspace writes relations in the current
//! form (`rel (roles)` or `$r isa rel, links (roles)`). The exceptions are the
//! frozen fixtures under a `tests/fixtures/` directory, which record the text
//! earlier releases shipped.

use std::path::{Path, PathBuf};

use gecko_extension_api::relation_syntax::deprecated_relation_lines;

/// The `.rs` and `.tql` files under `dir`, skipping build output and frozen
/// fixtures.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_dir() {
            let frozen = name == "fixtures"
                && path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|n| n.to_str())
                    == Some("tests");
            if name != "target" && !name.starts_with('.') && !frozen {
                sources(&path, out);
            }
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs" | "tql")
        ) {
            out.push(path);
        }
    }
}

#[test]
fn workspace_sources_use_current_relation_syntax() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let mut files = Vec::new();
    for dir in ["core", "extensions", "gecko-bin"] {
        sources(&root.join(dir), &mut files);
    }
    assert!(
        files.len() > 50,
        "the walk found only {} files",
        files.len()
    );

    let mut offenders = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("read source");
        for line in deprecated_relation_lines(&text) {
            offenders.push(format!(
                "{}:{line}",
                file.strip_prefix(root).unwrap_or(file).display()
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "role lists written before `isa`; write `rel (roles)` instead:\n{}",
        offenders.join("\n")
    );
}
