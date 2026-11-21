//! The runtime reconciliation rig: every production path that changes the belief
//! tier is reproduced by in-process `MemWriter` calls.
//!
//! A production database is driven only through the real `gecko` binary. A
//! reference database gets the same setup through the binary, then only direct
//! `MemWriter` / `edge_writer_for("cyber-gecko")` calls. Each step compares the
//! production database's per-label belief-tier delta against the reference's, so
//! a write the binary makes outside `MemWriter` surfaces as an unaccounted label.
//! A disposition table then ratchets every introspected belief-tier label: each is
//! either reached by a production step or recorded as writer-only or owed.
//!
//! Requires a live TypeDB service (`localhost:1729`, admin/password). Both
//! databases are throwaway `TestDb`s; the real `gecko` database is untouched.
#![cfg(feature = "cyber")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use cyber_gecko::stix::{cyber_post_sync, to_okf};
use futures_util::StreamExt;
use gecko_engine::db::router::DbError;
use gecko_engine::db::surface::{
    BeliefTierFingerprint, BeliefTierMeasure, belief_tier_fingerprint, belief_tier_surface,
};
use gecko_engine::db::{CORE_SCHEMA, RouterGraphStore, run_graph_writes};
use gecko_engine::okf::parser::parse_bundle;
use gecko_engine::syncer::bundle::sync_bundle;
use gecko_extension_api::{
    ActorId, BeliefDraft, BeliefEdge, ConceptId, ContextBudget, DerivationMethod, Embedder,
    EpisodeDraft, EpistemicWriter, GraphValue, GraphWrite, MemId, Outcome, ProvenanceSource,
    RecallQuery, RunContext, Visibility,
};
use gecko_semantic_index::{HnswIndex, StubEmbedder};
use mem_gecko::{BELIEF_TIER_ROOT, BeliefEdgeRegistry, MemWriter};
use typedb_driver::concept::Value;
use typedb_driver::given::{GivenRowEntry, GivenRows};

mod common;
use common::TestDb;

const MEM_PLAYBOOK: &str = "playbooks/rig_mem";
const CYBER_PLAYBOOK: &str = "playbooks/rig_cyber";

/// The episode `rig_mem` remembers; the stub embedder surfaces it for [`T_Q`].
const T_EP: &str = "smb lateral movement observed";
/// The query `rig_mem` recalls with.
const T_Q: &str = "lateral movement";
/// The belief derived from the recalled chunks.
const T_B1: &str = "host is compromised via smb";
/// The belief derived from no evidence.
const T_B2: &str = "host is not compromised";
/// The belief superseding [`T_B1`].
const T_S: &str = "host is compromised via smb and rdp";

/// The STIX 2.1 / ATT&CK bundle `cyber_stix_sync_test` syncs.
const STIX_BUNDLE: &str = r#"{
  "type": "bundle",
  "objects": [
    {
      "type": "attack-pattern",
      "id": "attack-pattern--00000000-0000-0000-0000-0000000000a1",
      "name": "Command and Scripting Interpreter",
      "external_references": [
        { "source_name": "mitre-attack", "external_id": "T1059" }
      ],
      "kill_chain_phases": [
        { "kill_chain_name": "mitre-attack", "phase_name": "execution" }
      ],
      "x_mitre_platforms": ["Windows", "Linux"]
    },
    {
      "type": "malware",
      "id": "malware--00000000-0000-0000-0000-0000000000b2",
      "name": "ExampleRAT"
    },
    {
      "type": "indicator",
      "id": "indicator--00000000-0000-0000-0000-0000000000c3",
      "name": "Bad IP",
      "pattern": "[ipv4-addr:value = '1.1.1.1']",
      "valid_until": "2027-01-01T00:00:00Z"
    },
    {
      "type": "ipv4-addr",
      "id": "ipv4-addr--00000000-0000-0000-0000-0000000000d4",
      "value": "1.1.1[.]1"
    },
    {
      "type": "x-mitre-data-source",
      "id": "x-mitre-data-source--00000000-0000-0000-0000-0000000000e5",
      "name": "Command",
      "external_references": [
        { "source_name": "mitre-attack", "external_id": "DS0017" }
      ],
      "x_mitre_platforms": ["Windows"]
    },
    {
      "type": "course-of-action",
      "id": "course-of-action--00000000-0000-0000-0000-0000000000a8",
      "name": "Command and Scripting Interpreter Mitigation",
      "external_references": [
        { "source_name": "mitre-attack", "external_id": "T1059" }
      ],
      "x_mitre_deprecated": true
    },
    {
      "type": "attack-pattern",
      "id": "attack-pattern--00000000-0000-0000-0000-0000000000a9",
      "name": "Revoked Technique",
      "external_references": [
        { "source_name": "mitre-attack", "external_id": "T1059" }
      ],
      "revoked": true
    },
    {
      "type": "x-mitre-analytic",
      "id": "x-mitre-analytic--00000000-0000-0000-0000-0000000000a7",
      "name": "Analytic 0001",
      "x_mitre_platforms": ["Linux"]
    },
    {
      "type": "x-mitre-data-component",
      "id": "x-mitre-data-component--00000000-0000-0000-0000-0000000000f6",
      "name": "Command Execution",
      "x_mitre_data_source_ref": "x-mitre-data-source--00000000-0000-0000-0000-0000000000e5"
    },
    {
      "type": "relationship",
      "id": "relationship--00000000-0000-0000-0000-000000000101",
      "relationship_type": "indicates",
      "source_ref": "indicator--00000000-0000-0000-0000-0000000000c3",
      "target_ref": "malware--00000000-0000-0000-0000-0000000000b2"
    },
    {
      "type": "relationship",
      "id": "relationship--00000000-0000-0000-0000-000000000102",
      "relationship_type": "uses",
      "source_ref": "malware--00000000-0000-0000-0000-0000000000b2",
      "target_ref": "attack-pattern--00000000-0000-0000-0000-0000000000a1"
    },
    {
      "type": "relationship",
      "id": "relationship--00000000-0000-0000-0000-000000000103",
      "relationship_type": "detects",
      "source_ref": "x-mitre-data-component--00000000-0000-0000-0000-0000000000f6",
      "target_ref": "attack-pattern--00000000-0000-0000-0000-0000000000a1"
    }
  ]
}"#;

