//! The closure functions `blast-radius`, `blast-radius-at`, `canonical-entity` and
//! `canonical-entity-at` return exactly what their reference bodies
//! (`tests/fixtures/closure_functions_ref.tql`, the bodies before the closures were
//! split into step functions) return, on random graphs.
//!
//! Each case writes a graph under its own concept-id prefix, so the cases share one
//! database without reaching each other: beliefs joined by `rests-on` and by
//! `derivation` (provable and not), populations whose windowed memberships hold
//! concepts and scope beliefs, and resolutions in random states and windows, derived
//! by random methods, resolving pairs of concepts.

use std::collections::BTreeSet;

use futures_util::StreamExt;
use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_engine::db::run_graph_writes;
use gecko_extension_api::GeckoExtension;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use ulid::Ulid;

mod common;
#[path = "common/provenance.rs"]
mod provenance;

const CORE_SCHEMA: &str = include_str!("../../gecko-engine/schema/core_schema.tql");
const REFERENCE: &str = include_str!("fixtures/closure_functions_ref.tql");

const BELIEFS: usize = 6;
const CONCEPTS: usize = 5;
const POPULATIONS: usize = 2;
const RESOLUTIONS: usize = 4;
const METHODS: [&str; 5] = [
    "type-join",
    "cardinality",
    "functional-dependency",
    "llm-synthesis",
    "human-assertion",
];
/// The times the as-of variants are asked at, between the window bounds below.
const AT: [&str; 3] = [
    "2026-02-15T00:00:00",
    "2026-04-15T00:00:00",
    "2026-06-15T00:00:00",
];
const FROMS: [&str; 2] = ["2026-01-01T00:00:00", "2026-03-01T00:00:00"];
const TOS: [&str; 2] = ["2026-04-01T00:00:00", "2026-05-01T00:00:00"];

#[derive(Debug, Clone)]
struct Membership {
    population: usize,
    concept: usize,
    from: Option<usize>,
    to: Option<usize>,
}

#[derive(Debug, Clone)]
struct Resolution {
    state: &'static str,
    from: usize,
    to: Option<usize>,
    methods: Vec<usize>,
    records: (usize, usize),
}

#[derive(Debug, Clone)]
struct Graph {
    rests_on: Vec<(usize, usize)>,
    derivations: Vec<(usize, usize, usize)>,
    scoped: Vec<(usize, usize)>,
    memberships: Vec<Membership>,
    resolutions: Vec<Resolution>,
}

fn graph() -> impl Strategy<Value = Graph> {
    let belief = 0..BELIEFS;
    let concept = 0..CONCEPTS;
    let membership = (
        0..POPULATIONS,
        concept.clone(),
        proptest::option::of(0..FROMS.len()),
        proptest::option::of(0..TOS.len()),
    )
        .prop_map(|(population, concept, from, to)| Membership {
            population,
            concept,
            from,
            to,
        });
    let resolution = (
        proptest::sample::select(vec!["asserted", "superseded", "retracted"]),
        0..FROMS.len(),
        proptest::option::of(0..TOS.len()),
        proptest::collection::vec(0..METHODS.len(), 0..=2),
        (concept.clone(), concept),
    )
        .prop_map(|(state, from, to, methods, records)| Resolution {
            state,
            from,
            to,
            methods,
            records,
        });
    (
        proptest::collection::vec((belief.clone(), belief.clone()), 0..=6),
        proptest::collection::vec((belief.clone(), belief.clone(), 0..METHODS.len()), 0..=6),
        proptest::collection::vec((belief, 0..POPULATIONS), 0..=4),
        proptest::collection::vec(membership, 0..=6),
        proptest::collection::vec(resolution, 0..=RESOLUTIONS),
    )
        .prop_map(
            |(rests_on, derivations, scoped, memberships, resolutions)| Graph {
                rests_on,
                derivations,
                scoped,
                memberships,
                resolutions,
            },
        )
}

