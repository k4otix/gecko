//! mem-gecko's TypeQL — the schema-specific query text and typed params for every
//! belief/episode write and every persisted-function read.
//!
//! mem **owns** this TQL (invariant: the engine owns the driver, mem owns its
//! queries). Every value crosses the [`GraphStore`](gecko_extension_api::GraphStore)
//! seam as an out-of-band typed [`GraphValue`] — never string-interpolated — so the
//! write path is injection-safe exactly like the syncer's `given`-stage writes. The
//! only text a builder assembles dynamically is *structure* (how many evidence
//! sources, how many claims), never a value.

use gecko_extension_api::{
    BeliefDraft, ConceptId, DateTime, DerivationMethod, EpisodeDraft, GraphValue, GraphWrite,
    MemId, ProvenanceSource, RunContext,
};

/// Open-set `origin-kind` discriminator for a run's provenance source (invariant 9:
/// `origin-kind` is an adapter-extensible open set, never a closed enum).
pub(crate) fn origin_kind(source: &ProvenanceSource) -> &'static str {
    match source {
        ProvenanceSource::ExecutableDoc { .. } => "executable-doc",
        ProvenanceSource::Sync { .. } => "sync",
        ProvenanceSource::Tool { .. } => "tool",
        ProvenanceSource::Manual => "manual",
    }
}

/// Entrenchment tier implied by a derivation method (invariant 7 flavour: how
/// defeasible the resulting belief is).
pub(crate) fn entrenchment_for(method: DerivationMethod) -> &'static str {
    match method {
        DerivationMethod::HumanAssertion => "user-stated",
        DerivationMethod::LlmSynthesis => "llm",
        DerivationMethod::TypeJoin
        | DerivationMethod::Cardinality
        | DerivationMethod::FunctionalDependency
        | DerivationMethod::TypeDbFunction
        | DerivationMethod::ExternalTool => "tool-derived",
    }
}

/// A tiny typed-parameter accumulator: keeps the `given` declaration, the variable
/// names, and the value row in lock-step so a builder cannot desync them.
pub(crate) struct Params {
    names: Vec<String>,
    decls: Vec<String>,
    row: Vec<GraphValue>,
}

impl Params {
    pub(crate) fn new() -> Self {
        Self {
            names: Vec::new(),
            decls: Vec::new(),
            row: Vec::new(),
        }
    }

    fn add(&mut self, name: &str, ty: &str, val: GraphValue) {
        self.names.push(name.to_string());
        self.decls.push(format!("${name}: {ty}"));
        self.row.push(val);
    }

    pub(crate) fn s(&mut self, name: &str, v: impl Into<String>) {
        self.add(name, "string", GraphValue::String(v.into()));
    }

    pub(crate) fn dt(&mut self, name: &str, v: DateTime) {
        self.add(name, "datetime", GraphValue::Datetime(v));
    }

    pub(crate) fn f(&mut self, name: &str, v: f64) {
        self.add(name, "double", GraphValue::Double(v));
    }

    fn given(&self) -> String {
        if self.decls.is_empty() {
            String::new()
        } else {
            format!("given {};\n", self.decls.join(", "))
        }
    }

    /// Finish as a write op.
    pub(crate) fn write(self, body: &str) -> GraphWrite {
        GraphWrite::single(format!("{}{}", self.given(), body), self.names, self.row)
    }

    /// Finish as `(query, vars, row)` for a read.
    pub(crate) fn read(self, body: &str) -> (String, Vec<String>, Vec<GraphValue>) {
        (format!("{}{}", self.given(), body), self.names, self.row)
    }
}

// ── Provenance-stamp primitives (invariant 2) ────────────────────────────────

/// Ensures the run's `doc-run` node exists (keyed on run-id).
fn ensure_run(ctx: &RunContext) -> GraphWrite {
    let mut p = Params::new();
    p.s("rid", ctx.run_id.to_string());
    p.write("put $run isa doc-run, has run-id == $rid;")
}

/// Ensures the actor's `agent` node exists (keyed on agent-id).
fn ensure_agent(ctx: &RunContext) -> GraphWrite {
    let mut p = Params::new();
    p.s("aid", ctx.actor.0.clone());
    p.write("put $ag isa agent, has agent-id == $aid;")
}

