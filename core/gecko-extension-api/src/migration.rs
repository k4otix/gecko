//! Shape checks that keep a [`SchemaMigration`] list in step with the shipped
//! schema it leads to.
//!
//! [`check_migrations`] accepts each entry only in one of these forms:
//!
//! - a **cleanup**, exactly `match $r isa T; not { $r links (R: $x); }; delete $r;`
//!   or, for several roles, `{ not { $r links (R0: $p0); }; } or { … $p1 … };`
//!   before the `delete`, where every role it names is declared `@card(1)` or
//!   `@card(1..…)` on `T` or a supertype of `T` in the shipped schema;
//! - a **type removal**, `match $r isa T; delete $r;`, where a later entry of the
//!   list undefines `T` and the shipped schema declares no `T`;
//! - an **update**, `match $r isa T, has A $v; $v > N; update $r has A M;` (or `<`,
//!   `>=`, `<=`) with nothing after it, where the shipped `owns A` of `T` (or a
//!   supertype) carries a `@range`;
//! - an `undefine @doc from X;`, where a shipped statement declares `X` followed by
//!   `@doc(`, ignoring an `@abstract` annotation after the label;
//! - an `undefine T;` or `undefine fun F;`, where the shipped schema declares no `T`
//!   or `F`;
//! - a `redefine X;`, where the shipped schema contains `X`.
//!
//! Every entry's `applies_when` text must also be absent from the shipped schema.

use crate::SchemaMigration;

/// Checks every entry of `migrations` against `schema`, the shipped schema text.
/// The error names the first offending migration's id and the rule it breaks.
// A schema-consistency check each fragment's tests run over its migration list.
#[doc(hidden)]
pub fn check_migrations(migrations: &[SchemaMigration], schema: &str) -> Result<(), String> {
    let raw = raw_statements(schema);
    let statements: Vec<String> = raw.iter().map(|s| strip_docs(s)).collect();
    for (at, m) in migrations.iter().enumerate() {
        if schema.contains(m.applies_when) {
            return Err(format!(
                "{}: the shipped schema still contains the replaced text",
                m.id
            ));
        }
        check_one(m, &migrations[at + 1..], schema, &raw, &statements)
            .map_err(|e| format!("{}: {e}", m.id))?;
    }
    Ok(())
}

fn is_label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Every needle that occurs in `text` at a label boundary: where a needle starts
/// (ends) with a label character, the character before (after) the match is not one.
/// Each fragment's tests run it over the shipped schema for the labels a cut deleted.
#[doc(hidden)]
pub fn deleted_labels_present(text: &str, needles: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    for needle in needles {
        let check_start = needle.chars().next().is_some_and(is_label_char);
        let check_end = needle.chars().next_back().is_some_and(is_label_char);
        let hit = text.match_indices(needle).any(|(i, _)| {
            let before_ok =
                !check_start || !text[..i].chars().next_back().is_some_and(is_label_char);
            let after_ok = !check_end
                || !text[i + needle.len()..]
                    .chars()
                    .next()
                    .is_some_and(is_label_char);
            before_ok && after_ok
        });
        if hit {
            found.push((*needle).to_string());
        }
    }
    found
}

