//! Link and code block extraction from markdown bodies.
//!
//! Extracts markdown links and code fences from concept bodies, and resolves relative
//! link paths to concept IDs.

use regex::Regex;
use std::sync::LazyLock;

use super::types::{OkfCitation, OkfLink, ScriptEngine};

/// Matches markdown links `[text](target)`. Group 1 captures an optional leading `!`
/// (marking an image link), group 2 the link text, group 3 the target.
static LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(!?)\[([^\]]+)\]\(([^)]+)\)").unwrap());

/// Matches fenced code blocks, capturing the info-string language tag (group 1)
/// and the block body (group 2). Extra info-string tokens after the language are
/// ignored; an untagged fence yields an empty language.
static CODE_BLOCK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```(\w*)[^\n]*\n([\s\S]*?)\n```").unwrap());

/// Extracts internal concept links and external citations from a markdown body, resolving
/// internal targets relative to the source concept.
pub fn extract_links(body: &str, source_concept_id: &str) -> (Vec<OkfLink>, Vec<OkfCitation>) {
    let mut links = Vec::new();
    let mut citations = Vec::new();

    for cap in LINK_RE.captures_iter(body) {
        if &cap[1] == "!" {
            continue; // image link, not a markdown link
        }
        let text = cap[2].trim();
        let target = cap[3].trim();

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

/// A fenced code block: its language tag (empty when the fence is untagged) and
/// its body, both trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeFence {
    pub lang: String,
    pub code: String,
}

/// Extracts every fenced code block with its language tag, in document order.
pub fn extract_code_fences(body: &str) -> Vec<CodeFence> {
    CODE_BLOCK_RE
        .captures_iter(body)
        .map(|cap| CodeFence {
            lang: cap[1].trim().to_string(),
            code: cap[2].trim().to_string(),
        })
        .collect()
}

/// Extracts a concept's executable program: the fenced code blocks whose language
/// resolves to `engine`, concatenated in document order (one program per concept).
///
/// Fences in any other language — a *different* engine, or a non-executable
/// language like `python`/`sql`/`mermaid` — are treated as documentation and
/// skipped, which is what lets a concept interleave prose, illustrative snippets,
/// and its actual program. Returns `None` when the concept declares no engine, or
/// declares one but has no matching fence.
pub fn extract_program(body: &str, engine: Option<&ScriptEngine>) -> Option<String> {
    let engine = engine?;
    let parts: Vec<String> = extract_code_fences(body)
        .into_iter()
        .filter(|f| ScriptEngine::from_lang(&f.lang).as_ref() == Some(engine))
        .map(|f| f.code)
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

/// Resolves a (possibly relative) markdown link target to a normalized concept ID,
/// relative to the source concept's directory.
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
    fn adjacent_links_both_extracted() {
        // Regression test: two links with no separating character between them must
        // both be extracted, not have the second one swallowed by the first match.
        let (links, _) = extract_links("[a](a.md)[c](c.md)", "root");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target_id, "a");
        assert_eq!(links[1].target_id, "c");

        // Image links are still skipped even when adjacent to a real link.
        let (links, _) = extract_links("![img](x.md)[real](y.md)", "root");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_id, "y");
    }

    #[test]
    fn test_extract_code_fences() {
        let body =
            "Some text\n```python\ndef foo():\n    pass\n```\nMore text\n```sql\nSELECT 1;\n```\n";
        let fences = extract_code_fences(body);
        assert_eq!(fences.len(), 2);
        assert_eq!(fences[0].lang, "python");
        assert!(fences[0].code.contains("def foo()"));
        assert_eq!(fences[1].lang, "sql");
        assert!(fences[1].code.contains("SELECT 1"));
    }

    #[test]
    fn test_extract_code_fences_empty() {
        assert!(extract_code_fences("No code blocks here.").is_empty());
    }

    #[test]
    fn test_extract_code_fences_untagged() {
        let fences = extract_code_fences("```\nplain\n```\n");
        assert_eq!(fences.len(), 1);
        assert_eq!(fences[0].lang, "");
        assert_eq!(fences[0].code, "plain");
    }

    #[test]
    fn test_extract_program_matches_engine_only() {
        // Only the js fence resolves to quickjs; the python fence is documentation.
        let body = "Intro.\n```python\nprint('doc only')\n```\nCode:\n```js\nconst a = 1;\n```\n";
        let program = extract_program(body, Some(&ScriptEngine::QuickJs)).unwrap();
        assert!(program.contains("const a = 1;"));
        assert!(!program.contains("doc only"));
    }

    #[test]
    fn test_extract_program_concatenates_in_document_order() {
        let body = "```js\nconst a = 1;\n```\nprose\n```javascript\nconst b = a + 1;\n```\n";
        let program = extract_program(body, Some(&ScriptEngine::QuickJs)).unwrap();
        assert_eq!(program, "const a = 1;\n\nconst b = a + 1;");
    }

    #[test]
    fn test_extract_program_none_without_engine() {
        assert!(extract_program("```js\nconst a = 1;\n```\n", None).is_none());
    }

    #[test]
    fn test_extract_program_none_when_no_fence_matches() {
        // Engine declared, but the only fence is a different (non-executable) language.
        assert!(
            extract_program("```python\nprint(1)\n```\n", Some(&ScriptEngine::QuickJs)).is_none()
        );
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