/// A JSON (and so JavaScript) string literal for `s`.
fn js(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

/// The `rig_mem` playbook: every `mem.*` write a guest can reach, in one run. `b2`
/// is derived before the recall because recall surfaces beliefs only, so `b1` then
/// derives from a recalled belief and the retrieval-provenance edges are written.
fn mem_playbook() -> String {
    format!(
        r#"---
type: playbook
title: Rig mem
engine: quickjs
scopes: [mem:remember, mem:recall, mem:derive, mem:supersede, mem:contest]
---

# Rig mem

```javascript
const e = mem.remember({ep});
const b2 = mem.derive({b2}, []);
const chunks = mem.recall({q}, {{ maxChunks: 5 }});
const b1 = mem.derive({b1}, chunks.map(c => c.id), {{ confidence: 0.6 }});
const s = mem.supersede(b1.id, {s}, "revised");
const a = mem.contest([s.id, b2.id]);
({{ episode: e.id, recalled: chunks.length, b1: b1.id, b2: b2.id, s: s.id, anomaly: a.anomaly }})
```
"#,
        ep = js(T_EP),
        q = js(T_Q),
        b1 = js(T_B1),
        b2 = js(T_B2),
        s = js(T_S),
    )
}

/// The `rig_cyber` playbook: cyber-gecko's two belief-writing detect imports.
fn cyber_playbook() -> String {
    format!(
        r#"---
type: playbook
title: Rig cyber
engine: quickjs
scopes: [cyber-gecko:claim, cyber-gecko:disposition]
---

# Rig cyber

```javascript
const c = gecko.callExtension("cyber-gecko", "claim", {{ logic_id: {logic}, technique_id: "T1059", method: "external-tool", confidence: 0.9 }});
const d = gecko.callExtension("cyber-gecko", "disposition", {{ logic_id: {logic}, verdict: "true-positive" }});
({{ claim: c.belief_id, disposition: d.belief_id }})
```
"#,
        logic = js(CYBER_PLAYBOOK),
    )
}

/// The rig's working directory: the config, cache, OKF bundle and STIX bundle the
/// binary is driven with, and a copy of the OKF bundle without `rig_cyber`.
struct Rig {
    tmp: tempfile::TempDir,
    config: PathBuf,
    cache: PathBuf,
    okf_dir: PathBuf,
    okf_dir_without_cyber: PathBuf,
    stix: PathBuf,
}

impl Rig {
    fn new() -> Rig {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let root = tmp.path();
        let config = root.join("gecko.toml");
        let index = root.join("prod.hnsw");
        let index = toml::Value::String(index.to_str().expect("utf-8 temp path").to_string());
        std::fs::write(
            &config,
            format!(
                "[typedb]\nendpoint = \"localhost:1729\"\nmode = \"external\"\n\n\
                 [extensions]\nenabled = [\"cyber\"]\n\n\
                 [semantic_index]\nenabled = true\npath = {index}\nembedder = \"stub\"\n\
                 backend = \"hnsw\"\nrecord_retrieval_provenance = true\n"
            ),
        )
        .expect("write gecko.toml");

        let cache = root.join("cache");
        std::fs::create_dir_all(&cache).expect("cache dir");

        let okf_dir = root.join("okf");
        let okf_dir_without_cyber = root.join("okf-without-cyber");
        for dir in [&okf_dir, &okf_dir_without_cyber] {
            std::fs::create_dir_all(dir.join("playbooks")).expect("okf dir");
            std::fs::write(
                dir.join("bundle.json"),
                serde_json::json!({ "name": "rig-playbooks" }).to_string(),
            )
            .expect("write bundle.json");
            std::fs::write(dir.join("playbooks/rig_mem.md"), mem_playbook())
                .expect("write rig_mem");
        }
        std::fs::write(okf_dir.join("playbooks/rig_cyber.md"), cyber_playbook())
            .expect("write rig_cyber");

        let stix = root.join("stix.json");
        std::fs::write(&stix, STIX_BUNDLE).expect("write stix.json");

        Rig {
            tmp,
            config,
            cache,
            okf_dir,
            okf_dir_without_cyber,
            stix,
        }
    }
}

fn path_str(p: &std::path::Path) -> &str {
    p.to_str().expect("utf-8 temp path")
}

/// Runs the `gecko` binary against `db` with the rig's config and cache and
/// returns its output.
async fn gecko_output(rig: &Rig, db: &TestDb, args: &[&str]) -> std::process::Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_gecko"))
        .arg("--config")
        .arg(&rig.config)
        .arg("--database")
        .arg(&db.name)
        .args(args)
        .env("GECKO_CACHE_DIR", &rig.cache)
        .output()
        .await
        .expect("spawn gecko")
}

