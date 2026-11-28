//! OKF concept and bundle parsing.
//!
//! Reads markdown files with YAML frontmatter, extracts typed metadata, code blocks, and
//! links, and assembles bundles from directory trees.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use sha2::{Digest, Sha256};
use thiserror::Error;

use super::linker::{extract_links, extract_program};
use super::provenance::{is_empty_value, take_provenance};
use super::types::{HierarchyEdge, OkfBundle, OkfConcept, SCRIPT_ENGINE_LANGS, ScriptEngine};
use crate::sandbox::wasm_executor::MAX_TIMEOUT_MS;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("I/O error reading '{path}': {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },

    #[error(
        "Concept file '{path}' has an invalid 'timeout-ms' ({value}); it must be an integer from 1 to {max}",
        max = MAX_TIMEOUT_MS
    )]
    InvalidTimeout { path: String, value: String },

    #[error(
        "Concept file '{path}' has an invalid 'engine' ({value}); it must be one of {}",
        SCRIPT_ENGINE_LANGS.join(", ")
    )]
    InvalidEngine { path: String, value: String },

    #[error(
        "Concept file '{path}' declares engine '{engine}' but has no code in a {} fence",
        or_list(&SCRIPT_ENGINE_LANGS)
    )]
    MissingProgram { path: String, engine: String },

    #[error("YAML parsing error in '{path}': {source}")]
    Yaml {
        path: String,
        source: serde_yaml::Error,
    },
}

/// Derives a concept ID from a file path relative to the bundle root.
///
/// The concept ID is the pure OKF bundle-relative path with the `.md` extension
/// removed and no namespace prefix, matching the OKF spec exactly. IDs must not
/// collide across the bundles that share a database; a colliding sync is refused.
///
/// # Example
/// ```
/// use std::path::Path;
/// use gecko_engine::okf::parser::file_to_concept_id;
///
/// assert_eq!(
///     file_to_concept_id(Path::new("bundle/tables/orders.md"), Path::new("bundle")),
///     "tables/orders"
/// );
/// ```
pub fn file_to_concept_id(file_path: &Path, bundle_root: &Path) -> String {
    let relative = file_path
        .strip_prefix(bundle_root)
        .expect("file_path must be under bundle_root");

    // Strip the .md extension and convert to forward-slash posix path
    let without_ext = relative.with_extension("");
    to_posix_string(&without_ext)
}

/// Converts a path to a forward-slash ("posix") string, regardless of platform.
fn to_posix_string(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Wraps an I/O error with the path that triggered it.
fn io_err(path: &Path, source: std::io::Error) -> ParseError {
    ParseError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Returns true if the file matches reserved names (index.md, log.md).
pub fn is_reserved_file(file_path: &Path) -> bool {
    if let Some(name) = file_path.file_name() {
        let name_lower = name.to_string_lossy().to_lowercase();
        matches!(name_lower.as_str(), "index.md" | "log.md")
    } else {
        false
    }
}

/// The version of how this parser reads an OKF file, salted into every concept's
/// `file-hash`. Bump it whenever a parser change alters what an unchanged file stores:
/// every stored hash then differs once, so the next sync re-reads each file.
pub const OKF_READ_VERSION: &str = "1";

/// A concept's change-detection hash: SHA-256 over [`OKF_READ_VERSION`], a NUL
/// separator, then the file's raw bytes, as lowercase hex.
pub fn concept_file_hash(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(OKF_READ_VERSION.as_bytes());
    hasher.update([0]);
    hasher.update(content);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The OKF keys GECKO reads whose null, empty-string or empty-list value is an absent
/// key (OKF §11.2). `type` is handled where it is read.
const OKF_KEYS: &[&str] = &[
    "title",
    "description",
    "resource",
    "tags",
    "generated",
    "timestamp",
    "verified",
    "status",
    "stale_after",
];

/// What a Markdown file's leading frontmatter block is, if any.
#[derive(Debug, PartialEq)]
enum Frontmatter<'a> {
    /// The file has no frontmatter.
    None,
    /// The first line opens frontmatter that no later line closes.
    Unclosed,
    /// The first line is blank and the first non-blank line is a delimiter.
    NearMiss,
    /// A frontmatter block and the body after it.
    Block { yaml: &'a str, body: &'a str },
}

/// What one Markdown file of a bundle is: a concept with its warnings, or a file that
/// is not a concept (OKF §3.2), with the warning to show when the author likely meant
/// one.
#[derive(Debug)]
pub enum ParsedFile {
    Concept(Box<OkfConcept>, Vec<String>),
    NotAConcept(Option<String>),
}

/// Splits raw file content into its frontmatter block and Markdown body (OKF §4).
///
/// One leading UTF-8 BOM is ignored. A delimiter is a line that is `---` once trailing
/// spaces, tabs and `\r` are trimmed. The block opens only on the first line and closes
/// at the next delimiter line.
fn split_frontmatter(content: &str) -> Frontmatter<'_> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let is_delimiter = |line: &str| line.trim_end_matches([' ', '\t', '\r', '\n']) == "---";
    let mut lines = content.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Frontmatter::None;
    };
    if !is_delimiter(first) {
        let near_miss = first.trim().is_empty()
            && content
                .split_inclusive('\n')
                .find(|l| !l.trim().is_empty())
                .is_some_and(is_delimiter);
        return if near_miss {
            Frontmatter::NearMiss
        } else {
            Frontmatter::None
        };
    }
    let yaml_start = first.len();
    let mut offset = yaml_start;
    for line in lines {
        if is_delimiter(line) {
            let yaml = &content[yaml_start..offset];
            let body = content[offset + line.len()..].trim_start_matches(['\r', '\n']);
            return Frontmatter::Block { yaml, body };
        }
        offset += line.len();
    }
    Frontmatter::Unclosed
}

/// Parses one Markdown file of a bundle. A file with frontmatter and a non-empty `type`
/// is a concept, returned with its frontmatter, links and code, and a warning for each
/// OKF value it could not read; any other file is not a concept (OKF §3.2).
pub fn parse_concept(file_path: &Path, bundle_root: &Path) -> Result<ParsedFile, ParseError> {
    let raw_content = std::fs::read_to_string(file_path).map_err(|e| io_err(file_path, e))?;

    let file_hash = concept_file_hash(raw_content.as_bytes());

    let path = file_path.display();
    let not_a_concept = |why: &str| {
        Ok(ParsedFile::NotAConcept(Some(format!(
            "Markdown file '{path}' {why}; it is not a concept and is not synced"
        ))))
    };
    let (yaml, body) = match split_frontmatter(&raw_content) {
        Frontmatter::Block { yaml, body } => (yaml, body),
        Frontmatter::None => return Ok(ParsedFile::NotAConcept(None)),
        Frontmatter::Unclosed => {
            return not_a_concept("opens frontmatter with '---' but never closes it");
        }
        Frontmatter::NearMiss => {
            return not_a_concept(
                "looks like it opens frontmatter after blank lines, but frontmatter must \
                 start on the first line",
            );
        }
    };

    // Empty or null frontmatter is an empty mapping; anything but a mapping is refused.
    let yaml_err = |source| ParseError::Yaml {
        path: path.to_string(),
        source,
    };
    let mut metadata: HashMap<String, serde_yaml::Value> =
        match serde_yaml::from_str(yaml).map_err(yaml_err)? {
            serde_yaml::Value::Null => HashMap::new(),
            value => serde_yaml::from_value(value).map_err(yaml_err)?,
        };

    // A null, empty-string or empty-list value of an OKF key is an absent key (§11.2).
    for key in OKF_KEYS {
        if metadata.get(*key).is_some_and(is_empty_value) {
            metadata.remove(*key);
        }
    }

    // A non-empty scalar `type` makes the file a concept (OKF §3.2).
    let concept_type = extract_string(&mut metadata, "type")
        .map(|t| t.trim().to_string())
        .unwrap_or_default();
    if concept_type.is_empty() {
        return not_a_concept("has no 'type'");
    }

    // Extract standard fields
    let title = extract_string(&mut metadata, "title");
    let description = extract_string(&mut metadata, "description");
    let resource_uri = extract_string(&mut metadata, "resource");

    let mut warnings = Vec::new();
    let tags = take_tags(&mut metadata, &path.to_string(), &mut warnings);
    let provenance = take_provenance(
        &mut metadata,
        &file_path.display().to_string(),
        &mut warnings,
    );
    let timestamp = provenance.timestamp;
    let provenance = provenance.provenance;

    // GECKO-specific extension fields
    // A present `engine` must name a supported engine; it is never coerced or dropped.
    let engine = match metadata.remove("engine") {
        None => None,
        Some(v) => match &v {
            serde_yaml::Value::String(s) if let Some(e) = ScriptEngine::from_lang(s) => {
                Some((e, s.clone()))
            }
            _ => {
                return Err(ParseError::InvalidEngine {
                    path: file_path.display().to_string(),
                    value: yaml_as_written(&v),
                });
            }
        },
    };
    let scopes = extract_string_list(&mut metadata, "scopes");
    let timeout_ms = match metadata.remove("timeout-ms") {
        None => None,
        Some(v) => match v.as_u64() {
            Some(t) if (1..=MAX_TIMEOUT_MS).contains(&t) => Some(t),
            _ => {
                return Err(ParseError::InvalidTimeout {
                    path: file_path.display().to_string(),
                    value: yaml_as_written(&v),
                });
            }
        },
    };

    // Remaining keys become extra_metadata
    let extra_metadata: HashMap<String, String> = metadata
        .into_iter()
        .map(|(k, v)| (k, kept_as_written(v)))
        .collect();

    let concept_id = file_to_concept_id(file_path, bundle_root);
    let source_path = to_posix_string(file_path.strip_prefix(bundle_root).unwrap_or(file_path));

    // One program per concept: pull only the fences whose language matches the
    // declared engine and concatenate them in document order.
    let program = extract_program(body, engine.as_ref().map(|(e, _)| e));
    if let (Some((_, written)), None) = (&engine, &program) {
        return Err(ParseError::MissingProgram {
            path: file_path.display().to_string(),
            engine: written.clone(),
        });
    }
    let engine = engine.map(|(e, _)| e);

    Ok(ParsedFile::Concept(
        Box::new(OkfConcept {
            concept_id,
            concept_type,
            type_hint: None,
            title,
            description,
            resource_uri,
            tags,
            timestamp,
            body: body.to_string(),
            program,
            extra_metadata,
            typed_attributes: Vec::new(),
            file_hash,
            source_path,
            engine,
            scopes,
            timeout_ms,
            provenance,
        }),
        warnings,
    ))
}

