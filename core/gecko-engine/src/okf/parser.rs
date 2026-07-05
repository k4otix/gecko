//! OKF concept and bundle parsing.
//!
//! Reads markdown files with YAML frontmatter, extracts code blocks and links, and resolves relative paths into normalized concept IDs.
//! extracts typed metadata, and assembles bundles from directory trees.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::linker::{extract_code_blocks, extract_links};
use super::types::{HierarchyEdge, OkfBundle, OkfConcept, ScriptEngine};

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("I/O error reading '{path}': {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },

    #[error("Concept file '{path}' is missing the required 'type' frontmatter field")]
    MissingType { path: String },

    #[error("Concept file '{path}' has an empty 'type' frontmatter field")]
    EmptyType { path: String },

    #[error("YAML parsing error in '{path}': {source}")]
    Yaml {
        path: String,
        source: serde_yaml::Error,
    },
}

/// Derives a concept ID from a file path relative to the bundle root.
///
/// Generates a normalized concept ID based on the file path relative to the bundle root.
///
/// # Example
/// ```
/// use std::path::Path;
/// use gecko_engine::okf::parser::file_to_concept_id;
///
/// assert_eq!(
///     file_to_concept_id(Path::new("bundle/tables/orders.md"), Path::new("bundle"), "bundle-name"),
///     "bundle-name:tables/orders"
/// );
/// ```
pub fn file_to_concept_id(file_path: &Path, bundle_root: &Path, bundle_name: &str) -> String {
    let relative = file_path
        .strip_prefix(bundle_root)
        .expect("file_path must be under bundle_root");

    // Strip the .md extension and convert to forward-slash posix path
    let without_ext = relative.with_extension("");
    let path_str = without_ext.to_string_lossy().replace('\\', "/");

    format!("{bundle_name}:{path_str}")
}

/// Returns true if the file matches reserved names (index.md, log.md).
///
/// Checks if a file path belongs to a reserved directory or is a dotfile/configuration file.
pub fn is_reserved_file(file_path: &Path) -> bool {
    if let Some(name) = file_path.file_name() {
        let name_lower = name.to_string_lossy().to_lowercase();
        matches!(name_lower.as_str(), "index.md" | "log.md")
    } else {
        false
    }
}

/// Computes SHA-256 hash of a file's raw content.
///
/// Computes a SHA256 hash of the raw file content.
pub fn compute_file_hash(file_path: &Path) -> Result<String, ParseError> {
    let content = std::fs::read(file_path).map_err(|e| ParseError::Io {
        path: file_path.display().to_string(),
        source: e,
    })?;

    let mut hasher = Sha256::new();
    hasher.update(&content);
    let result = hasher.finalize();
    Ok(result
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>())
}

/// Splits raw file content into YAML frontmatter and markdown body.
///
/// Expects the file to start with `---\n` and have a closing `---\n`.
fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    if !content.starts_with("---") {
        return (None, content);
    }

    // Find the closing --- (skip the first line)
    let after_open = &content[3..];
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);

    if let Some(close_pos) = after_open.find("\n---") {
        let yaml = &after_open[..close_pos];
        let body_start = close_pos + 4; // skip "\n---"
        let body = if body_start < after_open.len() {
            after_open[body_start..].trim_start_matches('\n')
        } else {
            ""
        };
        (Some(yaml), body)
    } else {
        (None, content)
    }
}

