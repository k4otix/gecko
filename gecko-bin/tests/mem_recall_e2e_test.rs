//! Live end-to-end: the guest `mem.*` surface driven through the REAL wasm
//! sandbox against live TypeDB (`localhost:1729`, admin/password).
//!
//! This is the first exercise of the guest namespace against a live DB. A real
//! guest JS program calls `mem.recall`, maps the surfaced chunk ids into
//! `mem.derive`, and we assert the retrieval-provenance correlation actually
//! fired in the graph: the synthesized belief carries `retrieval-provenance =
//! "semantic"`, an `informs-synthesis` edge ties it to the minted
//! `retrieval-event`, and the `surfaced` link over the recalled evidence is
//! `was-used = true`.
//!
//! The whole recall→derive pair shares ONE host-minted `RunContext` (the "gecko
//! run" identity) — that shared per-run scratch is what makes the correlation
//! cross the two mem calls. The wiring mirrors `gecko run` (`gecko-bin/src/main.rs`).
//!
//! Every test provisions a throwaway, self-deleting database + a temp index file
//! and cleans both up. The real `gecko` database is never touched.

use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::Mutex;
use uuid::Uuid;

use gecko_engine::db::RouterGraphStore;
use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_engine::sandbox::engine::{ExtensionCallback, HostImports};
use gecko_engine::sandbox::mem_host::epistemic_extension_callback;
use gecko_engine::sandbox::wasm_executor::WasmExecutor;
use gecko_extension_api::{
    ActorId, BeliefDraft, ConceptId, DateTime, DerivationMethod, Embedder, EpistemicWriter,
    GeckoExtension, ProvenanceSource, RunContext, Visibility,
};
use gecko_semantic_index::{HnswIndex, StubEmbedder};
use mem_gecko::{MemGecko, MemWriter};

const CORE_SCHEMA: &str = include_str!("../../core/gecko-engine/schema/core_schema.tql");
const DIM: usize = 64;

/// The real guest program: recall → derive over the recalled chunk ids. Returns
/// how many chunks surfaced and the synthesized belief id — proving the whole
/// guest↔host round-trip, not just a host-side dispatch.
const RECALL_DERIVE_PROGRAM: &str = r#"
    const chunks = mem.recall("lateral movement", { maxChunks: 10 });
    const b = mem.derive("host is compromised via smb", chunks.map(c => c.id));
    ({ recalled: chunks.length, belief: b.id })
"#;

/// A live throwaway database + a shared router, schema applied. Drops itself.
struct Fixture {
    shared: Arc<Mutex<TypeDbRouter>>,
    name: String,
}

impl Fixture {
    async fn new() -> Self {
        let name = format!("memc_e2e_{}", Uuid::new_v4().simple());
        let mut router = TypeDbRouter::new(DbConfig {
            address: "localhost:1729".to_string(),
            database: name.clone(),
            username: "admin".to_string(),
            password: "password".to_string(),
            tls: TlsMode::Disabled,
        });
        router.apply_schema(CORE_SCHEMA).await.expect("core schema");
        router
            .apply_schema(MemGecko::new().schema())
            .await
            .expect("mem schema");
        Fixture {
            shared: Arc::new(Mutex::new(router)),
            name,
        }
    }

    fn store(&self) -> Arc<RouterGraphStore> {
        Arc::new(RouterGraphStore::new(self.shared.clone()))
    }

    async fn raw_fetch(&self, tql: &str) -> Vec<serde_json::Value> {
        let mut router = self.shared.lock().await;
        let tx = router.begin_read().await.expect("read tx");
        let answer = tx.query(tql).await.expect("read query");
        let mut out = Vec::new();
        if answer.is_document_stream() {
            let mut stream = answer.into_documents();
            while let Some(Ok(doc)) = stream.next().await {
                out.push(serde_json::from_str(&doc.into_json().to_string()).unwrap());
            }
        }
        out
    }