/// Recursively parses a directory tree into a complete `OkfBundle`, finding all OKF
/// concepts.
pub fn parse_bundle(bundle_root_path: &Path) -> Result<OkfBundle, ParseError> {
    let bundle_root = bundle_root_path
        .canonicalize()
        .map_err(|e| io_err(bundle_root_path, e))?;

    // Bundle identity/metadata comes from an optional `bundle.json` manifest,
    // falling back to the directory name. This is metadata only — concept IDs are
    // NOT namespaced by it (IDs are plain bundle-relative paths and must not collide
    // across the bundles of one database).
    let (bundle_name, bundle_description) = read_bundle_manifest(&bundle_root);

    let mut concepts = Vec::new();
    let mut links = Vec::new();
    let mut citations = Vec::new();
    let mut hierarchy = Vec::new();
    let mut warnings = Vec::new();
    let okf_version = read_okf_version(&bundle_root, &mut warnings);

    // Collect and sort .md files for deterministic output
    let mut md_files: Vec<_> = walk_md_files(&bundle_root, &bundle_root, &mut warnings)?;
    md_files.sort();

    for file_path in &md_files {
        if is_reserved_file(file_path) {
            continue;
        }

        let (concept, concept_warnings) = match parse_concept(file_path, &bundle_root)? {
            ParsedFile::Concept(concept, w) => (*concept, w),
            ParsedFile::NotAConcept(w) => {
                warnings.extend(w);
                continue;
            }
        };
        warnings.extend(concept_warnings);

        // Extract links and citations
        let (concept_links, concept_citations) = extract_links(&concept.body, &concept.concept_id);
        links.extend(concept_links);
        citations.extend(concept_citations);

        // Determine parent directory for hierarchy. Since concept IDs are the
        // bundle-relative path (no prefix), a parent directory's path IS the
        // parent concept ID directly.
        let rel_path = file_path.strip_prefix(&bundle_root).unwrap();
        let parent_dir = rel_path.parent();

        if let Some(parent) = parent_dir {
            let parent_str = to_posix_string(parent);

            if parent_str.is_empty() || parent_str == "." {
                // Root-level concept
                hierarchy.push(HierarchyEdge {
                    parent_id: String::new(),
                    child_id: concept.concept_id.clone(),
                });
            } else {
                hierarchy.push(HierarchyEdge {
                    parent_id: parent_str,
                    child_id: concept.concept_id.clone(),
                });

                // Walk up parent directories to connect the directory hierarchy.
                let mut current = parent.to_path_buf();
                while let Some(grandparent) = current.parent() {
                    let gp_str = to_posix_string(grandparent);
                    if gp_str.is_empty() || gp_str == "." {
                        break;
                    }
                    let child_str = to_posix_string(&current);

                    hierarchy.push(HierarchyEdge {
                        parent_id: gp_str,
                        child_id: child_str,
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
        bundle_description,
        concepts,
        links,
        citations,
        hierarchy,
        okf_version,
        warnings,
    })
}

/// Reads a bundle's declared name (from `bundle.json`, falling back to the
/// directory name) without parsing the whole bundle. Used to derive the default
/// target database, one per bundle.
pub fn bundle_name(bundle_root_path: &Path) -> Result<String, ParseError> {
    let bundle_root = bundle_root_path
        .canonicalize()
        .map_err(|e| io_err(bundle_root_path, e))?;
    Ok(read_bundle_manifest(&bundle_root).0)
}

/// Reads bundle identity metadata from an optional `bundle.json` manifest at the
/// bundle root. Returns `(name, description)`. Falls back to the directory name
/// when the manifest is absent, unreadable, or has no `name` field. Parse/IO
/// errors are tolerated (OKF favors permissive consumption) — a bad manifest just
/// means we fall back to the directory name.
fn read_bundle_manifest(bundle_root: &Path) -> (String, Option<String>) {
    let dir_name = bundle_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let manifest_path = bundle_root.join("bundle.json");
    let Ok(contents) = std::fs::read_to_string(&manifest_path) else {
        return (dir_name, None);
    };

    match serde_json::from_str::<serde_json::Value>(&contents) {
        Ok(json) => {
            let name = json
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
                .unwrap_or(dir_name);
            let description = json
                .get("description")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string);
            (name, description)
        }
        Err(_) => (dir_name, None),
    }
}

/// Reads the `okf_version` the bundle-root `index.md` declares (OKF §8, §12), the
/// only frontmatter key an index file may carry. The file is matched by name as
/// [`is_reserved_file`] matches it, and a symlinked one is not read (OKF §3.2), with a
/// warning. A missing file, frontmatter or key, or an empty or null value, gives `None`;
/// a file that cannot be read, frontmatter that is not a YAML mapping, or a value that
/// is neither a non-blank string nor a finite number gives `None` and a warning, never
/// a failed parse.
fn read_okf_version(bundle_root: &Path, warnings: &mut Vec<String>) -> Option<String> {
    let files = std::fs::read_dir(bundle_root)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file() || t.is_symlink()))
        .map(|e| e.path())
        .collect();
    let index = pick_root_index(files)?;
    let path = index.display();
    if index
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        warnings.push(format!(
            "Bundle index '{path}' is a symlink; its okf_version is not recorded"
        ));
        return None;
    }
    let content = match std::fs::read_to_string(&index) {
        Ok(content) => content,
        Err(e) => {
            warnings.push(format!(
                "Bundle index '{path}' could not be read ({e}); its okf_version is not recorded"
            ));
            return None;
        }
    };
    let yaml = match split_frontmatter(&content) {
        Frontmatter::Block { yaml, .. } => yaml,
        Frontmatter::Unclosed => {
            warnings.push(format!(
                "Bundle index '{path}' opens frontmatter with '---' but never closes it; its \
                 okf_version is not recorded"
            ));
            return None;
        }
        Frontmatter::None | Frontmatter::NearMiss => return None,
    };
    let value = match serde_yaml::from_str::<serde_yaml::Value>(yaml) {
        Ok(serde_yaml::Value::Mapping(mut m)) => {
            m.remove("okf_version").filter(|v| !is_empty_value(v))?
        }
        Ok(serde_yaml::Value::Null) => return None,
        Ok(_) => {
            warnings.push(format!(
                "Bundle index '{path}' has frontmatter that is not a mapping; its \
                 okf_version is not recorded"
            ));
            return None;
        }
        Err(_) => {
            warnings.push(format!(
                "Bundle index '{path}' has frontmatter that is not valid YAML; its \
                 okf_version is not recorded"
            ));
            return None;
        }
    };
    match &value {
        serde_yaml::Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        serde_yaml::Value::Number(n) if n.as_f64().is_some_and(f64::is_finite) => {
            Some(yaml_as_written(&value))
        }
        _ => {
            warnings.push(format!(
                "Bundle index '{path}' has an invalid 'okf_version' ({}); it is not recorded",
                yaml_as_written(&value)
            ));
            None
        }
    }
}

