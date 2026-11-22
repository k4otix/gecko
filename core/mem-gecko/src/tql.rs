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
    ActorId, BeliefDraft, BeliefEdge, BeliefEdgeDecl, ConceptId, DateTime, DerivationMethod,
    Entrenchment, EpisodeDraft, GraphValue, GraphWrite, MemId, ProvenanceSource, RunContext,
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

/// Entrenchment strength for an entrenchment string read from the graph: **higher
/// is more entrenched** (`axiom` = 4 … `llm` = 0), the @values order. An unknown or
/// absent string ranks as the least entrenched (0) so it can never *block* a
/// supersede — the invariant-7 guard only rejects an explicit downgrade. Delegates
/// to [`Entrenchment::strength`] so the ranking has a single source of truth.
pub(crate) fn entrenchment_rank(entrenchment: &str) -> u8 {
    Entrenchment::from_str(entrenchment)
        .map(|e| e.strength())
        .unwrap_or(0)
}

/// Reads a belief's stored `entrenchment` attribute by concept-id (invariant-7
/// guard input). `None` when the belief is absent or carries no entrenchment.
pub(crate) fn read_entrenchment_read(old: &MemId) -> (String, Vec<String>, Vec<GraphValue>) {
    let mut p = Params::new();
    p.s("bid", old.0.clone());
    p.read(
        "match $b isa belief, has concept-id $bc, has entrenchment $e; $bc == $bid;
         fetch { \"e\": $e };",
    )
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

    pub(crate) fn i(&mut self, name: &str, v: i64) {
        self.add(name, "integer", GraphValue::Long(v));
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

/// Binds a list of concept-ids as typed string params and appends one `match` line
/// per id binding a fresh entity variable (`${entity_var}{i}`, of type `entity_type`,
/// via concept var `${concept_var}{i}`) equal to param `${param}{i}`. Returns the
/// bound entity variable names (e.g. `$m0`, `$m1`) in order so the caller can build
/// the role/insert fragment that references them. Factors the id-list-binding loop
/// shared by `derivation_op` / `contest_ops` / `rests_on_op` / `retrieval_event_ops`.
fn bind_id_list<I, S>(
    p: &mut Params,
    match_lines: &mut String,
    ids: I,
    entity_type: &str,
    entity_var: &str,
    concept_var: &str,
    param: &str,
) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut vars = Vec::new();
    for (i, id) in ids.into_iter().enumerate() {
        p.s(&format!("{param}{i}"), id.into());
        match_lines.push_str(&format!(
            "           ${entity_var}{i} isa {entity_type}, has concept-id ${concept_var}{i}; ${concept_var}{i} == ${param}{i};\n"
        ));
        vars.push(format!("${entity_var}{i}"));
    }
    vars
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
    for var in bind_id_list(
        &mut p,
        &mut match_lines,
        evidence.iter().map(|e| e.0.clone()),
        "memory-item",
        "m",
        "mc",
        "e",
    ) {
        source_roles.push_str(&format!(", source: {var}"));
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
    Some(p.write(&body).expecting(1))
}

/// `supersede`: link old → new and flip old's belief-state to "superseded". The
/// link matches only a currently asserted `old`, so superseding a superseded or
/// retracted belief matches no row.
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
    let link_op = link
        .write(
            "match
           $o isa belief, has concept-id $oc, has belief-state \"asserted\"; $oc == $oid;
           $n isa belief, has concept-id $nc; $nc == $nid;
         insert (superseded: $o, superseding: $n) isa supersession,
             has supersession-reason == $reason, has created-at == $ca;",
        )
        .expecting(1);

    let mut flip = Params::new();
    flip.s("oid", old.0.clone());
    let flip_op = flip
        .write(
            "match $o isa belief, has concept-id $oc; $oc == $oid;
         update $o has belief-state \"superseded\";",
        )
        .expecting(1);

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
    for var in bind_id_list(
        &mut p,
        &mut match_lines,
        claims.iter().map(|c| c.0.clone()),
        "belief",
        "cl",
        "cc",
        "c",
    ) {
        claim_roles.push_str(&format!(", claim: {var}"));
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
    vec![
        ensure_run(ctx),
        ensure_agent(ctx),
        p.write(&body).expecting(1),
    ]
}

/// `rests_on`: a `rests-on` relation (resting depends on assumptions 1..).
pub(crate) fn rests_on_op(resting: &MemId, assumptions: &[MemId]) -> GraphWrite {
    let mut p = Params::new();
    p.s("rid", resting.0.clone());
    let mut match_lines = String::from("$r isa memory-item, has concept-id $rc; $rc == $rid;\n");
    let mut roles = String::new();
    for var in bind_id_list(
        &mut p,
        &mut match_lines,
        assumptions.iter().map(|a| a.0.clone()),
        "memory-item",
        "as",
        "ac",
        "a",
    ) {
        roles.push_str(&format!(", assumption: {var}"));
    }
    let body = format!(
        "match\n           {match_lines}         insert (resting: $r{roles}) isa rests-on;"
    );
    p.write(&body).expecting(1)
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
    .expecting(1)
}

// ── Rebuild-from-graph reads ──────────────────────────────────────────────────

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
       not { $e has consolidation-state \"tombstoned\"; };
       $own isa ownership, links (owned: $e, owner: $ag);
       $ag has agent-id $aid;
     fetch { \"id\": $eid, \"text\": $ttl, \"owner\": $aid, \"vf\": $et };"
}

/// Single current-state-belief lookup by concept-id (dirty-set repair).
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

// ── Retrieval-provenance write path ───────────────────────────────────────────

/// `retrieval-event`: the stamped episodic record that a semantic retrieval
/// happened, plus a `surfaced` edge per gated candidate (with its similarity score
/// and `was-used=false`). This is an authoritative episodic write, DISTINCT from
/// the best-effort index upsert (invariant 8 — two ledgers).
pub(crate) fn retrieval_event_ops(
    ctx: &RunContext,
    ev_id: &MemId,
    method: &str,
    candidate_count: i64,
    surfaced: &[(ConceptId, f32)],
) -> Vec<GraphWrite> {
    let mut p = Params::new();
    p.s("eid", ev_id.0.clone());
    p.dt("now", ctx.occurred_at);
    p.s("rid", ctx.run_id.to_string());
    p.s("aid", ctx.actor.0.clone());
    p.s("kind", origin_kind(&ctx.source));
    p.s("meth", method);
    p.i("cc", candidate_count);
    let mut match_lines = String::from(
        "$run isa doc-run, has run-id $rr; $rr == $rid;\n           $ag isa agent, has agent-id $aa; $aa == $aid;\n",
    );
    let item_vars = bind_id_list(
        &mut p,
        &mut match_lines,
        surfaced.iter().map(|(cid, _)| cid.0.clone()),
        "memory-item",
        "it",
        "ic",
        "c",
    );
    let mut surf_inserts = String::new();
    for (idx, ((_, score), var)) in surfaced.iter().zip(item_vars.iter()).enumerate() {
        p.f(&format!("s{idx}"), *score as f64);
        surf_inserts.push_str(&format!(
            "\n           (surfacer: $re, item: {var}) isa surfaced, has similarity-score == $s{idx}, has was-used false;"
        ));
    }
    let body = format!(
        "match\n           {match_lines}         insert
           $re isa retrieval-event, has concept-id == $eid, has event-time == $now,
               has ingest-time == $now, has retrieval-method == $meth, has candidate-count == $cc;
           (memory: $re, origin: $run) isa source-link, has origin-kind == $kind;
           (owned: $re, owner: $ag) isa ownership;{surf_inserts}"
    );
    vec![ensure_run(ctx), ensure_agent(ctx), p.write(&body)]
}

/// `informs-synthesis(retrieval: $ev, synthesized: $belief)` — ties a synthesized
/// belief to the retrieval-event that fed it (the retrieval ledger).
pub(crate) fn informs_synthesis_op(ev: &MemId, belief: &MemId) -> GraphWrite {
    let mut p = Params::new();
    p.s("eid", ev.0.clone());
    p.s("bid", belief.0.clone());
    p.write(
        "match
           $re isa retrieval-event, has concept-id $rc; $rc == $eid;
           $b isa belief, has concept-id $bc; $bc == $bid;
         insert (retrieval: $re, synthesized: $b) isa informs-synthesis;",
    )
    .expecting(1)
}

/// Flips `was-used` to `true` on the `surfaced` edge linking retrieval-event `ev`
/// to the used evidence item.
pub(crate) fn set_was_used_op(ev: &MemId, item: &MemId) -> GraphWrite {
    let mut p = Params::new();
    p.s("eid", ev.0.clone());
    p.s("iid", item.0.clone());
    p.write(
        "match
           $re isa retrieval-event, has concept-id $rc; $rc == $eid;
           $it isa memory-item, has concept-id $ic; $ic == $iid;
           $s isa surfaced, links (surfacer: $re, item: $it);
         update $s has was-used true;",
    )
    .expecting(1)
}

/// Sets the write-once `retrieval-provenance` summary flag on a belief.
pub(crate) fn set_retrieval_provenance_op(belief: &MemId, provenance: &str) -> GraphWrite {
    let mut p = Params::new();
    p.s("bid", belief.0.clone());
    p.s("pv", provenance);
    p.write(
        "match $b isa belief, has concept-id $bc; $bc == $bid;
         insert $b has retrieval-provenance == $pv;",
    )
}

// ── Consolidation ("dreaming") ────────────────────────────────────────────────

/// The dedup cohort read: every owned plain `episode` that is neither tombstoned nor
/// resolving a prediction and shares its title and event-time with another such
/// episode, one row per (episode, owner).
pub(crate) fn duplicate_episode_candidates_read() -> &'static str {
    "match
       $e isa! episode, has concept-id $eid, has title $ttl, has event-time $et, has ingest-time $it;
       $f isa! episode, has title $ttl, has event-time $et;
       not { $e is $f; };
       not { $e has consolidation-state \"tombstoned\"; };
       not { $f has consolidation-state \"tombstoned\"; };
       not { $pe isa prediction-resolution, links (resolving-episode: $e); };
       not { $pf isa prediction-resolution, links (resolving-episode: $f); };
       $own isa ownership, links (owned: $e, owner: $ag);
       $ag has agent-id $aid;
     fetch { \"id\": $eid, \"title\": $ttl, \"et\": $et, \"it\": $it, \"owner\": $aid };"
}

/// The guarded dedup write: one `match … update … insert …` pipeline that applies
/// only when `keeper` and `loser` are both exactly `episode`s with the same title and
/// event-time and identical owner sets, neither resolves a prediction or is already
/// tombstoned, and `keeper` ingested first (ties broken by lowest concept-id). It
/// tombstones `loser` and records a `consolidation-step`. Op index 2 of
/// `[ensure_run, ensure_agent, this]`; it must match exactly one row, so a refused
/// pair surfaces as a row-count mismatch on that op.
pub(crate) fn dedup_episodes_ops(
    ctx: &RunContext,
    keeper: &MemId,
    loser: &MemId,
) -> Vec<GraphWrite> {
    let mut p = Params::new();
    p.s("kid", keeper.0.clone());
    p.s("lid", loser.0.clone());
    p.s("rid", ctx.run_id.to_string());
    p.s("aid", ctx.actor.0.clone());
    p.s("act", "tombstone");
    p.dt("now", ctx.occurred_at);
    let body = "match
           $k isa! episode, has concept-id $kc, has title $t, has event-time $et, has ingest-time $ki; $kc == $kid;
           $l isa! episode, has concept-id $lc, has title $t, has event-time $et, has ingest-time $li; $lc == $lid;
           { $ki < $li; } or { $ki == $li; $kc < $lc; };
           not { $k has consolidation-state \"tombstoned\"; };
           not { $l has consolidation-state \"tombstoned\"; };
           not { $p1 isa prediction-resolution, links (resolving-episode: $k); };
           not { $p2 isa prediction-resolution, links (resolving-episode: $l); };
           not { $o1 isa ownership, links (owned: $k, owner: $a1); not { $o2 isa ownership, links (owned: $l, owner: $a1); }; };
           not { $o3 isa ownership, links (owned: $l, owner: $a2); not { $o4 isa ownership, links (owned: $k, owner: $a2); }; };
           $run isa doc-run, has run-id $rr; $rr == $rid;
           $ag isa agent, has agent-id $aa; $aa == $aid;
         update $l has consolidation-state \"tombstoned\";
         insert (affected: $l, run: $run, performer: $ag) isa consolidation-step,
           has consolidation-action == $act, has created-at == $now;";
    vec![
        ensure_run(ctx),
        ensure_agent(ctx),
        p.write(body).expecting(1),
    ]
}

/// Retrieval-provenance retention (concrete helper): the `retrieval-event`s that
/// are **safe to tombstone** — those every one of whose `informs-synthesis` beliefs
/// is superseded. The retention RULE it enforces: never tombstone a retrieval-event
/// while any belief it synthesized is non-superseded; once all are superseded the
/// event may decay (its episodic detail is lost) — but each belief's write-once
/// `retrieval-provenance` flag persists (the axis is retained on the belief, not the
/// event). A retrieval-event that fed no belief (no `informs-synthesis`) is not
/// returned here — it is out of scope for provenance-guarded pruning.
pub(crate) fn retrieval_events_safe_to_tombstone_read() -> &'static str {
    "match
       $re isa retrieval-event, has concept-id $rid;
       informs-synthesis (retrieval: $re, synthesized: $b);
       not { informs-synthesis (retrieval: $re, synthesized: $b2); not { true == is-superseded($b2); }; };
       not { $re has consolidation-state \"tombstoned\"; };
     select $rid; distinct;
     fetch { \"id\": $rid };"
}

/// `resolve_prediction`: mint a resolving `episode` (run + actor stamped) and attach
/// it (plus the outcome) to the belief's open `prediction-resolution`. The last op
/// matches only a prediction with no outcome yet and must affect exactly one row.
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

    // 2. link the episode + outcome onto the open prediction-resolution.
    let mut link = Params::new();
    link.s("bid", b.0.clone());
    link.s("eid", ep_id.0.clone());
    link.s("oc", outcome);
    let link_op = link
        .write(
            "match
           $b isa belief, has concept-id $bc; $bc == $bid;
           $pr isa prediction-resolution, links (predicted-belief: $b);
           not { $pr has outcome $prev; };
           $ep isa episode, has concept-id $ec; $ec == $eid;
         insert $pr links (resolving-episode: $ep);
                $pr has outcome == $oc;",
        )
        .expecting(1);

    vec![ensure_run(ctx), ensure_agent(ctx), ep_op, link_op]
}

// ── Populations (materialized membership, SPEC-W2-W3 §3.5) ────────────────────

/// Reads a population's stored spec by `spec-hash`; empty when it does not exist.
pub(crate) fn population_spec_read(spec_hash: &str) -> (String, Vec<String>, Vec<GraphValue>) {
    let mut p = Params::new();
    p.s("h", spec_hash);
    p.read(
        "match $p isa population, has spec-hash $ph; $ph == $h;
         fetch { \"h\": $ph, \"dialect\": $p.spec-dialect, \"text\": $p.spec-text };",
    )
}

/// Inserts a population with its defining spec.
pub(crate) fn define_population_op(spec_hash: &str, dialect: &str, text: &str) -> GraphWrite {
    let mut p = Params::new();
    p.s("h", spec_hash);
    p.s("dl", dialect);
    p.s("tx", text);
    p.write(
        "insert $p isa population, has spec-hash == $h, has spec-dialect == $dl,
             has spec-text == $tx;",
    )
}

/// Every `population-member` row of a population — open and closed — with its
/// member's concept-id and window/freshness attributes.
pub(crate) fn population_rows_read(spec_hash: &str) -> (String, Vec<String>, Vec<GraphValue>) {
    let mut p = Params::new();
    p.s("h", spec_hash);
    p.read(
        "match $p isa population, has spec-hash $ph; $ph == $h;
               $pm isa population-member, links (owning-population: $p, member: $m);
               $m has concept-id $mid;
         fetch { \"id\": $mid, \"vf\": $pm.valid-from, \"vt\": $pm.valid-to,
                 \"ca\": $pm.computed-at };",
    )
}

/// The subset of `ids` that exist as `concept`s (the only type that may be a
/// population member). The id-count shapes the disjunction; every id is a param.
pub(crate) fn existing_concepts_read(ids: &[ConceptId]) -> (String, Vec<String>, Vec<GraphValue>) {
    let mut p = Params::new();
    let mut alts = Vec::with_capacity(ids.len());
    for (i, id) in ids.iter().enumerate() {
        let name = format!("c{i}");
        p.s(&name, id.0.clone());
        alts.push(format!("{{ $cid == ${name}; }}"));
    }
    let filter = match alts.len() {
        1 => "$cid == $c0;".to_string(),
        _ => format!("{};", alts.join(" or ")),
    };
    p.read(&format!(
        "match $c isa concept, has concept-id $cid; {filter}
         fetch {{ \"id\": $cid }};"
    ))
}

/// One write op over `members`, one row per member, binding `$h`, `$cid`, `$at`.
fn member_rows_op(body: &str, spec_hash: &str, members: &[ConceptId], at: DateTime) -> GraphWrite {
    GraphWrite {
        query: format!("given $h: string, $cid: string, $at: datetime;\n{body}"),
        vars: vec!["h".into(), "cid".into(), "at".into()],
        rows: members
            .iter()
            .map(|m| {
                vec![
                    GraphValue::String(spec_hash.to_string()),
                    GraphValue::String(m.0.clone()),
                    GraphValue::Datetime(at),
                ]
            })
            .collect(),
        expect_rows: None,
    }
}

/// Opens a new membership epoch row for each member at `at`. Guarded in the write
/// itself: nothing is inserted for a member that already has an open row, or a row
/// computed after `at`.
pub(crate) fn open_membership_op(
    spec_hash: &str,
    members: &[ConceptId],
    at: DateTime,
) -> GraphWrite {
    member_rows_op(
        "match $p isa population, has spec-hash $ph; $ph == $h;
               $c isa concept, has concept-id $cc; $cc == $cid;
               not { $o isa population-member, links (owning-population: $p, member: $c);
                     not { $o has valid-to $ovt; }; };
               not { $s isa population-member, links (owning-population: $p, member: $c),
                         has computed-at $sca;
                     $sca > $at; };
         insert (owning-population: $p, member: $c) isa population-member,
             has valid-from == $at, has computed-at == $at;",
        spec_hash,
        members,
        at,
    )
}

/// Closes each member's open row at `at`: sets `valid-to` and advances `computed-at`.
/// A row already computed after `at` is left alone.
pub(crate) fn close_membership_op(
    spec_hash: &str,
    members: &[ConceptId],
    at: DateTime,
) -> GraphWrite {
    member_rows_op(
        "match $p isa population, has spec-hash $ph; $ph == $h;
               $c isa concept, has concept-id $cc; $cc == $cid;
               $pm isa population-member, links (owning-population: $p, member: $c);
               not { $pm has valid-to $vt; };
               not { $pm has computed-at $ca; $ca > $at; };
         update $pm has valid-to == $at, has computed-at == $at;",
        spec_hash,
        members,
        at,
    )
}

/// Advances `computed-at` on each member's open row, leaving its window untouched.
/// A row already computed after `at` is left alone.
pub(crate) fn refresh_membership_op(
    spec_hash: &str,
    members: &[ConceptId],
    at: DateTime,
) -> GraphWrite {
    member_rows_op(
        "match $p isa population, has spec-hash $ph; $ph == $h;
               $c isa concept, has concept-id $cc; $cc == $cid;
               $pm isa population-member, links (owning-population: $p, member: $c);
               not { $pm has valid-to $vt; };
               not { $pm has computed-at $ca; $ca > $at; };
         update $pm has computed-at == $at;",
        spec_hash,
        members,
        at,
    )
}

// ── Persisted-function reads ──────────────────────────────────────────────────

/// A read query with its `given` row: query text, param names, param values.
pub(crate) type Read = (String, Vec<String>, Vec<GraphValue>);

/// Matches memory item `b` when `is-superseded($b)` holds.
pub(crate) fn is_superseded_read(b: &MemId) -> Read {
    let mut p = Params::new();
    p.s("bid", b.0.clone());
    p.read(
        "match $b isa memory-item, has concept-id $bc; $bc == $bid;
               true == is-superseded($b);
         fetch { \"ok\": $bc };",
    )
}

/// Matches belief `n` when `gate($n, $agent, $now)` holds.
pub(crate) fn gate_read(n: &MemId, agent: &ActorId, now: DateTime) -> Read {
    let mut p = Params::new();
    p.s("nid", n.0.clone());
    p.s("aid", agent.0.clone());
    p.dt("now", now);
    p.read(
        "match $b isa belief, has concept-id $bc; $bc == $nid;
               $ag isa agent, has agent-id $aa; $aa == $aid;
               true == gate($b, $ag, $now);
         fetch { \"ok\": $bc };",
    )
}

/// Matches beliefs `b1` and `b2` when `contradicts($b1, $b2)` holds.
pub(crate) fn contradicts_read(b1: &MemId, b2: &MemId) -> Read {
    let mut p = Params::new();
    p.s("id1", b1.0.clone());
    p.s("id2", b2.0.clone());
    p.read(
        "match $b1 isa belief, has concept-id $c1; $c1 == $id1;
               $b2 isa belief, has concept-id $c2; $c2 == $id2;
               true == contradicts($b1, $b2);
         fetch { \"ok\": $c1 };",
    )
}

/// `retrieval-score($m, $now)` of memory item `m`, with its `last-access`.
pub(crate) fn retrieval_score_read(m: &MemId, now: DateTime) -> Read {
    let mut p = Params::new();
    p.s("mid", m.0.clone());
    p.dt("now", now);
    p.read(
        "match $m isa memory-item, has concept-id $mc; $mc == $mid;
               let $s in retrieval-score($m, $now);
         fetch { \"s\": $s, \"la\": $m.last-access };",
    )
}

/// `select-for-context($agent, $now)`: each belief id, score and `last-access`.
pub(crate) fn select_for_context_read(agent: &ActorId, now: DateTime) -> Read {
    let mut p = Params::new();
    p.s("aid", agent.0.clone());
    p.dt("now", now);
    p.read(
        "match $ag isa agent, has agent-id $aa; $aa == $aid;
               let $b, $score in select-for-context($ag, $now);
               $b has concept-id $bid;
         fetch { \"id\": $bid, \"score\": $score, \"la\": $b.last-access };",
    )
}

/// `population-members($pop)` of the population keyed by `spec_hash`.
pub(crate) fn population_members_read(spec_hash: &str) -> Read {
    let mut p = Params::new();
    p.s("h", spec_hash);
    p.read(
        "match $p isa population, has spec-hash $ph; $ph == $h;
               let $m in population-members($p);
               $m has concept-id $mid;
         fetch { \"id\": $mid };",
    )
}

/// `blast-radius($r)` of memory item `retracted`.
pub(crate) fn blast_radius_read(retracted: &MemId) -> Read {
    let mut p = Params::new();
    p.s("rid", retracted.0.clone());
    p.read(
        "match $r isa memory-item, has concept-id $rc; $rc == $rid;
               let $m in blast-radius($r);
               $m has concept-id $mid;
         fetch { \"id\": $mid };",
    )
}

/// `blast-radius-at($r, $at)` of any concept or memory item `retracted`.
pub(crate) fn blast_radius_at_read(retracted: &ConceptId, at: DateTime) -> Read {
    let mut p = Params::new();
    p.s("rid", retracted.0.clone());
    p.dt("at", at);
    p.read(
        "match $r isa okf-concept, has concept-id $rc; $rc == $rid;
               let $m in blast-radius-at($r, $at);
               $m has concept-id $mid;
         fetch { \"id\": $mid };",
    )
}

/// `canonical-entity($rec)` of record `rec`.
pub(crate) fn canonical_entity_read(rec: &ConceptId) -> Read {
    let mut p = Params::new();
    p.s("rid", rec.0.clone());
    p.read(
        "match $r isa concept, has concept-id $rc; $rc == $rid;
               let $o in canonical-entity($r);
               $o has concept-id $oid;
         fetch { \"id\": $oid };",
    )
}

/// `retrieval-provenance-of($b)` of belief `b`.
pub(crate) fn retrieval_provenance_of_read(b: &MemId) -> Read {
    let mut p = Params::new();
    p.s("bid", b.0.clone());
    p.read(
        "match $b isa belief, has concept-id $bc; $bc == $bid;
               let $rv in retrieval-provenance-of($b);
               $rv has concept-id $rid;
         fetch { \"id\": $rid };",
    )
}

/// `derivation-chain($b)` of memory item `b`.
pub(crate) fn derivation_chain_read(b: &MemId) -> Read {
    let mut p = Params::new();
    p.s("bid", b.0.clone());
    p.read(
        "match $b isa memory-item, has concept-id $bc; $bc == $bid;
               let $m in derivation-chain($b);
               $m has concept-id $mid;
         fetch { \"id\": $mid };",
    )
}

/// `believed-at($t)`: each belief's id.
pub(crate) fn believed_at_read(at: DateTime) -> Read {
    let mut p = Params::new();
    p.dt("t", at);
    p.read(
        "match let $b in believed-at($t);
               $b has concept-id $bid;
         fetch { \"id\": $bid };",
    )
}

/// `believed-at($t)` restricted to beliefs `visible-to` `actor`: each belief's id and
/// title for as-of recall, newest `valid-from` first, ties broken by concept id.
/// Supersession is not applied as of `at`. An actor with no `agent` node matches
/// nothing, so its as-of recall is empty, as with `gated_candidate_read`.
pub(crate) fn believed_at_chunks_read(at: DateTime, actor: &ActorId) -> Read {
    let mut p = Params::new();
    p.dt("t", at);
    p.s("aid", actor.0.clone());
    p.read(
        "match $ag isa agent, has agent-id $aa; $aa == $aid;
               let $b in believed-at($t);
               true == visible-to($b, $ag);
               $b has concept-id $bid, has valid-from $vf;
         sort $vf desc, $bid asc;
         fetch { \"id\": $bid, \"title\": $b.title };",
    )
}

/// One recall candidate's title, `retrieval-score` and `last-access`, matched only
/// when it passes `gate` for `actor` at `now`.
pub(crate) fn gated_candidate_read(cid: &ConceptId, actor: &ActorId, now: DateTime) -> Read {
    let mut p = Params::new();
    p.s("cid", cid.0.clone());
    p.s("aid", actor.0.clone());
    p.dt("now", now);
    p.read(
        "match $b isa belief, has concept-id $bc; $bc == $cid;
               $ag isa agent, has agent-id $aa; $aa == $aid;
               true == gate($b, $ag, $now);
               let $act in retrieval-score($b, $now);
         fetch { \"title\": $b.title, \"act\": $act, \"la\": $b.last-access };",
    )
}

// ── Belief edges ──────────────────────────────────────────────────────────────
//
// Relation, role, player, key and attribute labels are interpolated into these
// queries: they cannot be `given` parameters. Every label comes from a
// `BeliefEdgeDecl` that the belief-edge registry admitted only after checking it
// against `^[a-z][a-z0-9-]*$`. Values still travel as `given` params.

/// The TypeQL value type that carries `v` in a `given` stage.
fn graph_value_type(v: &GraphValue) -> &'static str {
    match v {
        GraphValue::String(_) => "string",
        GraphValue::Datetime(_) => "datetime",
        GraphValue::Double(_) => "double",
        GraphValue::Long(_) => "integer",
        GraphValue::Boolean(_) => "boolean",
    }
}

/// One `decl` edge from the belief `belief` to the players `edge` names by key,
/// carrying `edge`'s attributes. The op must bind exactly one row, so a missing or
/// ambiguous player fails the write.
///
/// `edge` must already be validated against `decl`: each declared role supplied once,
/// and only declared attributes. Players bind in `decl.roles` order and attributes in
/// `decl.owned_attrs` order.
pub(crate) fn belief_edge_op(
    decl: &BeliefEdgeDecl,
    belief: &MemId,
    edge: &BeliefEdge,
) -> GraphWrite {
    let mut p = Params::new();
    p.s("b", belief.0.clone());
    let mut match_lines = String::from("$bel isa belief, has concept-id $bc; $bc == $b;\n");
    let mut role_players = format!("{}: $bel", decl.belief_role);
    for (i, role) in decl.roles.iter().enumerate() {
        let value = edge
            .players
            .iter()
            .find(|(r, _)| r == role.role)
            .map(|(_, v)| v);
        debug_assert!(
            value.is_some(),
            "edge validated: role {} supplied",
            role.role
        );
        let Some(value) = value else { continue };
        p.add(&format!("p{i}"), graph_value_type(value), value.clone());
        match_lines.push_str(&format!(
            "           $x{i} isa {}, has {} $k{i}; $k{i} == $p{i};\n",
            role.player, role.key
        ));
        role_players.push_str(&format!(", {}: $x{i}", role.role));
    }
    let mut has = String::new();
    for (i, attr) in decl.owned_attrs.iter().enumerate() {
        let Some((_, value)) = edge.attrs.iter().find(|(a, _)| a == attr.attr) else {
            continue;
        };
        p.add(&format!("a{i}"), attr.value.tql(), value.clone());
        has.push_str(&format!(", has {} == $a{i}", attr.attr));
    }
    let body = format!(
        "match\n           {match_lines}         insert ({role_players}) isa {}{has};",
        decl.relation
    );
    p.write(&body).expecting(1)
}

/// One condition of a belief edge's live-schema check and the type query that
/// confirms it.
pub(crate) struct SchemaCheck {
    /// The condition, as an error message names it.
    pub(crate) condition: String,
    /// A parameterless `fetch` that returns at least one document when the condition
    /// holds.
    pub(crate) read: Read,
}

impl SchemaCheck {
    fn new(condition: String, pattern: &str, var: &str) -> Self {
        let query = format!("match {pattern} let $l = label(${var}); fetch {{ \"l\": $l }};");
        Self {
            condition,
            read: (query, Vec::new(), Vec::new()),
        }
    }
}

/// The type queries that confirm the database schema matches `decl`. Every check
/// passes on at least one document. A check naming a label the schema lacks fails as a
/// query error rather than an empty answer, so an error also fails the check.
///
/// TypeDB cannot match annotations in a type query, so the key check confirms
/// only that the player owns its key, not that the key is `@key` or `@unique`. The
/// write's one-row count still refuses a key that matches more than one player.
pub(crate) fn belief_edge_schema_reads(decl: &BeliefEdgeDecl) -> Vec<SchemaCheck> {
    let rel = decl.relation;
    let mut checks = Vec::new();
    for role in std::iter::once(decl.belief_role).chain(decl.roles.iter().map(|r| r.role)) {
        checks.push(SchemaCheck::new(
            format!("`{rel}` relates `{role}`"),
            &format!("$r label {rel}; $r relates {role};"),
            "r",
        ));
    }
    checks.push(SchemaCheck::new(
        format!("`belief` plays `{rel}:{}`", decl.belief_role),
        &format!("$t label belief; $t plays {rel}:{};", decl.belief_role),
        "t",
    ));
    for role in decl.roles {
        let (player, r, key) = (role.player, role.role, role.key);
        checks.push(SchemaCheck::new(
            format!("`{player}` plays `{rel}:{r}`"),
            &format!("$t label {player}; $t plays {rel}:{r};"),
            "t",
        ));
        checks.push(SchemaCheck::new(
            format!("`{player}` owns its key `{key}`"),
            &format!("$t label {player}; $t owns {key};"),
            "t",
        ));
        // Holds when no type is both a memory item and a `player`, so the role can
        // never bind a belief, an episode or another memory node.
        checks.push(SchemaCheck::new(
            format!("no memory-item type is a `{player}`"),
            &format!("$m label memory-item; not {{ $s sub $m; $s sub {player}; }};"),
            "m",
        ));
    }
    for attr in decl.owned_attrs {
        let a = attr.attr;
        checks.push(SchemaCheck::new(
            format!("`{rel}` owns `{a}`"),
            &format!("$r label {rel}; $r owns {a};"),
            "r",
        ));
    }
    checks
}