    async fn drop_db(&self) {
        let mut router = self.shared.lock().await;
        // `raw_fetch`'s read transactions close asynchronously when dropped; on a
        // networked CI TypeDB that close can lag this immediate delete, yielding a
        // transient "[DBD2] ... database is in use". Retry until the server releases
        // the transaction. The `Drop` impl (a fresh connection) is the final
        // fallback and db names are unique, so a leak on persistent failure is
        // harmless — never fail an otherwise-passing test on cleanup.
        for attempt in 0..30 {
            match router.delete_database(&self.name).await {
                Ok(()) => return,
                Err(e) if e.to_string().contains("in use") => {
                    if attempt == 29 {
                        eprintln!(
                            "drop_db: '{}' still in use after retries; leaving it to the Drop fallback",
                            self.name
                        );
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Err(e) => panic!("drop db: {e}"),
            }
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let name = self.name.clone();
        let _ = std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Runtime::new() {
                rt.block_on(async move {
                    let mut db = TypeDbRouter::new(DbConfig {
                        address: "localhost:1729".to_string(),
                        database: name.clone(),
                        username: "admin".to_string(),
                        password: "password".to_string(),
                        tls: TlsMode::Disabled,
                    });
                    let _ = db.delete_database(&name).await;
                });
            }
        })
        .join();
    }
}

fn tmp_index_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("gecko-e2e-{}.hnsw", Uuid::new_v4()))
}

fn dt(s: &str) -> DateTime {
    chrono::DateTime::parse_from_rfc3339(s)
        .unwrap()
        .with_timezone(&chrono::Utc)
}

fn belief(text: &str, owner: &str, conf: f64) -> BeliefDraft {
    BeliefDraft {
        text: text.to_string(),
        owner: ActorId::new(owner),
        visibility: Visibility::Private,
        confidence: Some(conf),
        entrenchment: None,
    }
}

/// Build the index-enabled, provenance-ON writer — the shape that mints
/// retrieval-events. Returns the writer plus its temp index path (for cleanup).
fn provenance_writer(fx: &Fixture) -> (MemWriter, std::path::PathBuf) {
    let path = tmp_index_path();
    let embedder: Arc<dyn Embedder> = Arc::new(StubEmbedder::new(DIM));
    let index = Arc::new(HnswIndex::open(&path, embedder.model_id(), DIM).unwrap());
    let writer =
        MemWriter::new(fx.store(), Some(embedder), Some(index)).with_retrieval_provenance(true);
    (writer, path)
}

/// The SAME bridge `gecko run` wires (`gecko-bin/src/main.rs`): the mem writer is
/// also the reader, sharing one per-run scratch. `run_ctx` is moved in and is the
/// single per-run identity every mem call in the program observes.
fn run_callback(run_ctx: RunContext, writer: Arc<dyn EpistemicWriter>) -> ExtensionCallback {
    let base: ExtensionCallback = Arc::new(|_e, _f, _a| Err("base: no such extension".to_string()));
    let reader = writer.clone().as_epistemic_reader();
    epistemic_extension_callback(run_ctx, writer, reader, base)
}

