//! A token-level scanner that finds the type statements a TypeQL schema fragment makes.

/// What a schema statement does to the type it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// `entity <label> …`.
    Entity,
    /// `relation <label> …`.
    Relation,
    /// `attribute <label> …`.
    Attribute,
    /// `<label> relates|owns|plays|sub|alias|value|@annotation …`, or a `redefine` of
    /// `<label>`: extends a type introduced elsewhere.
    Touch,
}

/// One type statement in a schema fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TypeHead {
    pub(super) kind: Kind,
    pub(super) label: String,
    /// The supertype named by the statement's `sub` clause.
    pub(super) sub: Option<String>,
    /// Whether any `relates X as Y` clause appears in the statement.
    pub(super) relates_as: bool,
}

/// Returns the type statements of `fragment`, ignoring comments, string literals and
/// function bodies.
pub(super) fn type_heads(fragment: &str) -> Vec<TypeHead> {
    let tokens = tokenize(&strip_comments_and_strings(fragment));
    tokens
        .split(|t| t == ";")
        .filter_map(statement_head)
        .collect()
}

/// Removes `#`-to-end-of-line comments and `"…"` / `'…'` literals (honoring `\` escapes),
/// keeping line breaks. An unterminated quote is kept as ordinary text, so it cannot hide
/// the statements after it.
fn strip_comments_and_strings(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            quote @ ('"' | '\'') => match closing_quote(&chars, i + 1, quote) {
                Some(end) => {
                    out.push(' ');
                    i = end + 1;
                }
                None => {
                    out.push(quote);
                    i += 1;
                }
            },
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// The index of the `quote` that closes a literal whose body starts at `start`.
fn closing_quote(chars: &[char], start: usize, quote: char) -> Option<usize> {
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            c if c == quote => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// Splits on whitespace and `, ( ) : { }`, keeping each `;` as its own token and starting a
/// new token at each `@`, since a label ends before an attached annotation.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if c.is_whitespace() || matches!(c, ',' | '(' | ')' | ':' | '{' | '}' | ';') {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            if c == ';' {
                tokens.push(";".to_owned());
            }
        } else {
            if c == '@' && !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            current.push(c);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// TypeQL keywords that can open a statement inside a query or function body.
const KEYWORDS: &[&str] = &[
    "define",
    "redefine",
    "undefine",
    "match",
    "fetch",
    "insert",
    "put",
    "update",
    "delete",
    "select",
    "sort",
    "offset",
    "limit",
    "reduce",
    "require",
    "distinct",
    "return",
    "fun",
    "with",
    "let",
    "check",
    "first",
    "last",
    "or",
    "not",
    "try",
    "is",
    "isa",
    "isa!",
    "has",
    "links",
    "iid",
    "in",
    "of",
    "from",
    "as",
    "entity",
    "relation",
    "attribute",
    "struct",
    "relates",
    "owns",
    "plays",
    "sub",
    "sub!",
    "value",
    "alias",
    "label",
    "true",
    "false",
];

/// Whether `token` can be a type label: it starts with `_` or a letter, as TypeQL identifiers
/// do, and is not a keyword. Anything else (a variable, an annotation, punctuation) is not.
fn is_label(token: &str) -> bool {
    token.starts_with(|c: char| c == '_' || c.is_alphabetic()) && !KEYWORDS.contains(&token)
}

fn statement_head(statement: &[String]) -> Option<TypeHead> {
    let (redefine, rest) = match statement.first().map(String::as_str) {
        Some("define") => (false, &statement[1..]),
        Some("redefine") => (true, &statement[1..]),
        _ => (false, statement),
    };
    let first = rest.first()?.as_str();
    let declared = match first {
        "entity" => Some(Kind::Entity),
        "relation" => Some(Kind::Relation),
        "attribute" => Some(Kind::Attribute),
        _ => None,
    };
    let (kind, label, clauses) = match declared {
        Some(kind) => {
            let label = rest.get(1)?;
            let kind = if redefine { Kind::Touch } else { kind };
            (kind, label, &rest[2..])
        }
        None => {
            let second = rest.get(1)?.as_str();
            let extends = matches!(
                second,
                "relates" | "owns" | "plays" | "sub" | "alias" | "value"
            ) || second.starts_with('@');
            if !(is_label(first) && extends) {
                return None;
            }
            (Kind::Touch, &rest[0], &rest[1..])
        }
    };
    if !is_label(label) {
        return None;
    }
    let sub = clauses
        .iter()
        .position(|t| t == "sub")
        .and_then(|i| clauses.get(i + 1))
        .cloned();
    // `as` occurs in a type statement only as a `relates X as Y` override, which may carry
    // annotations between the role and the `as`.
    let relates_as = clauses.iter().any(|t| t == "relates") && clauses.iter().any(|t| t == "as");
    Some(TypeHead {
        kind,
        label: label.clone(),
        sub,
        relates_as,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemGecko;
    use gecko_extension_api::GeckoExtension;

    fn heads_named<'a>(heads: &'a [TypeHead], label: &str) -> Vec<&'a TypeHead> {
        heads.iter().filter(|h| h.label == label).collect()
    }

    #[test]
    fn scanner_finds_relation_heads_in_mem() {
        let mem = MemGecko::new();
        let heads = type_heads(mem.schema());
        for label in ["supersession", "scoped"] {
            let found = heads_named(&heads, label);
            assert!(
                found
                    .iter()
                    .any(|h| h.kind == Kind::Relation && h.sub.is_none()),
                "no un-subtyped relation head for {label}: {found:?}"
            );
        }
    }

    #[test]
    fn scanner_ignores_doc_strings_and_comments() {
        let heads = type_heads(
            "define\n  entity a @doc(\"relation fake-rel, relates x\");\n  # relation also-fake\n",
        );
        assert!(heads_named(&heads, "fake-rel").is_empty());
        assert!(heads_named(&heads, "also-fake").is_empty());
        assert_eq!(heads_named(&heads, "a")[0].kind, Kind::Entity);
    }

    #[test]
    fn scanner_strips_single_quoted_strings() {
        let heads = type_heads(
            "define relation my-edge @doc('a;b # c\\'d'), sub supersession, relates claim;",
        );
        assert_eq!(heads.len(), 1, "{heads:?}");
        assert_eq!(heads[0].label, "my-edge");
        assert_eq!(heads[0].sub.as_deref(), Some("supersession"));
    }

    #[test]
    fn scanner_keeps_text_after_an_unterminated_quote() {
        let heads = type_heads("define entity a @doc(\"oops);\n relation hidden-link, relates x;");
        assert_eq!(heads_named(&heads, "hidden-link").len(), 1, "{heads:?}");
    }

    #[test]
    fn scanner_splits_an_attached_annotation() {
        let heads = type_heads(
            "define relation my-edge@abstract, sub r1; x-link@doc(\"x\") relates q as p;",
        );
        assert_eq!(heads_named(&heads, "my-edge")[0].sub.as_deref(), Some("r1"));
        let touch = heads_named(&heads, "x-link");
        assert_eq!(touch[0].kind, Kind::Touch);
        assert!(touch[0].relates_as);
    }

    #[test]
    fn scanner_keeps_non_lowercase_labels() {
        let heads = type_heads("define entity Mid sub a; entity _low sub Mid; Upper owns x;");
        assert_eq!(heads_named(&heads, "Mid")[0].sub.as_deref(), Some("a"));
        assert_eq!(heads_named(&heads, "_low")[0].sub.as_deref(), Some("Mid"));
        assert_eq!(heads_named(&heads, "Upper")[0].kind, Kind::Touch);
    }

    #[test]
    fn scanner_records_annotation_only_touch() {
        let heads = type_heads("define x-link @abstract;");
        let touch = heads_named(&heads, "x-link");
        assert_eq!(touch.len(), 1);
        assert_eq!(touch[0].kind, Kind::Touch);
    }

    #[test]
    fn scanner_distinguishes_prefix_labels() {
        let heads = type_heads("define relation probe-link-x, relates a, relates b;");
        assert!(heads_named(&heads, "probe-link").is_empty());
        assert_eq!(heads_named(&heads, "probe-link-x").len(), 1);
    }

    #[test]
    fn scanner_records_sub_relates_as_and_touch() {
        let heads =
            type_heads("define relation r2 sub r1, relates q as p;\n x-link relates extra;");
        let r2 = heads_named(&heads, "r2");
        assert_eq!(r2.len(), 1);
        assert_eq!(r2[0].kind, Kind::Relation);
        assert_eq!(r2[0].sub.as_deref(), Some("r1"));
        assert!(r2[0].relates_as);
        let touch = heads_named(&heads, "x-link");
        assert_eq!(touch.len(), 1);
        assert_eq!(touch[0].kind, Kind::Touch);
    }

    #[test]
    fn scanner_function_bodies_yield_no_heads() {
        assert!(type_heads(crate::MEM_FUNCTIONS).is_empty());
        let mem = MemGecko::new();
        assert_eq!(type_heads(mem.schema()), type_heads(crate::MEM_TYPES));
    }
}