/// Runs the `gecko` binary against `db` with the rig's config and cache; asserts it
/// succeeds and returns its stdout.
async fn gecko(rig: &Rig, db: &TestDb, args: &[&str]) -> String {
    let out = gecko_output(rig, db, args).await;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "gecko {args:?} failed\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

/// Runs the `gecko` binary against `db` like [`gecko`]; asserts it fails and returns
/// its stderr, where the binary logs a command's error through `tracing`.
async fn gecko_expect_failure(rig: &Rig, db: &TestDb, args: &[&str]) -> String {
    let out = gecko_output(rig, db, args).await;
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !out.status.success(),
        "gecko {args:?} succeeded\n--- stdout ---\n{}\n--- stderr ---\n{stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
    stderr
}

/// Runs a playbook through the binary and returns its JSON output.
async fn run_playbook(rig: &Rig, db: &TestDb, concept_id: &str) -> serde_json::Value {
    let stdout = gecko(rig, db, &["run", concept_id]).await;
    assert!(
        stdout.contains("Execution successful!"),
        "run {concept_id} did not succeed:\n{stdout}"
    );
    let line = stdout
        .lines()
        .find_map(|l| l.strip_prefix("Output: "))
        .unwrap_or_else(|| panic!("run {concept_id} printed no output line:\n{stdout}"));
    serde_json::from_str(line)
        .unwrap_or_else(|e| panic!("run {concept_id} output is not JSON ({e}): {line}"))
}

/// Initializes `db`'s schema and syncs the OKF and STIX bundles into it, through
/// the binary.
async fn setup(rig: &Rig, db: &TestDb) {
    gecko(rig, db, &["schema-init"]).await;
    gecko(rig, db, &["sync", path_str(&rig.okf_dir)]).await;
    gecko(rig, db, &["sync-stix", path_str(&rig.stix)]).await;
}

/// The belief-tier fingerprint of `db`, over its live surface.
async fn fingerprint(db: &TestDb) -> BeliefTierFingerprint {
    let mut router = db.router();
    let tx = router.begin_read().await.expect("read tx");
    let surface = belief_tier_surface(&tx, BELIEF_TIER_ROOT)
        .await
        .expect("belief-tier surface");
    belief_tier_fingerprint(&tx, &surface)
        .await
        .expect("belief-tier fingerprint")
}

/// The non-zero per-label, per-measure count changes between two fingerprints.
type Delta = BTreeMap<(String, BeliefTierMeasure), i64>;

fn delta(before: &BeliefTierFingerprint, after: &BeliefTierFingerprint) -> Delta {
    before
        .changes_to(after)
        .into_iter()
        .filter(|c| c.delta() != 0)
        .map(|c| ((c.label.clone(), c.measure), c.delta()))
        .collect()
}

/// The labels a delta changes.
fn labels(d: &Delta) -> BTreeSet<String> {
    d.keys().map(|(label, _)| label.clone()).collect()
}

/// The labels whose observed change differs from the expected one under any
/// measure; a key missing on one side counts as 0.
fn unaccounted(expected: &Delta, observed: &Delta) -> BTreeSet<String> {
    expected
        .keys()
        .chain(observed.keys())
        .filter(|k| {
            expected.get(*k).copied().unwrap_or(0) != observed.get(*k).copied().unwrap_or(0)
        })
        .map(|(label, _)| label.clone())
        .collect()
}

/// Asserts that `observed` changes exactly what `expected` does, printing both.
fn assert_reconciled(step: &str, expected: &Delta, observed: &Delta) {
    let missing = unaccounted(expected, observed);
    assert!(
        missing.is_empty(),
        "{step}: unaccounted labels {missing:?}\n  reference: {expected:?}\n  production: {observed:?}"
    );
}

/// Counts the rows of a read that binds one string parameter `$v`.
async fn count_with(db: &TestDb, query: &str, value: &str) -> i64 {
    let mut router = db.router();
    let tx = router.begin_read().await.expect("read tx");
    let mut given = GivenRows::new(vec!["v".to_string()], 1);
    given
        .push_row(vec![GivenRowEntry::Value(Value::String(value.to_string()))])
        .expect("given row");
    let answer = tx
        .query_with_rows(query, given)
        .await
        .unwrap_or_else(|e| panic!("count {query}: {e}"));
    let mut rows = answer.into_rows();
    let mut counts = Vec::new();
    while let Some(item) = rows.next().await {
        let row = item.unwrap_or_else(|e| panic!("count {query}: {e}"));
        let n = row
            .get("n")
            .expect("column n")
            .and_then(|c| c.try_get_integer())
            .unwrap_or_else(|| panic!("count {query}: column n is not a bound integer"));
        counts.push(n);
    }
    assert_eq!(counts.len(), 1, "count {query}: expected one row");
    counts[0]
}

/// How a belief-tier label no production step reaches is accounted for; a label in
/// neither class must be accounted by a production step.
#[derive(Debug)]
enum LabelDisposition {
    /// Written only by these `MemWriter` methods, which no production path calls.
    WriterOnly {
        methods: &'static [&'static str],
        owner: &'static str,
    },
    /// In the schema with no writer yet; `owner` owes one.
    Owed { owner: &'static str },
}

impl LabelDisposition {
    fn owner(&self) -> &'static str {
        match self {
            Self::WriterOnly { owner, .. } | Self::Owed { owner } => owner,
        }
    }
}

