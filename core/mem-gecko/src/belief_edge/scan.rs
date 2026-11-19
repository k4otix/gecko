//! Splits a TypeQL schema fragment into statements of whole tokens, for the registry's
//! position rules.
//!
//! The scanner does not model the grammar. It strips comments and string literals and
//! tokenizes, so the registry can refuse anything it cannot prove safe (fail closed).

/// One `;`-terminated statement, as whole tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Statement {
    tokens: Vec<String>,
}

impl Statement {
    /// Whether `token` occurs in the statement as a whole token.
    pub(super) fn has(&self, token: &str) -> bool {
        self.tokens.iter().any(|t| t == token)
    }

    /// The statement after a leading `define`/`redefine` and then a leading
    /// `entity`/`relation`/`attribute`.
    fn body(&self) -> &[String] {
        let mut rest = self.tokens.as_slice();
        if let [t, tail @ ..] = rest
            && (t == "define" || t == "redefine")
        {
            rest = tail;
        }
        if let [t, tail @ ..] = rest
            && matches!(t.as_str(), "entity" | "relation" | "attribute")
        {
            rest = tail;
        }
        rest
    }

    /// The type the statement is about: its first token after the leading keywords, when
    /// that token can be a label.
    pub(super) fn first_label(&self) -> Option<&str> {
        self.body()
            .first()
            .map(String::as_str)
            .filter(|t| is_label(t))
    }

    /// Whether the statement renames a type (`[kind] <label> label <new>`) or a role
    /// (`<relation>:<role> label <new>`), the two shapes a TypeQL rename takes.
    pub(super) fn renames(&self) -> bool {
        match self.body() {
            [old, l, new] => l == "label" && is_label(old) && is_label(new),
            [rel, role, l, new] => l == "label" && is_label(rel) && is_label(role) && is_label(new),
            _ => false,
        }
    }

    /// Every token that directly follows a `sub` token.
    pub(super) fn supertypes(&self) -> impl Iterator<Item = &str> {
        self.tokens
            .windows(2)
            .filter(|w| w[0] == "sub")
            .map(|w| w[1].as_str())
    }

    /// The label this statement introduces with `[define] <kind> <label>`, where `kind` is
    /// `entity`, `relation` or `attribute`.
    pub(super) fn introduces(&self, kind: &str) -> Option<&str> {
        let rest = match self.tokens.first().map(String::as_str) {
            Some("define") => &self.tokens[1..],
            _ => self.tokens.as_slice(),
        };
        match rest {
            [k, label, ..] if k == kind && is_label(label) => Some(label),
            _ => None,
        }
    }
}

/// Returns the statements of `fragment`, with comments and string literals removed.
pub(super) fn statements(fragment: &str) -> Vec<Statement> {
    tokenize(&strip_comments_and_strings(fragment))
        .split(|t| t == ";")
        .filter(|s| !s.is_empty())
        .map(|s| Statement { tokens: s.to_vec() })
        .collect()
}

