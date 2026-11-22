//! The ownership and provenance a raw-seeded memory item needs to commit. Included
//! with `#[path = "common/provenance.rs"] mod provenance;`.
//!
//! The schema requires every memory item to play `ownership:owned` exactly once and
//! `source-link:memory` at least once. [`seed_provenance`] gives each memory item in
//! the open transaction that lacks one the shape the production writers insert: an
//! `ownership` by the seed agent with `private` visibility, and a `source-link` to
//! the seed `doc-run` tagged with an `origin-kind`.

use gecko_extension_api::{GraphValue, GraphWrite};

/// The agent that owns every raw-seeded memory item.
pub const SEED_AGENT: &str = "seed-agent";

/// The run every raw-seeded memory item is sourced from.
pub const SEED_RUN: &str = "seed-run";

/// Writes that give every memory item lacking an ownership one by [`SEED_AGENT`],
/// and every memory item lacking a source-link one to [`SEED_RUN`]. Run them in the
/// transaction that inserts the raw items, before its commit.
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
    ]
}