/// The insert for `g` under the concept-id prefix `p`.
fn insert(p: &str, g: &Graph) -> String {
    let mut q = String::from("insert\n");
    for i in 0..BELIEFS {
        q += &format!(
            "$b{i} isa belief, has concept-id \"{p}/b{i}\", has belief-state \"asserted\";\n"
        );
    }
    for i in 0..CONCEPTS {
        q += &format!("$k{i} isa concept, has concept-id \"{p}/k{i}\";\n");
    }
    for i in 0..POPULATIONS {
        q += &format!(
            "$p{i} isa population, has spec-hash \"{p}/pop{i}\", has spec-dialect \"typeql-3\", \
             has spec-text \"match $c isa concept;\";\n"
        );
    }
    for (a, r) in &g.rests_on {
        q += &format!("rests-on (assumption: $b{a}, resting: $b{r});\n");
    }
    for (s, d, m) in &g.derivations {
        q += &format!(
            "derivation (source: $b{s}, derived: $b{d}), has derivation-method \"{}\";\n",
            METHODS[*m]
        );
    }
    for (b, pop) in &g.scoped {
        q += &format!("scoped (scoped-belief: $b{b}, population: $p{pop});\n");
    }
    for m in &g.memberships {
        q += &format!(
            "population-member (owning-population: $p{}, member: $k{})",
            m.population, m.concept
        );
        if let Some(f) = m.from {
            q += &format!(", has valid-from {}", FROMS[f]);
        }
        if let Some(t) = m.to {
            q += &format!(", has valid-to {}", TOS[t]);
        }
        q += ";\n";
    }
    for (i, r) in g.resolutions.iter().enumerate() {
        q += &format!(
            "$r{i} isa resolution, has concept-id \"{p}/r{i}\", has belief-state \"{}\", \
             has valid-from {}",
            r.state, FROMS[r.from]
        );
        if let Some(t) = r.to {
            q += &format!(", has valid-to {}", TOS[t]);
        }
        q += ";\n";
        for m in &r.methods {
            q += &format!(
                "derivation (source: $b0, derived: $r{i}), has derivation-method \"{}\";\n",
                METHODS[*m]
            );
        }
        q += &format!(
            "resolves (resolution-belief: $r{i}, record-a: $k{}, record-b: $k{});\n",
            r.records.0, r.records.1
        );
    }
    q
}

async fn ids(router: &mut TypeDbRouter, query: &str) -> BTreeSet<String> {
    let tx = router.begin_read().await.expect("read tx");
    let answer = tx
        .query(query)
        .await
        .unwrap_or_else(|e| panic!("{query}: {e}"));
    let mut out = BTreeSet::new();
    let mut stream = answer.into_documents();
    while let Some(doc) = stream.next().await {
        let doc: serde_json::Value =
            serde_json::from_str(&doc.expect("document").into_json().to_string()).unwrap();
        out.insert(doc["id"].as_str().expect("id").to_string());
    }
    out
}

/// The ids `function` returns for the node `id`, with `extra` appended to its
/// arguments.
async fn closure(
    router: &mut TypeDbRouter,
    ty: &str,
    id: &str,
    function: &str,
    extra: &str,
) -> BTreeSet<String> {
    ids(
        router,
        &format!(
            "match $x isa {ty}, has concept-id \"{id}\"; let $m in {function}($x{extra}); \
             $m has concept-id $mid; fetch {{ \"id\": $mid }};"
        ),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closures_match_their_reference_bodies() {
    let name = format!("clostest_{}", Ulid::generate().to_string().to_lowercase());
    let mut router = TypeDbRouter::new(DbConfig {
        address: "localhost:1729".to_string(),
        database: name.clone(),
        username: "admin".to_string(),
        password: "password".to_string(),
        tls: TlsMode::Disabled,
    });
    router.apply_schema(CORE_SCHEMA).await.expect("core schema");
    router
        .apply_schema(mem_gecko::MemGecko::new().schema())
        .await
        .expect("mem schema");
    router
        .apply_schema(REFERENCE)
        .await
        .expect("reference functions");

    const CASES: usize = 10;
    let mut runner = TestRunner::deterministic();
    let graphs: Vec<Graph> = (0..CASES)
        .map(|_| graph().new_tree(&mut runner).expect("a graph").current())
        .collect();
    {
        let tx = router.begin_write().await.expect("write tx");
        for (n, g) in graphs.iter().enumerate() {
            let q = insert(&format!("c{n}"), g);
            tx.query(&q)
                .await
                .unwrap_or_else(|e| panic!("case {n}: {e}\n{q}"));
        }
        run_graph_writes(&tx, &provenance::seed_provenance())
            .await
            .expect("seed provenance");
        tx.commit().await.expect("commit");
    }

    let mut nonempty = 0;
    for (n, g) in graphs.iter().enumerate() {
        let p = format!("c{n}");
        let nodes = (0..BELIEFS)
            .map(|i| ("belief", format!("{p}/b{i}")))
            .chain((0..CONCEPTS).map(|i| ("concept", format!("{p}/k{i}"))));
        for (ty, id) in nodes {
            let mut checks = vec![("blast-radius", String::new())];
            for t in AT {
                checks.push(("blast-radius-at", format!(", {t}")));
            }
            if ty == "concept" {
                checks.push(("canonical-entity", String::new()));
                for t in AT {
                    checks.push(("canonical-entity-at", format!(", {t}")));
                }
            }
            for (function, extra) in checks {
                let got = closure(&mut router, ty, &id, function, &extra).await;
                let want = closure(&mut router, ty, &id, &format!("{function}-ref"), &extra).await;
                assert_eq!(
                    got, want,
                    "case {n}: {function}({id}{extra}) differs from its reference; graph: {g:?}"
                );
                if !got.is_empty() {
                    nonempty += 1;
                }
            }
        }
    }
    eprintln!("{nonempty} nonempty closures across {CASES} graphs");
    assert!(
        nonempty > 20,
        "the graphs reached too little: {nonempty} nonempty closures"
    );

    common::drop_db_with_retry(&mut router, &name).await;
}