/// Picks the bundle-root index file among the root's file paths: `index.md` exactly
/// when present, otherwise the first, in byte order, whose name matches it ignoring
/// case (as [`is_reserved_file`] does), so the choice never depends on directory order.
fn pick_root_index(mut paths: Vec<std::path::PathBuf>) -> Option<std::path::PathBuf> {
    paths.sort();
    let name = |p: &std::path::PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned());
    paths
        .iter()
        .position(|p| name(p).as_deref() == Some("index.md"))
        .or_else(|| {
            paths
                .iter()
                .position(|p| name(p).is_some_and(|n| n.eq_ignore_ascii_case("index.md")))
        })
        .map(|i| paths.swap_remove(i))
}

/// A frontmatter value as `metadata-json` keeps it: a string as is, anything else as
/// JSON, or as YAML when JSON cannot hold it (a non-finite number, a non-string key).
fn kept_as_written(v: serde_yaml::Value) -> String {
    match v {
        serde_yaml::Value::String(s) => s,
        other if has_non_finite(&other) => yaml_as_written(&other),
        other => serde_json::to_string(&other).unwrap_or_else(|_| yaml_as_written(&other)),
    }
}

/// Whether `v` holds a NaN or infinite number, which JSON would turn into `null`.
fn has_non_finite(v: &serde_yaml::Value) -> bool {
    match v {
        serde_yaml::Value::Number(n) => n.as_f64().is_some_and(|f| !f.is_finite()),
        serde_yaml::Value::Sequence(seq) => seq.iter().any(has_non_finite),
        serde_yaml::Value::Mapping(m) => m
            .iter()
            .any(|(k, v)| has_non_finite(k) || has_non_finite(v)),
        serde_yaml::Value::Tagged(t) => has_non_finite(&t.value),
        _ => false,
    }
}

/// Recursively finds the `.md` files of the bundle under `dir`, sorted (OKF §3.2).
///
/// `bundle_root` must be canonical. A file or directory whose name starts with `.` is
/// not part of the bundle and is skipped quietly. A symlink is never followed: its
/// target is either outside the bundle root, which a consumer must not read, or inside
/// it, where the walk reads it at its own path. Each skipped symlink gets a warning
/// naming why, except a root-level reserved name, which [`read_okf_version`] reports.
fn walk_md_files(
    dir: &Path,
    bundle_root: &Path,
    warnings: &mut Vec<String>,
) -> Result<Vec<std::path::PathBuf>, ParseError> {
    let mut entries = std::fs::read_dir(dir)
        .map_err(|e| io_err(dir, e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| io_err(dir, e))?;
    entries.sort_by_key(|e| e.file_name());

    let mut results = Vec::new();
    for entry in entries {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| io_err(&path, e))?;
        if file_type.is_symlink() {
            if dir == bundle_root && is_reserved_file(&path) {
                continue;
            }
            let reason = match std::fs::canonicalize(&path) {
                Err(_) => "is broken",
                Ok(target) if !target.starts_with(bundle_root) => "points outside the bundle root",
                Ok(_) => "points inside the bundle, where its target is read at its own path",
            };
            warnings.push(format!(
                "Symlink '{}' {reason}; it is not followed",
                path.display()
            ));
        } else if file_type.is_dir() {
            results.extend(walk_md_files(&path, bundle_root, warnings)?);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            results.push(path);
        }
    }
    Ok(results)
}

// -- Helper functions for extracting typed values from YAML --

