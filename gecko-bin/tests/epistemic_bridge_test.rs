//! A4 acceptance #2 — the activation hook delivers the writer, and the mem host-fn
//! bridge closes A1 acceptance bullet 3 end-to-end at the dispatch level.
//!
//! Two properties against a live graph (`localhost:1729`, admin/password):
//!
//! 1. **mem through the real bridge.** The engine's
//!    [`epistemic_extension_callback`] — the SAME [`ExtensionCallback`] `cmd_run`
//!    wires into the wasm pipeline — is invoked exactly as the sandbox host bridge
//!    would call it (`cb("mem", "remember", args)`). It produces a run-stamped
//!    episode in the graph, and a `run_id` the sandbox tries to smuggle in the
//!    payload is **ignored** in favour of the host-minted one (invariant 2 / A1
//!    bullet 3: the sandbox cannot forge or omit `run_id`).
//! 2. **The hook delivers a working writer to a domain extension.** A test-double
//!    domain extension's [`inject_epistemic_host_fns`] override receives the injected
//!    [`EpistemicWriter`]; using it, `assert_belief` produces a run-stamped belief.
//!
//! [`epistemic_extension_callback`]: gecko_engine::sandbox::mem_host::epistemic_extension_callback
//! [`ExtensionCallback`]: gecko_engine::sandbox::engine::ExtensionCallback
//! [`inject_epistemic_host_fns`]: gecko_extension_api::GeckoExtension::inject_epistemic_host_fns

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use futures_util::StreamExt;
use serde_json::json;
use tokio::sync::Mutex;

use gecko_engine::db::RouterGraphStore;
use gecko_engine::db::router::{DbConfig, TlsMode, TypeDbRouter};
use gecko_engine::sandbox::engine::ExtensionCallback;
use gecko_engine::sandbox::mem_host::epistemic_extension_callback;
use gecko_extension_api::{
    ActorId, BeliefDraft, ConceptId, DerivationMethod, EpistemicWriter, GeckoExtension,
    HostImportDef, ProvenanceSource, RunContext, SandboxCtx, Visibility,
};
use mem_gecko::{MemGecko, MemWriter};
use uuid::Uuid;

const CORE_SCHEMA: &str = include_str!("../../core/gecko-engine/schema/core_schema.tql");

/// A live throwaway database + a shared router, schema applied. Drops itself.
struct Fixture {
    shared: Arc<Mutex<TypeDbRouter>>,
    name: String,
}