// ── Write builders ───────────────────────────────────────────────────────────

/// `observe`: an `episode` with BOTH event-time and ingest-time (invariant 6), a
/// `source-link` → `doc-run` run stamp (invariant 2), and an `ownership` actor
/// stamp.
pub(crate) fn observe_ops(ctx: &RunContext, ep: &EpisodeDraft, id: &MemId) -> Vec<GraphWrite> {
    let mut p = Params::new();
    p.s("eid", id.0.clone());
    p.s("ttl", ep.text.clone());
    p.dt("et", ep.event_time);
    p.dt("it", ep.ingest_time);
    p.s("rid", ctx.run_id.to_string());
    p.s("aid", ctx.actor.0.clone());
    p.s("kind", origin_kind(&ctx.source));
    let body = "match
           $run isa doc-run, has run-id $rr; $rr == $rid;
           $ag isa agent, has agent-id $aa; $aa == $aid;
         insert
           $ep isa episode, has concept-id == $eid, has title == $ttl,
               has event-time == $et, has ingest-time == $it;
           (memory: $ep, origin: $run) isa source-link, has origin-kind == $kind;
           (owned: $ep, owner: $ag) isa ownership;";
    vec![ensure_run(ctx), ensure_agent(ctx), p.write(body)]
}

/// The belief-insert ops shared by `assert_belief` and `supersede`'s new belief.
/// Sets belief-state "asserted", valid-from, the ACT-R decay attrs (salience,
/// base-activation, last-access) so recall's `retrieval-score` has inputs, plus the
/// run + actor stamps.
pub(crate) fn insert_belief_ops(
    ctx: &RunContext,
    b: &BeliefDraft,
    id: &MemId,
    entrenchment: &str,
) -> Vec<GraphWrite> {
    let mut p = Params::new();
    p.s("bid", id.0.clone());
    p.s("ttl", b.text.clone());
    p.dt("vf", ctx.occurred_at);
    p.dt("la", ctx.occurred_at);
    p.s("rid", ctx.run_id.to_string());
    p.s("aid", ctx.actor.0.clone());
    p.s("kind", origin_kind(&ctx.source));
    p.s("vis", b.visibility.as_str());
    p.s("ent", entrenchment);
    p.f("sal", b.confidence.unwrap_or(0.5));
    p.f("ba", 1.0);
    let conf_frag = if let Some(c) = b.confidence {
        p.f("cf", c);
        ", has confidence == $cf"
    } else {
        ""
    };
    let body = format!(
        "match
           $run isa doc-run, has run-id $rr; $rr == $rid;
           $ag isa agent, has agent-id $aa; $aa == $aid;
         insert
           $b isa belief, has concept-id == $bid, has title == $ttl, has belief-state \"asserted\",
               has valid-from == $vf, has last-access == $la, has salience == $sal,
               has base-activation == $ba, has entrenchment == $ent{conf_frag};
           (memory: $b, origin: $run) isa source-link, has origin-kind == $kind;
           (owned: $b, owner: $ag) isa ownership, has visibility == $vis;"
    );
    vec![ensure_run(ctx), ensure_agent(ctx), p.write(&body)]
}

/// The `derivation` for a belief: its evidence MemIds ARE its `source`s (so
/// `derivation-chain` / `blast-radius` traverse them). Returns `None` for an
/// evidence-less belief — `derivation relates source @card(1..)` forbids a
/// sourceless derivation, and a belief with no evidence is a root, not a derived
/// claim.
pub(crate) fn derivation_op(
    id: &MemId,
    evidence: &[MemId],
    method: DerivationMethod,
    confidence: Option<f64>,
) -> Option<GraphWrite> {
    if evidence.is_empty() {
        return None;
    }
    let mut p = Params::new();
    p.s("bid", id.0.clone());
    p.s("dm", method.as_str());
    let mut match_lines = String::from("$b isa belief, has concept-id $bc; $bc == $bid;\n");
    let mut source_roles = String::new();
    for (i, e) in evidence.iter().enumerate() {
        p.s(&format!("e{i}"), e.0.clone());
        match_lines.push_str(&format!(
            "           $m{i} isa memory-item, has concept-id $mc{i}; $mc{i} == $e{i};\n"
        ));
        source_roles.push_str(&format!(", source: $m{i}"));
    }
    let conf_frag = if let Some(c) = confidence {
        p.f("cf", c);
        ", has confidence == $cf"
    } else {
        ""
    };
    let body = format!(
        "match\n           {match_lines}         insert (derived: $b{source_roles}) isa derivation, has derivation-method == $dm{conf_frag};"
    );
    Some(p.write(&body))
}