/// Removes `#` comments up to the next `\n` or `\r` and `"…"` / `'…'` literals (honoring `\`
/// escapes). An unterminated quote is kept as ordinary text, so it cannot hide the
/// statements after it.
fn strip_comments_and_strings(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '#' => {
                while i < chars.len() && !matches!(chars[i], '\n' | '\r') {
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

/// Splits on whitespace and `, ( ) : { } [ ]`, keeps each `;` as its own token, and starts a
/// new token at each `@`, since a label ends before an attached annotation.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if c.is_whitespace() || matches!(c, ',' | '(' | ')' | ':' | '{' | '}' | '[' | ']' | ';') {
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

/// Whether `token` can be a type label: it does not start with `$`, `@`, an ASCII digit or
/// ASCII punctuation other than `_`. Keywords are not excluded, since a label may spell one.
fn is_label(token: &str) -> bool {
    token
        .chars()
        .next()
        .is_some_and(|c| c == '_' || !(c.is_ascii_digit() || c.is_ascii_punctuation()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemGecko;
    use gecko_extension_api::GeckoExtension;

    fn mentions(stmts: &[Statement], token: &str) -> bool {
        stmts.iter().any(|s| s.has(token))
    }

    fn about<'a>(stmts: &'a [Statement], label: &str) -> Vec<&'a Statement> {
        stmts
            .iter()
            .filter(|s| s.first_label() == Some(label))
            .collect()
    }

    #[test]
    fn scanner_finds_relation_heads_in_mem() {
        let mem = MemGecko::new();
        let stmts = statements(mem.schema());
        for label in ["supersession", "scoped"] {
            let heads: Vec<_> = stmts
                .iter()
                .filter(|s| s.introduces("relation") == Some(label))
                .collect();
            assert_eq!(heads.len(), 1, "{label}");
            assert_eq!(heads[0].supertypes().count(), 0, "{label}");
        }
    }

    #[test]
    fn scanner_recognizes_type_and_role_renames() {
        for text in [
            "redefine belief label idea;",
            "redefine entity belief label idea;",
            "redefine\n  supersession:superseded label old;",
            "redefine\n  fun f() -> boolean:\n    match $t label belief;\n    return check;\n  plan label roadmap;",
        ] {
            assert!(statements(text).iter().any(Statement::renames), "{text}");
        }
        for text in [
            "define entity label sub concept;",
            "define entity sighting sub label;",
            "define\n  fun f() -> boolean:\n    match $t label belief;\n    return check;\n",
            "define\n  fun f() -> boolean:\n    match\n      $r label x-link;\n      $r relates claim;\n    return check;\n",
            "define entity a @doc(\"a label b\");",
        ] {
            assert!(!statements(text).iter().any(Statement::renames), "{text}");
        }
    }

    #[test]
    fn scanner_ignores_doc_strings_and_comments() {
        let stmts = statements(
            "define\n  entity a @doc(\"relation fake-rel, relates x\");\n  # relation also-fake\n",
        );
        assert!(!mentions(&stmts, "fake-rel"));
        assert!(!mentions(&stmts, "also-fake"));
        assert_eq!(stmts[0].introduces("entity"), Some("a"));
    }

    #[test]
    fn scanner_ends_a_comment_at_a_carriage_return() {
        let stmts = statements("define\n# c\rx-link relates extra;\n");
        assert_eq!(about(&stmts, "x-link").len(), 1, "{stmts:?}");
    }

    #[test]
    fn scanner_strips_single_quoted_strings() {
        let stmts = statements(
            "define relation my-edge @doc('a;b # c\\'d'), sub supersession, relates claim;",
        );
        assert_eq!(stmts.len(), 1, "{stmts:?}");
        assert_eq!(stmts[0].first_label(), Some("my-edge"));
        assert_eq!(stmts[0].supertypes().collect::<Vec<_>>(), ["supersession"]);
    }

    #[test]
    fn scanner_keeps_text_after_an_unterminated_quote() {
        let stmts = statements("define entity a @doc(\"oops);\n relation hidden-link, relates x;");
        assert!(mentions(&stmts, "hidden-link"), "{stmts:?}");
    }

    #[test]
    fn scanner_splits_an_attached_annotation() {
        let stmts = statements(
            "define relation my-edge@abstract, sub r1; x-link@doc(\"x\") relates q as p;",
        );
        assert_eq!(
            about(&stmts, "my-edge")[0].supertypes().collect::<Vec<_>>(),
            ["r1"]
        );
        assert!(about(&stmts, "x-link")[0].has("as"));
    }

    #[test]
    fn scanner_keeps_every_label_form() {
        let stmts = statements(
            "define entity Mid sub a; entity _low sub Mid; entity \u{2118}x sub _low; entity label sub value;",
        );
        assert_eq!(
            about(&stmts, "Mid")[0].supertypes().collect::<Vec<_>>(),
            ["a"]
        );
        assert_eq!(
            about(&stmts, "_low")[0].supertypes().collect::<Vec<_>>(),
            ["Mid"]
        );
        assert_eq!(
            about(&stmts, "\u{2118}x")[0]
                .supertypes()
                .collect::<Vec<_>>(),
            ["_low"]
        );
        assert_eq!(
            about(&stmts, "label")[0].supertypes().collect::<Vec<_>>(),
            ["value"]
        );
        assert!(statements("$x sub y;")[0].first_label().is_none());
    }

    #[test]
    fn scanner_records_annotation_only_touch() {
        let stmts = statements("define x-link @abstract;");
        assert_eq!(about(&stmts, "x-link").len(), 1);
    }

    #[test]
    fn scanner_distinguishes_prefix_labels() {
        let stmts = statements("define relation probe-link-x, relates a, relates b;");
        assert!(!mentions(&stmts, "probe-link"));
        assert!(mentions(&stmts, "probe-link-x"));
    }

    #[test]
    fn scanner_records_sub_relates_as_and_touch() {
        let stmts =
            statements("define relation r2 sub r1, relates q as p;\n x-link relates extra;");
        let r2 = about(&stmts, "r2");
        assert_eq!(r2.len(), 1);
        assert_eq!(r2[0].introduces("relation"), Some("r2"));
        assert_eq!(r2[0].supertypes().collect::<Vec<_>>(), ["r1"]);
        assert!(r2[0].has("as"));
        let touch = about(&stmts, "x-link");
        assert_eq!(touch.len(), 1);
        assert_eq!(touch[0].introduces("relation"), None);
    }

    #[test]
    fn scanner_function_bodies_yield_no_heads() {
        for s in statements(crate::MEM_FUNCTIONS) {
            for kind in ["entity", "relation", "attribute"] {
                assert_eq!(s.introduces(kind), None, "{s:?}");
            }
        }
    }
}