impl Fixture {
    async fn new() -> Self {
        let name = format!("a4test_{}", Uuid::new_v4().simple());
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

    /// mem's writer over the shared router, no index (the A5-disabled shape).
    fn writer(&self) -> Arc<dyn EpistemicWriter> {
        Arc::new(MemWriter::without_index(Arc::new(RouterGraphStore::new(
            self.shared.clone(),
        ))))
    }

    /// The `run-id` stamped on the episode `concept_id`'s `source-link` origin.
    async fn episode_run_id(&self, concept_id: &str) -> Option<String> {
        let mut router = self.shared.lock().await;
        let tx = router.begin_read().await.expect("read tx");
        let query = format!(
            r#"match $ep isa episode, has concept-id "{concept_id}";
                     (memory: $ep, origin: $run) isa source-link;
                     $run isa doc-run, has run-id $rid;
               fetch {{ "run": $rid }};"#
        );
        let answer = tx.query(&query).await.expect("run-id query");
        let mut out = None;
        if answer.is_document_stream() {
            let mut docs = answer.into_documents();
            if let Some(Ok(doc)) = docs.next().await {
                let json: serde_json::Value =
                    serde_json::from_str(&doc.into_json().to_string()).unwrap();
                out = json.get("run").and_then(|v| v.as_str()).map(str::to_string);
            }
        }
        out
    }

    /// The `run-id` stamped on belief `concept_id`.
    async fn belief_run_id(&self, concept_id: &str) -> Option<String> {
        let mut router = self.shared.lock().await;
        let tx = router.begin_read().await.expect("read tx");
        let query = format!(
            r#"match $b isa belief, has concept-id "{concept_id}";
                     (memory: $b, origin: $run) isa source-link;
                     $run isa doc-run, has run-id $rid;
               fetch {{ "run": $rid }};"#
        );
        let answer = tx.query(&query).await.expect("run-id query");
        let mut out = None;
        if answer.is_document_stream() {
            let mut docs = answer.into_documents();
            if let Some(Ok(doc)) = docs.next().await {
                let json: serde_json::Value =
                    serde_json::from_str(&doc.into_json().to_string()).unwrap();
                out = json.get("run").and_then(|v| v.as_str()).map(str::to_string);
            }
        }
        out
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

/// A domain extension test-double: captures the writer the A4.1 hook delivers.
#[derive(Default)]
struct DomainDouble {
    received: StdMutex<Option<Arc<dyn EpistemicWriter>>>,
}

impl GeckoExtension for DomainDouble {
    fn name(&self) -> &str {
        "domain-double"
    }
    fn schema(&self) -> &str {
        ""
    }
    fn host_imports(&self) -> Vec<HostImportDef> {
        vec![]
    }
    fn inject_epistemic_host_fns(&self, _ctx: &mut SandboxCtx, writer: Arc<dyn EpistemicWriter>) {
        *self.received.lock().unwrap() = Some(writer);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mem_host_fn_through_bridge_is_run_stamped_and_ignores_forged_run_id() {
    let fx = Fixture::new().await;

    // Activation: build mem's writer, run the injection hooks over the activated set
    // (mem forced-on). mem binds the writer into the sandbox context.
    let writer = fx.writer();
    let mut ctx = SandboxCtx::new();
    let mem = MemGecko::new();
    mem.inject_epistemic_host_fns(&mut ctx, writer.clone());
    assert!(
        ctx.has_epistemic_writer(),
        "mem's hook must bind the writer into the sandbox context"
    );

    // The host mints exactly one RunContext for the run, bound to the exec-doc.
    let run_ctx = RunContext::new(
        ActorId::new("system"),
        ProvenanceSource::ExecutableDoc {
            concept_id: ConceptId::new("playbooks/hello"),
        },
        chrono::Utc::now(),
    );
    let host_run_id = run_ctx.run_id.to_string();
    let forged_run_id = gecko_extension_api::RunId::new().to_string();
    assert_ne!(host_run_id, forged_run_id);

    // Build the SAME bridge cmd_run wires into the pipeline.
    let base: ExtensionCallback = Arc::new(|_e, _f, _a| Err("base: no such extension".to_string()));
    let writer = ctx.epistemic_writer().unwrap();
    let reader = writer.clone().as_epistemic_reader();
    let cb = epistemic_extension_callback(run_ctx, writer, reader, base);

    // Invoke it exactly as the wasm host bridge would — including a smuggled run_id.
    let out = cb(
        "mem",
        "remember",
        json!({
            "text": "observed a suspicious login",
            "run_id": forged_run_id,
            "actor": "attacker",
            "source": "Manual"
        }),
    )
    .expect("mem.remember dispatches through the bridge");

    let episode_id = out
        .get("id")
        .and_then(|v| v.as_str())
        .expect("bridge returns the new episode id")
        .to_string();
    assert!(episode_id.starts_with("mem/ep/"), "got {episode_id}");

    // The stored episode carries the HOST's run id — never the forged one.
    let stored = fx
        .episode_run_id(&episode_id)
        .await
        .expect("episode has a run stamp");
    assert_eq!(
        stored, host_run_id,
        "episode must carry the host-minted run_id"
    );
    assert_ne!(
        stored, forged_run_id,
        "the sandbox-smuggled run_id must be ignored (invariant 2)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hook_delivers_working_writer_to_domain_extension() {
    let fx = Fixture::new().await;
    let writer = fx.writer();

    // The domain extension receives the writer through the A4.1 hook.
    let domain = DomainDouble::default();
    let mut ctx = SandboxCtx::new();
    domain.inject_epistemic_host_fns(&mut ctx, writer.clone());

    let received = domain
        .received
        .lock()
        .unwrap()
        .clone()
        .expect("the hook must deliver the writer to the domain extension");

    // Using the writer it received, a domain host-fn reaches assert_belief and
    // produces a run-stamped belief in the live graph.
    let run_ctx = RunContext::new(
        ActorId::new("agent-7"),
        ProvenanceSource::ExecutableDoc {
            concept_id: ConceptId::new("playbooks/domain"),
        },
        chrono::Utc::now(),
    );
    let belief_id = received
        .assert_belief(
            &run_ctx,
            BeliefDraft {
                text: "the domain concluded X".to_string(),
                owner: run_ctx.actor.clone(),
                visibility: Visibility::Private,
                confidence: Some(0.8),
                entrenchment: None,
            },
            &[],
            DerivationMethod::LlmSynthesis,
        )
        .await
        .expect("domain writer.assert_belief succeeds");
    assert!(belief_id.0.starts_with("mem/bel/"));

    let stored = fx
        .belief_run_id(&belief_id.0)
        .await
        .expect("belief has a run stamp");
    assert_eq!(
        stored,
        run_ctx.run_id.to_string(),
        "belief must carry the host-minted run_id"
    );
}