/// `supersede`: link old → new and flip old's belief-state to "superseded".
pub(crate) fn supersession_ops(
    old: &MemId,
    new_id: &MemId,
    reason: &str,
    created_at: DateTime,
) -> Vec<GraphWrite> {
    let mut link = Params::new();
    link.s("oid", old.0.clone());
    link.s("nid", new_id.0.clone());
    link.s("reason", reason);
    link.dt("ca", created_at);
    let link_op = link.write(
        "match
           $o isa belief, has concept-id $oc; $oc == $oid;
           $n isa belief, has concept-id $nc; $nc == $nid;
         insert (superseded: $o, superseding: $n) isa supersession,
             has supersession-reason == $reason, has created-at == $ca;",
    );

    let mut flip = Params::new();
    flip.s("oid", old.0.clone());
    let flip_op = flip.write(
        "match $o isa belief, has concept-id $oc; $oc == $oid;
         update $o has belief-state \"superseded\";",
    );

    vec![link_op, flip_op]
}

/// `contest`: a `contradiction` → `anomaly` hub over `claims` (2..), run + actor
/// stamped.
pub(crate) fn contest_ops(ctx: &RunContext, anomaly: &MemId, claims: &[MemId]) -> Vec<GraphWrite> {
    let mut p = Params::new();
    p.s("anid", anomaly.0.clone());
    p.s("rid", ctx.run_id.to_string());
    p.s("aid", ctx.actor.0.clone());
    p.s("kind", origin_kind(&ctx.source));
    let mut match_lines = String::from(
        "$run isa doc-run, has run-id $rr; $rr == $rid;\n           $ag isa agent, has agent-id $aa; $aa == $aid;\n",
    );
    let mut claim_roles = String::new();
    for (i, c) in claims.iter().enumerate() {
        p.s(&format!("c{i}"), c.0.clone());
        match_lines.push_str(&format!(
            "           $cl{i} isa belief, has concept-id $cc{i}; $cc{i} == $c{i};\n"
        ));
        claim_roles.push_str(&format!(", claim: $cl{i}"));
    }
    // Strip the leading ", " so the role list starts clean.
    let claim_roles = claim_roles.trim_start_matches(", ").to_string();
    let body = format!(
        "match\n           {match_lines}         insert
           $an isa anomaly, has concept-id == $anid;
           (hub: $an, {claim_roles}) isa contradiction;
           (memory: $an, origin: $run) isa source-link, has origin-kind == $kind;
           (owned: $an, owner: $ag) isa ownership;"
    );
    vec![ensure_run(ctx), ensure_agent(ctx), p.write(&body)]
}

/// `rests_on`: a `rests-on` relation (resting depends on assumptions 1..).
pub(crate) fn rests_on_op(resting: &MemId, assumptions: &[MemId]) -> GraphWrite {
    let mut p = Params::new();
    p.s("rid", resting.0.clone());
    let mut match_lines = String::from("$r isa memory-item, has concept-id $rc; $rc == $rid;\n");
    let mut roles = String::new();
    for (i, a) in assumptions.iter().enumerate() {
        p.s(&format!("a{i}"), a.0.clone());
        match_lines.push_str(&format!(
            "           $as{i} isa memory-item, has concept-id $ac{i}; $ac{i} == $a{i};\n"
        ));
        roles.push_str(&format!(", assumption: $as{i}"));
    }
    let body = format!(
        "match\n           {match_lines}         insert (resting: $r{roles}) isa rests-on;"
    );
    p.write(&body)
}

/// `record_prediction`: a `prediction-resolution` seeded with the predicted
/// probability (resolving-episode filled later by `resolve_prediction`).
pub(crate) fn record_prediction_op(b: &MemId, p_prob: f64) -> GraphWrite {
    let mut p = Params::new();
    p.s("bid", b.0.clone());
    p.f("pp", p_prob);
    p.write(
        "match $b isa belief, has concept-id $bc; $bc == $bid;
         insert (predicted-belief: $b) isa prediction-resolution, has predicted-probability == $pp;",
    )
}

