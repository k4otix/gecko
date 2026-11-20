//! Builds the set of belief-edge relations each extension may write, refusing any it does
//! not own (design §3).
//!
//! Every check fails closed. A form the position rules cannot prove safe is refused.

use std::collections::{HashMap, HashSet};

use gecko_extension_api::{BeliefEdgeDecl, GeckoExtension, SchemaMigration};

use super::scan::{Mode, Statement, statements};

/// The name mem's extension registers under; its schema is scanned as mem's fragment.
const MEM_EXTENSION: &str = "mem-gecko";

/// The abstract root of every memory node. No non-belief role may be played by it, a
/// subtype of it, or a supertype of it or of any of its subtypes.
const MEMORY_ITEM: &str = "memory-item";

/// One extension's schema text and belief-edge declarations, as the registry scans them.
#[derive(Debug, Clone, Copy)]
pub struct ExtensionFragment<'a> {
    pub name: &'a str,
    pub schema: &'a str,
    pub migrations: &'a [SchemaMigration],
    pub decls: &'a [BeliefEdgeDecl],
}

impl<'a> ExtensionFragment<'a> {
    /// Reads the fragment from a registered extension.
    pub fn of(ext: &'a dyn GeckoExtension) -> Self {
        Self {
            name: ext.name(),
            schema: ext.schema(),
            migrations: ext.schema_migrations(),
            decls: ext.belief_edges(),
        }
    }

    /// The statements of the schema and of every migration.
    fn statements(&self) -> Vec<Statement> {
        let mut stmts = statements(self.schema);
        for m in self.migrations {
            stmts.extend(statements(m.tql));
        }
        stmts
    }
}

/// Why a set of belief-edge declarations was refused.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error(
        "extension `{extension}` declares belief edge `{relation}` with invalid label {label:?}: labels must match ^[a-z][a-z0-9-]*$"
    )]
    BadLabel {
        extension: String,
        relation: String,
        label: String,
    },
    #[error("extension `{extension}` does not own belief edge `{relation}`: {reason}")]
    NotOwned {
        extension: String,
        relation: String,
        reason: String,
    },
    #[error(
        "extension `{extension}` declares belief edge `{relation}`, which is a subtype of `{supertype}`"
    )]
    SubtypedRelation {
        extension: String,
        relation: String,
        supertype: String,
    },
    #[error(
        "extension `{extension}` declares belief edge `{relation}`, which overrides a role with `relates … as …`"
    )]
    RoleAlias { extension: String, relation: String },
    #[error("belief edge `{relation}` is declared more than once (by extension `{extension}`)")]
    DuplicateRelation { extension: String, relation: String },
    #[error(
        "extension `{extension}` declares belief edge `{relation}` with role `{role}` more than once"
    )]
    DuplicateRole {
        extension: String,
        relation: String,
        role: String,
    },
    #[error(
        "extension `{extension}` declares belief edge `{relation}` with role `{role}` played by `{player}`, which is a memory item, a supertype of one, or a type whose hierarchy cannot be resolved"
    )]
    MemoryItemPlayer {
        extension: String,
        relation: String,
        role: String,
        player: String,
    },
    #[error(
        "extension `{extension}` gives `{label}` a supertype, but the core or mem schema introduces that type"
    )]
    ReparentedType { extension: String, label: String },
    #[error(
        "extension `{extension}` has an `alias` token in its schema or a migration; the belief-edge registry cannot resolve aliased types"
    )]
    AliasUnsupported { extension: String },
    #[error(
        "extension `{extension}` renames a type or role with `label` in its schema or a migration; the belief-edge registry resolves types by their declared labels"
    )]
    RenameUnsupported { extension: String },
    #[error("extension `{extension}` {}", modifies_base_detail(.mode, .label))]
    ModifiesBase {
        extension: String,
        mode: &'static str,
        label: String,
    },
    #[error("extension `{extension}` is not registered; the belief-edge registry needs its schema")]
    MemMissing { extension: String },
}

/// The tail of the `ModifiesBase` message for `mode`.
fn modifies_base_detail(mode: &str, label: &str) -> String {
    match mode {
        "define" => format!(
            "defines a function named like core or mem function `{label}`; an extension may change only its own schema"
        ),
        _ => format!(
            "{mode}s core or mem type or function `{label}`; an extension may change only its own schema"
        ),
    }
}

/// The belief-edge relations each extension owns and may write.
#[derive(Debug, Clone, Default)]
pub struct BeliefEdgeRegistry {
    /// Relation label to its owning extension and declaration.
    by_relation: HashMap<String, (String, BeliefEdgeDecl)>,
}

