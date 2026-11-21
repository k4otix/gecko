//! Reads the attributes each entity type must own from TypeDB's canonical schema text.
//!
//! TypeDB 3.15 cannot query annotations, so the required set is parsed from the
//! text the server returns for `database schema`.

use std::collections::{BTreeSet, HashMap};

use super::router::DbError;

/// One entity statement: its supertype and the `owns` clauses it declares itself.
struct EntityDecl {
    sup: Option<String>,
    /// Declared attribute label to whether the declaration requires it.
    owns: Vec<(String, bool)>,
}

/// For every entity type in `schema`, the attribute labels it must own: those with a
/// minimum cardinality of at least one or `@key`, inherited through `sub`, where a
/// subtype's own `owns` declaration of an attribute replaces its supertype's.
pub fn required_attributes(schema: &str) -> Result<HashMap<String, BTreeSet<String>>, DbError> {
    let mut decls: HashMap<String, EntityDecl> = HashMap::new();
    for statement in split_top_level(schema, ';') {
        let statement = statement.trim();
        let statement = statement
            .strip_prefix("define")
            .map_or(statement, str::trim);
        let Some(body) = statement.strip_prefix("entity") else {
            continue;
        };
        if !body.starts_with(char::is_whitespace) {
            continue;
        }
        let (label, decl) = parse_entity(body)?;
        decls.insert(label, decl);
    }

    let mut resolved = HashMap::new();
    for label in decls.keys() {
        let required = resolve(label, &decls, &mut Vec::new())?;
        resolved.insert(label.clone(), required);
    }
    Ok(resolved)
}

/// Whether `schema` defines an attribute type labelled `label`.
pub fn defines_attribute(schema: &str, label: &str) -> bool {
    split_top_level(schema, ';').iter().any(|statement| {
        let statement = statement.trim();
        let statement = statement
            .strip_prefix("define")
            .map_or(statement, str::trim);
        let Some(body) = statement.strip_prefix("attribute") else {
            return false;
        };
        body.starts_with(char::is_whitespace)
            && split_top_level(body, ',')
                .first()
                .and_then(|head| split_words(head).into_iter().next())
                .is_some_and(|l| l == label)
    })
}

fn resolve(
    label: &str,
    decls: &HashMap<String, EntityDecl>,
    path: &mut Vec<String>,
) -> Result<BTreeSet<String>, DbError> {
    if path.iter().any(|p| p == label) {
        return Err(DbError::Schema(format!(
            "entity type '{label}' is its own supertype"
        )));
    }
    let decl = decls
        .get(label)
        .ok_or_else(|| DbError::Schema(format!("unknown entity supertype '{label}'")))?;
    let mut required = match &decl.sup {
        Some(sup) => {
            path.push(label.to_string());
            let inherited = resolve(sup, decls, path)?;
            path.pop();
            inherited
        }
        None => BTreeSet::new(),
    };
    for (attr, is_required) in &decl.owns {
        if *is_required {
            required.insert(attr.clone());
        } else {
            required.remove(attr);
        }
    }
    Ok(required)
}

fn parse_entity(body: &str) -> Result<(String, EntityDecl), DbError> {
    let mut clauses = split_top_level(body, ',').into_iter();
    let head = clauses.next().unwrap_or_default();
    let label = split_words(&head)
        .into_iter()
        .next()
        .ok_or_else(|| DbError::Schema(format!("entity statement without a label: '{body}'")))?;
    let mut decl = EntityDecl {
        sup: None,
        owns: Vec::new(),
    };
    for clause in clauses {
        let words = split_words(&clause);
        match words.first().map(String::as_str) {
            Some("sub") => {
                let sup = words.get(1).ok_or_else(|| {
                    DbError::Schema(format!("'sub' without a type in entity '{label}'"))
                })?;
                decl.sup = Some(sup.clone());
            }
            Some("owns") => {
                let attr = words.get(1).ok_or_else(|| {
                    DbError::Schema(format!("'owns' without an attribute in entity '{label}'"))
                })?;
                let attr = attr.trim_end_matches("[]").to_string();
                let required = words[2..]
                    .iter()
                    .try_fold(false, |acc, w| annotation_requires(w).map(|r| acc || r))?;
                decl.owns.push((attr, required));
            }
            _ => {}
        }
    }
    Ok((label, decl))
}

/// Whether one `owns` annotation makes the attribute required.
fn annotation_requires(word: &str) -> Result<bool, DbError> {
    if word == "@key" {
        return Ok(true);
    }
    let Some(args) = word
        .strip_prefix("@card(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return Ok(false);
    };
    let min = args.split("..").next().unwrap_or("").trim();
    min.parse::<u64>()
        .map(|n| n >= 1)
        .map_err(|_| DbError::Schema(format!("unreadable cardinality '{word}'")))
}

/// Splits on `sep` outside quoted strings and parentheses.
fn split_top_level(text: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut depth = 0usize;
    for c in text.chars() {
        if in_string {
            current.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                current.push(c);
            }
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            c if c == sep && depth == 0 => parts.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    parts.push(current);
    parts
}

/// Splits on whitespace outside quoted strings and parentheses.
fn split_words(text: &str) -> Vec<String> {
    split_top_level(&text.replace(char::is_whitespace, " "), ' ')
        .into_iter()
        .filter(|w| !w.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = r#"define
attribute a1,
 value string;
attribute a2 @doc("x; with, punctuation"),
 value string;
entity c @doc("y; z, w"),
  owns k @key,
  owns a1 @card(1..1),
  owns a2 @card(0..1);
entity o,
  sub c,
  owns a2 @card(1..3);
entity p,
  sub o,
  owns a1 @card(0..2);
"#;

    #[test]
    fn defines_attribute_matches_whole_labels_only() {
        assert!(defines_attribute(SCHEMA, "a1"));
        assert!(defines_attribute(SCHEMA, "a2"));
        assert!(!defines_attribute(SCHEMA, "a"));
        assert!(!defines_attribute(SCHEMA, "c"));
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn required_attributes_reads_cards_keys_and_inheritance() {
        let r = required_attributes(SCHEMA).unwrap();
        assert_eq!(r["c"], set(&["k", "a1"]));
        assert_eq!(r["o"], set(&["k", "a1", "a2"]));
        assert_eq!(r["p"], set(&["k", "a2"]));
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn required_attributes_reads_single_and_open_cards_and_abstract() {
        let r = required_attributes(
            "define\nentity e @abstract,\n owns a @card(2),\n owns b @card(1..),\n owns c @card(0..);\n",
        )
        .unwrap();
        assert_eq!(r["e"], set(&["a", "b"]));
    }

    #[test]
    fn required_attributes_ignores_relations_and_functions() {
        let schema = format!(
            "{SCHEMA}relation r,\n  relates x,\n  owns a1 @card(1..1);\n\
             fun f($e: c) -> {{ a1 }}:\n match $e isa c, has a1 $v;\n return {{ $v }};\n"
        );
        let r = required_attributes(&schema).unwrap();
        assert_eq!(r.len(), 3);
        assert!(!r.contains_key("r"));
    }

    #[test]
    fn required_attributes_rejects_an_unknown_supertype() {
        let err = required_attributes("define\nentity q,\n  sub missing;\n").unwrap_err();
        match err {
            DbError::Schema(m) => assert!(m.contains("missing"), "{m}"),
            other => panic!("wrong error: {other:?}"),
        }
    }
}
