//! Shape checks that keep a [`SchemaMigration`] list in step with the shipped
//! schema it leads to.
//!
//! [`check_migrations`] accepts each entry only in one of these forms:
//!
//! - a **cleanup**, `match $r isa T; not { $r links (R: $x); }; delete $r;` (one or
//!   more `not` clauses, joined by `{ … } or { … }`), where every role `R` it names
//!   is declared `@card(1…)` on `T` or a supertype of `T` in the shipped schema;
//! - a **type removal**, `match $r isa T; delete $r;`, where the same list also
//!   undefines `T` and the shipped schema declares no `T`;
//! - an **update**, `match $r isa T, has A $v; …; update $r has A …;` with nothing
//!   after it, where the shipped `owns A` of `T` (or a supertype) carries a `@range`;
//! - an `undefine @doc from X;`, where the shipped schema follows `X` with `@doc(`;
//! - an `undefine T;` or `undefine fun F;`, where the shipped schema declares no `T`
//!   or `F`;
//! - a `redefine X;`, where the shipped schema contains `X`.
//!
//! Every entry's `applies_when` text must also be absent from the shipped schema.

use crate::SchemaMigration;

/// Checks every entry of `migrations` against `schema`, the shipped schema text.
/// The error names the first offending migration's id and the rule it breaks.
pub fn check_migrations(migrations: &[SchemaMigration], schema: &str) -> Result<(), String> {
    let statements = statements(schema);
    for m in migrations {
        if schema.contains(m.applies_when) {
            return Err(format!(
                "{}: the shipped schema still contains the replaced text",
                m.id
            ));
        }
        check_one(m, migrations, schema, &statements).map_err(|e| format!("{}: {e}", m.id))?;
    }
    Ok(())
}

fn check_one(
    m: &SchemaMigration,
    all: &[SchemaMigration],
    schema: &str,
    statements: &[String],
) -> Result<(), String> {
    let tql = m.tql;
    if tql.starts_with("match ") {
        if tql.contains(" update ") {
            return check_update(tql, statements);
        }
        return check_cleanup(tql, all, statements);
    }
    if let Some(target) = tql.strip_prefix("undefine @doc from ") {
        let target = single_statement(target, "an `undefine @doc from X;`")?;
        if !schema.contains(&format!("{target} @doc(")) {
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

fn check_cleanup(tql: &str, all: &[SchemaMigration], statements: &[String]) -> Result<(), String> {
    const FORM: &str = "a cleanup is exactly one `match $r isa T; … delete $r;` statement";
    let rest = tql.strip_prefix("match $r isa ").ok_or(FORM)?;
    let (type_name, rest) = rest.split_once(';').ok_or(FORM)?;
    let body = rest.strip_suffix(" delete $r;").ok_or(FORM)?;
    if !is_label(type_name) || body.contains("delete") || body.contains("match") {
        return Err(FORM.to_string());
    }

    let mut roles = Vec::new();
    let mut remainder = String::new();
    let mut tail = body;
    while let Some(at) = tail.find("not { $r links (") {
        remainder.push_str(&tail[..at]);
        let after = &tail[at + "not { $r links (".len()..];
        let (role, after) = after.split_once(':').ok_or(FORM)?;
        let (player, after) = after.split_once(");").ok_or(FORM)?;
        let after = after.trim_start().strip_prefix("};").ok_or(FORM)?;
        if !is_label(role) || !player.trim().starts_with('$') {
            return Err(FORM.to_string());
        }
        roles.push(role.to_string());
        tail = after;
    }
    remainder.push_str(tail);
    let joiners_only = remainder
        .split_whitespace()
        .all(|t| matches!(t, "{" | "}" | "or" | "};" | ";"));
    if !joiners_only {
        return Err(format!(
            "{FORM}; between `isa {type_name};` and `delete` it holds only `not {{ $r links (…) }}` clauses"
        ));
    }

    if roles.is_empty() {
        let undefined = all
            .iter()
            .any(|o| o.tql == format!("undefine {type_name};"));
        if !undefined || !statements_of(statements, type_name).is_empty() {
            return Err(format!(
                "a cleanup without a `not` clause removes a type, so the list undefines \
                 {type_name} and the shipped schema no longer declares it"
            ));
        }
        return Ok(());
    }
    for role in &roles {
        let clause = find_clause(statements, type_name, &format!("relates {role}"))
            .ok_or_else(|| format!("the shipped {type_name} declares no role {role}"))?;
        if !clause.contains("@card(1") {
            return Err(format!(
                "the shipped {type_name} does not require role {role} with `@card(1`"
            ));
        }
    }
    Ok(())
}

fn check_update(tql: &str, statements: &[String]) -> Result<(), String> {
    const FORM: &str =
        "an update is exactly one `match $r isa T, …; update $r has A …;` with nothing after it";
    let rest = tql.strip_prefix("match $r isa ").ok_or(FORM)?;
    let type_name = rest
        .split([',', ';'])
        .next()
        .map(str::trim)
        .filter(|t| is_label(t))
        .ok_or(FORM)?;
    let (head, update) = rest.split_once(" update ").ok_or(FORM)?;
    if head.contains("update") || head.contains("delete") || head.contains("insert") {
        return Err(FORM.to_string());
    }
    let assignment = single_statement(update, FORM)?;
    let attribute = assignment
        .strip_prefix("$r has ")
        .and_then(|a| a.split_whitespace().next())
        .filter(|a| is_label(a))
        .ok_or(FORM)?;
    let clause = find_clause(statements, type_name, &format!("owns {attribute}"))
        .ok_or_else(|| format!("the shipped {type_name} does not own {attribute}"))?;
    if !clause.contains("@range(") {
        return Err(format!(
            "the shipped `owns {attribute}` of {type_name} carries no `@range`"
        ));
    }
    Ok(())
}

/// The first clause of `type_name`'s statements (or its supertypes') whose first
/// two words are `head`.
fn find_clause(statements: &[String], type_name: &str, head: &str) -> Option<String> {
    let mut current = type_name.to_string();
    for _ in 0..32 {
        let own = statements_of(statements, &current);
        let mut supertype = None;
        for statement in &own {
            let clauses = split_top_level(statement, ',');
            for clause in &clauses {
                let words: Vec<&str> = clause
                    .split_whitespace()
                    .take_while(|w| !w.starts_with("@doc("))
                    .collect();
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
fn statements(schema: &str) -> Vec<String> {
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
    relates note @doc("An optional role."),
    owns score @card(0..1) @range(-1.0..1.0),   # cosine
    owns count @card(0..1);
  entity thing @doc("A thing."),
    plays link:source;
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
        assert!(err.contains("holds only `not"), "{err}");
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
        assert!(err.contains("an update is exactly one"), "{err}");
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
    fn an_unknown_form_is_rejected() {
        let err = check("insert $x isa thing;").unwrap_err();
        assert!(err.contains("a migration is"), "{err}");
    }
}
