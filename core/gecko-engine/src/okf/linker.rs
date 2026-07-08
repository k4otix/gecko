//! Link and code block extraction from markdown bodies.
//!
//! Link extraction and path resolution.

use regex::Regex;
use std::sync::LazyLock;

use super::types::{OkfCitation, OkfLink};

/// Matches markdown links `[text](target)`, excluding images `![text](target)`.
static LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^!])\[([^\]]+)\]\(([^)]+)\)").unwrap());

/// Matches fenced code blocks.
static CODE_BLOCK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```\w*\n([\s\S]*?)\n```").unwrap());

/// Extracts internal concept links and external citations from a markdown body.
///
/// Extracts all markdown links (ignoring image links) and attempts to resolve them.
pub fn extract_links(body: &str, source_concept_id: &str) -> (Vec<OkfLink>, Vec<OkfCitation>) {
    let mut links = Vec::new();
    let mut citations = Vec::new();

    for cap in LINK_RE.captures_iter(body) {
        let text = cap[1].trim();
        let target = cap[2].trim();

        if target.starts_with("http://") || target.starts_with("https://") {
            // External citation
            citations.push(OkfCitation {
                source_id: source_concept_id.to_string(),
                target_url: target.to_string(),
                link_text: text.to_string(),
            });
        } else if target.ends_with(".md") || target.contains(".md#") || target.contains(".md?") {
            // Internal concept link
            let target_id = resolve_relative_path(target, source_concept_id);
            links.push(OkfLink {
                source_id: source_concept_id.to_string(),
                target_id,
                link_text: text.to_string(),
            });
        }
    }

    (links, citations)
}

/// Extracts code block content from fenced code blocks in markdown.
///
/// Extracts GECKO-specific executable code blocks (e.g. `rhai`, `javascript`).
pub fn extract_code_blocks(body: &str) -> Vec<String> {
    CODE_BLOCK_RE
        .captures_iter(body)
        .map(|cap| cap[1].trim().to_string())
        .collect()
}

/// Resolves a relative link path to a normalized concept ID.
///
/// Resolves a potentially relative path against a source document's namespace.
///
/// # Example
/// ```
/// use gecko_engine::okf::linker::resolve_relative_path;
///
/// assert_eq!(
///     resolve_relative_path("../tables/orders.md#schema", "datasets/users"),
///     "tables/orders"
/// );
/// ```
pub fn resolve_relative_path(target: &str, source_concept_id: &str) -> String {
    // Strip anchors and query parameters
    let path_part = target.split('#').next().unwrap_or(target);
    let path_part = path_part.split('?').next().unwrap_or(path_part);

    // Strip trailing .md
    let path_part = path_part.strip_suffix(".md").unwrap_or(path_part);

    // GECKO is single-bundle-scoped, so concept IDs are plain bundle-relative
    // paths. The source directory is everything before the last `/`.
    let source_dir = match source_concept_id.rfind('/') {
        Some(idx) => &source_concept_id[..idx],
        None => "", // root-level concept
    };

    // OKF absolute links begin with `/` and are bundle-root-relative; relative
    // links resolve against the source concept's directory.
    let combined = if source_dir.is_empty() || path_part.starts_with('/') {
        path_part.trim_start_matches('/').to_string()
    } else {
        format!("{source_dir}/{path_part}")
    };

    // Normalize by resolving `.` and `..` components.
    let mut parts: Vec<&str> = Vec::new();
    for component in combined.split('/') {
        match component {
            "." | "" => continue,
            ".." => {
                parts.pop();
            }
            _ => parts.push(component),
        }
    }

    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_relative_path_basic() {
        assert_eq!(
            resolve_relative_path("../tables/orders.md#schema", "datasets/users"),
            "tables/orders"
        );
    }

    #[test]
    fn test_resolve_relative_path_same_dir() {
        assert_eq!(
            resolve_relative_path("sibling.md", "datasets/users"),
            "datasets/sibling"
        );
    }

    #[test]
    fn test_resolve_relative_path_from_root() {
        assert_eq!(
            resolve_relative_path("sub/thing.md", "root_concept"),
            "sub/thing"
        );
    }

    #[test]
    fn test_resolve_relative_path_with_query() {
        assert_eq!(
            resolve_relative_path("other.md?version=2", "a/b"),
            "a/other"
        );
    }

    #[test]
    fn test_extract_links_mixed() {
        let body = r#"See [Orders](../tables/orders.md) for details.
Also check [Google](https://google.com) and ![image](pic.png).
And [Config](config.md#section)."#;

        let (links, citations) = extract_links(body, "datasets/users");

        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target_id, "tables/orders");
        assert_eq!(links[0].link_text, "Orders");
        assert_eq!(links[1].target_id, "datasets/config");
        assert_eq!(links[1].link_text, "Config");

        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].target_url, "https://google.com");
        assert_eq!(citations[0].link_text, "Google");
    }

    #[test]
    fn test_extract_links_excludes_images() {
        let body = "![alt](image.md)\n[real](link.md)";
        let (links, _) = extract_links(body, "root");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].link_text, "real");
    }

    #[test]
    fn test_extract_code_blocks() {
        let body =
            "Some text\n```python\ndef foo():\n    pass\n```\nMore text\n```sql\nSELECT 1;\n```\n";
        let blocks = extract_code_blocks(body);
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].contains("def foo()"));
        assert!(blocks[1].contains("SELECT 1"));
    }

    #[test]
    fn test_extract_code_blocks_empty() {
        let body = "No code blocks here.";
        let blocks = extract_code_blocks(body);
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_resolve_relative_path_absolute() {
        // OKF absolute links (leading `/`) are bundle-root-relative.
        assert_eq!(
            resolve_relative_path("/tables/orders.md", "datasets/users"),
            "tables/orders"
        );
    }

    #[test]
    fn test_resolve_relative_path_nested_source() {
        assert_eq!(
            resolve_relative_path("../orders.md", "a/b/users"),
            "a/orders"
        );
    }
}
