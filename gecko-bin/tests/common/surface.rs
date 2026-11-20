//! Schema-introspection helpers shared by the tests that derive the belief-edge
//! surface from a composed schema. Included with
//! `#[path = "common/surface.rs"] mod surface;` beside `mod common;`.

use std::collections::{BTreeSet, HashMap, HashSet};

use futures_util::StreamExt;

use super::common::TestDb;

/// Runs a raw-TQL read whose final stage is `select $<col>;` (one string column)
/// and collects the values, sorted for deterministic assertions.
///
/// A `Some(Err(_))` mid-stream is surfaced (via `.expect`), never silently swallowed
/// — an ended-early stream would otherwise mask a DB failure as a short result set.
pub async fn read_col(db: &TestDb, query: &str, col: &str) -> Vec<String> {
    let mut router = db.router();
    let tx = router.begin_read().await.expect("read tx");
    let answer = tx.query(query).await.expect("read query");
    assert!(answer.is_row_stream(), "select returns a row stream");
    let mut rows = answer.into_rows();
    let mut out = Vec::new();
    while let Some(item) = rows.next().await {
        let row = item.expect("row stream item");
        let val = row
            .get(col)
            .expect("column present")
            .expect("column bound")
            .try_get_string()
            .expect("column is a string")
            .to_string();
        out.push(val);
    }
    out.sort();
    out
}

/// Introspects the composed schema and returns the **belief-edge relation surface**:
/// every relation owning at least one role played EXCLUSIVELY by `memory-item`
/// subtypes. This is the introspection-derived monopoly surface SPEC-W5 §1.1 makes
/// authoritative — no hand-maintained list, so a newly-landed belief-edge relation is
/// caught with no test edit.
///
/// The query is exactly the §1.1 recipe: `$t sub memory-item; $t plays $r` yields the
/// candidate roles; the full player set of each role (`$t plays $r; $rel relates $r`)
/// decides exclusivity host-side; the owning **relation** label is decoded from
/// `label($rel)`. A relation qualifies on ≥1 exclusive role even if its other roles
/// admit records — `divergence-precedence` is caught via `belief plays :prevailing` /
/// `:overridden` though its `exception-scope` role is played by the record-tier
/// `concept`; `resolves`/`scoped` are caught via their belief role though `record-*` /
/// `population` are record/cache tier.
pub async fn belief_edge_surface(db: &TestDb) -> BTreeSet<String> {
    // 1. The memory-item subtype closure (reflexive: `sub` includes `memory-item`).
    let mi_subtypes: HashSet<String> = read_col(
        db,
        r#"match $t sub memory-item; let $l = label($t); select $l;"#,
        "l",
    )
    .await
    .into_iter()
    .collect();

    // 2. Every (player-type, role, owning-relation) triple in the schema. Roles are
    //    grouped by their (globally-unique) scoped label; the relation is read
    //    directly off `$rel relates $r`, never string-split from the role label.
    let mut router = db.router();
    let tx = router.begin_read().await.expect("read tx");
    let answer = tx
        .query(
            r#"match
                 $t plays $r;
                 $rel relates $r;
                 let $tl = label($t);
                 let $rl = label($r);
                 let $reln = label($rel);
               select $tl, $rl, $reln;"#,
        )
        .await
        .expect("introspection query");
    assert!(answer.is_row_stream(), "introspection returns a row stream");
    let mut rows = answer.into_rows();
    // scoped-role-label -> (owning-relation-label, set of player type labels)
    let mut role_players: HashMap<String, (String, HashSet<String>)> = HashMap::new();
    while let Some(item) = rows.next().await {
        let row = item.expect("introspection row");
        let get = |c: &str| {
            row.get(c)
                .expect("column present")
                .expect("column bound")
                .try_get_string()
                .expect("label is a string")
                .to_string()
        };
        let (player, role, relation) = (get("tl"), get("rl"), get("reln"));
        let entry = role_players
            .entry(role)
            .or_insert_with(|| (relation, HashSet::new()));
        entry.1.insert(player);
    }

    // 3. A relation is on the surface iff some role it owns is played by at least one
    //    type and ONLY by memory-item subtypes.
    let mut surface = BTreeSet::new();
    for (_role, (relation, players)) in role_players {
        if !players.is_empty() && players.iter().all(|p| mi_subtypes.contains(p)) {
            surface.insert(relation);
        }
    }
    surface
}