/// Joins `items` as an English list ending in "or": `a, b or c`.
fn or_list(items: &[&str]) -> String {
    match items.split_last() {
        None => String::new(),
        Some((last, [])) => (*last).to_string(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}

/// A frontmatter value shown as YAML, close to how the author wrote it (`.nan`,
/// `null`, `5`), on one line.
pub(crate) fn yaml_as_written(v: &serde_yaml::Value) -> String {
    serde_yaml::to_string(v)
        .map(|s| s.trim().replace('\n', " "))
        .unwrap_or_else(|_| format!("{v:?}"))
}

fn extract_string(map: &mut HashMap<String, serde_yaml::Value>, key: &str) -> Option<String> {
    map.remove(key).and_then(|v| match v {
        serde_yaml::Value::String(s) => Some(s),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    })
}

/// Takes `tags` out of `map` (OKF §11.2): one scalar is one tag, never split at
/// commas, and a list's scalar items are tags as written, with null items dropped. A
/// list or mapping item, or a `tags` mapping, is warned about; the raw `tags` then stays
/// in `map`, so it lands in `metadata-json` as written.
fn take_tags(
    map: &mut HashMap<String, serde_yaml::Value>,
    path: &str,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    use serde_yaml::Value;
    let scalar = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    };
    let mut warn = |key: &str, v: &Value| {
        warnings.push(format!(
            "Concept file '{path}' has an invalid '{key}' ({}); it is kept in metadata-json \
             and not typed",
            yaml_as_written(v)
        ));
    };
    let Some(v) = map.remove("tags") else {
        return Vec::new();
    };
    let mut keep = false;
    let tags = match &v {
        Value::Sequence(items) => items
            .iter()
            .enumerate()
            .filter(|(_, item)| !item.is_null())
            .filter_map(|(i, item)| {
                let tag = scalar(item);
                if tag.is_none() {
                    warn(&format!("tags[{i}]"), item);
                    keep = true;
                }
                tag
            })
            .collect(),
        other => match scalar(other) {
            Some(tag) => vec![tag],
            None => {
                warn("tags", other);
                keep = true;
                Vec::new()
            }
        },
    };
    if keep {
        map.insert("tags".to_string(), v);
    }
    tags
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The concept a file parses to; panics when the file is not a concept.
    fn parse_concept(
        file_path: &Path,
        bundle_root: &Path,
    ) -> Result<(OkfConcept, Vec<String>), ParseError> {
        match super::parse_concept(file_path, bundle_root)? {
            ParsedFile::Concept(c, w) => Ok((*c, w)),
            ParsedFile::NotAConcept(w) => panic!("not a concept: {w:?}"),
        }
    }

    /// Parses `content` as the file `page.md`.
    fn parse_file(content: &str) -> ParsedFile {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("page.md");
        fs::write(&path, content).unwrap();
        super::parse_concept(&path, dir.path()).unwrap()
    }

    /// The warning a file that is not a concept carries; panics on a concept or a quiet skip.
    fn skip_warning(content: &str) -> String {
        match parse_file(content) {
            ParsedFile::NotAConcept(Some(w)) => w,
            other => panic!("{content:?}: expected a skip with a warning, got {other:?}"),
        }
    }

    /// The concept `content` parses to; panics when it is not one.
    fn concept_of(content: &str) -> OkfConcept {
        match parse_file(content) {
            ParsedFile::Concept(c, _) => *c,
            other => panic!("{content:?}: expected a concept, got {other:?}"),
        }
    }

    fn is_quiet_skip(content: &str) -> bool {
        matches!(parse_file(content), ParsedFile::NotAConcept(None))
    }

    #[test]
    fn a_file_without_frontmatter_is_skipped_quietly() {
        assert!(is_quiet_skip("# Readme\n\nText.\n"));
    }

    #[test]
    fn a_file_without_type_is_not_a_concept() {
        for content in [
            "---\ntitle: X\n---\n",
            "---\ntype: ~\n---\n",
            "---\ntype: \"\"\n---\n",
            "---\ntype: \"  \"\n---\n",
            "---\n---\n",
            "---\n~\n---\n",
            "---\ntype: [a]\n---\n",
        ] {
            let w = skip_warning(content);
            assert!(
                w.contains("page.md") && w.contains("'type'"),
                "{content:?}: {w}"
            );
        }
    }

    #[test]
    fn an_unclosed_frontmatter_is_skipped_with_a_warning() {
        let w = skip_warning("---\ntype: guide\nbody");
        assert!(w.contains("page.md") && w.contains("never closes"), "{w}");
    }

    #[test]
    fn delimiters_are_exact_lines() {
        assert!(is_quiet_skip("----\ntype: x\n----\n"));
        assert!(is_quiet_skip("--- x\ntype: x\n---\n"));
        let w = skip_warning("---\ntype: guide\n----\n");
        assert!(w.contains("never closes"), "{w}");

        let crlf = concept_of("---\r\ntype: guide\r\ntitle: T\r\n---\r\nBody\r\n");
        assert_eq!(crlf.title.as_deref(), Some("T"));
        assert!(crlf.body.starts_with("Body"), "{:?}", crlf.body);

        let spaced = concept_of("--- \ntype: guide\n---\t\nBody");
        assert_eq!(spaced.concept_type, "guide");
        assert!(spaced.body.starts_with("Body"), "{:?}", spaced.body);
    }

    #[test]
    fn a_numeric_type_is_read_as_written() {
        assert_eq!(concept_of("---\ntype: 5\n---\n").concept_type, "5");
    }

    #[test]
    fn a_bom_before_frontmatter_is_ignored() {
        let c = concept_of("\u{feff}---\ntype: guide\n---\nBody");
        assert_eq!(c.concept_type, "guide");
    }

    #[test]
    fn a_near_miss_opener_is_skipped_with_a_warning() {
        for content in ["\n---\ntype: guide\n---\n", "  \n\n---\ntype: guide\n---\n"] {
            let w = skip_warning(content);
            assert!(
                w.contains("page.md") && w.contains("first line"),
                "{content:?}: {w}"
            );
        }
    }

    #[test]
    fn parse_bundle_skips_non_concepts() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("README.md"), "# Readme\n").unwrap();
        fs::write(dir.path().join("notype.md"), "---\ntitle: X\n---\n").unwrap();
        fs::write(dir.path().join("a.md"), "---\ntype: guide\n---\n").unwrap();
        let bundle = parse_bundle(dir.path()).unwrap();
        let ids: Vec<_> = bundle
            .concepts
            .iter()
            .map(|c| c.concept_id.as_str())
            .collect();
        assert_eq!(ids, ["a"]);
        assert_eq!(bundle.warnings.len(), 1, "{:?}", bundle.warnings);
        assert!(
            bundle.warnings[0].contains("notype.md"),
            "{:?}",
            bundle.warnings
        );
    }
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
            "tables/orders",
            file_to_concept_id(Path::new("bundle/tables/orders.md"), root)
        );
        assert_eq!(
            "simple",
            file_to_concept_id(Path::new("bundle/simple.md"), root)
        );
        assert_eq!(
            "a/b/c",
            file_to_concept_id(Path::new("bundle/a/b/c.md"), root)
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

    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn the_read_version_salts_the_file_hash() {
        let raw = "---\ntype: guide\n---\n";
        let c = concept_of(raw);
        assert_eq!(c.file_hash, concept_file_hash(raw.as_bytes()));
        assert_ne!(c.file_hash, sha256_hex(raw.as_bytes()));
        assert_eq!(c.file_hash.len(), 64);
    }

    #[test]
    fn test_parse_concept_full() {
        let dir = tempfile::tempdir().unwrap();
        create_test_bundle(dir.path());

        let concept = parse_concept(&dir.path().join("datasets/users.md"), dir.path())
            .unwrap()
            .0;

        assert_eq!(concept.concept_id, "datasets/users");
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

        let concept = parse_concept(&dir.path().join("simple.md"), dir.path())
            .unwrap()
            .0;

        assert_eq!(concept.concept_type, "Note");
        assert!(concept.title.is_none());
        assert!(concept.tags.is_empty());
        assert!(concept.body.contains("minimal concept"));
    }

    #[test]
    fn test_parse_concept_missing_type() {
        let dir = tempfile::tempdir().unwrap();
        create_invalid_bundle(dir.path());

        let result = super::parse_concept(&dir.path().join("no-type.md"), dir.path());
        assert!(matches!(result, Ok(ParsedFile::NotAConcept(Some(_)))));
    }

    #[test]
    fn test_parse_concept_program_none_for_documentation() {
        // metrics/revenue.md has a ```python fence but declares no engine, so its
        // code is documentation — no executable program is extracted.
        let dir = tempfile::tempdir().unwrap();
        create_test_bundle(dir.path());

        let concept = parse_concept(&dir.path().join("metrics/revenue.md"), dir.path())
            .unwrap()
            .0;

        assert!(concept.engine.is_none());
        assert!(concept.program.is_none());
    }

    #[test]
    fn test_parse_concept_program_concatenates_matching_fences() {
        // Engine-matched extraction: only fences whose language maps to the
        // declared engine form the program, concatenated in document order; other
        // languages (here python) are treated as prose.
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("calc.md"),
            r#"---
type: playbook
title: Calc
engine: quickjs
---

First we set up:

```js
const a = 1;
```

An illustrative aside (not executed):

```python
print("ignored")
```

Then compute:

```javascript
const b = a + 1;
```
"#,
        )
        .unwrap();

        let concept = parse_concept(&dir.path().join("calc.md"), dir.path())
            .unwrap()
            .0;

        assert_eq!(concept.engine, Some(ScriptEngine::QuickJs));
        let program = concept.program.expect("expected an executable program");
        assert_eq!(program, "const a = 1;\n\nconst b = a + 1;");
        assert!(!program.contains("ignored"));
    }

    #[test]
    fn test_parse_concept_gecko_extension_fields() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("playbook.md"),
            r#"---
type: Playbook
title: Containment
engine: quickjs
consumes: [alert, asset]
produces: [containment-result]
scopes: [mde:isolate]
timeout-ms: 5000
---

Playbook body.

