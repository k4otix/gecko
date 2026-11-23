//! Finds the deprecated tuple-first relation pattern, a parenthesized role list
//! followed by `isa` and the relation type, in query or schema text. The current
//! forms are `rel (role: $x)` and `$r isa rel, links (role: $x)`.

/// The 1-based line of every `)` that is followed, after any whitespace or Rust
/// string line continuation (`\` then a newline), by the word `isa`; a
/// source-consistency check the schema crates' tests run over their query text.
#[doc(hidden)]
pub fn deprecated_relation_lines(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut line = 1;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            line += 1;
            continue;
        }
        if b != b')' {
            continue;
        }
        let mut j = i + 1;
        loop {
            match bytes.get(j) {
                Some(c) if c.is_ascii_whitespace() => j += 1,
                Some(b'\\') if matches!(bytes.get(j + 1), Some(b'\n')) => j += 2,
                _ => break,
            }
        }
        let word_ends = |k: usize| {
            bytes
                .get(k)
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || *c == b'-' || *c == b'_'))
        };
        if bytes[j..].starts_with(b"isa") && word_ends(j + 3) {
            lines.push(line);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` with every `~` replaced by `)`, so this file's own source holds no
    /// match.
    fn q(text: &str) -> String {
        text.replace('~', ")")
    }

    #[test]
    fn finds_a_role_list_followed_by_isa() {
        assert_eq!(deprecated_relation_lines(&q("match (a: $x~ isa rel;")), [1]);
    }

    #[test]
    fn finds_it_across_a_line_break_and_names_the_bracket_line() {
        assert_eq!(
            deprecated_relation_lines(&q("match\n  (a: $x, b: $y~\n    isa rel;")),
            [2]
        );
    }

    #[test]
    fn a_string_line_continuation_is_whitespace() {
        assert_eq!(
            deprecated_relation_lines(&q("\"(a: $x~ \\\n        isa rel;\"")),
            [1]
        );
    }

    #[test]
    fn a_backslash_not_before_a_newline_ends_the_gap() {
        assert!(deprecated_relation_lines(&q("(a: $x~ \\ isa rel;")).is_empty());
    }

    #[test]
    fn finds_every_occurrence() {
        let text = q("insert (a: $x~ isa r;\n(b: $y~ isa s, has n 1;\n(c: $z~ isa! t;");
        assert_eq!(deprecated_relation_lines(&text), [1, 2, 3]);
    }

    #[test]
    fn current_forms_and_other_words_are_not_matched() {
        for text in [
            "match rel (a: $x);",
            "match $r isa rel, links (a: $x);",
            "match $x isa thing; let $y in f($x);",
            "fetch { \"n\": count($x~ };",
            "match (a: $x~ isaac rel;",
            "match (a: $x~ is a;",
        ] {
            assert!(deprecated_relation_lines(&q(text)).is_empty(), "{text}");
        }
    }
}