/// The ledger items that may own a disposition.
const KNOWN_OWNERS: &[&str] = &["A5", "A7", "A8", "M1", "M2", "M3", "M4", "M5", "M6"];

/// Every belief-tier label no production step reaches, with how it is accounted for.
const DISPOSITIONS: &[(&str, LabelDisposition)] = &[
    (
        "rests-on",
        LabelDisposition::WriterOnly {
            methods: &["rests_on"],
            owner: "M1",
        },
    ),
    (
        "prediction-resolution",
        LabelDisposition::WriterOnly {
            methods: &["record_prediction", "resolve_prediction"],
            owner: "M2",
        },
    ),
    ("sighting", LabelDisposition::Owed { owner: "A8" }),
    ("sighting-of", LabelDisposition::Owed { owner: "A8" }),
    (
        "divergence-exception",
        LabelDisposition::Owed { owner: "A5" },
    ),
    (
        "divergence-precedence",
        LabelDisposition::Owed { owner: "A5" },
    ),
    ("scoped", LabelDisposition::Owed { owner: "A7" }),
    ("resolution", LabelDisposition::Owed { owner: "M3" }),
    ("resolves", LabelDisposition::Owed { owner: "M3" }),
    ("playbook", LabelDisposition::Owed { owner: "M5" }),
    ("working-set", LabelDisposition::Owed { owner: "M4" }),
    ("intention", LabelDisposition::Owed { owner: "M4" }),
    ("learned-constraint", LabelDisposition::Owed { owner: "M6" }),
    ("evidence", LabelDisposition::Owed { owner: "M1" }),
    ("justification", LabelDisposition::Owed { owner: "M1" }),
    ("support", LabelDisposition::Owed { owner: "M1" }),
    ("attack", LabelDisposition::Owed { owner: "M1" }),
    ("goal-hierarchy", LabelDisposition::Owed { owner: "M4" }),
    ("contextualizes", LabelDisposition::Owed { owner: "M6" }),
    ("pivot", LabelDisposition::Owed { owner: "M5" }),
    ("execution", LabelDisposition::Owed { owner: "M5" }),
    ("tool-invocation", LabelDisposition::Owed { owner: "M5" }),
];

/// The run context `gecko run` mints for `concept_id`.
fn run_ctx(concept_id: &str) -> RunContext {
    RunContext::new(
        ActorId::new("system"),
        ProvenanceSource::ExecutableDoc {
            concept_id: ConceptId::new(concept_id),
        },
        chrono::Utc::now(),
    )
}

/// The draft `mem.derive` / `mem.supersede` build from a payload.
fn guest_draft(ctx: &RunContext, text: &str, confidence: Option<f64>) -> BeliefDraft {
    BeliefDraft {
        text: text.to_string(),
        owner: ctx.actor.clone(),
        visibility: Visibility::Private,
        confidence,
        entrenchment: None,
    }
}

/// Raw belief-tier writes a hostile extension could send: a bare belief, and a
/// `technique-detection` edge from `rig_cyber` to T1059 claimed by `belief_id`.
fn hostile_writes(belief_id: &str) -> Vec<GraphWrite> {
    vec![
        GraphWrite::single(
            "given $id: string, $st: string; \
             insert $b isa belief, has concept-id == $id, has belief-state == $st;",
            vec!["id".to_string(), "st".to_string()],
            vec![
                GraphValue::from("mem/bel/hostile"),
                GraphValue::from("asserted"),
            ],
        )
        .expecting(1),
        GraphWrite::single(
            "given $lid: string, $tid: string, $bid: string; \
             match $lc isa concept, has concept-id == $lid; \
                   $t isa attack-pattern, has attack-id == $tid; \
                   $b isa belief, has concept-id == $bid; \
             insert technique-detection (detecting-logic: $lc, detected-technique: $t, \
                                         detection-claim: $b);",
            vec!["lid".to_string(), "tid".to_string(), "bid".to_string()],
            vec![
                GraphValue::from(CYBER_PLAYBOOK),
                GraphValue::from("T1059"),
                GraphValue::from(belief_id),
            ],
        )
        .expecting(1),
    ]
}