```js
1
```
"#,
        )
        .unwrap();

        let concept = parse_concept(&dir.path().join("playbook.md"), dir.path())
            .unwrap()
            .0;

        assert_eq!(concept.engine, Some(ScriptEngine::QuickJs));
        assert_eq!(concept.extra_metadata["consumes"], r#"["alert","asset"]"#);
        assert_eq!(
            concept.extra_metadata["produces"],
            r#"["containment-result"]"#
        );
        assert_eq!(concept.scopes, vec!["mde:isolate"]);
        assert_eq!(concept.timeout_ms, Some(5000));
    }

    /// Parses a concept file with `frontmatter` lines and `body`.
    fn parse_with(frontmatter: &str, body: &str) -> Result<OkfConcept, ParseError> {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("playbook.md"),
            format!("---\ntype: playbook\n{frontmatter}\n---\n\n{body}"),
        )
        .unwrap();
        parse_concept(&dir.path().join("playbook.md"), dir.path()).map(|(c, _)| c)
    }

    const JS_FENCE: &str = "```js\nconsole.log(1)\n```\n";

    #[test]
    fn an_unsupported_engine_is_refused_as_written() {
        for (line, shown) in [
            ("engine: javscript", Some("javscript")),
            ("engine: python", Some("python")),
            ("engine: no", Some("no")),
            ("engine: on", Some("on")),
            ("engine: 1e3", Some("1000.0")),
            ("engine: 5", Some("5")),
            ("engine: true", Some("true")),
            ("engine: null", Some("null")),
            ("engine:", Some("null")),
            ("engine: [js]", None),
            ("engine: {a: 1}", None),
        ] {
            let err = parse_with(line, JS_FENCE).unwrap_err();
            let ParseError::InvalidEngine { value, .. } = &err else {
                panic!("{line}: {err}");
            };
            if let Some(shown) = shown {
                assert_eq!(value, shown, "{line}");
            }
            let msg = err.to_string();
            assert!(
                msg.contains("playbook.md") && msg.contains("one of js, javascript, quickjs"),
                "{line}: {msg}"
            );
        }
    }

    #[test]
    fn engine_spellings_from_lang_accepts_parse() {
        for line in ["engine: JS", "engine: \" javascript \"", "engine: quickjs"] {
            let concept = parse_with(line, JS_FENCE).unwrap_or_else(|e| panic!("{line}: {e}"));
            assert_eq!(concept.engine, Some(ScriptEngine::QuickJs), "{line}");
        }
    }

    #[test]
    fn a_declared_engine_without_a_program_is_refused() {
        for body in [
            "Prose only.\n",
            "```javscript\nconsole.log(1)\n```\n",
            "```python\nprint(1)\n```\n",
            "```js\n\n```\n",
            "```js\n```\n",
        ] {
            let err = parse_with("engine: quickjs", body).unwrap_err();
            let ParseError::MissingProgram { engine, .. } = &err else {
                panic!("{body:?}: {err}");
            };
            assert_eq!(engine, "quickjs");
            let msg = err.to_string();
            assert!(
                msg.contains("playbook.md")
                    && msg.contains("no code in a js, javascript or quickjs fence"),
                "{body:?}: {msg}"
            );
        }
    }

    #[test]
    fn a_blank_fence_beside_a_real_one_is_skipped() {
        let concept = parse_with(
            "engine: quickjs",
            "```js\n\n```\n\n```js\nconsole.log(1)\n```\n",
        )
        .unwrap();
        assert_eq!(concept.program.as_deref(), Some("console.log(1)"));
    }

    #[test]
    fn a_fence_language_is_matched_like_engine() {
        for body in [
            "```JS\nconsole.log(1)\n```\n",
            "```js   \nconsole.log(1)\n```\n",
        ] {
            let concept = parse_with("engine: quickjs", body).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(
                concept.program.as_deref(),
                Some("console.log(1)"),
                "{body:?}"
            );
        }
    }

    #[test]
    fn one_unrunnable_playbook_fails_the_whole_bundle() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("playbooks")).unwrap();
        fs::write(
            dir.path().join("playbooks/good.md"),
            format!("---\ntype: playbook\nengine: quickjs\n---\n\n{JS_FENCE}"),
        )
        .unwrap();
        fs::write(
            dir.path().join("playbooks/typo.md"),
            format!("---\ntype: playbook\nengine: javscript\n---\n\n{JS_FENCE}"),
        )
        .unwrap();
        let err = parse_bundle(dir.path()).unwrap_err();
        assert!(matches!(err, ParseError::InvalidEngine { .. }), "{err}");
        assert!(err.to_string().contains("typo.md"), "{err}");
    }

    #[test]
    fn a_fence_without_an_engine_stays_documentation() {
        let concept = parse_with("title: notes", JS_FENCE).unwrap();
        assert_eq!(concept.engine, None);
        assert_eq!(concept.program, None);
    }

    /// Parses a playbook whose frontmatter ends with `timeout_line` (none when empty).
    fn parse_timeout(timeout_line: &str) -> Result<OkfConcept, ParseError> {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("playbook.md"),
            format!("---\ntype: playbook\nengine: quickjs\n{timeout_line}\n---\n\n```js\n1\n```\n"),
        )
        .unwrap();
        parse_concept(&dir.path().join("playbook.md"), dir.path()).map(|(c, _)| c)
    }

    #[test]
    fn timeout_ms_accepts_an_integer_in_range() {
        assert_eq!(
            parse_timeout("timeout-ms: 500").unwrap().timeout_ms,
            Some(500)
        );
        assert_eq!(
            parse_timeout(&format!("timeout-ms: {MAX_TIMEOUT_MS}"))
                .unwrap()
                .timeout_ms,
            Some(MAX_TIMEOUT_MS)
        );
        assert_eq!(parse_timeout("").unwrap().timeout_ms, None);
    }

    #[test]
    fn an_invalid_timeout_is_shown_as_written() {
        let msg = parse_timeout("timeout-ms: .nan").unwrap_err().to_string();
        assert!(msg.to_lowercase().contains("nan"), "{msg}");
        let msg = parse_timeout("timeout-ms: [1, 2]").unwrap_err().to_string();
        assert!(msg.contains('1') && msg.contains('2'), "{msg}");
    }

    #[test]
    fn timeout_ms_refuses_anything_else() {
        for line in [
            "timeout-ms: 0",
            "timeout-ms: -1",
            "timeout-ms: 5.0",
            "timeout-ms: \"500ms\"",
            "timeout-ms: null",
            "timeout-ms:",
            "timeout-ms: 600001",
        ] {
            let err = parse_timeout(line).unwrap_err();
            assert!(
                matches!(err, ParseError::InvalidTimeout { .. }),
                "{line}: {err}"
            );
            let msg = err.to_string();
            assert!(
                msg.contains("playbook.md") && msg.contains("600000"),
                "{line}: {msg}"
            );
        }
    }

    #[test]
    fn test_parse_bundle() {
        let dir = tempfile::tempdir().unwrap();
        create_test_bundle(dir.path());

        let bundle = parse_bundle(dir.path()).unwrap();

        assert_eq!(bundle.concepts.len(), 3); // index.md skipped

        // Concept IDs are bundle-relative paths with no namespace prefix (OKF spec).
        let ids: HashSet<&str> = bundle
            .concepts
            .iter()
            .map(|c| c.concept_id.as_str())
            .collect();
        assert!(ids.contains("datasets/users"));
        assert!(ids.contains("tables/orders"));
        assert!(ids.contains("metrics/revenue"));

        // Check hierarchy edges exist (also prefix-free).
        let hierarchy_set: HashSet<(&str, &str)> = bundle
            .hierarchy
            .iter()
            .map(|h| (h.parent_id.as_str(), h.child_id.as_str()))
            .collect();
        assert!(hierarchy_set.contains(&("datasets", "datasets/users")));
        assert!(hierarchy_set.contains(&("tables", "tables/orders")));
        assert!(hierarchy_set.contains(&("metrics", "metrics/revenue")));
    }

    #[test]
    fn test_parse_bundle_reads_manifest() {
        let dir = tempfile::tempdir().unwrap();
        create_minimal_bundle(dir.path());
        fs::write(
            dir.path().join("bundle.json"),
            r#"{"name": "acme-playbooks", "description": "Acme's playbook bundle."}"#,
        )
        .unwrap();

        let bundle = parse_bundle(dir.path()).unwrap();
        assert_eq!(bundle.bundle_name, "acme-playbooks");
        assert_eq!(
            bundle.bundle_description.as_deref(),
            Some("Acme's playbook bundle.")
        );
    }

    #[test]
    fn test_parse_bundle_manifest_falls_back_to_dir_name() {
        let dir = tempfile::tempdir().unwrap();
        create_minimal_bundle(dir.path());
        // No bundle.json, and a malformed one should also fall back gracefully.
        fs::write(dir.path().join("bundle.json"), "{ not valid json").unwrap();

        let bundle = parse_bundle(dir.path()).unwrap();
        // Falls back to the (canonicalized) directory's file name.
        let expected = dir
            .path()
            .canonicalize()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(bundle.bundle_name, expected);
        assert!(bundle.bundle_description.is_none());
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
        let Frontmatter::Block { yaml, body } = split_frontmatter(content) else {
            panic!("expected a block");
        };
        assert!(yaml.contains("type: Test"));
        assert!(body.contains("Body text here"));
    }

    #[test]
    fn test_split_frontmatter_no_yaml() {
        assert_eq!(split_frontmatter("Just body text.\n"), Frontmatter::None);
    }

    #[test]
    fn test_split_frontmatter_malformed() {
        let content = "---\ntype: Test\ntitle: Hello\n\nNo closing dashes.\n";
        assert_eq!(split_frontmatter(content), Frontmatter::Unclosed);
    }

    #[test]
    fn test_parse_concept_empty_type() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("no_type.md");
        let content = "---\ntitle: No Type\n---\nBody";
        std::fs::write(&file_path, content).unwrap();

        let result = super::parse_concept(&file_path, dir.path());
        assert!(matches!(result, Ok(ParsedFile::NotAConcept(Some(_)))));
    }

    // -- OKF provenance --

    use super::super::types::{OkfProvenance, TrustTier, Verification};
    use chrono::{DateTime, Utc};

    const AT: &str = "2026-11-28T02:42:19.865Z";

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// Parses a concept with `frontmatter` lines, returning it with its warnings.
    fn parse_warned(frontmatter: &str) -> (OkfConcept, Vec<String>) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("page.md"),
            format!("---\ntype: guide\n{frontmatter}\n---\n\nBody.\n"),
        )
        .unwrap();
        parse_concept(&dir.path().join("page.md"), dir.path()).unwrap()
    }

    /// Asserts exactly one warning, naming the file, `key` and `value`.
    fn assert_one_warning(warnings: &[String], key: &str, value: &str) {
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let w = &warnings[0];
        assert!(
            w.contains("page.md") && w.contains(&format!("'{key}'")) && w.contains(value),
            "{w}"
        );
    }

    #[test]
    fn generated_fills_timestamp_and_generated_by() {
        let (c, w) = parse_warned(&format!(
            "generated: {{ by: openwiki/0.9.0, at: \"{AT}\" }}"
        ));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.timestamp, Some(utc(AT)));
        assert_eq!(c.provenance.generated_by.as_deref(), Some("openwiki/0.9.0"));
        assert!(!c.extra_metadata.contains_key("generated"));
    }

    #[test]
    fn an_unconventional_actor_is_accepted() {
        let (c, w) = parse_warned(&format!("generated: {{ by: claude-code, at: {AT} }}"));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.provenance.generated_by.as_deref(), Some("claude-code"));
    }

    #[test]
    fn generated_wins_over_a_legacy_timestamp() {
        let b = "2020-01-01T00:00:00Z";
        let (c, w) = parse_warned(&format!(
            "generated: {{ by: x/1, at: {AT} }}\ntimestamp: {b}"
        ));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.timestamp, Some(utc(AT)));
        assert_eq!(
            c.extra_metadata.get("timestamp").map(String::as_str),
            Some(b)
        );
    }

    #[test]
    fn a_legacy_timestamp_is_read_without_generated() {
        let (c, w) = parse_warned(&format!("timestamp: {AT}"));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.timestamp, Some(utc(AT)));
        assert!(!c.extra_metadata.contains_key("timestamp"));
    }

    #[test]
    fn a_legacy_timestamp_is_read_when_generated_at_is_bad() {
        let (c, w) = parse_warned(&format!(
            "generated: {{ by: x/1, at: soon }}\ntimestamp: {AT}"
        ));
        assert_one_warning(&w, "generated.at", "soon");
        assert_eq!(c.timestamp, Some(utc(AT)));
        assert_eq!(c.provenance.generated_by.as_deref(), Some("x/1"));
        assert!(!c.extra_metadata.contains_key("timestamp"));
        assert!(c.extra_metadata.contains_key("generated"));
    }

    #[test]
    fn generated_without_by_keeps_its_at() {
        let (c, w) = parse_warned(&format!("generated: {{ at: {AT} }}"));
        assert_one_warning(&w, "generated.by", "at");
        assert_eq!(c.timestamp, Some(utc(AT)));
        assert_eq!(c.provenance.generated_by, None);
        assert!(c.extra_metadata.contains_key("generated"));
    }

    #[test]
    fn an_unknown_generated_subkey_is_kept_without_warning() {
        let (c, w) = parse_warned(&format!("generated: {{ by: x/1, at: {AT}, model: m }}"));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.timestamp, Some(utc(AT)));
        assert_eq!(c.provenance.generated_by.as_deref(), Some("x/1"));
        assert!(c.extra_metadata["generated"].contains("model"));
    }

    #[test]
    fn an_unknown_verified_subkey_is_kept_without_warning() {
        let (c, w) = parse_warned(&format!("verified: [{{ by: a/1, at: {AT}, note: n }}]"));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.provenance.verifications.len(), 1);
        assert!(c.extra_metadata.contains_key("verified"));
    }

    #[test]
    fn a_malformed_timestamp_warns_and_is_kept() {
        for (line, key, value) in [
            ("timestamp: yesterday", "timestamp", "yesterday"),
            ("stale_after: next tuesday", "stale_after", "next tuesday"),
            (
                "stale_after: \"2026-06-01T00:00:00\"",
                "stale_after",
                "2026-06-01T00:00:00",
            ),
            // ISO 8601 without seconds: only RFC 3339 forms are typed.
            (
                "stale_after: 2026-06-30T14:00Z",
                "stale_after",
                "2026-06-30T14:00Z",
            ),
        ] {
            let (c, w) = parse_warned(line);
            assert_one_warning(&w, key, value);
            assert_eq!(c.timestamp, None, "{line}");
            let stale = (key == "stale_after").then_some(super::super::provenance::STALE_SENTINEL);
            assert_eq!(c.provenance.stale_after, stale, "{line}");
            assert!(c.extra_metadata.contains_key(key), "{line}");
        }
    }

    #[test]
    fn a_date_only_value_is_midnight_utc() {
        let at = utc("2026-06-30T00:00:00Z");
        let midnight = Some(at);
        for d in ["2026-06-30", "\"2026-06-30\""] {
            let (c, w) = parse_warned(&format!("stale_after: {d}"));
            assert!(w.is_empty(), "{d}: {w:?}");
            assert_eq!(c.provenance.stale_after, midnight, "{d}");
            assert!(!c.extra_metadata.contains_key("stale_after"));

            let (c, w) = parse_warned(&format!("generated: {{ by: a/1, at: {d} }}"));
            assert!(w.is_empty(), "{d}: {w:?}");
            assert_eq!(c.timestamp, midnight, "{d}");

            let (c, w) = parse_warned(&format!("verified: [{{ by: a/1, at: {d} }}]"));
            assert!(w.is_empty(), "{d}: {w:?}");
            assert_eq!(c.provenance.verifications[0].at, at, "{d}");
        }
    }

    #[test]
    fn not_quite_dates_are_invalid() {
        for v in [
            "+2026-6-01",
            "2026- 6-30",
            "2026-06- 1",
            "-026-06-30",
            "2026-06-30T14:00:00",
        ] {
            let (_, w) = parse_warned(&format!("stale_after: \"{v}\""));
            assert_one_warning(&w, "stale_after", v);
        }
    }

    /// Regression: chrono's RFC 3339 reader already takes these variants.
    #[test]
    fn rfc3339_variants_are_accepted() {
        for (v, want) in [
            ("2026-06-30t14:00:00z", "2026-06-30T14:00:00Z"),
            ("2026-06-30 14:00:00Z", "2026-06-30T14:00:00Z"),
            ("2026-06-30T14:00:00.5+02:00", "2026-06-30T12:00:00.5Z"),
        ] {
            let (c, w) = parse_warned(&format!("generated: {{ by: a/1, at: \"{v}\" }}"));
            assert!(w.is_empty(), "{v}: {w:?}");
            assert_eq!(c.timestamp, Some(utc(want)), "{v}");
        }
    }

    #[test]
    fn empty_and_null_values_are_absent() {
        for key in [
            "title",
            "description",
            "resource",
            "tags",
            "generated",
            "timestamp",
            "verified",
            "status",
            "stale_after",
        ] {
            for v in ["~", "\"\"", "[]"] {
                let (c, w) = parse_warned(&format!("{key}: {v}"));
                assert!(w.is_empty(), "{key}: {v}: {w:?}");
                assert!(!c.extra_metadata.contains_key(key), "{key}: {v}");
                assert_eq!(c.title, None, "{key}: {v}");
                assert_eq!(c.description, None, "{key}: {v}");
                assert_eq!(c.resource_uri, None, "{key}: {v}");
                assert!(c.tags.is_empty(), "{key}: {v}");
                assert_eq!(c.timestamp, None, "{key}: {v}");
                assert_eq!(c.provenance, OkfProvenance::default(), "{key}: {v}");
            }
        }
        let (c, w) = parse_warned("generated: { by: a/1, at: ~ }");
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.provenance.generated_by.as_deref(), Some("a/1"));
        assert_eq!(c.timestamp, None);
    }

    /// Regression: extension keys at any level are kept as written (OKF §4.1).
    #[test]
    fn extension_keys_land_in_metadata_json() {
        let (c, w) = parse_warned(&format!(
            "x-acme-owner: team:data\ngenerated: {{ by: a/1, at: {T1}, x-acme-run: 7 }}"
        ));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.extra_metadata["x-acme-owner"], "team:data");
        assert!(c.extra_metadata["generated"].contains("x-acme-run"));
        assert_eq!(c.provenance.generated_by.as_deref(), Some("a/1"));
    }

    #[test]
    fn a_scalar_tags_is_one_tag() {
        for (line, want) in [
            ("tags: \"a, b\"", "a, b"),
            ("tags: solo", "solo"),
            ("tags: 5", "5"),
        ] {
            let (c, w) = parse_warned(line);
            assert!(w.is_empty(), "{line}: {w:?}");
            assert_eq!(c.tags, [want], "{line}");
            assert!(!c.extra_metadata.contains_key("tags"), "{line}");
        }
    }

    #[test]
    fn tag_items_are_kept_as_written() {
        let (c, w) = parse_warned("tags: [a, 2024, true, ~]");
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.tags, ["a", "2024", "true"]);

        let (c, w) = parse_warned("tags: [a, [b]]");
        assert_one_warning(&w, "tags[1]", "b");
        assert_eq!(c.tags, ["a"]);
        assert!(c.extra_metadata.contains_key("tags"));

        let (c, w) = parse_warned("tags: { a: b }");
        assert_one_warning(&w, "tags", "a: b");
        assert!(c.tags.is_empty());
        assert!(c.extra_metadata.contains_key("tags"));
    }

    /// Regression: the one-item rule is OKF's, for `tags`; GECKO's `scopes` keeps its
    /// reading.
    #[test]
    fn a_scalar_scopes_is_unchanged() {
        let c = parse_with("engine: js\nscopes: mem:recall", JS_FENCE).unwrap();
        assert!(c.scopes.is_empty(), "{:?}", c.scopes);
    }

    #[test]
    fn an_unreadable_stale_after_is_stale() {
        use super::super::provenance::STALE_SENTINEL;
        for (v, shown) in [
            ("next tuesday", "next tuesday"),
            ("\"  \"", "'  '"),
            ("2026-06-30T14:00:00", "2026-06-30T14:00:00"),
        ] {
            let (c, w) = parse_warned(&format!("stale_after: {v}"));
            assert_eq!(c.provenance.stale_after, Some(STALE_SENTINEL), "{v}");
            assert!(c.extra_metadata.contains_key("stale_after"), "{v}");
            assert_one_warning(&w, "stale_after", shown);
            assert!(w[0].contains("stored as stale"), "{w:?}");
        }
    }

    #[test]
    fn unquoted_and_quoted_timestamps_read_alike() {
        let (a, _) = parse_warned("stale_after: 2026-06-01T00:00:00Z");
        let (b, _) = parse_warned("stale_after: \"2026-06-01T00:00:00Z\"");
        assert_eq!(a.provenance.stale_after, Some(utc("2026-06-01T00:00:00Z")));
        assert_eq!(a.provenance.stale_after, b.provenance.stale_after);
        assert!(!a.extra_metadata.contains_key("stale_after"));
    }

    #[test]
    fn an_offset_timestamp_is_the_same_instant() {
        let (c, w) = parse_warned("stale_after: 2026-06-01T09:00:00+09:00");
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.provenance.stale_after, Some(utc("2026-06-01T00:00:00Z")));
    }

    fn tier(verified: &str) -> Option<TrustTier> {
        parse_warned(verified).0.provenance.trust_tier
    }

    #[test]
    fn trust_tiers_follow_the_verifiers() {
        assert_eq!(tier("title: none"), None);
        assert_eq!(tier("verified: []"), None);
        let (bare, w) = parse_warned(&format!("verified: {{ by: openwiki/0.9.0, at: {AT} }}"));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(
            bare.provenance.trust_tier,
            Some(TrustTier::MachineConfirmed)
        );
        assert_eq!(
            bare.provenance.verifications,
            [Verification {
                by: "openwiki/0.9.0".into(),
                at: utc(AT)
            }]
        );
        assert_eq!(
            tier(&format!(
                "verified: [{{ by: a/1, at: {AT} }}, {{ by: human:alice, at: {AT} }}]"
            )),
            Some(TrustTier::HumanReviewed)
        );
        for by in ["HUMAN:alice", "\"human:\"", "\"human:  \""] {
            assert_eq!(
                tier(&format!("verified: [{{ by: {by}, at: {AT} }}]")),
                Some(TrustTier::MachineConfirmed),
                "{by}"
            );
        }
    }

    const T1: &str = "2026-06-01T00:00:00Z";
    const T2: &str = "2026-06-02T00:00:00Z";
    const T3: &str = "2026-06-03T00:00:00Z";

    /// Inputs of the currency rule, each with the tier it gives.
    fn currency_cases() -> Vec<(String, Option<TrustTier>)> {
        use TrustTier::{HumanReviewed as H, MachineConfirmed as M};
        vec![
            (
                format!(
                    "generated: {{ by: g/1, at: {T2} }}\n\
                     verified: [{{ by: human:alice, at: {T1} }}, {{ by: a/1, at: {T3} }}]"
                ),
                Some(M),
            ),
            (
                format!(
                    "generated: {{ by: g/1, at: {T2} }}\nverified: [{{ by: human:alice, at: {T2} }}]"
                ),
                Some(H),
            ),
            (
                format!("timestamp: {T2}\nverified: [{{ by: human:alice, at: {T1} }}]"),
                None,
            ),
            (
                format!("timestamp: {T2}\nverified: [{{ by: human:alice, at: {T3} }}]"),
                Some(H),
            ),
            (
                format!("generated: {{ at: {T2} }}\nverified: [{{ by: human:alice, at: {T1} }}]"),
                None,
            ),
            (
                format!("verified: [{{ by: human:alice, at: {T1} }}]"),
                Some(H),
            ),
            (
                format!("generated: {{ by: g/1 }}\nverified: [{{ by: human:alice, at: {T1} }}]"),
                Some(H),
            ),
            (
                format!(
                    "generated: {{ by: g/1, at: soon }}\nverified: [{{ by: human:alice, at: {T1} }}]"
                ),
                Some(H),
            ),
            (
                format!(
                    "generated: {{ by: g/1, at: {T3} }}\n\
                     verified: [{{ by: human:alice, at: {T1} }}, {{ by: a/1, at: {T2} }}]"
                ),
                None,
            ),
            ("verified: []".to_string(), None),
            ("verified: [{ by: b/1 }]".to_string(), None),
        ]
    }

    #[test]
    fn only_current_events_give_the_tier() {
        let (c, _) = parse_warned(&currency_cases()[0].0);
        assert_eq!(c.provenance.trust_tier, Some(TrustTier::MachineConfirmed));
        assert_eq!(c.provenance.verifications.len(), 2);
    }

    #[test]
    fn an_event_at_generated_at_is_current() {
        assert_eq!(tier(&currency_cases()[1].0), Some(TrustTier::HumanReviewed));
    }

    #[test]
    fn a_legacy_timestamp_is_the_currency_time() {
        assert_eq!(tier(&currency_cases()[2].0), None);
        assert_eq!(tier(&currency_cases()[3].0), Some(TrustTier::HumanReviewed));
    }

    #[test]
    fn generated_at_without_by_is_the_currency_time() {
        let (c, w) = parse_warned(&currency_cases()[4].0);
        assert_eq!(c.provenance.trust_tier, None);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("'generated.by'"), "{w:?}");
    }

    #[test]
    fn with_no_content_time_every_event_is_current() {
        for (input, want) in &currency_cases()[5..8] {
            assert_eq!(tier(input), *want, "{input}");
        }
    }

    #[test]
    fn no_current_event_writes_no_tier() {
        for (input, _) in &currency_cases()[8..] {
            assert_eq!(tier(input), None, "{input}");
        }
    }

    #[test]
    fn the_tier_agrees_with_the_stored_times() {
        for (input, want) in currency_cases() {
            let (c, _) = parse_warned(&input);
            let current: Vec<_> = c
                .provenance
                .verifications
                .iter()
                .filter(|e| c.timestamp.is_none_or(|t| e.at >= t))
                .collect();
            let from_graph = if current.iter().any(|e| e.by.starts_with("human:")) {
                Some(TrustTier::HumanReviewed)
            } else if current.is_empty() {
                None
            } else {
                Some(TrustTier::MachineConfirmed)
            };
            assert_eq!(c.provenance.trust_tier, from_graph, "{input}");
            assert_eq!(c.provenance.trust_tier, want, "{input}");
        }
    }

    #[test]
    fn a_malformed_verified_writes_no_tier() {
        let (c, w) = parse_warned("verified: 5");
        assert_one_warning(&w, "verified", "5");
        assert_eq!(c.provenance.trust_tier, None);
        assert!(c.provenance.verifications.is_empty());
        assert!(c.extra_metadata.contains_key("verified"));
    }

    #[test]
    fn a_bad_verified_entry_is_skipped() {
        let (c, w) = parse_warned(&format!(
            "verified: [{{ by: a/1, at: {AT} }}, {{ by: b/1 }}]"
        ));
        assert_one_warning(&w, "verified[1]", "b/1");
        assert_eq!(c.provenance.verifications.len(), 1);
        assert_eq!(c.provenance.trust_tier, Some(TrustTier::MachineConfirmed));
        assert!(c.extra_metadata.contains_key("verified"));

        let (c, w) = parse_warned("verified: [{ by: b/1 }]");
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(c.provenance.trust_tier, None);
    }

    #[test]
    fn a_duplicate_verification_event_is_written_once() {
        let (c, w) = parse_warned(&format!(
            "verified: [{{ by: a/1, at: {AT} }}, {{ by: a/1, at: {AT} }}]"
        ));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.provenance.verifications.len(), 1);
    }

    #[test]
    fn status_is_stored_as_written() {
        let (c, w) = parse_warned("status: published");
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.provenance.status.as_deref(), Some("published"));
        assert!(!c.extra_metadata.contains_key("status"));

        let (c, w) = parse_warned("status: 3");
        assert_one_warning(&w, "status", "3");
        assert_eq!(c.provenance.status, None);
        assert!(c.extra_metadata.contains_key("status"));
    }

    #[test]
    fn a_blank_status_stays_malformed() {
        let (c, w) = parse_warned("status: \"  \"");
        assert_one_warning(&w, "status", "'  '");
        assert_eq!(c.provenance.status, None);

        let (c, w) = parse_warned(&format!("verified: [{{ by: \"  \", at: {AT} }}]"));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("'verified[0]'"), "{w:?}");
        assert_eq!(c.provenance.trust_tier, None);
    }

    #[test]
    fn a_v01_concept_has_no_provenance() {
        let (c, w) = parse_warned("title: Plain\ntags: [a]");
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.provenance, OkfProvenance::default());
        assert_eq!(c.timestamp, None);
    }

    #[test]
    fn parse_bundle_collects_warnings_in_file_order() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["b.md", "a.md"] {
            fs::write(
                dir.path().join(name),
                format!("---\ntype: guide\nstale_after: bad-{name}\n---\n"),
            )
            .unwrap();
        }
        let bundle = parse_bundle(dir.path()).unwrap();
        assert_eq!(bundle.warnings.len(), 2, "{:?}", bundle.warnings);
        assert!(
            bundle.warnings[0].contains("bad-a.md"),
            "{:?}",
            bundle.warnings
        );
        assert!(
            bundle.warnings[1].contains("bad-b.md"),
            "{:?}",
            bundle.warnings
        );
    }

    /// Parses a one-concept bundle whose root `index.md` (when given) holds `index`.
    fn bundle_with_index(index: Option<&str>) -> OkfBundle {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("page.md"), "---\ntype: guide\n---\n").unwrap();
        if let Some(index) = index {
            fs::write(dir.path().join("index.md"), index).unwrap();
        }
        parse_bundle(dir.path()).unwrap()
    }

    #[test]
    fn okf_version_is_read_from_the_root_index() {
        for (line, version) in [
            ("okf_version: \"0.2\"", "0.2"),
            ("okf_version: 0.2", "0.2"),
            ("okf_version: \"9.9\"", "9.9"),
        ] {
            let b = bundle_with_index(Some(&format!("---\n{line}\n---\n\n# Files\n")));
            assert_eq!(b.okf_version.as_deref(), Some(version), "{line}");
            assert!(b.warnings.is_empty(), "{:?}", b.warnings);
        }
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Index.md"),
            "---\nokf_version: \"0.2\"\n---\n",
        )
        .unwrap();
        assert_eq!(
            parse_bundle(dir.path()).unwrap().okf_version.as_deref(),
            Some("0.2")
        );
    }

    #[test]
    fn a_missing_or_keyless_root_index_records_no_version() {
        for index in [None, Some("# Files\n"), Some("---\ntitle: x\n---\n")] {
            let b = bundle_with_index(index);
            assert_eq!(b.okf_version, None, "{index:?}");
            assert!(b.warnings.is_empty(), "{:?}", b.warnings);
        }
    }

    #[test]
    fn an_invalid_okf_version_warns() {
        for line in ["okf_version: null", "okf_version:", "okf_version: \"\""] {
            let b = bundle_with_index(Some(&format!("---\n{line}\n---\n")));
            assert_eq!(b.okf_version, None, "{line}");
            assert!(b.warnings.is_empty(), "{line}: {:?}", b.warnings);
        }
        for (line, shown) in [
            ("okf_version: true", "true"),
            ("okf_version: [0.2]", "0.2"),
            ("okf_version: .nan", ".nan"),
            ("okf_version: .inf", ".inf"),
        ] {
            let b = bundle_with_index(Some(&format!("---\n{line}\n---\n")));
            assert_eq!(b.okf_version, None, "{line}");
            assert_eq!(b.warnings.len(), 1, "{line}: {:?}", b.warnings);
            let w = &b.warnings[0];
            assert!(
                w.contains("index.md") && w.contains("'okf_version'") && w.contains(shown),
                "{w}"
            );
        }
        for (bad, says) in [
            ("---\nokf_version: [\n---\n", "not valid YAML"),
            ("---\njust a scalar\n---\n", "not a mapping"),
        ] {
            let b = bundle_with_index(Some(bad));
            assert_eq!(b.okf_version, None);
            assert_eq!(b.warnings.len(), 1, "{bad:?}: {:?}", b.warnings);
            assert!(b.warnings[0].contains(says), "{:?}", b.warnings);
        }
    }

    #[test]
    fn a_subdirectory_index_version_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(
            dir.path().join("sub/index.md"),
            "---\nokf_version: \"0.2\"\n---\n",
        )
        .unwrap();
        let b = parse_bundle(dir.path()).unwrap();
        assert_eq!(b.okf_version, None);
        assert!(b.warnings.is_empty(), "{:?}", b.warnings);
    }

    #[test]
    fn a_kept_value_without_a_json_form_is_kept_as_yaml() {
        let (c, w) = parse_warned("status: .nan");
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(c.extra_metadata["status"], ".nan");

        let (c, _) = parse_warned("generated: { by: x/1, ? [a, b] : c }");
        let kept = &c.extra_metadata["generated"];
        assert!(
            kept.contains("x/1") && kept.contains('a') && kept.contains('c'),
            "{kept:?}"
        );
    }

    #[test]
    fn an_unreadable_root_index_warns() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("index.md"), [0xff, 0xfe, 0x00]).unwrap();
        let b = parse_bundle(dir.path()).unwrap();
        assert_eq!(b.okf_version, None);
        assert_eq!(b.warnings.len(), 1, "{:?}", b.warnings);
        assert!(
            b.warnings[0].contains("could not be read"),
            "{:?}",
            b.warnings
        );
    }

    #[test]
    fn the_exact_index_name_is_preferred() {
        let names = |ns: &[&str]| ns.iter().map(std::path::PathBuf::from).collect::<Vec<_>>();
        assert_eq!(
            pick_root_index(names(&["Index.md", "index.md", "INDEX.md"])),
            Some("index.md".into())
        );
        assert_eq!(
            pick_root_index(names(&["Index.md", "INDEX.md", "notes.md"])),
            Some("INDEX.md".into()),
            "without the exact name, the first in byte order"
        );
        assert_eq!(pick_root_index(names(&["notes.md"])), None);
    }

    // -- The bundle walk (OKF §3.2) --

    const GUIDE: &str = "---\ntype: guide\n---\n";

    fn ids(bundle: &OkfBundle) -> Vec<&str> {
        bundle
            .concepts
            .iter()
            .map(|c| c.concept_id.as_str())
            .collect()
    }

    #[test]
    fn dot_paths_are_not_part_of_the_bundle() {
        let dir = tempfile::tempdir().unwrap();
        for rel in [".hidden/x.md", ".draft.md", "sub/.y.md", "sub/z.md"] {
            let path = dir.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, GUIDE).unwrap();
        }
        let bundle = parse_bundle(dir.path()).unwrap();
        assert_eq!(ids(&bundle), ["sub/z"]);
        assert!(bundle.warnings.is_empty(), "{:?}", bundle.warnings);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_never_followed() {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("out.md"), GUIDE).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("real.md"), GUIDE).unwrap();
        symlink(outside.path().join("out.md"), root.join("a-out.md")).unwrap();
        symlink(root.join("missing.md"), root.join("b-broken.md")).unwrap();
        symlink(root.join("real.md"), root.join("c-alias.md")).unwrap();
        symlink(root, root.join("d-loop")).unwrap();

        let bundle = parse_bundle(root).unwrap();
        assert_eq!(ids(&bundle), ["real"]);
        let w = &bundle.warnings;
        assert_eq!(w.len(), 4, "{w:?}");
        for (warning, (name, reason)) in w.iter().zip([
            ("a-out.md", "outside the bundle root"),
            ("b-broken.md", "broken"),
            ("c-alias.md", "inside the bundle"),
            ("d-loop", "inside the bundle"),
        ]) {
            assert!(
                warning.contains(name) && warning.contains(reason),
                "{name}: {warning}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_root_index_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("meta.txt"),
            "---\nokf_version: \"0.3\"\n---\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(dir.path().join("meta.txt"), dir.path().join("index.md"))
            .unwrap();
        let bundle = parse_bundle(dir.path()).unwrap();
        assert_eq!(bundle.okf_version, None);
        assert_eq!(bundle.warnings.len(), 1, "{:?}", bundle.warnings);
        assert!(
            bundle.warnings[0].contains("index.md"),
            "{:?}",
            bundle.warnings
        );
    }

    /// Regression: the bundle path itself may be a symlink.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_bundle_root_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let bundle_dir = dir.path().join("bundle");
        fs::create_dir(&bundle_dir).unwrap();
        fs::write(bundle_dir.join("page.md"), GUIDE).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&bundle_dir, &link).unwrap();
        let bundle = parse_bundle(&link).unwrap();
        assert_eq!(ids(&bundle), ["page"]);
        assert!(bundle.warnings.is_empty(), "{:?}", bundle.warnings);
    }
}
