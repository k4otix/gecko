//! Re-appliable schema: planning a `define` fragment against a database's stored
//! functions.
//!
//! TypeDB re-`define`s types idempotently but refuses to `define` a function that
//! already exists (`[FUN5]`), whether or not its body changed, and validates each
//! `redefine`d function against the functions stored at that moment. To keep schema
//! application safe on an already-initialized database, a fragment is split into
//! its type statements and its individual `fun` blocks, each function is compared
//! with the stored one, and the queries are ordered so that a function is written
//! only after every function it calls: types first, then new functions (`define`)
//! and changed functions (`redefine`) callee-first. Unchanged functions are left
//! alone.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

static FUN_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*fun\s+([A-Za-z_][A-Za-z0-9_-]*)\s*\(").expect("valid fun-start regex")
});

static CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^A-Za-z0-9_$-])([A-Za-z_][A-Za-z0-9_-]*)\s*\(").expect("valid call regex")
});

/// One `fun` definition, without a leading `define`/`redefine` keyword.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDef {
    /// The function's name.
    pub name: String,
    /// The definition text, from `fun` through its `return` statement.
    pub text: String,
}

impl FunctionDef {
    /// The parameter and return types, whitespace removed, without annotations.
    fn signature(&self) -> String {
        let header = self.text.lines().next().unwrap_or("").trim();
        let header = header.split(" @").next().unwrap_or(header);
        let header = header.trim_end().trim_end_matches(':');
        header.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// The definition with indentation, blank lines and comment lines removed, for
    /// comparison with the text a database exports.
    fn normalized(&self) -> String {
        self.text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The names this function calls, other than itself.
    fn calls(&self) -> HashSet<String> {
        CALL.captures_iter(&self.text)
            .map(|c| c[1].to_string())
            .filter(|n| *n != self.name)
            .collect()
    }
}

/// A schema fragment split into its type statements and its functions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitSchema {
    /// Every statement outside a function, without any `define` keyword lines.
    pub types: String,
    /// Each function, in source order.
    pub functions: Vec<FunctionDef>,
}

/// Where the splitter is relative to the current function.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// Outside any function.
    Types,
    /// Inside a function, before its `return`.
    Body,
    /// Inside a function's `return` statement, before its closing `;`.
    Return,
}

/// Splits a schema fragment made of one or more `define` blocks. A function ends at
/// the `;` that closes its `return` statement; a statement after it belongs to the
/// type section, and blank and comment lines after it stay with the function.
pub fn split(schema: &str) -> Result<SplitSchema, String> {
    let mut types = String::new();
    let mut functions: Vec<FunctionDef> = Vec::new();
    let mut state = State::Types;
    for line in schema.lines() {
        let trimmed = line.trim();
        if trimmed == "define" {
            continue;
        }
        let ignorable = trimmed.is_empty() || trimmed.starts_with('#');
        if let Some(caps) = FUN_START.captures(line) {
            if state != State::Types {
                let current = functions.last().map_or("", |f| f.name.as_str());
                return Err(format!(
                    "function '{current}' has no return statement before function '{}'",
                    &caps[1]
                ));
            }
            functions.push(FunctionDef {
                name: caps[1].to_string(),
                text: String::new(),
            });
            state = State::Body;
        } else if state == State::Body && trimmed.starts_with("return") {
            state = State::Return;
        }
        let target = match functions.last_mut() {
            Some(f) if state != State::Types || ignorable => &mut f.text,
            _ => &mut types,
        };
        target.push_str(line);
        target.push('\n');
        if state == State::Return && trimmed.ends_with(';') {
            state = State::Types;
        }
    }
    if state != State::Types {
        let current = functions.last().map_or("", |f| f.name.as_str());
        return Err(format!("function '{current}' has no return statement"));
    }
    Ok(SplitSchema { types, functions })
}