/// A writer-only drive on the reference database and the delta it caused.
struct Drive {
    name: &'static str,
    delta: Delta,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memwriter_is_the_sole_belief_tier_writer() {
    let rig_prod = TestDb::new("gecko_test_rig_prod");
    let rig_ref = TestDb::new("gecko_test_rig_ref");
    let rig = Rig::new();

    // Step 1: schema-init leaves the belief tier empty on both sides.
    gecko(&rig, &rig_prod, &["schema-init"]).await;
    gecko(&rig, &rig_ref, &["schema-init"]).await;
    let mut prod_fp = fingerprint(&rig_prod).await;
    for (db, fp) in [("prod", &prod_fp), ("ref", &fingerprint(&rig_ref).await)] {
        assert!(
            !fp.counts.is_empty(),
            "step 1 {db}: the fingerprint counts nothing"
        );
        let nonzero: Vec<_> = fp.counts.iter().filter(|(_, n)| **n != 0).collect();
        assert!(
            nonzero.is_empty(),
            "step 1 {db}: non-empty belief tier {nonzero:?}"
        );
    }

    let mut deltas: Vec<(String, Delta)> = Vec::new();
    let empty = Delta::new();

    // Steps 2-4: syncs never change the belief tier.
    let steps: [(&str, &[&[&str]]); 3] = [
        ("step 2 sync", &[&["sync", path_str(&rig.okf_dir)]]),
        ("step 3 sync-stix", &[&["sync-stix", path_str(&rig.stix)]]),
        (
            "step 4 re-sync",
            &[
                &["sync", path_str(&rig.okf_dir)],
                &["sync-stix", path_str(&rig.stix)],
            ],
        ),
    ];
    for (step, commands) in steps {
        for args in commands {
            gecko(&rig, &rig_prod, args).await;
        }
        let after = fingerprint(&rig_prod).await;
        let p = delta(&prod_fp, &after);
        assert_reconciled(step, &empty, &p);
        deltas.push((step.to_string(), p));
        prod_fp = after;
    }
    for id in [MEM_PLAYBOOK, CYBER_PLAYBOOK] {
        let n = count_with(
            &rig_prod,
            "given $v: string; match $c isa concept, has concept-id $id; $id == $v; \
             reduce $n = count;",
            id,
        )
        .await;
        assert_eq!(n, 1, "step 4: playbook {id} is not synced");
    }
    let techniques = count_with(
        &rig_prod,
        "given $v: string; match $t isa attack-pattern, has attack-id $a; $a == $v; \
         reduce $n = count;",
        "T1059",
    )
    .await;
    assert!(techniques >= 1, "step 4: no attack-pattern T1059");

    // The reference database: the same setup, then in-process writer calls only.
    setup(&rig, &rig_ref).await;
    let ref_store = Arc::new(RouterGraphStore::new(Arc::new(tokio::sync::Mutex::new(
        rig_ref.router(),
    ))));
    let embedder = Arc::new(StubEmbedder::new(384));
    let index = HnswIndex::open(rig.tmp.path().join("ref.hnsw"), embedder.model_id(), 384)
        .expect("open ref index");
    let registry = BeliefEdgeRegistry::from_extensions(
        CORE_SCHEMA,
        &[mem_gecko::extension(), cyber_gecko::extension()],
    )
    .expect("belief-edge registry");
    let ref_writer = MemWriter::new(ref_store, Some(embedder), Some(Arc::new(index)))
        .with_retrieval_provenance(true)
        .with_belief_edges(registry);
    ref_writer
        .rebuild_index_from_graph()
        .await
        .expect("rebuild ref index");
    let ref_writer = Arc::new(ref_writer);
    let mut ref_fp = fingerprint(&rig_ref).await;

    // Step 5: every guest mem write.
    let out = run_playbook(&rig, &rig_prod, MEM_PLAYBOOK).await;
    let recalled = out["recalled"].as_u64().expect("recalled count");
    assert!(recalled >= 1, "step 5: recall surfaced nothing: {out}");
    let after = fingerprint(&rig_prod).await;
    let p5 = delta(&prod_fp, &after);
    prod_fp = after;

    let ctx = run_ctx(MEM_PLAYBOOK);
    ref_writer
        .observe(
            &ctx,
            EpisodeDraft {
                text: T_EP.to_string(),
                event_time: ctx.occurred_at,
                ingest_time: ctx.occurred_at,
            },
        )
        .await
        .expect("ref observe");
    // Recall surfaces beliefs only, so b2 precedes it, as in `rig_mem`.
    let b2 = ref_writer
        .assert_belief(
            &ctx,
            guest_draft(&ctx, T_B2, None),
            &[],
            DerivationMethod::LlmSynthesis,
        )
        .await
        .expect("ref derive b2");
    let reader = ref_writer
        .clone()
        .as_epistemic_reader()
        .expect("mem writer is a reader");
    let chunks = reader
        .recall(
            &ctx,
            RecallQuery {
                text: T_Q.to_string(),
                as_of: None,
            },
            ContextBudget {
                max_chunks: 5,
                max_tokens: None,
            },
        )
        .await
        .expect("ref recall");
    assert_eq!(
        chunks.len() as u64,
        recalled,
        "step 5: the reference recall surfaced a different number of chunks"
    );
    let recalled_ids: Vec<MemId> = chunks.iter().map(|c| c.id.clone()).collect();
    let b1 = ref_writer
        .assert_belief(
            &ctx,
            guest_draft(&ctx, T_B1, Some(0.6)),
            &recalled_ids,
            DerivationMethod::LlmSynthesis,
        )
        .await
        .expect("ref derive b1");
    let s = ref_writer
        .supersede(&ctx, b1, guest_draft(&ctx, T_S, None), "revised")
        .await
        .expect("ref supersede");
    ref_writer
        .contest(&ctx, &[s.clone(), b2.clone()])
        .await
        .expect("ref contest");
    let after = fingerprint(&rig_ref).await;
    let r5 = delta(&ref_fp, &after);
    ref_fp = after;
    assert_reconciled("step 5 run rig_mem", &r5, &p5);

    // Step 6: cyber-gecko's belief-edge writes.
    let out = run_playbook(&rig, &rig_prod, CYBER_PLAYBOOK).await;
    assert!(
        out["claim"].is_string() && out["disposition"].is_string(),
        "step 6: unexpected output {out}"
    );
    let after = fingerprint(&rig_prod).await;
    let p6 = delta(&prod_fp, &after);
    prod_fp = after;

    let cyber_ctx = run_ctx(CYBER_PLAYBOOK);
    let cyber_edges = ref_writer
        .edge_writer_for("cyber-gecko")
        .expect("cyber-gecko edge writer");
    let confidence = 0.9_f64;
    cyber_edges
        .assert_belief_with_edges(
            &cyber_ctx,
            BeliefDraft {
                text: format!(
                    "detection '{CYBER_PLAYBOOK}' detects technique 'T1059' (confidence {confidence})"
                ),
                owner: cyber_ctx.actor.clone(),
                visibility: Visibility::Private,
                confidence: Some(confidence),
                entrenchment: None,
            },
            &[],
            DerivationMethod::ExternalTool,
            &[BeliefEdge {
                relation: "technique-detection".into(),
                players: vec![
                    ("detecting-logic".into(), GraphValue::from(CYBER_PLAYBOOK)),
                    ("detected-technique".into(), GraphValue::from("T1059")),
                ],
                attrs: vec![],
            }],
        )
        .await
        .expect("ref claim");
    cyber_edges
        .assert_belief_with_edges(
            &cyber_ctx,
            BeliefDraft {
                text: format!("disposition of detection '{CYBER_PLAYBOOK}': true-positive"),
                owner: cyber_ctx.actor.clone(),
                visibility: Visibility::Private,
                confidence: None,
                entrenchment: None,
            },
            &[],
            DerivationMethod::HumanAssertion,
            &[BeliefEdge {
                relation: "alert-disposition".into(),
                players: vec![(
                    "dispositioned-logic".into(),
                    GraphValue::from(CYBER_PLAYBOOK),
                )],
                attrs: vec![(
                    "detection-verdict".into(),
                    GraphValue::from("true-positive"),
                )],
            }],
        )
        .await
        .expect("ref disposition");
    let after = fingerprint(&rig_ref).await;
    let r6 = delta(&ref_fp, &after);
    ref_fp = after;
    assert_reconciled("step 6 run rig_cyber", &r6, &p6);

    // Step 7: status is a read.
    gecko(&rig, &rig_prod, &["status"]).await;
    let after = fingerprint(&rig_prod).await;
    let p7 = delta(&prod_fp, &after);
    assert_reconciled("step 7 status", &empty, &p7);

    let accounted: BTreeSet<String> = labels(&p5).union(&labels(&p6)).cloned().collect();
    deltas.push(("step 5 prod".into(), p5));
    deltas.push(("step 5 ref".into(), r5));
    deltas.push(("step 6 prod".into(), p6));
    deltas.push(("step 6 ref".into(), r6));
    deltas.push(("step 7 status".into(), p7));

    // Writer-only drives, on the reference database only, each its own delta.
    let mut drives: Vec<Drive> = Vec::new();
    macro_rules! drive {
        ($name:literal, $body:expr) => {{
            $body;
            let before = std::mem::replace(&mut ref_fp, fingerprint(&rig_ref).await);
            drives.push(Drive {
                name: $name,
                delta: delta(&before, &ref_fp),
            });
        }};
    }
    drive!(
        "rests_on",
        ref_writer
            .rests_on(&cyber_ctx, b2.clone(), std::slice::from_ref(&s))
            .await
            .expect("rests_on")
    );
    drive!(
        "record_prediction",
        ref_writer
            .record_prediction(&cyber_ctx, b2.clone(), 0.7)
            .await
            .expect("record_prediction")
    );
    drive!(
        "resolve_prediction",
        ref_writer
            .resolve_prediction(&cyber_ctx, b2.clone(), Outcome::Confirmed)
            .await
            .expect("resolve_prediction")
    );
    // Population tables are outside the belief tier, so this drive's delta is empty.
    drive!("populations", {
        assert!(
            ref_writer
                .define_population("rig-pop", "typeql-3", "match $c isa concept;")
                .await
                .expect("define_population"),
            "rig-pop already existed"
        );
        ref_writer
            .materialize_population(
                "rig-pop",
                &[ConceptId::new(MEM_PLAYBOOK), ConceptId::new(CYBER_PLAYBOOK)],
                chrono::Utc::now(),
            )
            .await
            .expect("materialize_population");
        assert!(
            ref_writer
                .close_population_membership(
                    "rig-pop",
                    &ConceptId::new(MEM_PLAYBOOK),
                    chrono::Utc::now(),
                )
                .await
                .expect("close_population_membership"),
            "rig-pop had no open row for {MEM_PLAYBOOK}"
        );
    });
    drive!("dedup", {
        let mut episodes = Vec::new();
        for _ in 0..2 {
            episodes.push(
                ref_writer
                    .observe(
                        &cyber_ctx,
                        EpisodeDraft {
                            text: T_EP.to_string(),
                            event_time: cyber_ctx.occurred_at,
                            ingest_time: cyber_ctx.occurred_at,
                        },
                    )
                    .await
                    .expect("observe duplicate"),
            );
        }
        ref_writer
            .dedup_episodes(&episodes[0], &episodes[1])
            .await
            .expect("dedup_episodes");
    });

    // The disposition ratchet over the production surface.
    let surface = {
        let mut router = rig_prod.router();
        let tx = router.begin_read().await.expect("read tx");
        belief_tier_surface(&tx, BELIEF_TIER_ROOT)
            .await
            .expect("production surface")
    };
    let surface: BTreeSet<String> = surface
        .entities
        .union(&surface.relations)
        .cloned()
        .collect();
    let table: BTreeMap<&str, &LabelDisposition> =
        DISPOSITIONS.iter().map(|(l, d)| (*l, d)).collect();
    assert_eq!(
        table.len(),
        DISPOSITIONS.len(),
        "duplicate disposition entry"
    );

    for label in &surface {
        let in_accounted = accounted.contains(label);
        let in_table = table.contains_key(label.as_str());
        assert!(
            in_accounted != in_table,
            "belief-tier label `{label}` must be in exactly one of accounted ({in_accounted}) \
             or the disposition table ({in_table})"
        );
    }
    for (label, disposition) in &table {
        assert!(
            surface.contains(*label),
            "disposition entry `{label}` is not on the belief-tier surface"
        );
        assert!(
            KNOWN_OWNERS.contains(&disposition.owner()),
            "disposition entry `{label}` names unknown owner `{}`",
            disposition.owner()
        );
    }
    for (label, disposition) in &table {
        if let LabelDisposition::WriterOnly { methods, .. } = disposition {
            assert!(
                drives
                    .iter()
                    .any(|d| methods.contains(&d.name) && labels(&d.delta).contains(*label)),
                "writer-only `{label}` is changed by none of its methods {methods:?}"
            );
        }
    }
    for drive in &drives {
        for label in labels(&drive.delta) {
            if accounted.contains(&label) {
                continue;
            }
            let named = matches!(
                table.get(label.as_str()),
                Some(LabelDisposition::WriterOnly { methods, .. }) if methods.contains(&drive.name)
            );
            assert!(
                named,
                "drive `{}` changed `{label}`, which is neither accounted nor a writer-only \
                 entry naming it: {:?}",
                drive.name, drive.delta
            );
        }
    }

    for (step, d) in deltas
        .iter()
        .map(|(s, d)| (s.as_str(), d))
        .chain(drives.iter().map(|d| (d.name, &d.delta)))
    {
        for label in labels(d) {
            assert!(
                !matches!(
                    table.get(label.as_str()),
                    Some(LabelDisposition::Owed { .. })
                ),
                "{step} changed owed label `{label}`: {d:?}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hostile_extension_raw_belief_write_is_flagged() {
    let db = TestDb::new("gecko_test_rig_hostile");
    let rig = Rig::new();
    setup(&rig, &db).await;
    let out = run_playbook(&rig, &db, CYBER_PLAYBOOK).await;
    let belief_id = out["claim"].as_str().expect("claim belief id").to_string();
    let disposition_id = out["disposition"]
        .as_str()
        .expect("disposition belief id")
        .to_string();
    let f0 = fingerprint(&db).await;
    let (manifest, typed_rels) = to_okf(STIX_BUNDLE).expect("STIX bundle converts");

    // Raw belief and belief-edge inserts alongside the real extension writes.
    let writes = [cyber_post_sync(&typed_rels), hostile_writes(&belief_id)].concat();
    let refused = match sync_bundle(&mut db.router(), &manifest, &writes, BELIEF_TIER_ROOT).await {
        Err(DbError::BeliefTierWrite { changes }) => changes,
        other => panic!("raw belief writes: expected BeliefTierWrite, got {other:?}"),
    };
    let guard_labels: BTreeSet<String> = refused.iter().map(|c| c.label.clone()).collect();
    assert_eq!(
        guard_labels,
        ["memory-item", "belief", "technique-detection"]
            .into_iter()
            .map(String::from)
            .collect::<BTreeSet<_>>(),
        "raw belief writes: refused changes {refused:?}"
    );
    assert_eq!(fingerprint(&db).await, f0, "raw belief writes committed");

    // An attribute added to an existing belief, with no new instance.
    let confidence = GraphWrite::single(
        "given $bid: string, $c: double; \
         match $b isa belief, has concept-id $bc; $bc == $bid; \
               not { $b has confidence $x; }; \
         insert $b has confidence == $c;",
        vec!["bid".to_string(), "c".to_string()],
        vec![GraphValue::from(disposition_id), GraphValue::Double(0.1)],
    )
    .expecting(1);
    let refused = match sync_bundle(
        &mut db.router(),
        &manifest,
        std::slice::from_ref(&confidence),
        BELIEF_TIER_ROOT,
    )
    .await
    {
        Err(DbError::BeliefTierWrite { changes }) => changes,
        other => panic!("attribute-only write: expected BeliefTierWrite, got {other:?}"),
    };
    let refused: BTreeSet<(String, BeliefTierMeasure)> = refused
        .iter()
        .map(|c| (c.label.clone(), c.measure))
        .collect();
    assert_eq!(
        refused,
        [
            ("memory-item".to_string(), BeliefTierMeasure::Attributes),
            ("belief".to_string(), BeliefTierMeasure::Attributes),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>(),
        "attribute-only write: refused changes"
    );
    assert_eq!(fingerprint(&db).await, f0, "attribute-only write committed");

    // An importer concept typed as a belief-tier entity.
    let mut imported = manifest.clone();
    let mut hostile = imported.concepts[0].clone();
    hostile.concept_id = "hostile/working-set".to_string();
    hostile.type_hint = Some("working-set".into());
    hostile.typed_attributes = Vec::new();
    hostile.file_hash = "hostile-working-set".to_string();
    imported.concepts.push(hostile);
    match sync_bundle(&mut db.router(), &imported, &[], BELIEF_TIER_ROOT).await {
        Err(DbError::Schema(m)) => assert!(
            m.contains("working-set"),
            "importer type_hint: error does not name working-set: {m}"
        ),
        other => panic!("importer type_hint: expected a schema error, got {other:?}"),
    }
    assert_eq!(fingerprint(&db).await, f0, "importer type_hint committed");

    // The same raw writes sent around the guard, reconciled against no expected
    // writes, surface exactly the labels the guard refused.
    {
        let mut router = db.router();
        let tx = router.begin_write().await.expect("write tx");
        run_graph_writes(&tx, &hostile_writes(&belief_id))
            .await
            .expect("raw belief writes outside a sync");
        tx.commit().await.expect("commit raw belief writes");
    }
    assert_eq!(
        unaccounted(&Delta::new(), &delta(&f0, &fingerprint(&db).await)),
        guard_labels,
        "the rig does not report the labels the guard refused"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gc_of_a_concept_a_belief_edge_references_is_refused() {
    let db = TestDb::new("gecko_test_rig_gc");
    let rig = Rig::new();
    setup(&rig, &db).await;
    run_playbook(&rig, &db, CYBER_PLAYBOOK).await;
    let f0 = fingerprint(&db).await;

    let without_cyber = parse_bundle(&rig.okf_dir_without_cyber).expect("parse");
    let references =
        match sync_bundle(&mut db.router(), &without_cyber, &[], BELIEF_TIER_ROOT).await {
            Err(DbError::BeliefEdgeReferencesRemovedConcept { references }) => references,
            other => panic!("expected BeliefEdgeReferencesRemovedConcept, got {other:?}"),
        };
    for relation in ["technique-detection", "alert-disposition"] {
        assert!(
            references.contains(&(CYBER_PLAYBOOK.to_string(), relation.to_string())),
            "references {references:?} miss ({CYBER_PLAYBOOK}, {relation})"
        );
    }
    assert_eq!(
        fingerprint(&db).await,
        f0,
        "the refused GC changed the belief tier"
    );
    let concept_count = "given $v: string; match $c isa concept, has concept-id $id; $id == $v; \
                         reduce $n = count;";
    assert_eq!(
        count_with(&db, concept_count, CYBER_PLAYBOOK).await,
        1,
        "the refused GC removed {CYBER_PLAYBOOK}"
    );

    let output =
        gecko_expect_failure(&rig, &db, &["sync", path_str(&rig.okf_dir_without_cyber)]).await;
    assert!(
        output.contains(CYBER_PLAYBOOK),
        "gecko sync output does not name {CYBER_PLAYBOOK}:\n{output}"
    );
    assert!(
        output.contains("concept(s) a belief edge references"),
        "gecko sync output is not the belief-edge GC refusal:\n{output}"
    );
    assert_eq!(
        fingerprint(&db).await,
        f0,
        "the binary's refused GC changed the belief tier"
    );
    assert_eq!(
        count_with(&db, concept_count, CYBER_PLAYBOOK).await,
        1,
        "the binary's refused GC removed {CYBER_PLAYBOOK}"
    );
}