/// Checks one entry; `later` holds the entries after it in the list.
fn check_one(
    m: &SchemaMigration,
    later: &[SchemaMigration],
    schema: &str,
    raw: &[String],
    statements: &[String],
) -> Result<(), String> {
    let tql = m.tql;
    let words = normalize(tql);
    if words.starts_with("match ") {
        if words.contains(" update ") {
            return check_update(tql, statements);
        }
        return check_cleanup(tql, later, statements);
    }
    if let Some(target) = tql.strip_prefix("undefine @doc from ") {
        let target = single_statement(target, "an `undefine @doc from X;`")?;
        let reinstalled = raw.iter().any(|s| {
            let declared = ["entity ", "relation ", "attribute "]
                .iter()
                .find_map(|kind| s.strip_prefix(kind))
                .unwrap_or(s);
            without_abstract(&normalize(declared))
                .starts_with(&format!("{} @doc(", normalize(target)))
        });
        if !reinstalled {
            return Err(format!(
                "the shipped schema gives {target} no @doc to replace the undefined one"
            ));
        }
        return Ok(());
    }
    if let Some(function) = tql.strip_prefix("undefine fun ") {
        let function = single_statement(function, "an `undefine fun F;`")?;
        if schema.contains(&format!("fun {function}(")) {
            return Err(format!("the shipped schema still declares fun {function}"));
        }
        return Ok(());
    }
    if let Some(type_name) = tql.strip_prefix("undefine ") {
        let type_name = single_statement(type_name, "an `undefine T;`")?;
        if !is_label(type_name) {
            return Err(format!(
                "an `undefine T;` names one type label, not {type_name:?}"
            ));
        }
        if !statements_of(statements, type_name).is_empty() {
            return Err(format!("the shipped schema still declares {type_name}"));
        }
        return Ok(());
    }
    if let Some(installed) = tql.strip_prefix("redefine ") {
        let installed = single_statement(installed, "a `redefine X;`")?;
        if !schema.contains(installed) {
            return Err(format!("the shipped schema does not define {installed}"));
        }
        return Ok(());
    }
    Err(
        "a migration is a `match … delete $r;` cleanup, a `match … update …;`, an \
         `undefine @doc from X;`, an `undefine T;`, an `undefine fun F;` or a `redefine X;`"
            .to_string(),
    )
}

/// `rest` with its one trailing `;` removed, refusing any other `;` outside a
/// string literal.
fn single_statement<'a>(rest: &'a str, form: &str) -> Result<&'a str, String> {
    match rest.strip_suffix(';') {
        Some(body) if split_top_level(body, ';').len() == 1 => Ok(body.trim()),
        _ => Err(format!("{form} is exactly one statement")),
    }
}

/// The one cleanup text that deletes the rows of `type_name` missing any of
/// `roles`: a single `not` clause on `$x`, or one `{ … }` branch per role on
/// `$p0`, `$p1`, … joined by `or`. With no roles it deletes every row.
fn canonical_cleanup(type_name: &str, roles: &[String]) -> String {
    let body = match roles {
        [] => String::new(),
        [role] => format!(" not {{ $r links ({role}: $x); }};"),
        _ => {
            let branches: Vec<String> = roles
                .iter()
                .enumerate()
                .map(|(i, role)| format!("{{ not {{ $r links ({role}: $p{i}); }}; }}"))
                .collect();
            format!(" {};", branches.join(" or "))
        }
    };
    format!("match $r isa {type_name};{body} delete $r;")
}

fn check_cleanup(
    tql: &str,
    later: &[SchemaMigration],
    statements: &[String],
) -> Result<(), String> {
    const FORM: &str = "a cleanup is exactly one `match $r isa T; … delete $r;` statement";
    let normalized = normalize(tql);
    let tql = normalized.as_str();
    let type_name = tql
        .strip_prefix("match $r isa ")
        .and_then(|rest| rest.split_once(';'))
        .map(|(t, _)| t.trim())
        .filter(|t| is_label(t))
        .ok_or(FORM)?;
    let mut roles = Vec::new();
    let mut tail = tql;
    while let Some(at) = tail.find("links (") {
        let after = &tail[at + "links (".len()..];
        let (role, rest) = after.split_once(':').ok_or(FORM)?;
        roles.push(role.trim().to_string());
        tail = rest;
    }
    let canonical = canonical_cleanup(type_name, &roles);
    if normalize(tql) != normalize(&canonical) {
        return Err(format!("{FORM}, written as `{canonical}`"));
    }

    if roles.is_empty() {
        let undefined = later
            .iter()
            .any(|o| o.tql == format!("undefine {type_name};"));
        if !undefined || !statements_of(statements, type_name).is_empty() {
            return Err(format!(
                "a cleanup without a `not` clause removes a type, so a later entry undefines \
                 {type_name} and the shipped schema no longer declares it"
            ));
        }
        return Ok(());
    }
    for role in &roles {
        let clause = find_clause(statements, type_name, &format!("relates {role}"))
            .ok_or_else(|| format!("the shipped {type_name} declares no role {role}"))?;
        let required = clause
            .split_whitespace()
            .any(|w| w == "@card(1)" || w.starts_with("@card(1.."));
        if !required {
            return Err(format!(
                "the shipped {type_name} does not require role {role} with `@card(1`"
            ));
        }
    }
    Ok(())
}