impl BeliefEdgeRegistry {
    /// Validates every extension's declarations against the core, mem and extension
    /// fragments and returns the registry, or the first refusal.
    pub fn build(
        core_schema: &str,
        mem_schema: &str,
        extensions: &[ExtensionFragment],
    ) -> Result<Self, RegistryError> {
        let core = statements(core_schema);
        let mem = statements(mem_schema);
        let ext_stmts: Vec<Vec<Statement>> = extensions
            .iter()
            .map(ExtensionFragment::statements)
            .collect();
        let hierarchy = Hierarchy::new(core.iter().chain(&mem).chain(ext_stmts.iter().flatten()));

        let base_types = introduced_labels(core.iter().chain(&mem));
        let base_fns: HashSet<&str> = core
            .iter()
            .chain(&mem)
            .filter_map(Statement::introduces_function)
            .collect();
        for (ext, stmts) in extensions.iter().zip(&ext_stmts) {
            // No rule resolves an alias to the type it names, so none is allowed.
            if stmts.iter().any(|s| s.has("alias")) {
                return Err(RegistryError::AliasUnsupported {
                    extension: ext.name.to_owned(),
                });
            }
            // A rename moves a label out from under every rule that resolved it.
            if stmts.iter().any(Statement::renames) {
                return Err(RegistryError::RenameUnsupported {
                    extension: ext.name.to_owned(),
                });
            }
            check_no_base_modification(ext.name, stmts, &base_types, &base_fns)?;
            check_no_reparenting(ext.name, stmts, &base_types)?;
        }

        let mut by_relation: HashMap<String, (String, BeliefEdgeDecl)> = HashMap::new();
        for (i, ext) in extensions.iter().enumerate() {
            for decl in ext.decls {
                check_labels(ext.name, decl)?;
                check_shape(ext.name, decl)?;
                if by_relation.contains_key(decl.relation) {
                    return Err(RegistryError::DuplicateRelation {
                        extension: ext.name.to_owned(),
                        relation: decl.relation.to_owned(),
                    });
                }
                let others = [("the core schema", &core), ("mem", &mem)]
                    .into_iter()
                    .chain(
                        extensions
                            .iter()
                            .zip(&ext_stmts)
                            .enumerate()
                            .filter(|(j, _)| *j != i)
                            .map(|(_, (e, s))| (e.name, s)),
                    );
                check_ownership(ext.name, decl, &ext_stmts[i], others)?;
                check_players(ext.name, decl, &hierarchy)?;
                by_relation.insert(
                    decl.relation.to_owned(),
                    (ext.name.to_owned(), decl.clone()),
                );
            }
        }
        Ok(Self { by_relation })
    }

    /// Builds the registry from the registered extensions, taking mem's schema and
    /// migrations from the `mem-gecko` extension and every other extension as a declaring
    /// fragment.
    pub fn from_extensions(
        core_schema: &str,
        extensions: &[Box<dyn GeckoExtension>],
    ) -> Result<Self, RegistryError> {
        let mem = extensions
            .iter()
            .find(|e| e.name() == MEM_EXTENSION)
            .ok_or_else(|| RegistryError::MemMissing {
                extension: MEM_EXTENSION.to_owned(),
            })?;
        // Each text is closed with its own `;` so no statement runs into the next text.
        let mut mem_text = format!("{}\n;\n", mem.schema());
        for m in mem.schema_migrations() {
            mem_text.push_str(m.tql);
            mem_text.push_str("\n;\n");
        }
        let fragments: Vec<ExtensionFragment> = extensions
            .iter()
            .filter(|e| e.name() != MEM_EXTENSION)
            .map(|e| ExtensionFragment::of(e.as_ref()))
            .collect();
        Self::build(core_schema, &mem_text, &fragments)
    }

    /// The declaration of `relation`, if `extension` owns it.
    pub fn decl(&self, extension: &str, relation: &str) -> Option<&BeliefEdgeDecl> {
        self.by_relation
            .get(relation)
            .filter(|(owner, _)| owner == extension)
            .map(|(_, decl)| decl)
    }

    /// The extension that owns `relation`.
    pub fn owner(&self, relation: &str) -> Option<&str> {
        self.by_relation
            .get(relation)
            .map(|(owner, _)| owner.as_str())
    }

    /// Whether `extension` owns any belief-edge relation.
    pub fn declares_any(&self, extension: &str) -> bool {
        self.by_relation
            .values()
            .any(|(owner, _)| owner == extension)
    }
}

/// Whether `label` matches `^[a-z][a-z0-9-]*$`, the form safe to interpolate into query text.
fn valid_label(label: &str) -> bool {
    let mut chars = label.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn check_labels(extension: &str, decl: &BeliefEdgeDecl) -> Result<(), RegistryError> {
    let labels = [decl.relation, decl.belief_role]
        .into_iter()
        .chain(decl.roles.iter().flat_map(|r| [r.role, r.player, r.key]))
        .chain(decl.owned_attrs.iter().map(|a| a.attr));
    for label in labels {
        if !valid_label(label) {
            return Err(RegistryError::BadLabel {
                extension: extension.to_owned(),
                relation: decl.relation.to_owned(),
                label: label.to_owned(),
            });
        }
    }
    Ok(())
}

fn check_shape(extension: &str, decl: &BeliefEdgeDecl) -> Result<(), RegistryError> {
    let mut roles = HashSet::new();
    for role in std::iter::once(decl.belief_role).chain(decl.roles.iter().map(|r| r.role)) {
        if !roles.insert(role) {
            return Err(RegistryError::DuplicateRole {
                extension: extension.to_owned(),
                relation: decl.relation.to_owned(),
                role: role.to_owned(),
            });
        }
    }
    Ok(())
}

/// Every label a statement introduces with `[define] entity|relation|attribute <label>`.
fn introduced_labels<'s>(stmts: impl Iterator<Item = &'s Statement>) -> HashSet<&'s str> {
    stmts
        .flat_map(|s| ["entity", "relation", "attribute"].map(|kind| s.introduces(kind)))
        .flatten()
        .collect()
}

/// Refuses an extension statement that defines a function named like one in `base_fns`, and
/// any `undefine` or `redefine` statement that names a type in `base_types` or a function in
/// `base_fns`: an extension may change only its own schema.
fn check_no_base_modification(
    extension: &str,
    stmts: &[Statement],
    base_types: &HashSet<&str>,
    base_fns: &HashSet<&str>,
) -> Result<(), RegistryError> {
    let refuse = |mode: Mode, label: &str| RegistryError::ModifiesBase {
        extension: extension.to_owned(),
        mode: match mode {
            Mode::Define => "define",
            Mode::Redefine => "redefine",
            Mode::Undefine => "undefine",
        },
        label: label.to_owned(),
    };
    for stmt in stmts {
        if let Some(name) = stmt.introduces_function()
            && base_fns.contains(name)
        {
            return Err(refuse(stmt.mode(), name));
        }
        if stmt.mode() != Mode::Define
            && let Some(token) = stmt
                .tokens()
                .find(|t| base_types.contains(t) || base_fns.contains(t))
        {
            return Err(refuse(stmt.mode(), token));
        }
    }
    Ok(())
}

