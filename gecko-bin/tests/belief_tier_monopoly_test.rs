//! Static lint over production sources: no raw belief-tier write TQL outside
//! `core/mem-gecko/src/tql.rs`.
//!
//! The belief-tier surface is introspected from the composed core→mem→cyber schema
//! (the `memory-item` subtype closure plus every belief-edge relation), so a new
//! belief-tier type is covered with no edit here. A *hit* is a unit of query text —
//! a Rust string literal, or a whole `.tql` file with its comments and quoted strings
//! removed — that contains a write verb (`insert`, `put`, `update`, `delete`) and a
//! surface label that follows the token `isa` or is directly followed by `(`.
//!
//! The detector and its unit tests need no service; the live lint needs TypeDB and
//! the `cyber` feature (it composes the cyber schema).

#[cfg(feature = "cyber")]
use std::collections::BTreeMap;
use std::collections::BTreeSet;
#[cfg(feature = "cyber")]
use std::path::{Path, PathBuf};

#[cfg(feature = "cyber")]
use cyber_gecko::CyberGecko;
#[cfg(feature = "cyber")]
use gecko_engine::extension::GeckoExtension;
#[cfg(feature = "cyber")]
use mem_gecko::MemGecko;

#[cfg(feature = "cyber")]
mod common;
#[cfg(feature = "cyber")]
#[path = "common/surface.rs"]
mod surface;
#[cfg(feature = "cyber")]
use common::TestDb;
#[cfg(feature = "cyber")]
use surface::{belief_edge_surface, read_col};

/// The language a scanned source is written in, which decides what a query unit is.
#[derive(Clone, Copy, Debug)]
enum SourceKind {
    /// Each string literal's contents is one unit; comments and code are ignored.
    Rust,
    /// The whole text is one unit, with `#` comments and quoted strings removed.
    Tql,
}

/// Query text, one character at a time, each tagged with its 1-based source line.
type Unit = Vec<(char, usize)>;

const WRITE_VERBS: [&str; 4] = ["insert", "put", "update", "delete"];

fn is_label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Returns the 1-based line and label of every raw belief-tier write in `source`.
fn belief_tier_hits(
    source: &str,
    kind: SourceKind,
    surface: &BTreeSet<String>,
) -> Vec<(usize, String)> {
    let chars: Vec<char> = source.chars().collect();
    let units = match kind {
        SourceKind::Rust => lex_rust(&chars).literals,
        SourceKind::Tql => vec![tql_unit(&chars)],
    };
    units
        .iter()
        .flat_map(|unit| unit_hits(unit, surface))
        .collect()
}

/// The 1-based line of every character in `chars`.
fn line_numbers(chars: &[char]) -> Vec<usize> {
    let mut line = 1;
    chars
        .iter()
        .map(|&c| {
            let this = line;
            if c == '\n' {
                line += 1;
            }
            this
        })
        .collect()
}

/// A lexed Rust source: which characters are code (outside literals and comments),
/// and the decoded contents of every string literal.
struct RustLex {
    code: Vec<bool>,
    literals: Vec<Unit>,
}

