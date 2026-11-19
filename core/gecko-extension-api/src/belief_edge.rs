//! Declarations and the write trait for extension-owned relations that attach to a belief.

use async_trait::async_trait;

use crate::epistemic::{BeliefDraft, DerivationMethod};
use crate::error::EpistemicError;
use crate::graph::GraphValue;
use crate::ids::MemId;
use crate::provenance::RunContext;

/// The TypeDB value type of an attribute an edge relation owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrValueType {
    /// A TypeDB `string`.
    String,
    /// A TypeDB `datetime`.
    Datetime,
    /// A TypeDB `double`.
    Double,
    /// A TypeDB `integer`.
    Long,
    /// A TypeDB `boolean`.
    Boolean,
}

impl AttrValueType {
    /// The TypeQL value-type keyword for this type.
    pub fn tql(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Datetime => "datetime",
            Self::Double => "double",
            Self::Long => "integer",
            Self::Boolean => "boolean",
        }
    }

    /// Whether `v` is the [`GraphValue`] variant that carries this type.
    pub fn admits(&self, v: &GraphValue) -> bool {
        matches!(
            (self, v),
            (Self::String, GraphValue::String(_))
                | (Self::Datetime, GraphValue::Datetime(_))
                | (Self::Double, GraphValue::Double(_))
                | (Self::Long, GraphValue::Long(_))
                | (Self::Boolean, GraphValue::Boolean(_))
        )
    }
}

/// A non-belief role of an edge relation and the entity type that plays it.
#[derive(Debug, Clone, PartialEq)]
pub struct RoleDecl {
    /// The role label on the relation.
    pub role: &'static str,
    /// The entity type that plays the role.
    pub player: &'static str,
    /// The key attribute that identifies a player of that type.
    pub key: &'static str,
}

/// An attribute an edge relation owns.
#[derive(Debug, Clone, PartialEq)]
pub struct AttrDecl {
    /// The attribute label.
    pub attr: &'static str,
    /// The attribute's value type.
    pub value: AttrValueType,
    /// Whether every edge of the relation must supply it.
    pub required: bool,
}

/// An extension's declaration of a relation that links a belief to other entities.
#[derive(Debug, Clone, PartialEq)]
pub struct BeliefEdgeDecl {
    /// The relation label.
    pub relation: &'static str,
    /// The role the belief plays in the relation.
    pub belief_role: &'static str,
    /// The relation's other roles.
    pub roles: &'static [RoleDecl],
    /// The attributes the relation owns.
    pub owned_attrs: &'static [AttrDecl],
}

/// One edge to write alongside a belief, keyed by role and attribute labels.
#[derive(Debug, Clone, PartialEq)]
pub struct BeliefEdge {
    /// The declared relation label.
    pub relation: String,
    /// Role label and the key value of the entity playing it.
    pub players: Vec<(String, GraphValue)>,
    /// Attribute label and value for the relation's owned attributes.
    pub attrs: Vec<(String, GraphValue)>,
}

/// Writes a belief and its declared edges in one atomic transaction.
#[async_trait]
pub trait BeliefEdgeWriter: Send + Sync {
    /// Asserts a belief backed by `evidence` and writes `edges` with it, all or nothing.
    async fn assert_belief_with_edges(
        &self,
        ctx: &RunContext,
        draft: BeliefDraft,
        evidence: &[MemId],
        method: DerivationMethod,
        edges: &[BeliefEdge],
    ) -> Result<MemId, EpistemicError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attr_value_type_maps_to_tql_and_admits_its_variant() {
        let vals = [
            GraphValue::String("s".into()),
            GraphValue::Datetime(chrono::Utc::now()),
            GraphValue::Double(1.5),
            GraphValue::Long(3),
            GraphValue::Boolean(true),
        ];
        let types = [
            (AttrValueType::String, "string"),
            (AttrValueType::Datetime, "datetime"),
            (AttrValueType::Double, "double"),
            (AttrValueType::Long, "integer"),
            (AttrValueType::Boolean, "boolean"),
        ];
        for (i, (t, tql)) in types.iter().enumerate() {
            assert_eq!(t.tql(), *tql);
            for (j, v) in vals.iter().enumerate() {
                assert_eq!(t.admits(v), i == j, "{t:?} vs {v:?}");
            }
        }
    }
}