// ── Positive: live retrieval-provenance correlation through the sandbox ──────
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recall_then_derive_records_correlation() {
    let fx = Fixture::new().await;
    let (mem_writer, index_path) = provenance_writer(&fx);

    // Seed a prior belief the recall will surface — asserted DIRECTLY on the writer,
    // earlier, as the SAME actor the run uses. The exact "smb lateral movement
    // observed" text is the proven pair the stub embedder surfaces for the query
    // "lateral movement" below.
    let evidence = mem_writer
        .assert_belief(
            &RunContext::new(
                ActorId::new("agent-1"),
                ProvenanceSource::Manual,
                dt("2026-01-01T00:00:00Z"),
            ),
            belief("smb lateral movement observed", "agent-1", 0.9),
            &[],
            DerivationMethod::HumanAssertion,
        )
        .await
        .unwrap();

    // The ONE per-run context — the "gecko run" identity, SAME actor as the seed.
    let run_ctx = RunContext::new(
        ActorId::new("agent-1"),
        ProvenanceSource::ExecutableDoc {
            concept_id: ConceptId::new("playbooks/mem-recall-demo"),
        },
        dt("2026-01-02T00:00:00Z"),
    );

    let writer: Arc<dyn EpistemicWriter> = Arc::new(mem_writer);
    let cb = run_callback(run_ctx, writer);

    // Run the REAL guest program in the sandbox with recall + derive scopes granted.
    let executor = WasmExecutor::new().unwrap();
    let scopes = vec!["mem:recall".to_string(), "mem:derive".to_string()];
    let result = executor
        .evaluate(
            RECALL_DERIVE_PROGRAM,
            Uuid::new_v4(),
            &HostImports::default(),
            &scopes,
            Some(30_000),
            Some(cb),
        )
        .await;

    // The whole guest↔host round-trip succeeded.
    assert!(result.success, "guest run failed: {:?}", result.error);
    let recalled = result.output["recalled"].as_u64().unwrap_or(0);
    assert!(
        recalled >= 1,
        "mem.recall surfaced no chunks to JS (got {recalled}); output={:?}",
        result.output
    );
    let synth_id = result.output["belief"]
        .as_str()
        .expect("mem.derive returned the synthesized belief id to JS");

    // Graph state — exactly one semantic retrieval-event was minted.
    let events = fx
        .raw_fetch(r#"match $re isa retrieval-event, has retrieval-method $m; fetch { "m": $m };"#)
        .await;
    assert_eq!(
        events.len(),
        1,
        "exactly one semantic retrieval-event recorded, got {events:?}"
    );
    assert_eq!(events[0]["m"], "semantic");

    // The synthesized belief carries retrieval-provenance = "semantic".
    let prov = fx
        .raw_fetch(&format!(
            r#"match $b isa belief, has concept-id "{synth_id}", has retrieval-provenance $p; fetch {{ "p": $p }};"#
        ))
        .await;
    assert_eq!(prov.len(), 1, "synthesized belief has the retrieval axis");
    assert_eq!(prov[0]["p"], "semantic");

    // …and an informs-synthesis edge back to the retrieval-event, whose surfaced
    // link over the recalled evidence is flagged was-used = true.
    let link = fx
        .raw_fetch(&format!(
            r#"match
                 $b isa belief, has concept-id "{synth_id}";
                 $re isa retrieval-event;
                 (retrieval: $re, synthesized: $b) isa informs-synthesis;
                 (surfacer: $re, item: $ev) isa surfaced, has was-used $u;
                 $ev has concept-id "{}";
               fetch {{ "u": $u }};"#,
            evidence.0
        ))
        .await;
    assert_eq!(
        link.len(),
        1,
        "informs-synthesis edge ties belief ↔ retrieval-event over the surfaced evidence"
    );
    assert_eq!(
        link[0]["u"], true,
        "the surfaced evidence is flagged was-used"
    );

    std::fs::remove_file(&index_path).ok();
    fx.drop_db().await;
}

// ── Negative: S3 default-deny rejects un-scoped mem.recall before the callback ──
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unscoped_recall_is_denied_by_s3_gate() {
    // A raw callback that panics if invoked — this pins the guarantee that the S3
    // default-deny gate rejects the un-scoped mem.recall BEFORE any host dispatch.
    // No writer/reader/index is wired: if the gate were broken the callback would
    // fire and panic, failing the test loudly, which is exactly the point.
    let cb: gecko_engine::sandbox::engine::ExtensionCallback = std::sync::Arc::new(|_e, _f, _a| {
        panic!("host callback must not run when the scope is denied")
    });

    // Grant ONLY mem:derive — the program's first call, mem.recall, is un-scoped.
    let executor = WasmExecutor::new().unwrap();
    let scopes = vec!["mem:derive".to_string()];
    let result = executor
        .evaluate(
            RECALL_DERIVE_PROGRAM,
            Uuid::new_v4(),
            &HostImports::default(),
            &scopes,
            Some(30_000),
            Some(cb),
        )
        .await;

    assert!(
        !result.success,
        "un-scoped mem.recall must be rejected, but the run succeeded: {:?}",
        result.output
    );
    let err = result.error.unwrap_or_default();
    assert!(
        err.contains("not granted"),
        "S3 gate should explain the scope denial, got: {err}"
    );
}