/// The functions stored in a database, read from its exported schema text.
pub fn stored_functions(exported: &str) -> Result<HashMap<String, FunctionDef>, String> {
    Ok(split(exported)?
        .functions
        .into_iter()
        .map(|f| (f.name.clone(), f))
        .collect())
}

/// What the plan does with one of the fragment's functions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Change {
    New,
    Changed,
}

/// The schema-transaction queries that apply `schema` to a database storing the
/// functions in `stored`: one `define` of the type statements, then each new
/// function `define`d and each changed function `redefine`d, callee-first.
/// Functions that call each other are written together; when one of them already
/// exists with a different signature, the change cannot be expressed as a sequence
/// of `redefine`s and the plan is refused. When none of the fragment's functions
/// exist yet, or the fragment is an explicit `redefine`/`undefine`, it is applied
/// unchanged.
pub fn plan_queries(
    schema: &str,
    stored: &HashMap<String, FunctionDef>,
) -> Result<Vec<String>, String> {
    if schema
        .lines()
        .any(|l| matches!(l.trim(), "redefine" | "undefine"))
    {
        return Ok(vec![schema.to_string()]);
    }
    let split = split(schema)?;
    if !split.functions.iter().any(|f| stored.contains_key(&f.name)) {
        return Ok(vec![schema.to_string()]);
    }

    let pending: Vec<(FunctionDef, Change)> = split
        .functions
        .into_iter()
        .filter_map(|f| match stored.get(&f.name) {
            None => Some((f, Change::New)),
            Some(s) if s.normalized() != f.normalized() => Some((f, Change::Changed)),
            Some(_) => None,
        })
        .collect();

    let mut queries = Vec::new();
    if split.types.lines().any(|l| {
        let t = l.trim();
        !t.is_empty() && !t.starts_with('#')
    }) {
        queries.push(format!("define\n{}", split.types));
    }
    for group in callee_first_groups(&pending) {
        let changed_signature: Vec<&str> = group
            .iter()
            .map(|&i| &pending[i])
            .filter(|(f, c)| *c == Change::Changed && stored[&f.name].signature() != f.signature())
            .map(|(f, _)| f.name.as_str())
            .collect();
        if group.len() > 1 && !changed_signature.is_empty() {
            let names: Vec<&str> = group.iter().map(|&i| pending[i].0.name.as_str()).collect();
            return Err(format!(
                "functions {} call each other and {} changes signature; a stored \
                 database cannot be moved to this schema by redefinition \
                 (apply it to a fresh database, or undefine the functions first)",
                names.join(", "),
                changed_signature.join(", ")
            ));
        }
        let new: String = group
            .iter()
            .filter(|&&i| pending[i].1 == Change::New)
            .map(|&i| pending[i].0.text.as_str())
            .collect();
        if !new.is_empty() {
            queries.push(format!("define\n{new}"));
        }
        for &i in &group {
            if pending[i].1 == Change::Changed {
                queries.push(format!("redefine\n{}", pending[i].0.text));
            }
        }
    }
    Ok(queries)
}