/// Refuses an extension statement that is about a type in `base_types` and has a `sub`
/// token: the supertypes of core and mem types are fixed by the schemas that introduce them.
fn check_no_reparenting(
    extension: &str,
    stmts: &[Statement],
    base_types: &HashSet<&str>,
) -> Result<(), RegistryError> {
    for stmt in stmts {
        if let Some(label) = stmt.first_label()
            && base_types.contains(label)
            && stmt.has("sub")
        {
            return Err(RegistryError::ReparentedType {
                extension: extension.to_owned(),
                label: label.to_owned(),
            });
        }
    }
    Ok(())
}

/// The declared relation plus every subtype of it the declarer's own fragment defines,
/// directly or through another such subtype.
fn own_closure<'s>(relation: &'s str, own: &'s [Statement]) -> HashSet<&'s str> {
    let mut closure = HashSet::from([relation]);
    loop {
        let before = closure.len();
        for stmt in own {
            if let Some(label) = stmt.first_label()
                && stmt.supertypes().any(|s| closure.contains(s))
            {
                closure.insert(label);
            }
        }
        if closure.len() == before {
            return closure;
        }
    }
}

/// Refuses `decl` unless the declarer alone mentions the relation and its subtypes, and
/// defines the relation once cleanly: no `sub` on it and no `as` alongside it.
fn check_ownership<'s>(
    extension: &str,
    decl: &BeliefEdgeDecl,
    own: &[Statement],
    others: impl Iterator<Item = (&'s str, &'s Vec<Statement>)>,
) -> Result<(), RegistryError> {
    let not_owned = |reason: String| RegistryError::NotOwned {
        extension: extension.to_owned(),
        relation: decl.relation.to_owned(),
        reason,
    };
    let closure = own_closure(decl.relation, own);
    for (name, stmts) in others {
        for stmt in stmts {
            if let Some(label) = closure.iter().find(|l| stmt.has(l)) {
                return Err(not_owned(format!("{name} also mentions `{label}`")));
            }
        }
    }
    for stmt in own {
        if stmt.first_label() == Some(decl.relation) && stmt.has("sub") {
            return Err(RegistryError::SubtypedRelation {
                extension: extension.to_owned(),
                relation: decl.relation.to_owned(),
                supertype: stmt.supertypes().next().unwrap_or("<missing>").to_owned(),
            });
        }
        if stmt.has(decl.relation) && stmt.has("as") {
            return Err(RegistryError::RoleAlias {
                extension: extension.to_owned(),
                relation: decl.relation.to_owned(),
            });
        }
    }
    if !own
        .iter()
        .any(|s| s.introduces("relation") == Some(decl.relation))
    {
        return Err(not_owned(
            "its schema fragment has no `relation` statement for it".to_owned(),
        ));
    }
    Ok(())
}

/// The `sub` links of every scanned fragment, and which labels some statement introduces.
struct Hierarchy<'s> {
    parents: HashMap<&'s str, Vec<&'s str>>,
    /// `memory-item`, every type below it, and every type above any of those.
    memory_lineage: HashSet<&'s str>,
    introduced: HashSet<&'s str>,
    entities: HashSet<&'s str>,
}

impl<'s> Hierarchy<'s> {
    fn new(stmts: impl Iterator<Item = &'s Statement>) -> Self {
        let mut h = Self {
            parents: HashMap::new(),
            memory_lineage: HashSet::new(),
            introduced: HashSet::new(),
            entities: HashSet::new(),
        };
        for stmt in stmts {
            if let Some(label) = stmt.first_label() {
                for parent in stmt.supertypes() {
                    h.parents.entry(label).or_default().push(parent);
                }
            }
            for kind in ["entity", "relation", "attribute"] {
                if let Some(label) = stmt.introduces(kind) {
                    h.introduced.insert(label);
                    if kind == "entity" {
                        h.entities.insert(label);
                    }
                }
            }
        }
        h.memory_lineage = h
            .parents
            .keys()
            .copied()
            .chain([MEMORY_ITEM])
            .map(|label| h.reachable(label))
            .filter(|above| above.contains(MEMORY_ITEM))
            .flatten()
            .collect();
        h
    }

    fn parents_of(&self, label: &str) -> &[&'s str] {
        self.parents.get(label).map_or(&[], Vec::as_slice)
    }

    /// Every label reachable from `from` through `sub` links, `from` included.
    fn reachable(&self, from: &'s str) -> HashSet<&'s str> {
        let mut seen = HashSet::from([from]);
        let mut pending = vec![from];
        while let Some(current) = pending.pop() {
            for &parent in self.parents_of(current) {
                if seen.insert(parent) {
                    pending.push(parent);
                }
            }
        }
        seen
    }

    /// Whether `player` is provably neither a memory item nor a supertype of one. Every
    /// type above it must be introduced, ending in an entity root.
    fn clear_of_memory_items(&self, player: &'s str) -> bool {
        if self.memory_lineage.contains(player) {
            return false;
        }
        self.reachable(player).into_iter().all(|label| {
            if self.parents_of(label).is_empty() {
                self.entities.contains(label)
            } else {
                self.introduced.contains(label)
            }
        })
    }
}

fn check_players(
    extension: &str,
    decl: &BeliefEdgeDecl,
    hierarchy: &Hierarchy,
) -> Result<(), RegistryError> {
    for role in decl.roles {
        if !hierarchy.clear_of_memory_items(role.player) {
            return Err(RegistryError::MemoryItemPlayer {
                extension: extension.to_owned(),
                relation: decl.relation.to_owned(),
                role: role.role.to_owned(),
                player: role.player.to_owned(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemGecko;
    use gecko_extension_api::{AttrDecl, AttrValueType, HostImportDef, RoleDecl};

    const CORE: &str = include_str!("../../../gecko-engine/schema/core_schema.tql");

    const PROBE_SCHEMA: &str = "define\n  relation probe-link @doc(\"Links a belief to a concept.\"),\n      relates claim,\n      relates subject,\n      owns probe-note;\n  attribute probe-note, value string;\n  belief plays probe-link:claim;\n  concept plays probe-link:subject;\n";

    const CONCEPT_ROLE: &[RoleDecl] = &[RoleDecl {
        role: "subject",
        player: "concept",
        key: "concept-id",
    }];

    const NOTE_ATTR: &[AttrDecl] = &[AttrDecl {
        attr: "probe-note",
        value: AttrValueType::String,
        required: false,
    }];

    fn decl(relation: &'static str) -> BeliefEdgeDecl {
        BeliefEdgeDecl {
            relation,
            belief_role: "claim",
            roles: CONCEPT_ROLE,
            owned_attrs: NOTE_ATTR,
        }
    }

    fn decl_with_player(relation: &'static str, player: &'static str) -> BeliefEdgeDecl {
        let roles: &'static [RoleDecl] = Box::leak(Box::new([RoleDecl {
            role: "subject",
            player,
            key: "concept-id",
        }]));
        BeliefEdgeDecl {
            relation,
            belief_role: "claim",
            roles,
            owned_attrs: &[],
        }
    }

    fn frag<'a>(
        name: &'a str,
        schema: &'a str,
        decls: &'a [BeliefEdgeDecl],
    ) -> ExtensionFragment<'a> {
        ExtensionFragment {
            name,
            schema,
            migrations: &[],
            decls,
        }
    }