fn check_update(tql: &str, statements: &[String]) -> Result<(), String> {
    const FORM: &str = "an update is exactly `match $r isa T, has A $v; $v <op> <number>; \
                        update $r has A <number>;`, with `<op>` one of `>`, `<`, `>=`, `<=`";
    let words: Vec<String> = normalize(tql)
        .replace(';', " ; ")
        .replace(',', " , ")
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let w: Vec<&str> = words.iter().map(String::as_str).collect();
    let [
        "match",
        "$r",
        "isa",
        type_name,
        ",",
        "has",
        attribute,
        var,
        ";",
        guard_var,
        op,
        bound,
        ";",
        "update",
        "$r",
        "has",
        updated,
        value,
        ";",
    ] = w.as_slice()
    else {
        return Err(FORM.to_string());
    };
    let number = |n: &str| n.parse::<f64>().is_ok();
    if !is_label(type_name)
        || !is_label(attribute)
        || updated != attribute
        || !var.starts_with('$')
        || guard_var != var
        || !matches!(*op, ">" | "<" | ">=" | "<=")
        || !number(bound)
        || !number(value)
    {
        return Err(FORM.to_string());
    }
    let clause = find_clause(statements, type_name, &format!("owns {attribute}"))
        .ok_or_else(|| format!("the shipped {type_name} does not own {attribute}"))?;
    if !clause.split_whitespace().any(|w| w.starts_with("@range(")) {
        return Err(format!(
            "the shipped `owns {attribute}` of {type_name} carries no `@range`"
        ));
    }
    Ok(())
}

/// The first clause of `type_name`'s statements (or its supertypes') whose first
/// two words are `head`. The statements carry no `@doc` text.
fn find_clause(statements: &[String], type_name: &str, head: &str) -> Option<String> {
    let mut current = type_name.to_string();
    for _ in 0..32 {
        let own = statements_of(statements, &current);
        let mut supertype = None;
        for statement in &own {
            let clauses = split_top_level(statement, ',');
            for clause in &clauses {
                let words: Vec<&str> = clause.split_whitespace().collect();
                if matches!(words.first(), Some(&("relates" | "owns" | "plays"))) {
                    continue;
                }
                if let Some(i) = words.iter().position(|w| *w == "sub") {
                    supertype = words.get(i + 1).map(|s| s.to_string());
                }
            }
            for clause in &clauses {
                let words: Vec<&str> = clause.split_whitespace().collect();
                let first_two = words.iter().take(2).copied().collect::<Vec<_>>().join(" ");
                let header_tail = words
                    .iter()
                    .position(|w| *w == "relates" || *w == "owns")
                    .map(|i| {
                        words[i..]
                            .iter()
                            .take(2)
                            .copied()
                            .collect::<Vec<_>>()
                            .join(" ")
                    });
                if first_two == head || header_tail.as_deref() == Some(head) {
                    return Some(clause.clone());
                }
            }
        }
        current = supertype?;
    }
    None
}

/// `text` with its whitespace runs collapsed to single spaces and trimmed.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A normalized declaration with an `@abstract` annotation (and its trailing `,`)
/// after the label removed, so `item @abstract, sub root @doc(` reads as
/// `item sub root @doc(`.
fn without_abstract(declared: &str) -> String {
    match declared.split_once(' ') {
        Some((label, rest)) => match rest.strip_prefix("@abstract") {
            // Only a whole `@abstract` token: `declared` is normalized, so a space or
            // a comma ends it.
            Some(after) if after.starts_with([' ', ',']) => {
                let after = after.trim_start().trim_start_matches(',').trim_start();
                format!("{label} {after}")
            }
            _ => declared.to_string(),
        },
        None => declared.to_string(),
    }
}

