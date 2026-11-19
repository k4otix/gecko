//! Builds the set of belief-edge relations each extension may write, refusing any it does
//! not own (design §3).

use std::collections::{HashMap, HashSet};

use gecko_extension_api::{BeliefEdgeDecl, GeckoExtension, SchemaMigration};

use super::scan::{Kind, TypeHead, type_heads};

/// The name mem's extension registers under; its schema is scanned as mem's fragment.
const MEM_EXTENSION: &str = "mem-gecko";

/// The abstract root of every memory node; no non-belief role may be played by it, a
/// subtype of it, or a supertype of it.
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

    fn heads(&self) -> Vec<TypeHead> {
        let mut heads = type_heads(self.schema);
        for m in self.migrations {
            heads.extend(type_heads(m.tql));
        }
        heads
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
        "extension `{extension}` declares belief edge `{relation}` with role `{role}` played by `{player}`, which is or contains a memory item"
    )]
    MemoryItemPlayer {
        extension: String,
        relation: String,
        role: String,
        player: String,
    },
    #[error("extension `{extension}` is not registered; the belief-edge registry needs its schema")]
    MemMissing { extension: String },
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
        let core_heads = type_heads(core_schema);
        let mem_heads = type_heads(mem_schema);
        let ext_heads: Vec<Vec<TypeHead>> = extensions.iter().map(|e| e.heads()).collect();

        let mut supertypes: HashMap<&str, &str> = HashMap::new();
        for head in core_heads
            .iter()
            .chain(&mem_heads)
            .chain(ext_heads.iter().flatten())
        {
            if let (Kind::Entity, Some(sub)) = (head.kind, &head.sub) {
                supertypes.insert(&head.label, sub);
            }
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
                let others = std::iter::once(("the core schema", &core_heads))
                    .chain(std::iter::once(("mem", &mem_heads)))
                    .chain(
                        extensions
                            .iter()
                            .zip(&ext_heads)
                            .enumerate()
                            .filter(|(j, _)| *j != i)
                            .map(|(_, (e, h))| (e.name, h)),
                    );
                check_ownership(ext.name, decl, &ext_heads[i], others)?;
                check_players(ext.name, decl, &supertypes)?;
                by_relation.insert(
                    decl.relation.to_owned(),
                    (ext.name.to_owned(), decl.clone()),
                );
            }
        }
        Ok(Self { by_relation })
    }

    /// Builds the registry from the registered extensions, taking mem's schema from the
    /// `mem-gecko` extension and every other extension as a declaring fragment.
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
        let fragments: Vec<ExtensionFragment> = extensions
            .iter()
            .filter(|e| e.name() != MEM_EXTENSION)
            .map(|e| ExtensionFragment::of(e.as_ref()))
            .collect();
        Self::build(core_schema, mem.schema(), &fragments)
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

fn check_ownership<'h>(
    extension: &str,
    decl: &BeliefEdgeDecl,
    own: &[TypeHead],
    others: impl Iterator<Item = (&'h str, &'h Vec<TypeHead>)>,
) -> Result<(), RegistryError> {
    let not_owned = |reason: String| RegistryError::NotOwned {
        extension: extension.to_owned(),
        relation: decl.relation.to_owned(),
        reason,
    };
    let own_heads: Vec<&TypeHead> = own
        .iter()
        .filter(|h| h.kind == Kind::Relation && h.label == decl.relation)
        .collect();
    if own_heads.is_empty() {
        return Err(not_owned(
            "its schema fragment has no `relation` statement for it".to_owned(),
        ));
    }
    for (name, heads) in others {
        if heads
            .iter()
            .any(|h| matches!(h.kind, Kind::Relation | Kind::Touch) && h.label == decl.relation)
        {
            return Err(not_owned(format!("{name} also defines or extends it")));
        }
    }
    for head in own_heads {
        if let Some(supertype) = &head.sub {
            return Err(RegistryError::SubtypedRelation {
                extension: extension.to_owned(),
                relation: decl.relation.to_owned(),
                supertype: supertype.clone(),
            });
        }
        if head.relates_as {
            return Err(RegistryError::RoleAlias {
                extension: extension.to_owned(),
                relation: decl.relation.to_owned(),
            });
        }
    }
    Ok(())
}

/// Whether walking `sub` links up from `from` reaches `to`.
fn chain_reaches(supertypes: &HashMap<&str, &str>, from: &str, to: &str) -> bool {
    let mut seen = HashSet::new();
    let mut current = from;
    while let Some(&parent) = supertypes.get(current) {
        if parent == to {
            return true;
        }
        if !seen.insert(parent) {
            return false;
        }
        current = parent;
    }
    false
}

fn check_players(
    extension: &str,
    decl: &BeliefEdgeDecl,
    supertypes: &HashMap<&str, &str>,
) -> Result<(), RegistryError> {
    for role in decl.roles {
        let p = role.player;
        if p == MEMORY_ITEM
            || chain_reaches(supertypes, p, MEMORY_ITEM)
            || chain_reaches(supertypes, MEMORY_ITEM, p)
        {
            return Err(RegistryError::MemoryItemPlayer {
                extension: extension.to_owned(),
                relation: decl.relation.to_owned(),
                role: role.role.to_owned(),
                player: p.to_owned(),
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
        assert_not_owned(&err, "mem also defines");
    }

    #[test]
    fn refuses_a_core_relation() {
        let decls = [decl("hierarchy")];
        let schema = "define relation hierarchy, relates claim, relates subject;";
        let err = build(&[frag("probe-ext", schema, &decls)]).unwrap_err();
        assert_not_owned(&err, "the core schema also defines");
        let err = build(&[frag("probe-ext", PROBE_SCHEMA, &decls)]).unwrap_err();
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
        assert_not_owned(&err, "x-ext also defines");
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
        let err = build(&[frag(
            "probe-ext",
            "define relation my-edge sub supersession, relates claim, relates subject;",
            &decls,
        )])
        .unwrap_err();
        assert!(
            matches!(err, RegistryError::SubtypedRelation { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn refuses_a_relates_as_role() {
        let decls = [decl("my-edge")];
        let err = build(&[frag(
            "probe-ext",
            "define relation my-edge, relates claim, relates subject as superseded;",
            &decls,
        )])
        .unwrap_err();
        assert!(matches!(err, RegistryError::RoleAlias { .. }), "{err:?}");
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
        let schema = format!("{PROBE_SCHEMA}  entity sighting sub execution-episode;\n");
        for player in [
            "belief",
            "episode",
            "okf-concept",
            "memory-item",
            "sighting",
        ] {
            let decls = [decl_with_player("probe-link", player)];
            let err = build(&[frag("probe-ext", &schema, &decls)]).unwrap_err();
            assert!(
                matches!(err, RegistryError::MemoryItemPlayer { .. }),
                "{player}: {err:?}"
            );
        }
        let decls = [decl_with_player("probe-link", "concept")];
        build(&[frag("probe-ext", &schema, &decls)]).expect("concept is not a memory item");
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