    fn build(extensions: &[ExtensionFragment]) -> Result<BeliefEdgeRegistry, RegistryError> {
        let mem = MemGecko::new();
        BeliefEdgeRegistry::build(CORE, mem.schema(), extensions)
    }

    #[test]
    fn refuses_an_extension_that_modifies_a_base_type_or_function() {
        let decls = [decl("probe-link")];
        let rows: [(&str, &str, &str); 13] = [
            ("undefine belief;", "undefine", "belief"),
            (
                "redefine relation my-edge sub supersession;",
                "redefine",
                "supersession",
            ),
            ("redefine entity belief sub foo;", "redefine", "belief"),
            (
                "undefine owns confidence from belief;",
                "undefine",
                "confidence",
            ),
            (
                "undefine plays probe-link:claim from belief;",
                "undefine",
                "belief",
            ),
            (
                "undefine @abstract from memory-item;",
                "undefine",
                "memory-item",
            ),
            ("undefine fun believed-at;", "undefine", "believed-at"),
            (
                "redefine attribute confidence value string;",
                "redefine",
                "confidence",
            ),
            (
                "redefine belief owns confidence @card(1);",
                "redefine",
                "belief",
            ),
            (
                "redefine\nfun believed-at($t: datetime) -> { belief }:\n match $b isa belief;\n return { $b };",
                "redefine",
                "believed-at",
            ),
            (
                "redefine probe-link owns confidence @card(1);",
                "redefine",
                "confidence",
            ),
            (
                "redefine probe-link relates evidence @card(0..1);",
                "redefine",
                "evidence",
            ),
            (
                "define\nfun believed-at($b: belief) -> boolean:\n match $b isa belief;\n return check;",
                "define",
                "believed-at",
            ),
        ];
        for (text, mode, label) in rows {
            let migration = [SchemaMigration {
                id: "probe/modify",
                applies_when: "",
                tql: text,
            }];
            let fragment = if text.contains("probe-link") {
                ExtensionFragment {
                    name: "probe-ext",
                    schema: PROBE_SCHEMA,
                    migrations: &migration,
                    decls: &decls,
                }
            } else {
                frag("probe-ext", text, &[])
            };
            match build(&[fragment]) {
                Err(RegistryError::ModifiesBase {
                    extension,
                    mode: got_mode,
                    label: got_label,
                }) => {
                    assert_eq!(extension, "probe-ext", "{text}");
                    assert_eq!(got_mode, mode, "{text}");
                    assert_eq!(got_label, label, "{text}");
                }
                other => panic!("{text}: expected ModifiesBase, got {other:?}"),
            }
        }
    }

    #[test]
    fn accepts_an_extension_that_modifies_only_its_own_types() {
        let decls = [decl("probe-link")];
        let migrations = [
            SchemaMigration {
                id: "probe/undefine",
                applies_when: "",
                tql: "undefine owns probe-note from probe-link;",
            },
            SchemaMigration {
                id: "probe/redefine",
                applies_when: "",
                tql: "redefine relation probe-link @abstract;",
            },
        ];
        let fragment = ExtensionFragment {
            name: "probe-ext",
            schema: PROBE_SCHEMA,
            migrations: &migrations,
            decls: &decls,
        };
        build(&[fragment]).expect("builds");
    }

    #[test]
    fn refuses_an_explicit_redefine_of_an_own_function_whose_body_names_a_base_type() {
        let text = "redefine\nfun probe-fn() -> boolean:\n match $b isa belief, has belief-state \"asserted\";\n return check;";
        match build(&[frag("probe-ext", text, &[])]) {
            Err(RegistryError::ModifiesBase { mode, label, .. }) => {
                assert_eq!(mode, "redefine");
                assert_eq!(label, "belief");
            }
            other => panic!("expected ModifiesBase, got {other:?}"),
        }
    }