/// `statement` with every `@doc("…")` annotation removed.
fn strip_docs(statement: &str) -> String {
    let mut out = String::with_capacity(statement.len());
    let mut rest = statement;
    while let Some(at) = rest.find("@doc(\"") {
        out.push_str(&rest[..at]);
        let inner = &rest[at + "@doc(\"".len()..];
        let mut escaped = false;
        let mut close = None;
        for (i, ch) in inner.char_indices() {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                close = Some(i);
                break;
            }
        }
        match close.and_then(|i| inner[i + 1..].strip_prefix(')').map(|r| (i, r))) {
            Some((_, after)) => rest = after,
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The statements declaring `type_name`: those whose first word is the type, or
/// whose first two words are a kind keyword and the type.
fn statements_of<'a>(statements: &'a [String], type_name: &str) -> Vec<&'a String> {
    statements
        .iter()
        .filter(|s| {
            let mut words = s.split_whitespace();
            match words.next() {
                Some(w) if w == type_name => true,
                Some("entity" | "relation" | "attribute") => words
                    .next()
                    .map(|w| w.trim_end_matches(',') == type_name)
                    .unwrap_or(false),
                _ => false,
            }
        })
        .collect()
}

/// The schema's top-level statements, without comments or a leading `define`.
fn raw_statements(schema: &str) -> Vec<String> {
    split_top_level(&strip_comments(schema), ';')
        .into_iter()
        .map(|s| {
            let s = s.trim();
            s.strip_prefix("define")
                .filter(|r| r.starts_with(char::is_whitespace))
                .unwrap_or(s)
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// `text` with each `#` comment outside a string literal removed.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;
    for ch in text.chars() {
        if in_comment {
            if ch == '\n' {
                in_comment = false;
                out.push(ch);
            }
            continue;
        }
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            '#' => in_comment = true,
            _ => out.push(ch),
        }
    }
    out
}

/// `text` split at each `sep` outside string literals, parentheses and braces.
fn split_top_level(text: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for ch in text.chars() {
        if in_string {
            current.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth -= 1,
            _ => {}
        }
        if ch == sep && depth == 0 {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }
    parts.push(current);
    parts
}

fn is_label(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = r#"define
  attribute score value double;
  attribute count value integer;
  relation base @abstract @doc("A base."),
    relates source @card(1) @doc("The source; required."),
    relates target @card(1) @doc("The target.");
  relation link sub base @doc("A link."),
    relates note @doc("An optional role, never @card(1) and never @range(0..1)."),
    owns score @card(0..1) @range(-1.0..1.0),   # cosine
    owns count @card(0..1);
  entity thing @doc("A thing."),
    plays link:source;
  entity x-sensor sub base-sensor @doc("A sensor variant.");
  entity base-sensor @doc("A sensor; owns count @range(0..) is prose here."),
    owns count;
"#;

    fn entry(tql: &'static str) -> SchemaMigration {
        SchemaMigration {
            id: "test/entry",
            applies_when: "text no schema holds",
            tql,
        }
    }

    fn check(tql: &'static str) -> Result<(), String> {
        check_migrations(&[entry(tql)], SCHEMA)
    }

    /// Abstract types, with and without a comma after `@abstract`.
    const ABSTRACT_SCHEMA: &str = r#"define
  entity root @doc("A root.");
  entity item @abstract, sub root @doc("An abstract item."),
    owns name;
  relation edge @abstract @doc("An abstract edge."),
    relates end;
  attribute name value string;
"#;

    #[test]
    fn deleted_labels_present_matches_whole_labels_only() {
        let rel = "relation scoped";
        let role = "scoped:";
        assert_eq!(
            deleted_labels_present("relation scoped @doc(", &[rel]),
            [rel]
        );
        assert_eq!(
            deleted_labels_present("plays scoped:scoped-belief", &[role]),
            [role]
        );
        assert_eq!(
            deleted_labels_present("blast-radius-at(", &["blast-radius-at"]),
            ["blast-radius-at"]
        );
        assert!(deleted_labels_present("blast-radius(", &["blast-radius-at"]).is_empty());
        for text in [
            "relation scoped-thing",
            "relation scoped_thing",
            "relation scopedX",
            "for a scoped member",
        ] {
            assert!(
                deleted_labels_present(text, &[rel, role]).is_empty(),
                "{text:?} must match neither needle"
            );
        }
        for text in ["plays unscoped:role", "plays exception-scoped:role"] {
            assert!(
                deleted_labels_present(text, &[role]).is_empty(),
                "{text:?} must not match {role:?}"
            );
        }
    }

    #[test]
    fn a_doc_undefine_on_an_abstract_type_is_accepted() {
        for tql in [
            "undefine @doc from item sub root;",
            "undefine @doc from edge;",
        ] {
            check_migrations(&[entry(tql)], ABSTRACT_SCHEMA)
                .unwrap_or_else(|e| panic!("{tql}: {e}"));
        }
    }

    #[test]
    fn only_a_whole_abstract_annotation_is_skipped() {
        for (schema, tql) in [
            (
                "define\n  entity root @doc(\"A root.\");\n  entity item @abstractish sub root @doc(\"x\");\n",
                "undefine @doc from item sub root;",
            ),
            (
                "define\n  entity root @doc(\"A root.\");\n  entity item @abstractsub root @doc(\"x\");\n",
                "undefine @doc from item sub root;",
            ),
            (
                "define\n  relation edge @abstract@doc(\"x\");\n",
                "undefine @doc from edge;",
            ),
        ] {
            let err = check_migrations(&[entry(tql)], schema).unwrap_err();
            assert!(err.contains("no @doc to replace"), "{tql}: {err}");
        }
    }

    #[test]
    fn a_doc_undefine_on_an_abstract_type_still_matches_its_supertype() {
        let err = check_migrations(
            &[entry("undefine @doc from item sub other;")],
            ABSTRACT_SCHEMA,
        )
        .unwrap_err();
        assert!(err.contains("no @doc to replace"), "{err}");
    }

    #[test]
    fn well_formed_entries_are_accepted() {
        check("match $r isa link; { not { $r links (source: $p0); }; } or { not { $r links (target: $p1); }; }; delete $r;").unwrap();
        check("match $r isa link; not { $r links (source: $x); }; delete $r;").unwrap();
        check("match $r isa link, has score $s; $s > 1.0; update $r has score 1.0;").unwrap();
        check("undefine @doc from link sub base;").unwrap();
        check("undefine fun gone;").unwrap();
        check("redefine relation link sub base @doc(\"A link.\");").unwrap();
        check_migrations(
            &[
                entry("match $r isa gone; delete $r;"),
                entry("undefine gone;"),
            ],
            SCHEMA,
        )
        .unwrap();
    }

    #[test]
    fn a_cleanup_naming_an_optional_role_is_rejected() {
        let err = check("match $r isa link; not { $r links (note: $x); }; delete $r;").unwrap_err();
        assert!(err.contains("does not require role note"), "{err}");
    }

    #[test]
    fn a_cleanup_naming_an_undeclared_role_is_rejected() {
        let err =
            check("match $r isa link; not { $r links (owner: $x); }; delete $r;").unwrap_err();
        assert!(err.contains("declares no role owner"), "{err}");
    }

    #[test]
    fn a_cleanup_with_a_second_statement_is_rejected() {
        let err = check(
            "match $r isa link; not { $r links (source: $x); }; delete $r; match $t isa thing; delete $r;",
        )
        .unwrap_err();
        assert!(
            err.contains("exactly one `match $r isa T; … delete $r;`"),
            "{err}"
        );
    }

    #[test]
    fn a_cleanup_with_an_extra_constraint_is_rejected() {
        let err =
            check("match $r isa link; $r has score $s; not { $r links (source: $x); }; delete $r;")
                .unwrap_err();
        assert!(err.contains("written as"), "{err}");
    }

    #[test]
    fn a_type_removal_the_list_does_not_undefine_is_rejected() {
        let err = check("match $r isa link; delete $r;").unwrap_err();
        assert!(err.contains("removes a type"), "{err}");
    }

    #[test]
    fn an_update_of_an_unranged_attribute_is_rejected() {
        let err =
            check("match $r isa link, has count $c; $c < 0; update $r has count 0;").unwrap_err();
        assert!(err.contains("carries no `@range`"), "{err}");
    }

    #[test]
    fn an_update_followed_by_another_statement_is_rejected() {
        let err =
            check("match $r isa link, has score $s; $s > 1.0; update $r has score 1.0; delete $r;")
                .unwrap_err();
        assert!(err.contains("an update is exactly"), "{err}");
    }

    #[test]
    fn an_undefine_doc_the_shipped_schema_does_not_replace_is_rejected() {
        let err = check("undefine @doc from thing sub base;").unwrap_err();
        assert!(err.contains("no @doc to replace"), "{err}");
    }

    #[test]
    fn an_undefine_doc_with_a_second_statement_is_rejected() {
        let err = check("undefine @doc from link sub base; undefine thing;").unwrap_err();
        assert!(err.contains("exactly one statement"), "{err}");
    }

    #[test]
    fn an_undefine_of_a_shipped_type_is_rejected() {
        let err = check("undefine thing;").unwrap_err();
        assert!(err.contains("still declares thing"), "{err}");
    }

    #[test]
    fn a_redefine_the_shipped_schema_does_not_hold_is_rejected() {
        let err = check("redefine relation link @doc(\"Other.\");").unwrap_err();
        assert!(err.contains("does not define"), "{err}");
    }

    #[test]
    fn a_replaced_text_still_shipped_is_rejected() {
        let m = SchemaMigration {
            id: "test/stale",
            applies_when: "relation link",
            tql: "undefine fun gone;",
        };
        let err = check_migrations(&[m], SCHEMA).unwrap_err();
        assert!(err.contains("still contains the replaced text"), "{err}");
    }

    #[test]
    fn a_cleanup_with_an_empty_branch_is_rejected() {
        let err =
            check("match $r isa link; { not { $r links (source: $x); }; } or { }; delete $r;")
                .unwrap_err();
        assert!(err.contains("written as"), "{err}");
    }

    #[test]
    fn a_role_whose_doc_mentions_a_card_is_still_optional() {
        let err = check("match $r isa link; not { $r links (note: $x); }; delete $r;").unwrap_err();
        assert!(err.contains("does not require role note"), "{err}");
    }

    #[test]
    fn an_owns_named_only_in_a_doc_is_not_ranged() {
        let err = check("match $r isa base-sensor, has count $c; $c < 0; update $r has count 0;")
            .unwrap_err();
        assert!(err.contains("carries no `@range`"), "{err}");
    }

    #[test]
    fn an_update_without_a_guard_on_the_attribute_is_rejected() {
        for tql in [
            "match $r isa link, has score $s; update $r has score 1.0;",
            "match $r isa link, has score $s, has count $c; $c > 1; update $r has score 1.0;",
            "match $r isa link, has score $s; $s == 1.0; update $r has score 1.0;",
        ] {
            let err = check(tql).unwrap_err();
            assert!(err.contains("an update is exactly"), "{tql}: {err}");
        }
    }

    #[test]
    fn an_undefine_doc_matches_the_whole_type_label() {
        let err = check("undefine @doc from sensor sub base-sensor;").unwrap_err();
        assert!(err.contains("no @doc to replace"), "{err}");
        check("undefine @doc from x-sensor sub base-sensor;").unwrap();
    }

    #[test]
    fn a_type_removal_after_its_undefine_is_rejected() {
        let err = check_migrations(
            &[
                entry("undefine gone;"),
                entry("match $r isa gone; delete $r;"),
            ],
            SCHEMA,
        )
        .unwrap_err();
        assert!(err.contains("a later entry undefines gone"), "{err}");
    }

    #[test]
    fn an_unknown_form_is_rejected() {
        let err = check("insert $x isa thing;").unwrap_err();
        assert!(err.contains("a migration is"), "{err}");
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        /// Every cleanup `SCHEMA` admits, written out independently of
        /// `canonical_cleanup`: `link` and its supertype `base`, each with one or both
        /// of their required roles in either order.
        fn admitted_cleanups() -> Vec<String> {
            let mut out = Vec::new();
            for ty in ["link", "base"] {
                for role in ["source", "target"] {
                    out.push(format!(
                        "match $r isa {ty}; not {{ $r links ({role}: $x); }}; delete $r;"
                    ));
                }
                for (a, b) in [("source", "target"), ("target", "source")] {
                    out.push(format!(
                        "match $r isa {ty}; {{ not {{ $r links ({a}: $p0); }}; }} or \
                         {{ not {{ $r links ({b}: $p1); }}; }}; delete $r;"
                    ));
                }
            }
            out
        }

        /// Fragments a hand edit could leave in a cleanup: an empty or extra branch,
        /// a further constraint, an optional role, a second statement, stray syntax.
        const FRAGMENTS: [&str; 14] = [
            " or { }",
            " { };",
            " $r has score $s;",
            " not { $r links (note: $y); };",
            " { not { $r links (note: $p2); }; } or",
            " delete $r;",
            " match $t isa thing;",
            ";",
            "{",
            "}",
            " or",
            " (source: $x)",
            " $x",
            " not",
        ];

        #[derive(Debug, Clone)]
        enum Edit {
            Insert(usize, usize),
            Delete(usize, usize),
            Duplicate(usize, usize),
        }

        fn edit() -> impl Strategy<Value = Edit> {
            prop_oneof![
                (any::<usize>(), 0..FRAGMENTS.len()).prop_map(|(at, f)| Edit::Insert(at, f)),
                (any::<usize>(), 1usize..12).prop_map(|(at, n)| Edit::Delete(at, n)),
                (any::<usize>(), 1usize..20).prop_map(|(at, n)| Edit::Duplicate(at, n)),
            ]
        }

        /// `text` with `e` applied; the texts are ASCII, so every index is a char
        /// boundary.
        fn apply(text: &str, e: &Edit) -> String {
            let len = text.len();
            match *e {
                Edit::Insert(at, f) => {
                    let at = at % (len + 1);
                    format!("{}{}{}", &text[..at], FRAGMENTS[f], &text[at..])
                }
                Edit::Delete(at, n) => {
                    let at = at % len;
                    let end = (at + n).min(len);
                    format!("{}{}", &text[..at], &text[end..])
                }
                Edit::Duplicate(at, n) => {
                    let at = at % len;
                    let end = (at + n).min(len);
                    format!("{}{}", &text[..end], &text[at..])
                }
            }
        }

        fn accepted(tql: String) -> bool {
            let tql: &'static str = Box::leak(tql.into_boxed_str());
            check_migrations(&[entry(tql)], SCHEMA).is_ok()
        }

        proptest! {
            /// A cleanup the checker accepts deletes exactly the rows missing a
            /// required role: whatever was edited into or out of an admitted text, an
            /// accepted result is one of the admitted texts.
            #[test]
            fn an_accepted_cleanup_is_an_admitted_one(
                start in 0usize..8,
                edits in proptest::collection::vec(edit(), 1..=3),
            ) {
                let admitted: Vec<String> =
                    admitted_cleanups().iter().map(|t| normalize(t)).collect();
                let mut text = admitted_cleanups()[start].clone();
                for e in &edits {
                    text = apply(&text, e);
                }
                if accepted(text.clone()) {
                    prop_assert!(
                        admitted.contains(&normalize(&text)),
                        "accepted a cleanup that is not admitted: {text}"
                    );
                }
            }

            /// Whitespace is not meaning: an admitted cleanup with its spaces turned
            /// into runs of spaces, tabs and newlines is still accepted.
            #[test]
            fn an_admitted_cleanup_survives_any_whitespace(
                start in 0usize..8,
                gaps in proptest::collection::vec(
                    proptest::sample::select(vec![" ", "  ", "\n", "\t", "\n    "]), 64),
            ) {
                let text = admitted_cleanups()[start].clone();
                let mut spaced = String::new();
                for (i, word) in text.split(' ').enumerate() {
                    if i > 0 {
                        spaced.push_str(gaps[i % gaps.len()]);
                    }
                    spaced.push_str(word);
                }
                prop_assert!(accepted(spaced.clone()), "rejected {spaced:?}");
            }
        }
    }
}