/// Parses an individual OKF concept document.
///
/// Parses a markdown file into an `OkfConcept` by extracting frontmatter and links.
pub fn parse_concept(
    file_path: &Path,
    bundle_root: &Path,
    bundle_name: &str,
) -> Result<OkfConcept, ParseError> {
    let raw_content = std::fs::read_to_string(file_path).map_err(|e| ParseError::Io {
        path: file_path.display().to_string(),
        source: e,
    })?;

    let file_hash = {
        let mut hasher = Sha256::new();
        hasher.update(raw_content.as_bytes());
        let result = hasher.finalize();
        result
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>()
    };

    let (yaml_str, body) = split_frontmatter(&raw_content);

    // Parse YAML frontmatter into a flexible map
    let mut metadata: HashMap<String, serde_yaml::Value> = if let Some(yaml) = yaml_str {
        serde_yaml::from_str(yaml).map_err(|e| ParseError::Yaml {
            path: file_path.display().to_string(),
            source: e,
        })?
    } else {
        HashMap::new()
    };

    // OKF 'type' is required
    let concept_type = metadata
        .remove("type")
        .and_then(|v| match v {
            serde_yaml::Value::String(s) => Some(s),
            other => Some(other.as_str()?.to_string()),
        })
        .ok_or_else(|| ParseError::MissingType {
            path: file_path.display().to_string(),
        })?;

    let concept_type = concept_type.trim().to_string();
    if concept_type.is_empty() {
        return Err(ParseError::EmptyType {
            path: file_path.display().to_string(),
        });
    }

    // Extract standard fields
    let title = extract_string(&mut metadata, "title");
    let description = extract_string(&mut metadata, "description");
    let resource_uri = extract_string(&mut metadata, "resource");

    let tags = extract_string_list(&mut metadata, "tags");
    let timestamp = extract_timestamp(&mut metadata, "timestamp");

    // GECKO-specific extension fields
    let consumes = extract_string_list(&mut metadata, "consumes");
    let produces = extract_string_list(&mut metadata, "produces");
    let engine =
        extract_string(&mut metadata, "engine").and_then(|s| match s.to_lowercase().as_str() {
            "rhai" => Some(ScriptEngine::Rhai),
            "quickjs" => Some(ScriptEngine::QuickJs),
            _ => None,
        });
    let scopes = extract_string_list(&mut metadata, "scopes");
    let timeout_ms = metadata.remove("timeout-ms").and_then(|v| v.as_u64());

    // Remaining keys become extra_metadata
    let extra_metadata: HashMap<String, String> = metadata
        .into_iter()
        .map(|(k, v)| {
            let val = match v {
                serde_yaml::Value::String(s) => s,
                other => serde_json::to_string(&other).unwrap_or_default(),
            };
            (k, val)
        })
        .collect();

    let concept_id = file_to_concept_id(file_path, bundle_root, bundle_name);
    let source_path = file_path
        .strip_prefix(bundle_root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .replace('\\', "/");

    let code_blocks = extract_code_blocks(body);

    Ok(OkfConcept {
        concept_id,
        concept_type,
        title,
        description,
        resource_uri,
        tags,
        timestamp,
        body: body.to_string(),
        code_blocks,
        extra_metadata,
        file_hash,
        source_path,
        consumes,
        produces,
        engine,
        scopes,
        timeout_ms,
    })
}

/// Parses a full directory tree as an OKF Bundle.
///
/// Recursively parses a directory bundle to find all OKF concepts.
pub fn parse_bundle(bundle_root_path: &Path) -> Result<OkfBundle, ParseError> {
    let bundle_root = bundle_root_path
        .canonicalize()
        .map_err(|e| ParseError::Io {
            path: bundle_root_path.display().to_string(),
            source: e,
        })?;

    let bundle_name = bundle_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut concepts = Vec::new();
    let mut links = Vec::new();
    let mut citations = Vec::new();
    let mut hierarchy = Vec::new();

    // Collect and sort .md files for deterministic output
    let mut md_files: Vec<_> = walk_md_files(&bundle_root)?;
    md_files.sort();

    for file_path in &md_files {
        if is_reserved_file(file_path) {
            continue;
        }

        let concept = parse_concept(file_path, &bundle_root, &bundle_name)?;

        // Extract links and citations
        let (concept_links, concept_citations) = extract_links(&concept.body, &concept.concept_id);
        links.extend(concept_links);
        citations.extend(concept_citations);

        // Determine parent directory for hierarchy
        let rel_path = file_path.strip_prefix(&bundle_root).unwrap();
        let parent_dir = rel_path.parent();

        if let Some(parent) = parent_dir {
            let parent_str = parent.to_string_lossy().replace('\\', "/");

            if parent_str.is_empty() || parent_str == "." {
                // Root-level concept
                hierarchy.push(HierarchyEdge {
                    parent_id: String::new(),
                    child_id: concept.concept_id.clone(),
                });
            } else {
                let prefixed_parent = format!("{bundle_name}:{parent_str}");
                hierarchy.push(HierarchyEdge {
                    parent_id: prefixed_parent,
                    child_id: concept.concept_id.clone(),
                });

                // Walk up parent directories to connect directory hierarchy
                let mut current = parent.to_path_buf();
                while let Some(grandparent) = current.parent() {
                    let gp_str = grandparent.to_string_lossy().replace('\\', "/");
                    if gp_str.is_empty() || gp_str == "." {
                        break;
                    }
                    let gp_prefixed = format!("{bundle_name}:{gp_str}");
                    let child_str = current.to_string_lossy().replace('\\', "/");
                    let child_prefixed = format!("{bundle_name}:{child_str}");

                    hierarchy.push(HierarchyEdge {
                        parent_id: gp_prefixed,
                        child_id: child_prefixed,
                    });
                    current = grandparent.to_path_buf();
                }
            }
        }

        concepts.push(concept);
    }

    // Deduplicate hierarchy edges
    let mut seen: HashSet<(String, String)> = HashSet::new();
    hierarchy.retain(|edge| seen.insert((edge.parent_id.clone(), edge.child_id.clone())));

    Ok(OkfBundle {
        bundle_path: bundle_root.to_string_lossy().to_string(),
        bundle_name,
        concepts,
        links,
        citations,
        hierarchy,
    })
}

/// Recursively finds all `.md` files under a directory.
fn walk_md_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, ParseError> {
    let mut results = Vec::new();

    let entries = std::fs::read_dir(dir).map_err(|e| ParseError::Io {
        path: dir.display().to_string(),
        source: e,
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| ParseError::Io {
            path: dir.display().to_string(),
            source: e,
        })?;
        let path = entry.path();

        if path.is_dir() {
            results.extend(walk_md_files(&path)?);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            results.push(path);
        }
    }

    Ok(results)
}

// -- Helper functions for extracting typed values from YAML --

fn extract_string(map: &mut HashMap<String, serde_yaml::Value>, key: &str) -> Option<String> {
    map.remove(key).and_then(|v| match v {
        serde_yaml::Value::String(s) => Some(s),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    })
}

fn extract_string_list(map: &mut HashMap<String, serde_yaml::Value>, key: &str) -> Vec<String> {
    map.remove(key)
        .and_then(|v| match v {
            serde_yaml::Value::Sequence(seq) => Some(
                seq.into_iter()
                    .filter_map(|item| match item {
                        serde_yaml::Value::String(s) => Some(s),
                        other => Some(other.as_str()?.to_string()),
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

fn extract_timestamp(
    map: &mut HashMap<String, serde_yaml::Value>,
    key: &str,
) -> Option<DateTime<Utc>> {
    let val = map.remove(key)?;
    match val {
        serde_yaml::Value::String(s) => {
            let normalized = s.replace('Z', "+00:00");
            DateTime::parse_from_rfc3339(&normalized)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Creates a temporary bundle directory with test fixtures.
    fn create_test_bundle(dir: &Path) {
        // datasets/users.md
        let datasets = dir.join("datasets");
        fs::create_dir_all(&datasets).unwrap();
        fs::write(
            datasets.join("users.md"),
            r#"---
type: Dataset
title: Users Dataset
description: Contains user registration data
resource: db://myproject.users
tags: [users, demographics]
timestamp: 2026-01-01T00:00:00Z
custom_field: some-extension-value
---

The users dataset tracks registration data.

See [Orders](../tables/orders.md) for related data.
"#,
        )
        .unwrap();

        // tables/orders.md
        let tables = dir.join("tables");
        fs::create_dir_all(&tables).unwrap();
        fs::write(
            tables.join("orders.md"),
            r#"---
type: Table
title: Orders Table
---

Order records. Links to [Users](../datasets/users.md).
Check [Docs](https://example.com/docs).
"#,
        )
        .unwrap();

        // metrics/revenue.md (with code block)
        let metrics = dir.join("metrics");
        fs::create_dir_all(&metrics).unwrap();
        fs::write(
            metrics.join("revenue.md"),
            r#"---
type: Metric
title: Monthly Revenue
---

Revenue calculation:

```python
def calculate_monthly_revenue():
    return sum(orders)
```
"#,
        )
        .unwrap();

        // Reserved files (should be skipped)
        fs::write(dir.join("index.md"), "---\ntype: Index\n---\n").unwrap();
    }

    /// Creates a minimal bundle with just a type field.
    fn create_minimal_bundle(dir: &Path) {
        fs::write(
            dir.join("simple.md"),
            "---\ntype: Note\n---\n\nA minimal concept.\n",
        )
        .unwrap();
    }

    /// Creates an invalid bundle with missing type.
    fn create_invalid_bundle(dir: &Path) {
        fs::write(
            dir.join("no-type.md"),
            "---\ntitle: Missing Type\n---\n\nNo type field.\n",
        )
        .unwrap();
    }

    #[test]
    fn test_file_to_concept_id() {
        let root = Path::new("bundle");
        assert_eq!(
            "test-bundle:tables/orders",
            file_to_concept_id(Path::new("bundle/tables/orders.md"), root, "test-bundle")
        );
        assert_eq!(
            "test-bundle:simple",
            file_to_concept_id(Path::new("bundle/simple.md"), root, "test-bundle")
        );
        assert_eq!(
            "test-bundle:a/b/c",
            file_to_concept_id(Path::new("bundle/a/b/c.md"), root, "test-bundle")
        );
    }

    #[test]
    fn test_is_reserved_file() {
        assert!(is_reserved_file(Path::new("index.md")));
        assert!(is_reserved_file(Path::new("log.md")));
        assert!(is_reserved_file(Path::new("dir/INDEX.md")));
        assert!(!is_reserved_file(Path::new("concept.md")));
        assert!(!is_reserved_file(Path::new("myindex.md")));
    }

    #[test]
    fn test_compute_file_hash() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.md");
        fs::write(&file, "hello world").unwrap();

        let hash1 = compute_file_hash(&file).unwrap();
        let hash2 = compute_file_hash(&file).unwrap();
        assert_eq!(hash1.len(), 64);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_parse_concept_full() {
        let dir = tempfile::tempdir().unwrap();
        create_test_bundle(dir.path());

        let concept = parse_concept(
            &dir.path().join("datasets/users.md"),
            dir.path(),
            "test-bundle",
        )
        .unwrap();

        assert_eq!(concept.concept_id, "test-bundle:datasets/users");
        assert_eq!(concept.concept_type, "Dataset");
        assert_eq!(concept.title.as_deref(), Some("Users Dataset"));
        assert_eq!(
            concept.description.as_deref(),
            Some("Contains user registration data")
        );
        assert_eq!(
            concept.resource_uri.as_deref(),
            Some("db://myproject.users")
        );
        assert_eq!(concept.tags, vec!["users", "demographics"]);
        assert!(concept.timestamp.is_some());
        assert!(concept.body.contains("users dataset tracks"));
        assert_eq!(
            concept
                .extra_metadata
                .get("custom_field")
                .map(|s| s.as_str()),
            Some("some-extension-value")
        );
    }

    #[test]
    fn test_parse_concept_minimal() {
        let dir = tempfile::tempdir().unwrap();
        create_minimal_bundle(dir.path());

        let concept =
            parse_concept(&dir.path().join("simple.md"), dir.path(), "test-bundle").unwrap();

        assert_eq!(concept.concept_type, "Note");
        assert!(concept.title.is_none());
        assert!(concept.tags.is_empty());
        assert!(concept.body.contains("minimal concept"));
    }

    #[test]
    fn test_parse_concept_missing_type() {
        let dir = tempfile::tempdir().unwrap();
        create_invalid_bundle(dir.path());

        let result = parse_concept(&dir.path().join("no-type.md"), dir.path(), "test-bundle");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ParseError::MissingType { .. }
        ));
    }

    #[test]
    fn test_parse_concept_code_blocks() {
        let dir = tempfile::tempdir().unwrap();
        create_test_bundle(dir.path());

        let concept = parse_concept(
            &dir.path().join("metrics/revenue.md"),
            dir.path(),
            "test-bundle",
        )
        .unwrap();

        assert_eq!(concept.code_blocks.len(), 1);
        assert!(concept.code_blocks[0].contains("calculate_monthly_revenue"));
    }

    #[test]
    fn test_parse_concept_gecko_extension_fields() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("playbook.md"),
            r#"---
type: Playbook
title: Containment
engine: rhai
consumes: [alert, asset]
produces: [containment-result]
scopes: [mde:isolate]
timeout-ms: 5000
---

Playbook body.
"#,
        )
        .unwrap();

        let concept =
            parse_concept(&dir.path().join("playbook.md"), dir.path(), "test-bundle").unwrap();

        assert_eq!(concept.engine, Some(ScriptEngine::Rhai));
        assert_eq!(concept.consumes, vec!["alert", "asset"]);
        assert_eq!(concept.produces, vec!["containment-result"]);
        assert_eq!(concept.scopes, vec!["mde:isolate"]);
        assert_eq!(concept.timeout_ms, Some(5000));
    }

    #[test]
    fn test_parse_bundle() {
        let dir = tempfile::tempdir().unwrap();
        create_test_bundle(dir.path());

        let bundle = parse_bundle(dir.path()).unwrap();

        assert_eq!(bundle.concepts.len(), 3); // index.md skipped
        let bundle_name = bundle.bundle_name;
        let ids: HashSet<_> = bundle
            .concepts
            .iter()
            .map(|c| c.concept_id.as_str())
            .collect();
        assert!(ids.contains(&format!("{}:datasets/users", bundle_name) as &str));
        assert!(ids.contains(&format!("{}:tables/orders", bundle_name) as &str));
        assert!(ids.contains(&format!("{}:metrics/revenue", bundle_name) as &str));

        // Check hierarchy edges exist
        let hierarchy_set: HashSet<_> = bundle
            .hierarchy
            .iter()
            .map(|h| (h.parent_id.as_str(), h.child_id.as_str()))
            .collect();
        let d_str = format!("{}:datasets", bundle_name);
        let u_str = format!("{}:datasets/users", bundle_name);
        assert!(hierarchy_set.contains(&(d_str.as_str(), u_str.as_str())));
        let t_str = format!("{}:tables", bundle_name);
        let o_str = format!("{}:tables/orders", bundle_name);
        assert!(hierarchy_set.contains(&(t_str.as_str(), o_str.as_str())));
        let m_str = format!("{}:metrics", bundle_name);
        let r_str = format!("{}:metrics/revenue", bundle_name);
        assert!(hierarchy_set.contains(&(m_str.as_str(), r_str.as_str())));
    }

    #[test]
    fn test_parse_bundle_skips_reserved() {
        let dir = tempfile::tempdir().unwrap();
        create_test_bundle(dir.path());

        let bundle = parse_bundle(dir.path()).unwrap();
        for c in &bundle.concepts {
            assert!(!c.concept_id.contains("index"));
            assert!(!c.concept_id.contains("log"));
        }
    }

    #[test]
    fn test_split_frontmatter() {
        let content = "---\ntype: Test\ntitle: Hello\n---\n\nBody text here.\n";
        let (yaml, body) = split_frontmatter(content);
        assert!(yaml.is_some());
        assert!(yaml.unwrap().contains("type: Test"));
        assert!(body.contains("Body text here"));
    }

    #[test]
    fn test_split_frontmatter_no_yaml() {
        let content = "Just body text.\n";
        let (yaml, body) = split_frontmatter(content);
        assert!(yaml.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn test_split_frontmatter_malformed() {
        let content = "---\ntype: Test\ntitle: Hello\n\nNo closing dashes.\n";
        let (yaml, body) = split_frontmatter(content);
        assert!(yaml.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn test_parse_concept_empty_type() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("no_type.md");
        let content = "---\ntitle: No Type\n---\nBody";
        std::fs::write(&file_path, content).unwrap();

        let result = parse_concept(&file_path, dir.path(), "test_bundle");
        assert!(matches!(result, Err(ParseError::MissingType { path: _ })));
    }
}