    #[test]
    fn rename_of_a_base_type_still_reports_rename_unsupported() {
        let err = build(&[frag("probe-ext", "redefine belief label idea;", &[])]).unwrap_err();
        assert!(
            matches!(err, RegistryError::RenameUnsupported { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn accepts_an_owned_relation() {
        let decls = [decl("probe-link")];
        let reg = build(&[frag("probe-ext", PROBE_SCHEMA, &decls)]).expect("builds");
        assert_eq!(reg.decl("probe-ext", "probe-link"), Some(&decls[0]));
        assert_eq!(reg.owner("probe-link"), Some("probe-ext"));
        assert!(reg.declares_any("probe-ext"));
        assert!(!reg.declares_any("other-ext"));
        assert_eq!(reg.decl("other-ext", "probe-link"), None);
    }

    #[test]
    fn refuses_a_mem_relation() {
        // The declarer re-defines the relation itself, so only mem's head refuses it.
        let decls = [decl("supersession")];
        let schema = "define relation supersession, relates claim, relates subject;";
        let err = build(&[frag("probe-ext", schema, &decls)]).unwrap_err();
        assert_not_owned(&err, "mem also mentions");
    }

    #[test]
    fn refuses_a_core_relation() {
        let decls = [decl("hierarchy")];
        let schema = "define relation hierarchy, relates claim, relates subject;";
        let err = build(&[frag("probe-ext", schema, &decls)]).unwrap_err();
        assert_not_owned(&err, "the core schema also mentions");
        let ghost = [decl("ghost-link")];
        let err = build(&[frag("probe-ext", PROBE_SCHEMA, &ghost)]).unwrap_err();
        assert_not_owned(&err, "no `relation` statement");
    }

    #[test]
    fn refuses_another_extensions_relation() {
        let decls = [decl("x-link")];
        let err = build(&[
            frag(
                "x-ext",
                "define relation x-link, relates claim, relates subject;",
                &[],
            ),
            frag(
                "y-ext",
                "define relation x-link, relates claim, relates subject;",
                &decls,
            ),
        ])
        .unwrap_err();
        assert_not_owned(&err, "x-ext also mentions");
    }

    #[test]
    fn refuses_a_relation_another_extension_subtypes() {
        let decls = [decl("x-link")];
        let err = build(&[
            frag(
                "x-ext",
                "define relation x-link, relates claim, relates subject;",
                &decls,
            ),
            frag(
                "y-ext",
                "define relation y-sub sub x-link, relates claim, relates subject;",
                &[],
            ),
        ])
        .unwrap_err();
        assert_not_owned(&err, "y-ext also mentions");

        // The owner may subtype its own relation.
        build(&[frag(
            "x-ext",
            "define relation x-link, relates claim, relates subject; relation x-sub sub x-link;",
            &decls,
        )])
        .expect("an owner's own subtype is allowed");
    }

    const X_LINK: &str = "define relation x-link, relates claim, relates subject;\n";

    /// Builds with X owning and declaring `x-link` and Y contributing `y_schema`.
    fn build_x_and_y(x_extra: &str, y_schema: &str) -> Result<BeliefEdgeRegistry, RegistryError> {
        let decls = [decl("x-link")];
        let x_schema = format!("{X_LINK}{x_extra}");
        build(&[
            frag("x-ext", &x_schema, &decls),
            frag("y-ext", y_schema, &[]),
        ])
    }

    #[test]
    fn refuses_keyword_and_xid_named_intermediates() {
        for mid in [
            "label",
            "value",
            "check",
            "limit",
            "select",
            "distinct",
            "\u{2118}mid",
        ] {
            let schema = format!(
                "{PROBE_SCHEMA}  entity {mid} sub execution-episode;\n  entity sighting sub {mid};\n"
            );
            let decls = [decl_with_player("probe-link", "sighting")];
            let err = build(&[frag("probe-ext", &schema, &decls)]).unwrap_err();
            assert!(
                matches!(err, RegistryError::MemoryItemPlayer { .. }),
                "{mid}: {err:?}"
            );
        }
        let schema = format!("{PROBE_SCHEMA}  entity check sub execution-episode;\n");
        let decls = [decl_with_player("probe-link", "check")];
        let err = build(&[frag("probe-ext", &schema, &decls)]).unwrap_err();
        assert!(
            matches!(err, RegistryError::MemoryItemPlayer { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn refuses_statements_after_a_cr_terminated_comment() {
        let err = build_x_and_y("", "define\n# c\rx-link relates extra;\n").unwrap_err();
        assert_not_owned(&err, "y-ext also mentions");

        let schema = format!("{PROBE_SCHEMA}  # c\rentity sighting sub execution-episode;\n");
        let decls = [decl_with_player("probe-link", "sighting")];
        let err = build(&[frag("probe-ext", &schema, &decls)]).unwrap_err();
        assert!(
            matches!(err, RegistryError::MemoryItemPlayer { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn refuses_a_transitive_subtype_from_another_fragment() {
        let err = build_x_and_y(
            "relation x-sub sub x-link;\n",
            "define relation y-sub sub x-sub, relates a;",
        )
        .unwrap_err();
        assert_not_owned(&err, "y-ext also mentions");
    }

    #[test]
    fn refuses_a_keyword_named_subtype_from_another_fragment() {
        let err = build_x_and_y("", "define relation label sub x-link, relates a;").unwrap_err();
        assert_not_owned(&err, "y-ext also mentions");
    }

    #[test]
    fn refuses_plays_from_another_fragment() {
        let err = build_x_and_y("", "define concept plays x-link:subject;").unwrap_err();
        assert_not_owned(&err, "y-ext also mentions");
    }

    #[test]
    fn refuses_an_unresolvable_player() {
        let decls = [decl_with_player("probe-link", "ghost")];
        let err = build(&[frag("probe-ext", PROBE_SCHEMA, &decls)]).unwrap_err();
        assert!(
            matches!(err, RegistryError::MemoryItemPlayer { .. }),
            "{err:?}"
        );
    }

    const CYBER_SHAPED: &str = "define\n  entity cyber-object sub concept @doc(\"Root.\"),\n      owns attack-id @card(0..1) @unique;\n  attribute attack-id, value string;\n  entity attack-pattern sub cyber-object @doc(\"A technique.\"),\n      plays technique-detection:detected-technique;\n  relation technique-detection @doc(\"Logic covers a technique.\"),\n      relates detecting-logic @doc(\"The logic.\"),\n      relates detected-technique @doc(\"The technique.\"),\n      relates detection-claim @doc(\"The belief.\");\n  concept plays technique-detection:detecting-logic;\n  belief plays technique-detection:detection-claim;\n";

    const CYBER_ROLES: &[RoleDecl] = &[
        RoleDecl {
            role: "detecting-logic",
            player: "concept",
            key: "concept-id",
        },
        RoleDecl {
            role: "detected-technique",
            player: "attack-pattern",
            key: "attack-id",
        },
    ];

    #[test]
    fn accepts_a_cyber_shaped_extension() {
        let decls = [BeliefEdgeDecl {
            relation: "technique-detection",
            belief_role: "detection-claim",
            roles: CYBER_ROLES,
            owned_attrs: &[],
        }];
        let reg = build(&[frag("cyber-ext", CYBER_SHAPED, &decls)]).expect("builds");
        assert_eq!(reg.owner("technique-detection"), Some("cyber-ext"));
    }

    #[test]
    fn refuses_alias_and_value_touches() {
        let err = build_x_and_y("", "define x-link alias other-name;").unwrap_err();
        assert_alias_unsupported(&err, "y-ext");
        let err = build_x_and_y("", "define x-link value string;").unwrap_err();
        assert_not_owned(&err, "y-ext also mentions");
    }

    fn assert_alias_unsupported(err: &RegistryError, by: &str) {
        match err {
            RegistryError::AliasUnsupported { extension } => assert_eq!(extension, by),
            other => panic!("expected AliasUnsupported, got {other:?}"),
        }
        assert!(err.to_string().contains(by), "{err}");
    }

    #[test]
    fn refuses_an_extension_fragment_with_an_alias() {
        // An alias would re-parent a mem type under a label no other rule resolves.
        let foo = [decl_with_player("probe-link", "foo")];
        let aliased = format!("{PROBE_SCHEMA}  entity foo;\n  belief alias b2;\n  b2 sub foo;\n");
        let err = build(&[frag("probe-ext", &aliased, &foo)]).unwrap_err();
        assert_alias_unsupported(&err, "probe-ext");

        let plain = format!("{PROBE_SCHEMA}  entity foo;\n");
        let err = build_with_migration(
            "probe-ext",
            &plain,
            Some("define belief alias b2; b2 sub foo;"),
            &foo,
        )
        .unwrap_err();
        assert_alias_unsupported(&err, "probe-ext");

        // An extension that declares nothing is refused as well.
        let probe = [decl("probe-link")];
        let err = build(&[
            frag("probe-ext", PROBE_SCHEMA, &probe),
            frag(
                "y-ext",
                "define entity foo; belief alias b2; b2 sub foo;",
                &[],
            ),
        ])
        .unwrap_err();
        assert_alias_unsupported(&err, "y-ext");

        // The word inside a comment or a string is not a token.
        let quoted =
            format!("{PROBE_SCHEMA}  # alias\n  entity foo @doc(\"An alias for nothing.\");\n");
        build(&[frag("probe-ext", &quoted, &foo)]).expect("builds");
    }

    fn assert_rename_unsupported(err: &RegistryError, by: &str) {
        match err {
            RegistryError::RenameUnsupported { extension } => assert_eq!(extension, by),
            other => panic!("expected RenameUnsupported, got {other:?}"),
        }
        assert!(err.to_string().contains(by), "{err}");
    }

    #[test]
    fn refuses_an_extension_fragment_that_renames_a_type_or_role() {
        // A rename of a mem type or role.
        for rename in [
            "redefine\n  belief label idea;\n",
            "redefine\n  entity belief label idea;\n",
            "redefine\n  supersession:superseded label old;\n",
        ] {
            let err = build_x_and_y("", rename).unwrap_err();
            assert_rename_unsupported(&err, "y-ext");
        }
        // A rename of the declarer's own relation, which its declaration names by label.
        let err = build_x_and_y("redefine\n  x-link label y-link;\n", "").unwrap_err();
        assert_rename_unsupported(&err, "x-ext");
        // A rename carried by a migration.
        let probe = [decl("probe-link")];
        let err = build_with_migration(
            "probe-ext",
            PROBE_SCHEMA,
            Some("redefine concept label thing-x;"),
            &probe,
        )
        .unwrap_err();
        assert_rename_unsupported(&err, "probe-ext");

        // A function body's `$t label …` pattern is not a rename.
        let with_fun = format!(
            "{PROBE_SCHEMA}  fun probe-link-exists() -> boolean:\n    match $t label probe-link;\n    return check;\n"
        );
        build(&[frag("probe-ext", &with_fun, &probe)]).expect("builds");
    }

    fn assert_not_owned(err: &RegistryError, why: &str) {
        match err {
            RegistryError::NotOwned { reason, .. } => {
                assert!(reason.contains(why), "{reason:?} lacks {why:?}")
            }
            other => panic!("expected NotOwned, got {other:?}"),
        }
    }

    #[test]
    fn refuses_a_role_addition() {
        let decls = [decl("x-link")];
        for addition in [
            "define relation x-link, relates extra;",
            "define x-link relates extra;",
        ] {
            let err = build(&[
                frag(
                    "x-ext",
                    "define relation x-link, relates claim, relates subject;",
                    &decls,
                ),
                frag("y-ext", addition, &[]),
            ])
            .unwrap_err();
            assert!(
                matches!(err, RegistryError::NotOwned { .. }),
                "{addition}: {err:?}"
            );
        }
    }

    #[test]
    fn refuses_a_subtype_relation() {
        let decls = [decl("my-edge")];
        for (schema, migration) in [
            (
                "define relation my-edge sub supersession, relates claim, relates subject;",
                None,
            ),
            (
                "define relation my-edge, relates claim, relates subject; relation parent-edge, relates claim, relates subject;",
                Some("redefine relation my-edge sub parent-edge;"),
            ),
            (
                "define relation my-edge, relates claim, relates subject; my-edge sub supersession;",
                None,
            ),
            (
                "define relation my-edge @doc('a;b'), sub supersession, relates claim;",
                None,
            ),
            (
                "define relation my-edge, relates claim, relates subject; my-edge@abstract, sub supersession;",
                None,
            ),
            (
                "define relation my-edge@abstract, sub supersession, relates claim, relates subject;",
                None,
            ),
        ] {
            let err = build_with_migration("probe-ext", schema, migration, &decls).unwrap_err();
            assert!(
                matches!(err, RegistryError::SubtypedRelation { .. }),
                "{schema} / {migration:?}: {err:?}"
            );
        }
    }

    #[test]
    fn refuses_a_relates_as_role() {
        let decls = [decl("my-edge")];
        let base = "define relation my-edge, relates claim, relates subject;";
        for (schema, migration) in [
            (
                "define relation my-edge, relates claim, relates subject as superseded;",
                None,
            ),
            (
                "define relation my-edge, relates claim; my-edge relates subject as superseded;",
                None,
            ),
            (
                base,
                Some("redefine relation my-edge relates subject as superseded;"),
            ),
            (
                "define relation my-edge, relates claim; my-edge@doc(\"x\") relates subject as superseded;",
                None,
            ),
        ] {
            let err = build_with_migration("probe-ext", schema, migration, &decls).unwrap_err();
            assert!(
                matches!(err, RegistryError::RoleAlias { .. }),
                "{schema} / {migration:?}: {err:?}"
            );
        }
    }

    fn build_with_migration(
        name: &str,
        schema: &str,
        migration: Option<&'static str>,
        decls: &[BeliefEdgeDecl],
    ) -> Result<BeliefEdgeRegistry, RegistryError> {
        let migrations: Vec<SchemaMigration> = migration
            .into_iter()
            .map(|tql| SchemaMigration {
                id: "probe/m",
                applies_when: "never",
                tql,
            })
            .collect();
        build(&[ExtensionFragment {
            name,
            schema,
            migrations: &migrations,
            decls,
        }])
    }

    #[test]
    fn refuses_bad_labels() {
        for bad in ["Probe-Link", "probe_link", "probe link", ""] {
            let decls = [decl(bad)];
            let err = build(&[frag("probe-ext", PROBE_SCHEMA, &decls)]).unwrap_err();
            assert!(
                matches!(err, RegistryError::BadLabel { .. }),
                "{bad:?}: {err:?}"
            );
        }
        let decls = [decl_with_player("probe-link", "Concept")];
        let err = build(&[frag("probe-ext", PROBE_SCHEMA, &decls)]).unwrap_err();
        assert!(matches!(err, RegistryError::BadLabel { .. }), "{err:?}");
    }

    #[test]
    fn refuses_duplicate_relation_and_roles() {
        let decls = [decl("probe-link"), decl("probe-link")];
        let err = build(&[frag("probe-ext", PROBE_SCHEMA, &decls)]).unwrap_err();
        assert!(
            matches!(err, RegistryError::DuplicateRelation { .. }),
            "{err:?}"
        );

        let mut same_role = decl("probe-link");
        same_role.belief_role = "subject";
        let decls = [same_role];
        let err = build(&[frag("probe-ext", PROBE_SCHEMA, &decls)]).unwrap_err();
        assert!(
            matches!(err, RegistryError::DuplicateRole { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn refuses_memory_item_players() {
        for (extra, migration) in [
            ("  entity sighting sub execution-episode;\n", None),
            (
                "  entity sighting;\n  sighting sub execution-episode;\n",
                None,
            ),
            (
                "  entity Mid sub execution-episode;\n  entity sighting;\n",
                Some("redefine entity sighting sub Mid;"),
            ),
            (
                "  entity Mid sub execution-episode;\n  entity sighting sub Mid;\n",
                None,
            ),
            (
                "  entity _mid sub execution-episode;\n  entity sighting sub _mid;\n",
                None,
            ),
            ("  entity sighting@abstract, sub execution-episode;\n", None),
        ] {
            let schema = format!("{PROBE_SCHEMA}{extra}");
            for player in [
                "belief",
                "episode",
                "okf-concept",
                "memory-item",
                "sighting",
            ] {
                let decls = [decl_with_player("probe-link", player)];
                let err =
                    build_with_migration("probe-ext", &schema, migration, &decls).unwrap_err();
                assert!(
                    matches!(err, RegistryError::MemoryItemPlayer { .. }),
                    "{extra} / {migration:?} / {player}: {err:?}"
                );
            }
            let decls = [decl_with_player("probe-link", "concept")];
            build_with_migration("probe-ext", &schema, migration, &decls)
                .expect("concept is not a memory item");
        }

        // A second supertype on memory-item makes that type a supertype too.
        let err = build_with_mem_extra(
            "define entity probe-root; memory-item sub probe-root;",
            &[frag(
                "probe-ext",
                PROBE_SCHEMA,
                &[decl_with_player("probe-link", "probe-root")],
            )],
        )
        .unwrap_err();
        assert!(
            matches!(err, RegistryError::MemoryItemPlayer { .. }),
            "{err:?}"
        );
    }

    /// Builds with `extra` appended to mem's schema text.
    fn build_with_mem_extra(
        extra: &str,
        extensions: &[ExtensionFragment],
    ) -> Result<BeliefEdgeRegistry, RegistryError> {
        let mem_text = format!("{}\n;\n{extra}", MemGecko::new().schema());
        BeliefEdgeRegistry::build(CORE, &mem_text, extensions)
    }

    #[test]
    fn refuses_an_ancestor_of_a_memory_item_subtype() {
        for extra in [
            "define entity foo; belief sub foo;",
            "define entity foo; redefine entity belief sub foo;",
            "define entity top; entity foo sub top; execution-episode sub foo;",
        ] {
            for player in ["foo", "belief", "execution-episode"] {
                let decls = [decl_with_player("probe-link", player)];
                let err = build_with_mem_extra(extra, &[frag("probe-ext", PROBE_SCHEMA, &decls)])
                    .unwrap_err();
                assert!(
                    matches!(err, RegistryError::MemoryItemPlayer { .. }),
                    "{extra} / {player}: {err:?}"
                );
            }
            let decls = [decl_with_player("probe-link", "concept")];
            build_with_mem_extra(extra, &[frag("probe-ext", PROBE_SCHEMA, &decls)])
                .expect("concept stays clear of memory items");
        }
        let decls = [decl_with_player("probe-link", "top")];
        let err = build_with_mem_extra(
            "define entity top; entity foo sub top; execution-episode sub foo;",
            &[frag("probe-ext", PROBE_SCHEMA, &decls)],
        )
        .unwrap_err();
        assert!(
            matches!(err, RegistryError::MemoryItemPlayer { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn refuses_an_extension_that_reparents_a_core_or_mem_type() {
        let foo = [decl_with_player("probe-link", "foo")];
        let concept = [decl("probe-link")];
        for (extra, migration, decls, label) in [
            ("  entity foo;\n  belief sub foo;\n", None, &foo, "belief"),
            (
                "  entity foo;\n  concept sub foo;\n",
                None,
                &concept,
                "concept",
            ),
            (
                "  entity foo;\n  memory-item@abstract, sub foo;\n",
                None,
                &foo,
                "memory-item",
            ),
            (
                "  entity foo;\n  relation supersession sub foo;\n",
                None,
                &concept,
                "supersession",
            ),
        ] {
            let schema = format!("{PROBE_SCHEMA}{extra}");
            let err = build_with_migration("probe-ext", &schema, migration, decls).unwrap_err();
            match &err {
                RegistryError::ReparentedType {
                    extension,
                    label: got,
                } => {
                    assert_eq!(extension, "probe-ext");
                    assert_eq!(got, label);
                }
                other => panic!("{extra} / {migration:?}: {other:?}"),
            }
            assert!(err.to_string().contains(label), "{err}");
        }

        // An extension that declares nothing is refused as well.
        let probe = [decl("probe-link")];
        let err = build(&[
            frag("probe-ext", PROBE_SCHEMA, &probe),
            frag("y-ext", "define entity foo; concept sub foo;", &[]),
        ])
        .unwrap_err();
        assert!(
            matches!(&err, RegistryError::ReparentedType { extension, .. } if extension == "y-ext"),
            "{err:?}"
        );
    }

    #[test]
    fn refuses_an_aliased_relation() {
        // The declared label is the alias of a mem relation.
        let decls = [decl("foo")];
        let err = build(&[frag(
            "x-ext",
            "define supersession alias foo;\n relation foo, relates claim, relates subject;",
            &decls,
        )])
        .unwrap_err();
        assert_alias_unsupported(&err, "x-ext");

        // The owner aliases its relation, and another extension extends the alias.
        let err = build_x_and_y("x-link alias xl;\n", "define xl relates extra;").unwrap_err();
        assert_alias_unsupported(&err, "x-ext");

        // An alias on the owner's own subtype hides a mention the same way.
        let err = build_x_and_y(
            "relation x-sub sub x-link;\n x-sub alias xs;\n",
            "define xs relates extra;",
        )
        .unwrap_err();
        assert_alias_unsupported(&err, "x-ext");
    }

    #[test]
    fn migration_text_counts_as_the_extensions_fragment() {
        let migrations = [SchemaMigration {
            id: "probe/add-link",
            applies_when: "never",
            tql: "define relation late-link, relates claim, relates subject;",
        }];
        let decls = [decl("late-link")];
        let fragment = ExtensionFragment {
            name: "probe-ext",
            schema: "define attribute probe-note, value string;",
            migrations: &migrations,
            decls: &decls,
        };
        let reg = build(&[fragment]).expect("builds");
        assert_eq!(reg.owner("late-link"), Some("probe-ext"));
    }

    struct ProbeExt;

    const PROBE_DECLS: &[BeliefEdgeDecl] = &[BeliefEdgeDecl {
        relation: "probe-link",
        belief_role: "claim",
        roles: CONCEPT_ROLE,
        owned_attrs: NOTE_ATTR,
    }];

    impl GeckoExtension for ProbeExt {
        fn name(&self) -> &str {
            "probe-ext"
        }
        fn schema(&self) -> &str {
            PROBE_SCHEMA
        }
        fn belief_edges(&self) -> &[BeliefEdgeDecl] {
            PROBE_DECLS
        }
        fn host_imports(&self) -> Vec<HostImportDef> {
            vec![]
        }
    }

    #[test]
    fn from_extensions_excludes_mem_and_requires_it() {
        let exts: Vec<Box<dyn GeckoExtension>> = vec![crate::extension(), Box::new(ProbeExt)];
        let reg = BeliefEdgeRegistry::from_extensions(CORE, &exts).expect("builds");
        assert_eq!(reg.owner("probe-link"), Some("probe-ext"));
        assert!(!reg.declares_any("mem-gecko"));

        let without_mem: Vec<Box<dyn GeckoExtension>> = vec![Box::new(ProbeExt)];
        let err = BeliefEdgeRegistry::from_extensions(CORE, &without_mem).unwrap_err();
        assert!(matches!(err, RegistryError::MemMissing { .. }), "{err:?}");
    }

    #[test]
    fn errors_name_the_extension() {
        let decls = [decl("supersession")];
        let err = build(&[frag("probe-ext", PROBE_SCHEMA, &decls)]).unwrap_err();
        let shown = err.to_string();
        assert!(shown.contains("probe-ext"), "{shown}");
        assert!(shown.contains("supersession"), "{shown}");
    }
}
