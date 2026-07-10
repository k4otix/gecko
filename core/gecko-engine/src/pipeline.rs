//! The execution and contextual pipeline (design §5).
//!
//! Orchestrates the lifecycle of a playbook execution:
//! trigger → handle allocation → sandbox execution → record → RAII cleanup.
//!
//! This is the single execution path. Callers (`gecko run`, and future bundle- or
//! trigger-driven runs) describe *what* to run with a [`PlaybookRun`] and hand it
//! here; the pipeline owns the state-handle lifecycle, the shared sandbox pool, and
//! the record hook.

use tracing::{error, info, warn};
use uuid::Uuid;

use crate::db::router::{DbError, TypeDbRouter};
use crate::okf::types::OkfConcept;
use crate::sandbox::engine::{ExecutionResult, ExtensionCallback, HostImports};
use crate::sandbox::wasm_pool::SandboxPool;
use crate::state::registry::StateRegistry;

/// A source-agnostic description of the program to execute.
///
/// Built either from a parsed [`OkfConcept`] (a bundle/in-memory run, via
/// [`PlaybookRun::from_concept`]) or reconstructed from the knowledge graph (the
/// `gecko run <concept-id>` path). Holding only the executable essentials keeps the
/// pipeline independent of how the concept was obtained.
#[derive(Debug, Clone, Copy)]
pub struct PlaybookRun<'a> {
    /// The concept's ID (for logging and the execution record).
    pub concept_id: &'a str,
    /// The single concatenated program to run (one program per concept).
    pub program: &'a str,
    /// Granted host-capability scopes; the sandbox default-denies ungranted calls (S3).
    pub scopes: &'a [String],
    /// Optional execution timeout; the sandbox applies its default when `None`.
    pub timeout_ms: Option<u64>,
}

impl<'a> PlaybookRun<'a> {
    /// Borrows the executable parts of a parsed concept. Returns `None` when the
    /// concept has no program (it is documentation, not executable).
    pub fn from_concept(concept: &'a OkfConcept) -> Option<Self> {
        Some(Self {
            concept_id: &concept.concept_id,
            program: concept.program.as_deref()?,
            scopes: &concept.scopes,
            timeout_ms: concept.timeout_ms,
        })
    }
}

/// Result of a pipeline execution.
#[derive(Debug)]
pub struct PipelineResult {
    /// The handle ID used for this execution.
    pub handle_id: Uuid,
    /// The sandbox execution result.
    pub execution: ExecutionResult,
    /// Whether the execution was successfully recorded/logged by
    /// [`record_execution`]. This is a logging outcome, not database durability:
    /// the current `record_execution` only logs, so `true` means the log call
    /// succeeded, not that anything was committed to TypeDB.
    pub recorded: bool,
}

/// Errors during pipeline execution.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("Database error: {0}")]
    Db(#[from] DbError),

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
}

/// Executes a playbook through the pipeline (design §5).
///
/// 1. Trigger (caller supplies the [`PlaybookRun`])
/// 2. Handle allocation from the [`StateRegistry`]
/// 3. Sandbox instantiation + execution in the one WASM boundary
/// 4. Record the execution (logged on success; skipped on failure)
/// 5. RAII cleanup (automatic when the state handle drops)
///
/// All synced code is untrusted and runs in the WASM sandbox; the run's granted
/// `scopes` gate host capabilities (S3). Returns the execution result even when the
/// script itself failed — inspect [`PipelineResult::execution`].
pub async fn execute_playbook(
    run: &PlaybookRun<'_>,
    db: &mut TypeDbRouter,
    state_registry: &StateRegistry,
    sandbox_pool: &SandboxPool,
    host_imports: &HostImports,
    extension_callback: Option<ExtensionCallback>,
) -> Result<PipelineResult, PipelineError> {
    info!(concept_id = %run.concept_id, "Pipeline: executing playbook");

    // Step 2: allocate a state handle (RAII-cleaned when it drops below).
    let handle = state_registry.allocate();
    let handle_id = handle.id;
    info!(handle = %handle_id, "Pipeline: state handle allocated");

    // Step 3: sandbox execution. The extension callback bridges permitted host
    // capability calls; the sandbox enforces the run's scopes (S3).
    let execution = sandbox_pool
        .execute(
            run.program,
            handle_id,
            host_imports,
            run.scopes,
            run.timeout_ms,
            extension_callback,
        )
        .await;

    // Step 4: record on success; skip recording on failure.
    let recorded = if execution.success {
        match record_execution(db, run.concept_id, &execution) {
            Ok(()) => {
                info!(concept_id = %run.concept_id, "Pipeline: execution recorded");
                true
            }
            Err(e) => {
                warn!(error = %e, "Pipeline: failed to record execution, but script succeeded");
                false
            }
        }
    } else {
        error!(
            concept_id = %run.concept_id,
            error = execution.error.as_deref().unwrap_or("unknown"),
            "Pipeline: execution failed, not recording"
        );
        false
    };

    // Step 5: `handle` drops here, triggering StateHandleGuard::drop() (RAII).
    Ok(PipelineResult {
        handle_id,
        execution,
        recorded,
    })
}

/// Records a successful execution event.
///
/// For now this logs the execution; full mem-gecko episode generation (writing a
/// contextualizing relation into the graph) lands when that extension is wired in.
fn record_execution(
    _db: &mut TypeDbRouter,
    concept_id: &str,
    result: &ExecutionResult,
) -> Result<(), DbError> {
    info!(
        concept_id,
        duration_ms = result.duration_ms,
        engine = ?result.engine,
        "Execution recorded"
    );
    Ok(())
}