/// Lexes Rust source just far enough to separate code, comments, string literals
/// (ordinary with escapes and `\` line continuations, raw `r#"…"#`, and their byte
/// forms), char literals and lifetimes.
fn lex_rust(chars: &[char]) -> RustLex {
    let n = chars.len();
    let lines = line_numbers(chars);
    let at = |k: usize| chars.get(k).copied();
    let mut code = vec![false; n];
    let mut literals = Vec::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c == '/' && at(i + 1) == Some('/') {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && at(i + 1) == Some('*') {
            let mut depth = 0usize;
            while i < n {
                if chars[i] == '/' && at(i + 1) == Some('*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && at(i + 1) == Some('/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if let Some(end) = raw_string_open(chars, i) {
            // `end` indexes the opening quote; count the `#`s between `r` and it.
            let hashes = end - (i + 1) - usize::from(c == 'b');
            let mut unit = Vec::new();
            let mut k = end + 1;
            loop {
                if k >= n {
                    break;
                }
                if chars[k] == '"' && (1..=hashes).all(|h| at(k + h) == Some('#')) {
                    k += 1 + hashes;
                    break;
                }
                unit.push((chars[k], lines[k]));
                k += 1;
            }
            literals.push(unit);
            i = k;
        } else if c == '"' {
            let mut unit = Vec::new();
            let mut k = i + 1;
            while k < n && chars[k] != '"' {
                if chars[k] == '\\' {
                    match at(k + 1) {
                        Some('\n') | Some('\r') => {
                            unit.push((' ', lines[k]));
                            k += 1;
                            while k < n && chars[k].is_whitespace() {
                                k += 1;
                            }
                        }
                        Some('u') => {
                            unit.push((' ', lines[k]));
                            while k < n && chars[k] != '}' {
                                k += 1;
                            }
                            k += 1;
                        }
                        Some('x') => {
                            unit.push((' ', lines[k]));
                            k += 4;
                        }
                        Some(e @ ('"' | '\'' | '\\')) => {
                            unit.push((e, lines[k]));
                            k += 2;
                        }
                        _ => {
                            unit.push((' ', lines[k]));
                            k += 2;
                        }
                    }
                } else {
                    unit.push((chars[k], lines[k]));
                    k += 1;
                }
            }
            literals.push(unit);
            i = k + 1;
        } else if c == '\'' {
            if let Some(end) = char_literal_end(chars, i) {
                i = end;
            } else {
                // A lifetime or label: the quote is code.
                code[i] = true;
                i += 1;
            }
        } else {
            code[i] = true;
            i += 1;
        }
    }
    RustLex { code, literals }
}

/// If a raw string literal (`r"`, `r#"`, `br#"`) opens at `i`, the index of its
/// opening quote.
fn raw_string_open(chars: &[char], i: usize) -> Option<usize> {
    let starts_ident = |k: usize| k == 0 || !is_ident_char(chars[k - 1]);
    let r = match chars[i] {
        'r' if starts_ident(i) => i,
        'b' if starts_ident(i) && chars.get(i + 1) == Some(&'r') => i + 1,
        _ => return None,
    };
    let mut k = r + 1;
    while chars.get(k) == Some(&'#') {
        k += 1;
    }
    (chars.get(k) == Some(&'"')).then_some(k)
}

/// If a char literal (`'x'`, `'\''`, `'\u{..}'`) opens at `i`, the index just past
/// its closing quote; `None` for a lifetime or loop label.
fn char_literal_end(chars: &[char], i: usize) -> Option<usize> {
    let at = |k: usize| chars.get(k).copied();
    if at(i + 1) == Some('\\') {
        let mut k = i + 2;
        match at(k) {
            Some('u') => {
                while k < chars.len() && chars[k] != '}' {
                    k += 1;
                }
                k += 1;
            }
            Some('x') => k += 3,
            _ => k += 1,
        }
        (at(k) == Some('\'')).then_some(k + 1)
    } else {
        (at(i + 2) == Some('\'')).then_some(i + 3)
    }
}

/// The whole `.tql` text as one unit, with `#` comments and `"…"`/`'…'` strings
/// (honoring `\` escapes) each replaced by a single space.
fn tql_unit(chars: &[char]) -> Unit {
    let lines = line_numbers(chars);
    let n = chars.len();
    let mut unit = Vec::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c == '#' {
            unit.push((' ', lines[i]));
            while i < n && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '"' || c == '\'' {
            unit.push((' ', lines[i]));
            i += 1;
            while i < n && chars[i] != c {
                i += if chars[i] == '\\' { 2 } else { 1 };
            }
            i += 1;
        } else {
            unit.push((c, lines[i]));
            i += 1;
        }
    }
    unit
}

/// The hits in one unit: none unless it holds a write verb, then every surface
/// label after the token `isa` or directly before `(`.
fn unit_hits(unit: &Unit, surface: &BTreeSet<String>) -> Vec<(usize, String)> {
    enum Tok {
        Label(String, usize),
        Punct(char),
    }
    let mut toks = Vec::new();
    let mut k = 0;
    while k < unit.len() {
        let (c, line) = unit[k];
        if is_label_char(c) {
            let mut label = String::new();
            while k < unit.len() && is_label_char(unit[k].0) {
                label.push(unit[k].0);
                k += 1;
            }
            toks.push(Tok::Label(label, line));
        } else {
            if !c.is_whitespace() {
                toks.push(Tok::Punct(c));
            }
            k += 1;
        }
    }
    let has_verb = toks
        .iter()
        .any(|t| matches!(t, Tok::Label(l, _) if WRITE_VERBS.contains(&l.as_str())));
    if !has_verb {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (idx, tok) in toks.iter().enumerate() {
        let Tok::Label(label, line) = tok else {
            continue;
        };
        if !surface.contains(label) {
            continue;
        }
        // `isa` or the exact-type form `isa!`.
        let before = if idx > 0 && matches!(&toks[idx - 1], Tok::Punct('!')) {
            idx.checked_sub(2)
        } else {
            idx.checked_sub(1)
        };
        let after_isa = before.is_some_and(|b| matches!(&toks[b], Tok::Label(p, _) if p == "isa"));
        let before_paren = matches!(toks.get(idx + 1), Some(Tok::Punct('(')));
        if after_isa || before_paren {
            hits.push((*line, label.clone()));
        }
    }
    hits
}

/// `rust_source` with the braced body of every inline `#[cfg(test)] mod <name> { … }`
/// blanked to spaces, keeping every newline so line numbers are unchanged. Other
/// attributes may sit between `#[cfg(test)]` and the `mod`, which may carry a
/// `pub`/`pub(..)` visibility. An out-of-line `mod <name>;` blanks nothing.
fn production_text(rust_source: &str) -> String {
    let mut chars: Vec<char> = rust_source.chars().collect();
    let code = lex_rust(&chars).code;
    let n = chars.len();
    let marker: Vec<char> = "#[cfg(test)]".chars().collect();
    let is_code_at = |k: usize| k < n && code[k];
    let mut i = 0;
    while i < n {
        let is_marker = i + marker.len() <= n
            && (i..i + marker.len()).all(|k| code[k])
            && chars[i..i + marker.len()] == marker[..];
        if !is_marker {
            i += 1;
            continue;
        }
        let mut k = i + marker.len();
        let skip_ws = |k: &mut usize| {
            while *k < n && (!code[*k] || chars[*k].is_whitespace()) {
                *k += 1;
            }
        };
        let word_at = |k: usize, w: &str| {
            let w: Vec<char> = w.chars().collect();
            k + w.len() <= n
                && chars[k..k + w.len()] == w[..]
                && !(k + w.len() < n && is_ident_char(chars[k + w.len()]))
        };
        // Further attributes.
        skip_ws(&mut k);
        while is_code_at(k) && chars[k] == '#' && chars.get(k + 1) == Some(&'[') {
            k = match matching_close(&chars, &code, k + 1, '[', ']') {
                Some(close) => close + 1,
                None => n,
            };
            skip_ws(&mut k);
        }
        // Optional visibility.
        if word_at(k, "pub") {
            k += 3;
            skip_ws(&mut k);
            if is_code_at(k) && chars[k] == '(' {
                k = match matching_close(&chars, &code, k, '(', ')') {
                    Some(close) => close + 1,
                    None => n,
                };
                skip_ws(&mut k);
            }
        }
        if !word_at(k, "mod") {
            i += marker.len();
            continue;
        }
        k += 3;
        skip_ws(&mut k);
        while k < n && is_ident_char(chars[k]) {
            k += 1;
        }
        skip_ws(&mut k);
        if !(is_code_at(k) && chars[k] == '{') {
            i = k;
            continue;
        }
        let close = matching_close(&chars, &code, k, '{', '}').unwrap_or(n);
        for c in &mut chars[k + 1..close] {
            if *c != '\n' {
                *c = ' ';
            }
        }
        i = close;
    }
    chars.into_iter().collect()
}

/// The index of the code-position `close` matching the code-position `open` at
/// `start`, counting only delimiters that are code (not in literals or comments).
fn matching_close(
    chars: &[char],
    code: &[bool],
    start: usize,
    open: char,
    close: char,
) -> Option<usize> {
    let mut depth = 0usize;
    for k in start..chars.len() {
        if !code[k] {
            continue;
        }
        if chars[k] == open {
            depth += 1;
        } else if chars[k] == close {
            depth -= 1;
            if depth == 0 {
                return Some(k);
            }
        }
    }
    None
}

/// The belief-tier surface of the composed schema: the `memory-item` subtype
/// closure together with every belief-edge relation.
#[cfg(feature = "cyber")]
async fn belief_tier_surface(db: &TestDb) -> BTreeSet<String> {
    let mut surface: BTreeSet<String> = read_col(
        db,
        r#"match $t sub memory-item; let $l = label($t); select $l;"#,
        "l",
    )
    .await
    .into_iter()
    .collect();
    surface.extend(belief_edge_surface(db).await);
    surface
}

/// Every file under `dir` (recursively) whose extension is `ext`, skipping any
/// directory `skip_dir` rejects by name.
#[cfg(feature = "cyber")]
fn walk(dir: &Path, ext: &str, skip_dir: &dyn Fn(&str) -> bool, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}"));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("entry in {dir:?}: {e}"))
            .path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_else(|| panic!("non-UTF-8 path {path:?}"))
            .to_string();
        if path.is_dir() {
            if !skip_dir(&name) {
                walk(&path, ext, skip_dir, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            out.push(path);
        }
    }
}

/// The production Rust sources: every `.rs` under the `src/` of each `core/*` and
/// `extensions/*` crate and of `gecko-bin`, outside `tests`/`target` directories and
/// excluding files named `tests.rs`.
#[cfg(feature = "cyber")]
fn production_rust_files(root: &Path) -> Vec<PathBuf> {
    let mut crates = vec![root.join("gecko-bin")];
    for group in ["core", "extensions"] {
        let dir = root.join(group);
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}")) {
            let path = entry
                .unwrap_or_else(|e| panic!("entry in {dir:?}: {e}"))
                .path();
            if path.is_dir() {
                crates.push(path);
            }
        }
    }
    let mut files = Vec::new();
    for krate in crates {
        let src = krate.join("src");
        if src.is_dir() {
            walk(&src, "rs", &|n| n == "tests" || n == "target", &mut files);
        }
    }
    files.retain(|p| p.file_name().and_then(|n| n.to_str()) != Some("tests.rs"));
    files
}

/// The production schema sources: every `.tql` under `core/`, `extensions/` and
/// `gecko-bin/`, outside `tests`, `target` and dot-directories.
#[cfg(feature = "cyber")]
fn production_tql_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for top in ["core", "extensions", "gecko-bin"] {
        walk(
            &root.join(top),
            "tql",
            &|n| n == "tests" || n == "target" || n.starts_with('.'),
            &mut files,
        );
    }
    files
}

/// No production source outside `core/mem-gecko/src/tql.rs` carries raw write TQL
/// against the introspected belief-tier surface; belief-tier writes go through mem's
/// writers and the belief-edge seam.
#[cfg(feature = "cyber")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_raw_belief_tier_tql_outside_mem_tql() {
    let db = TestDb::new("gecko_test_monopoly");
    let mut router = db.router();
    router
        .apply_schema(gecko_engine::db::CORE_SCHEMA)
        .await
        .expect("core schema applies");
    router
        .apply_schema(MemGecko::new().schema())
        .await
        .expect("mem substrate schema applies");
    router
        .apply_schema(CyberGecko::new().schema())
        .await
        .expect("cyber schema applies");

    let surface = belief_tier_surface(&db).await;
    for expected in [
        "belief",
        "technique-detection",
        "alert-disposition",
        "supersession",
    ] {
        assert!(
            surface.contains(expected),
            "belief-tier surface must contain `{expected}`; got {surface:?}"
        );
    }

    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
        .canonicalize()
        .expect("workspace root");
    let mut hits_by_file: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
    let mut scan = |path: &Path, kind: SourceKind| {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        let text = match kind {
            SourceKind::Rust => production_text(&text),
            SourceKind::Tql => text,
        };
        let rel = path
            .strip_prefix(&root)
            .expect("scanned path is under the workspace root")
            .to_string_lossy()
            .into_owned();
        hits_by_file.insert(rel, belief_tier_hits(&text, kind, &surface));
    };
    for path in production_rust_files(&root) {
        scan(&path, SourceKind::Rust);
    }
    for path in production_tql_files(&root) {
        scan(&path, SourceKind::Tql);
    }

    for expected in [
        "extensions/cyber-gecko/src/detect/mod.rs",
        "core/mem-gecko/src/writer/mod.rs",
    ] {
        assert!(
            hits_by_file.contains_key(expected),
            "the scan must cover `{expected}`; scanned {:?}",
            hits_by_file.keys().collect::<Vec<_>>()
        );
    }

    let mem_tql = hits_by_file
        .remove("core/mem-gecko/src/tql.rs")
        .expect("the scan covers core/mem-gecko/src/tql.rs");
    for expected in ["belief", "supersession"] {
        assert!(
            mem_tql.iter().any(|(_, l)| l == expected),
            "positive control: mem's tql.rs must show a `{expected}` write; got {mem_tql:?}"
        );
    }

    let violations: Vec<String> = hits_by_file
        .iter()
        .flat_map(|(path, hits)| {
            hits.iter()
                .map(move |(line, label)| format!("{path}:{line}: {label}"))
        })
        .collect();
    assert!(
        violations.is_empty(),
        "raw belief-tier write TQL outside core/mem-gecko/src/tql.rs:\n{}",
        violations.join("\n")
    );
}