// ── A5.4 rebuild-from-graph reads ─────────────────────────────────────────────

/// Enumerates current-state **beliefs** (asserted, not superseded/retracted) with
/// their embeddable title text and denormalized FilterMeta (owner/visibility). The
/// index is fully rebuildable from these (invariant 8). Deterministic ordering is
/// imposed host-side (sorted by concept-id) so any reconstruction yields an
/// identical ANN graph.
pub(crate) fn enumerate_beliefs_read() -> &'static str {
    "match
       $b isa belief, has concept-id $bid, has title $ttl, has belief-state $st;
       $st == \"asserted\";
       $b has valid-from $vf;
       $own isa ownership, links (owned: $b, owner: $ag);
       $own has visibility $vis;
       $ag has agent-id $aid;
     fetch { \"id\": $bid, \"text\": $ttl, \"owner\": $aid, \"vis\": $vis, \"vf\": $vf };"
}

/// Enumerates current-state **episodes** (excluding retrieval-events, which are
/// provenance records, not embeddable content) with their title text and owner.
pub(crate) fn enumerate_episodes_read() -> &'static str {
    "match
       $e isa episode, has concept-id $eid, has title $ttl, has event-time $et;
       not { $e isa retrieval-event; };
       $own isa ownership, links (owned: $e, owner: $ag);
       $ag has agent-id $aid;
     fetch { \"id\": $eid, \"text\": $ttl, \"owner\": $aid, \"vf\": $et };"
}

/// Single current-state-belief lookup by concept-id (dirty-set repair, A5.4).
pub(crate) fn fetch_belief_embeddable(cid: &ConceptId) -> (String, Vec<String>, Vec<GraphValue>) {
    let mut p = Params::new();
    p.s("cid", cid.0.clone());
    p.read(
        "match
           $b isa belief, has concept-id $bc; $bc == $cid;
           $b has title $ttl, has belief-state $st; $st == \"asserted\";
           $b has valid-from $vf;
           $own isa ownership, links (owned: $b, owner: $ag);
           $own has visibility $vis;
           $ag has agent-id $aid;
         fetch { \"id\": $bc, \"text\": $ttl, \"owner\": $aid, \"vis\": $vis, \"vf\": $vf };",
    )
}

/// `resolve_prediction`: mint a resolving `episode` (run + actor stamped) and attach
/// it (plus the outcome) to the belief's `prediction-resolution`.
pub(crate) fn resolve_prediction_ops(
    ctx: &RunContext,
    b: &MemId,
    ep_id: &MemId,
    outcome: &str,
) -> Vec<GraphWrite> {
    // 1. resolving episode.
    let mut ep = Params::new();
    ep.s("eid", ep_id.0.clone());
    ep.s("ttl", format!("prediction resolved: {outcome}"));
    ep.dt("now", ctx.occurred_at);
    ep.s("rid", ctx.run_id.to_string());
    ep.s("aid", ctx.actor.0.clone());
    ep.s("kind", origin_kind(&ctx.source));
    let ep_op = ep.write(
        "match
           $run isa doc-run, has run-id $rr; $rr == $rid;
           $ag isa agent, has agent-id $aa; $aa == $aid;
         insert
           $ep isa episode, has concept-id == $eid, has title == $ttl,
               has event-time == $now, has ingest-time == $now;
           (memory: $ep, origin: $run) isa source-link, has origin-kind == $kind;
           (owned: $ep, owner: $ag) isa ownership;",
    );

    // 2. link the episode + outcome onto the existing prediction-resolution.
    let mut link = Params::new();
    link.s("bid", b.0.clone());
    link.s("eid", ep_id.0.clone());
    link.s("oc", outcome);
    let link_op = link.write(
        "match
           $b isa belief, has concept-id $bc; $bc == $bid;
           $pr isa prediction-resolution, links (predicted-belief: $b);
           $ep isa episode, has concept-id $ec; $ec == $eid;
         insert $pr links (resolving-episode: $ep);
                $pr has outcome == $oc;",
    );

    vec![ensure_run(ctx), ensure_agent(ctx), ep_op, link_op]
}