/// Groups the functions into strongly connected components of the call graph,
/// ordered so that every component comes after the components it calls (Tarjan's
/// algorithm emits them in that order). Members keep source order within a group.
fn callee_first_groups(functions: &[(FunctionDef, Change)]) -> Vec<Vec<usize>> {
    let index_of: HashMap<&str, usize> = functions
        .iter()
        .enumerate()
        .map(|(i, (f, _))| (f.name.as_str(), i))
        .collect();
    let edges: Vec<Vec<usize>> = functions
        .iter()
        .map(|(f, _)| {
            let mut out: Vec<usize> = f
                .calls()
                .iter()
                .filter_map(|n| index_of.get(n.as_str()).copied())
                .collect();
            out.sort_unstable();
            out
        })
        .collect();

    struct Tarjan<'a> {
        edges: &'a [Vec<usize>],
        next: usize,
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        stack: Vec<usize>,
        on_stack: Vec<bool>,
        groups: Vec<Vec<usize>>,
    }
    impl Tarjan<'_> {
        fn visit(&mut self, v: usize) {
            self.index[v] = Some(self.next);
            self.low[v] = self.next;
            self.next += 1;
            self.stack.push(v);
            self.on_stack[v] = true;
            for i in 0..self.edges[v].len() {
                let w = self.edges[v][i];
                match self.index[w] {
                    None => {
                        self.visit(w);
                        self.low[v] = self.low[v].min(self.low[w]);
                    }
                    Some(wi) if self.on_stack[w] => self.low[v] = self.low[v].min(wi),
                    Some(_) => {}
                }
            }
            if Some(self.low[v]) == self.index[v] {
                let mut group = Vec::new();
                while let Some(w) = self.stack.pop() {
                    self.on_stack[w] = false;
                    group.push(w);
                    if w == v {
                        break;
                    }
                }
                group.sort_unstable();
                self.groups.push(group);
            }
        }
    }

    let n = functions.len();
    let mut t = Tarjan {
        edges: &edges,
        next: 0,
        index: vec![None; n],
        low: vec![0; n],
        stack: Vec::new(),
        on_stack: vec![false; n],
        groups: Vec::new(),
    };
    for v in 0..n {
        if t.index[v].is_none() {
            t.visit(v);
        }
    }
    t.groups
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

    /// A database export: functions unindented, no comments.
    fn stored(defs: &[&str]) -> HashMap<String, FunctionDef> {
        stored_functions(&format!("define\nentity thing;\n{}", defs.join(""))).unwrap()
    }

    const ONE: &str = "fun one() -> { thing }:\n  match $t isa thing;\n  return { $t };\n";
    const TWO: &str = "fun two($t: thing) -> boolean:\n  match $t has name $n;\n  return check;\n";

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
    fn a_statement_after_a_function_belongs_to_the_types() {
        let s = split(
            "define\n  fun f() -> boolean:\n    match $x isa thing;\n    return check;\n  \
             # trailing\n  thing owns name @card(0..1);\n  entity late;\n",
        )
        .unwrap();
        assert!(s.types.contains("thing owns name @card(0..1);"));
        assert!(s.types.contains("entity late;"));
        assert!(!s.functions[0].text.contains("owns"));
        assert!(s.functions[0].text.contains("# trailing"));
    }

    #[test]
    fn a_multi_line_return_stays_in_the_function() {
        let s = split("define\n  fun f() -> { thing, name }:\n    match $t isa thing, has name $n;\n    return {\n      $t, $n\n    };\n  entity late;\n").unwrap();
        assert!(s.functions[0].text.contains("$t, $n"));
        assert!(s.types.contains("entity late;"));
    }

    #[test]
    fn a_function_without_return_is_rejected() {
        let err = split("define\n  fun f() -> boolean:\n    match $x isa thing;\n  fun g() -> boolean:\n    match $x isa thing;\n    return check;\n").unwrap_err();
        assert!(err.contains("'f'"), "{err}");
    }

    #[test]
    fn fresh_database_applies_the_fragment_unchanged() {
        assert_eq!(
            plan_queries(SCHEMA, &HashMap::new()).unwrap(),
            vec![SCHEMA.to_string()]
        );
    }

    #[test]
    fn unchanged_functions_are_left_alone() {
        let q = plan_queries(SCHEMA, &stored(&[ONE, TWO])).unwrap();
        assert_eq!(q.len(), 1);
        assert!(q[0].starts_with("define\n") && q[0].contains("entity thing"));
        assert!(!q[0].contains("fun "));
    }

    #[test]
    fn changed_functions_are_redefined_and_new_ones_defined() {
        let old_one =
            "fun one() -> { thing }:\n  match $t isa thing, has name $n;\n  return { $t };\n";
        let q = plan_queries(SCHEMA, &stored(&[old_one])).unwrap();
        assert_eq!(q.len(), 3);
        assert!(q[0].contains("entity thing") && !q[0].contains("fun "));
        assert!(q[1].starts_with("redefine\n  fun one()"));
        assert!(q[2].starts_with("define\n  fun two("));
    }

    #[test]
    fn a_callee_is_written_before_its_caller() {
        // `b` precedes `a` in source but calls it; both change.
        let fragment = "define
  fun b($x: thing) -> boolean:
      match let $y in a($x, $x);
      return check;
  fun a($x: thing, $z: thing) -> { thing }:
      match $x isa thing; $z isa thing;
      return { $x };
";
        let old = [
            "fun a($x: thing) -> { thing }:\n  match $x isa thing;\n  return { $x };\n",
            "fun b($x: thing) -> boolean:\n  match let $y in a($x);\n  return check;\n",
        ];
        let q = plan_queries(fragment, &stored(&old)).unwrap();
        assert_eq!(q.len(), 2);
        assert!(q[0].starts_with("redefine\n  fun a("), "{q:?}");
        assert!(q[1].starts_with("redefine\n  fun b("), "{q:?}");
    }

    #[test]
    fn a_new_caller_is_defined_after_its_changed_callee() {
        let fragment = "define
  fun c($x: thing) -> boolean:
      match let $y in a($x, $x);
      return check;
  fun a($x: thing, $z: thing) -> { thing }:
      match $x isa thing; $z isa thing;
      return { $x };
";
        let old = ["fun a($x: thing) -> { thing }:\n  match $x isa thing;\n  return { $x };\n"];
        let q = plan_queries(fragment, &stored(&old)).unwrap();
        assert!(q[0].starts_with("redefine\n  fun a("), "{q:?}");
        assert!(q[1].starts_with("define\n  fun c("), "{q:?}");
    }

    #[test]
    fn a_mutual_signature_change_is_refused() {
        let fragment = "define
  fun a($x: thing, $z: thing) -> { thing }:
      match let $y in b($x);
      return { $y };
  fun b($x: thing) -> { thing }:
      match let $y in a($x, $x);
      return { $y };
";
        let old = [
            "fun a($x: thing) -> { thing }:\n  match let $y in b($x);\n  return { $y };\n",
            "fun b($x: thing) -> { thing }:\n  match let $y in a($x);\n  return { $y };\n",
        ];
        let err = plan_queries(fragment, &stored(&old)).unwrap_err();
        assert!(
            err.contains("a, b") && err.contains("changes signature"),
            "{err}"
        );
    }

    #[test]
    fn mutual_recursion_with_unchanged_signatures_is_redefined() {
        let fragment = "define
  fun a($x: thing) -> { thing }:
      match let $y in b($x); $y isa thing;
      return { $y };
  fun b($x: thing) -> { thing }:
      match let $y in a($x);
      return { $y };
";
        let old = [
            "fun a($x: thing) -> { thing }:\n  match let $y in b($x);\n  return { $y };\n",
            "fun b($x: thing) -> { thing }:\n  match let $y in a($x);\n  return { $y };\n",
        ];
        let q = plan_queries(fragment, &stored(&old)).unwrap();
        assert_eq!(
            q,
            vec![format!(
                "redefine\n{}",
                split(fragment).unwrap().functions[0].text
            )]
        );
    }

    #[test]
    fn calls_ignore_longer_names_and_self() {
        let f = &split(
            "define\n  fun blast($r: thing) -> { thing }:\n    match let $m in blast-at($r); let $n in blast($m);\n    return { $n };\n",
        )
        .unwrap()
        .functions[0];
        assert_eq!(f.calls(), HashSet::from(["blast-at".to_string()]));
    }

    #[test]
    fn explicit_redefine_fragment_passes_through() {
        let r = "redefine\n  fun one() -> boolean:\n    match $x isa thing;\n    return check;\n";
        assert_eq!(
            plan_queries(r, &stored(&[ONE])).unwrap(),
            vec![r.to_string()]
        );
    }
}