mod detector {
    use super::*;

    fn surface() -> BTreeSet<String> {
        [
            "belief",
            "technique-detection",
            "alert-disposition",
            "attack",
            "supersession",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }

    fn labels(hits: &[(usize, String)]) -> Vec<&str> {
        hits.iter().map(|(_, l)| l.as_str()).collect()
    }

    #[test]
    fn flags_the_pre_seam_cyber_literals() {
        let claim = r#"    let op = GraphWrite::single(
        "given $l: string, $tech: string, $b: string; \
         match $lc isa concept, has concept-id $lcid; $lcid == $l; \
               $t isa attack-pattern, has attack-id $taid; $taid == $tech; \
               $bel isa belief, has concept-id $bid; $bid == $b; \
               not { $r isa technique-detection, \
                     links (detecting-logic: $lc, detected-technique: $t, detection-claim: $bel); }; \
         insert $nr isa technique-detection, \
                links (detecting-logic: $lc, detected-technique: $t, detection-claim: $bel);",
"#;
        let disposition = r#"    let op = GraphWrite::single(
        "given $l: string, $b: string, $v: string; \
         match $lc isa concept, has concept-id $lcid; $lcid == $l; \
               $bel isa belief, has concept-id $bid; $bid == $b; \
         insert $r isa alert-disposition, \
                links (dispositioned-logic: $lc, disposition-belief: $bel), \
                has detection-verdict == $v;",
"#;
        let s = surface();
        let claim_hits = belief_tier_hits(claim, SourceKind::Rust, &s);
        assert!(!claim_hits.is_empty(), "claim literal must be flagged");
        assert!(labels(&claim_hits).contains(&"technique-detection"));
        let disp_hits = belief_tier_hits(disposition, SourceKind::Rust, &s);
        assert!(!disp_hits.is_empty(), "disposition literal must be flagged");
        assert!(labels(&disp_hits).contains(&"alert-disposition"));
    }

    #[test]
    fn flags_an_isa_bang_delete() {
        let hits = belief_tier_hits(
            "match $b isa! belief; delete $b;",
            SourceKind::Tql,
            &surface(),
        );
        assert_eq!(labels(&hits), vec!["belief"], "{hits:?}");
    }

    #[test]
    fn flags_an_anonymous_relation_insert() {
        let src = r#""match $b isa concept; insert technique-detection (detection-claim: $b);""#;
        let hits = belief_tier_hits(src, SourceKind::Rust, &surface());
        assert!(labels(&hits).contains(&"technique-detection"), "{hits:?}");
    }

    #[test]
    fn tql_units_ignore_comments_and_doc_strings() {
        let s = surface();
        assert_eq!(
            belief_tier_hits("insert $b isa belief;", SourceKind::Tql, &s).len(),
            1
        );
        assert!(
            belief_tier_hits(
                "# insert $b isa belief;\nmatch $b isa belief;",
                SourceKind::Tql,
                &s
            )
            .is_empty()
        );
        assert!(belief_tier_hits(
            "fun f() -> { belief } @doc(\"Rows to delete.\"):\n match $b isa belief;\n return { $b };",
            SourceKind::Tql,
            &s
        )
        .is_empty());
    }

    #[test]
    fn flags_match_then_update_of_a_belief() {
        let src = r#""match $b isa belief, has concept-id == $id; update $b has belief-state \"retracted\";""#;
        let hits = belief_tier_hits(src, SourceKind::Rust, &surface());
        assert!(labels(&hits).contains(&"belief"), "{hits:?}");
    }

    #[test]
    fn flags_put_delete_and_raw_string_forms() {
        let s = surface();
        for src in [
            r#""put $x isa belief;""#,
            r#""match $r isa supersession; delete $r;""#,
            r##"r#"insert $r isa technique-detection, links (detection-claim: $b);"#"##,
        ] {
            assert!(
                !belief_tier_hits(src, SourceKind::Rust, &s).is_empty(),
                "must flag {src}"
            );
        }
    }

    #[test]
    fn ignores_reads_record_types_and_comments() {
        let s = surface();
        for src in [
            r#""match $d isa technique-detection; fetch { \"d\": $d };""#,
            r#""insert $t isa attack-pattern, has attack-id \"T1\";""#,
            "// insert $b isa belief;",
        ] {
            let hits = belief_tier_hits(src, SourceKind::Rust, &s);
            assert!(hits.is_empty(), "must not flag {src}: {hits:?}");
        }
    }

    #[test]
    fn survives_char_literals_and_lifetimes() {
        let src = "fn f<'a>(c: char) -> bool { c == '\"' || c == '\\'' }\nconst Q: &str = \"insert $b isa belief;\";";
        let hits = belief_tier_hits(src, SourceKind::Rust, &surface());
        assert_eq!(hits, vec![(2, "belief".to_string())]);
    }

    #[test]
    fn production_text_keeps_code_after_an_out_of_line_test_mod() {
        let s = surface();
        let out_of_line = "#[cfg(test)]\nmod tests;\nconst Q: &str = \"insert $b isa belief;\";";
        assert_eq!(
            belief_tier_hits(&production_text(out_of_line), SourceKind::Rust, &s).len(),
            1
        );
        let inline = "#[cfg(test)]\nmod tests {\n const Q: &str = \"insert $b isa belief;\";\n}";
        assert!(belief_tier_hits(&production_text(inline), SourceKind::Rust, &s).is_empty());
        let brace_in_string = "#[cfg(test)]\nmod tests {\n const B: &str = \"{\";\n const Q: &str = \"insert $b isa belief;\";\n}\nconst P: &str = \"insert $b isa belief;\";";
        assert_eq!(
            belief_tier_hits(&production_text(brace_in_string), SourceKind::Rust, &s),
            vec![(6, "belief".to_string())]
        );
    }
}
