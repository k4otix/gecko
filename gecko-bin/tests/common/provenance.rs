//! The ownership and provenance a raw-seeded memory item needs to commit. Included
//! with `#[path = "common/provenance.rs"] mod provenance;`.
//!
//! The schema requires every memory item to play `ownership:owned` exactly once and
//! `source-link:memory` at least once. [`seed_provenance`] matches the whole
//! database, not only the items the open transaction inserted: every memory item
//! that lacks an ownership gets one by the seed agent, always with `private`
//! visibility (production writers set a visibility only on beliefs), and every one
//! that lacks a source-link gets one to the seed `doc-run` tagged with an
//! `origin-kind`. The schema also requires every belief to own `valid-from`,
//! `salience`, `base-activation` and `last-access`; each belief lacking one gets
//! the value of [`SEED_TIME`] or the writer's default.
//!
//! Its twin is `core/mem-gecko/tests/common/provenance.rs`; keep the two files identical.

use gecko_extension_api::{GraphValue, GraphWrite};

/// The agent that owns every raw-seeded memory item.
pub const SEED_AGENT: &str = "seed-agent";

/// The run every raw-seeded memory item is sourced from.
pub const SEED_RUN: &str = "seed-run";

/// The `valid-from` and `last-access` of a raw-seeded belief that has none.
pub const SEED_TIME: &str = "2026-01-01T00:00:00";

/// Writes that give every memory item in the database lacking an ownership a
/// `private` one by [`SEED_AGENT`], every one lacking a source-link one to
/// [`SEED_RUN`], and every belief its missing required fields. Run them in the transaction that inserts the raw items, before its
/// commit. The twin of this function is in `core/mem-gecko/tests/common/provenance.rs`.
pub fn seed_provenance() -> Vec<GraphWrite> {
    vec![
        GraphWrite::single(
            "given $aid: string; put $ag isa agent, has agent-id == $aid;",
            vec!["aid".into()],
            vec![GraphValue::from(SEED_AGENT)],
        ),
        GraphWrite::single(
            "given $rid: string; put $run isa doc-run, has run-id == $rid;",
            vec!["rid".into()],
            vec![GraphValue::from(SEED_RUN)],
        ),
        GraphWrite::single(
            "given $aid: string, $vis: string; \
             match $ag isa agent, has agent-id $a; $a == $aid; \
                   $m isa memory-item; \
                   not { $o isa ownership, links (owned: $m); }; \
             insert (owned: $m, owner: $ag) isa ownership, has visibility == $vis;",
            vec!["aid".into(), "vis".into()],
            vec![GraphValue::from(SEED_AGENT), GraphValue::from("private")],
        ),
        GraphWrite::single(
            "given $rid: string, $kind: string; \
             match $run isa doc-run, has run-id $r; $r == $rid; \
                   $m isa memory-item; \
                   not { $l isa source-link, links (memory: $m); }; \
             insert (memory: $m, origin: $run) isa source-link, has origin-kind == $kind;",
            vec!["rid".into(), "kind".into()],
            vec![GraphValue::from(SEED_RUN), GraphValue::from("test-seed")],
        ),
        belief_default("valid-from", "datetime", GraphValue::Datetime(seed_time())),
        belief_default("salience", "double", GraphValue::Double(0.5)),
        belief_default("base-activation", "double", GraphValue::Double(1.0)),
        belief_default("last-access", "datetime", GraphValue::Datetime(seed_time())),
    ]
}

/// [`SEED_TIME`] as a datetime.
fn seed_time() -> gecko_extension_api::DateTime {
    chrono::NaiveDateTime::parse_from_str(SEED_TIME, "%Y-%m-%dT%H:%M:%S")
        .expect("SEED_TIME is a datetime")
        .and_utc()
}

/// A write giving every belief that lacks `attr` the value `value` of type `ty`.
fn belief_default(attr: &str, ty: &str, value: GraphValue) -> GraphWrite {
    GraphWrite::single(
        format!(
            "given $v: {ty}; \
             match $b isa belief; not {{ $b has {attr} $x; }}; \
             insert $b has {attr} == $v;"
        ),
        vec!["v".into()],
        vec![value],
    )
}
