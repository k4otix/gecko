//! The per-extension [`BeliefEdgeWriter`]: validates a request against the bound
//! extension's declarations, confirms each relation against the live schema once, and
//! writes the belief with its edges in one transaction (design §1a, §4).

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use gecko_extension_api::{
    BeliefDraft, BeliefEdge, BeliefEdgeDecl, BeliefEdgeWriter, BeliefState, DerivationMethod,
    EpistemicError, GraphValue, MemId, RowCountMismatch, RunContext,
};

use super::beliefs::{
    BeliefOps, name_missing_evidence, name_missing_provenance, validate_confidence,
};
use super::{MemWriter, Result, mint};
use crate::belief_edge::BeliefEdgeRegistry;
use crate::tql;

/// A [`BeliefEdgeWriter`] bound to one extension, named by the host.
pub(super) struct BoundEdgeWriter {
    writer: Arc<MemWriter>,
    extension: String,
}

impl BoundEdgeWriter {
    pub(super) fn new(writer: Arc<MemWriter>, extension: &str) -> Self {
        Self {
            writer,
            extension: extension.to_owned(),
        }
    }

    /// Confirms `decl` against the database schema, unless an earlier call already
    /// did. Only a pass is remembered.
    async fn live_check(&self, decl: &BeliefEdgeDecl) -> Result<()> {
        let rel = decl.relation;
        let checked = |w: &MemWriter| {
            w.checked_relations
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(rel)
        };
        // Concurrent first writes may both run the check; the only cost is duplicate reads.
        if checked(&self.writer) {
            return Ok(());
        }
        for check in tql::belief_edge_schema_reads(decl) {
            let (q, v, r) = &check.read;
            let failure = match self.writer.graph.read(q, v, r).await {
                Ok(docs) if !docs.is_empty() => continue,
                Ok(_) => "no matching schema".to_owned(),
                Err(e) => e.to_string(),
            };
            return Err(EpistemicError::InvalidInput(format!(
                "belief edge `{rel}` does not match the database schema: {} ({failure})",
                check.condition
            )));
        }
        self.writer
            .checked_relations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(rel.to_owned());
        Ok(())
    }
}

#[async_trait]
impl BeliefEdgeWriter for BoundEdgeWriter {
    async fn assert_belief_with_edges(
        &self,
        ctx: &RunContext,
        draft: BeliefDraft,
        evidence: &[MemId],
        method: DerivationMethod,
        edges: &[BeliefEdge],
    ) -> Result<MemId> {
        let decls = edges
            .iter()
            .map(|e| validate_edge(&self.writer.belief_edges, &self.extension, e))
            .collect::<Result<Vec<_>>>()?;
        validate_confidence(draft.confidence, Some(method))?;

        let mut seen = HashSet::new();
        for decl in &decls {
            if seen.insert(decl.relation) {
                self.live_check(decl).await?;
            }
        }

        let id = mint("mem/bel");
        let BeliefOps {
            mut ops,
            derivation,
            provenance,
        } = self.writer.belief_ops(ctx, &draft, &id, evidence, method);
        let first_edge = ops.len();
        ops.extend(
            decls
                .iter()
                .zip(edges)
                .map(|(decl, edge)| tql::belief_edge_op(decl, &id, edge)),
        );
        // AUTHORITATIVE: the belief and every edge commit together or not at all.
        self.writer.graph.write(&ops).await.map_err(|e| {
            let e = name_missing_evidence(e, derivation, &id, evidence);
            let e = name_missing_provenance(e, &provenance, &id);
            name_failing_edge(e, first_edge, &decls, edges)
        })?;
        // Best-effort embed + upsert; failure marks dirty, never fails the write.
        let meta = self.writer.belief_meta(&draft, ctx, BeliefState::Asserted);
        self.writer.best_effort_index(&id, &draft.text, meta);
        Ok(id)
    }
}

/// Rewrites a row-count mismatch on an edge op (index `first_edge` onward) into an
/// error naming that edge's relation and each role's key value; any other error is
/// returned unchanged.
fn name_failing_edge(
    err: EpistemicError,
    first_edge: usize,
    decls: &[&BeliefEdgeDecl],
    edges: &[BeliefEdge],
) -> EpistemicError {
    let Some(m) = RowCountMismatch::from_error(&err) else {
        return err;
    };
    let Some(i) = m.op.checked_sub(first_edge) else {
        return err;
    };
    let (Some(decl), Some(edge)) = (decls.get(i), edges.get(i)) else {
        return err;
    };
    let players = decl
        .roles
        .iter()
        .map(|role| {
            let value = edge
                .players
                .iter()
                .find(|(r, _)| r == role.role)
                .map_or_else(|| "<missing>".to_owned(), |(_, v)| shown(v));
            format!("{} {}={value}", role.role, role.key)
        })
        .collect::<Vec<_>>()
        .join(", ");
    let rows = if m.got == 1 { "row" } else { "rows" };
    EpistemicError::Storage(format!(
        "belief edge {} ({players}) matched {} {rows}; nothing was committed",
        decl.relation, m.got
    ))
}

/// A key value as an error message shows it.
fn shown(v: &GraphValue) -> String {
    match v {
        GraphValue::String(s) => s.clone(),
        GraphValue::Datetime(dt) => dt.to_rfc3339(),
        GraphValue::Double(d) => d.to_string(),
        GraphValue::Long(i) => i.to_string(),
        GraphValue::Boolean(b) => b.to_string(),
    }
}

/// The declaration `edge` is written under, once `edge` is shown to be a relation
/// `extension` owns with each declared role supplied once and only declared, well-typed
/// attributes.
fn validate_edge<'r>(
    registry: &'r BeliefEdgeRegistry,
    extension: &str,
    edge: &BeliefEdge,
) -> Result<&'r BeliefEdgeDecl> {
    let rel = edge.relation.as_str();
    let invalid = |why: String| EpistemicError::InvalidInput(format!("belief edge `{rel}`: {why}"));
    let Some(decl) = registry.decl(extension, rel) else {
        return Err(invalid(match registry.owner(rel) {
            Some(owner) => {
                format!("the relation is registered to extension `{owner}`, not `{extension}`")
            }
            None => "no extension declares this relation".to_owned(),
        }));
    };

    let mut roles = HashSet::new();
    for (role, _) in &edge.players {
        if !decl.roles.iter().any(|r| r.role == role) {
            return Err(invalid(format!("unknown role `{role}`")));
        }
        if !roles.insert(role.as_str()) {
            return Err(invalid(format!("role `{role}` is supplied more than once")));
        }
    }
    if let Some(missing) = decl.roles.iter().find(|r| !roles.contains(r.role)) {
        return Err(invalid(format!("role `{}` is not supplied", missing.role)));
    }

    let mut attrs = HashSet::new();
    for (attr, value) in &edge.attrs {
        let Some(declared) = decl.owned_attrs.iter().find(|a| a.attr == attr) else {
            return Err(invalid(format!("unknown attribute `{attr}`")));
        };
        if !attrs.insert(attr.as_str()) {
            return Err(invalid(format!(
                "attribute `{attr}` is supplied more than once"
            )));
        }
        if !declared.value.admits(value) {
            return Err(invalid(format!(
                "attribute `{attr}` takes a {} value",
                declared.value.tql()
            )));
        }
    }
    if let Some(missing) = decl
        .owned_attrs
        .iter()
        .find(|a| a.required && !attrs.contains(a.attr))
    {
        return Err(invalid(format!(
            "required attribute `{}` is not supplied",
            missing.attr
        )));
    }
    Ok(decl)
}
