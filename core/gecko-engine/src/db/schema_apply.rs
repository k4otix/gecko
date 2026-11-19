//! Re-appliable schema: splitting a `define` fragment into types and functions.
//!
//! TypeDB re-`define`s types idempotently but refuses to `define` a function that
//! already exists (`[FUN5]`), whether or not its body changed. To keep schema
//! application safe on an already-initialized database, a fragment is split into
//! its type definitions and its individual `fun` blocks: types and new functions
//! are `define`d, and functions the database already holds are `redefine`d, so
//! a changed body replaces the stored one.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

static FUN_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*fun\s+([A-Za-z_][A-Za-z0-9_-]*)\s*\(").expect("valid fun-start regex")
});

static TYPE_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(entity|relation|attribute|struct)\s").expect("valid type-start regex")
});

/// One `fun` definition, without a leading `define`/`redefine` keyword.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDef {
    /// The function's name.
    pub name: String,
    /// The definition text, from `fun` through the end of its body.
    pub text: String,
}

/// A schema fragment split into its type definitions and its functions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitSchema {
    /// Everything before the first function, without any `define` keyword lines.
    pub types: String,
    /// Each function, in source order.
    pub functions: Vec<FunctionDef>,
}

/// Splits a schema fragment made of one or more `define` blocks. Every type
/// definition must precede the first function; a fragment that defines a type
/// after a function is rejected, since that type would be carried into a
/// function's `redefine`.
pub fn split(schema: &str) -> Result<SplitSchema, String> {
    let mut types = String::new();
    let mut functions: Vec<FunctionDef> = Vec::new();
    for line in schema.lines() {
        if line.trim() == "define" {
            continue;
        }
        if let Some(caps) = FUN_START.captures(line) {
            functions.push(FunctionDef {
                name: caps[1].to_string(),
                text: String::new(),
            });
        } else if let Some(current) = functions.last()
            && TYPE_START.is_match(line)
        {
            return Err(format!(
                "schema defines a type after function '{}': {}",
                current.name,
                line.trim()
            ));
        }
        let target = match functions.last_mut() {
            Some(f) => &mut f.text,
            None => &mut types,
        };
        target.push_str(line);
        target.push('\n');
    }
    Ok(SplitSchema { types, functions })
}

/// The names of every function defined in `schema` (e.g. a database's exported
/// schema text).
pub fn function_names(schema: &str) -> HashSet<String> {
    schema
        .lines()
        .filter_map(|l| FUN_START.captures(l).map(|c| c[1].to_string()))
        .collect()
}

/// The schema-transaction queries that apply `schema` to a database already
/// holding the functions in `existing`: one `define` of the types plus every new
/// function, then one `redefine` per function that already exists. When none of
/// the fragment's functions exist yet, or the fragment is not a `define` (an
/// explicit `redefine`/`undefine`), it is applied unchanged.
pub fn plan_queries(schema: &str, existing: &HashSet<String>) -> Result<Vec<String>, String> {
    if schema
        .lines()
        .any(|l| matches!(l.trim(), "redefine" | "undefine"))
    {
        return Ok(vec![schema.to_string()]);
    }
    let split = split(schema)?;
    let (redefine, define): (Vec<_>, Vec<_>) = split
        .functions
        .into_iter()
        .partition(|f| existing.contains(&f.name));
    if redefine.is_empty() {
        return Ok(vec![schema.to_string()]);
    }
    let mut queries = Vec::new();
    let mut define_body = split.types;
    for f in &define {
        define_body.push_str(&f.text);
    }
    if define_body.lines().any(|l| {
        let t = l.trim();
        !t.is_empty() && !t.starts_with('#')
    }) {
        queries.push(format!("define\n{define_body}"));
    }
    for f in redefine {
        queries.push(format!("redefine\n{}", f.text));
    }
    Ok(queries)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = "define
  # a comment
  entity thing, owns name;
  attribute name, value string;

  fun one() -> { thing }:
      match $t isa thing;
      return { $t };

  # section header
define
  fun two($t: thing) -> boolean:
      match $t has name $n;
      return check;
";

    fn existing(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn split_separates_types_from_functions() {
        let s = split(SCHEMA).unwrap();
        assert!(s.types.contains("entity thing"));
        assert!(!s.types.contains("fun "));
        assert!(!s.types.contains("define"));
        let names: Vec<_> = s.functions.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["one", "two"]);
        assert!(s.functions[0].text.starts_with("  fun one()"));
        assert!(s.functions[1].text.contains("return check;"));
    }

    #[test]
    fn split_rejects_a_type_after_a_function() {
        let bad = "define\n  fun f() -> boolean:\n    match $x isa thing;\n    return check;\n  entity late;\n";
        assert!(split(bad).unwrap_err().contains("late"));
    }

    #[test]
    fn fresh_database_applies_the_fragment_unchanged() {
        assert_eq!(
            plan_queries(SCHEMA, &existing(&[])).unwrap(),
            vec![SCHEMA.to_string()]
        );
    }

    #[test]
    fn existing_functions_are_redefined_and_new_ones_defined() {
        let q = plan_queries(SCHEMA, &existing(&["one"])).unwrap();
        assert_eq!(q.len(), 2);
        assert!(q[0].starts_with("define\n"));
        assert!(q[0].contains("entity thing") && q[0].contains("fun two("));
        assert!(!q[0].contains("fun one("));
        assert!(q[1].starts_with("redefine\n  fun one()"));
    }

    #[test]
    fn functions_only_fragment_with_all_existing_emits_only_redefines() {
        let funs = "define\n  fun one() -> boolean:\n    match $x isa thing;\n    return check;\n";
        let q = plan_queries(funs, &existing(&["one"])).unwrap();
        assert_eq!(
            q,
            vec![
                "redefine\n  fun one() -> boolean:\n    match $x isa thing;\n    return check;\n"
                    .to_string()
            ]
        );
    }

    #[test]
    fn explicit_redefine_fragment_passes_through() {
        let r = "redefine\n  fun one() -> boolean:\n    match $x isa thing;\n    return check;\n";
        assert_eq!(
            plan_queries(r, &existing(&["one"])).unwrap(),
            vec![r.to_string()]
        );
    }

    #[test]
    fn function_names_reads_an_exported_schema() {
        assert_eq!(function_names(SCHEMA), existing(&["one", "two"]));
    }
}
