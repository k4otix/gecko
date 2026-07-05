//! The 8-step execution and contextual pipeline (design §5).
//!
//! Orchestrates the full lifecycle of a playbook execution:
//! trigger → handle allocation → graph resolution → transaction →
//! sandbox → execution → commit/rollback → cleanup.

use tracing::{error, info, warn};
use uuid::Uuid;

use crate::db::router::{DbError, TypeDbRouter};
use crate::okf::types::OkfConcept;
use crate::sandbox::engine::ExecutionResult;
use crate::sandbox::engine::HostImports;
use crate::sandbox::wasm_pool::SandboxPool;
use crate::state::registry::StateRegistry;

/// Result of a pipeline execution.
#[derive(Debug)]
pub struct PipelineResult {
    /// The handle ID used for this execution.
    pub handle_id: Uuid,
    /// The sandbox execution result.
    pub execution: ExecutionResult,
    /// Whether the TypeDB transaction was committed.
    pub committed: bool,
}

/// Errors during pipeline execution.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("Concept '{0}' has no code blocks to execute")]
    NoCodeBlocks(String),

    #[error("Concept '{0}' has no engine specified in frontmatter")]
    NoEngine(String),

    #[error("Database error: {0}")]
    Db(#[from] DbError),

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
}

/// Executes a playbook concept through the full 8-step pipeline.
///
/// Design §5:
/// 1. Trigger (caller provides the concept)
/// 2. Handle allocation from StateRegistry
/// 3. Graph resolution (concept already resolved by caller)
/// 4. Transaction start
/// 5. Sandbox instantiation
/// 6. Script execution
/// 7. Commit/rollback + optional episode generation
/// 8. RAII cleanup (automatic via StateHandleGuard drop)
pub async fn execute_playbook(
    concept: &OkfConcept,
    db: &mut TypeDbRouter,
    state_registry: &StateRegistry,
    sandbox_pool: &SandboxPool,
    host_imports: &HostImports,
) -> Result<PipelineResult, PipelineError> {
    // Step 1: Trigger (concept provided by caller)
    info!(concept_id = %concept.concept_id, "Pipeline: executing playbook");

    // Validate that the concept has executable content
    if concept.code_blocks.is_empty() {
        return Err(PipelineError::NoCodeBlocks(concept.concept_id.clone()));
    }

    let engine = concept
        .engine
        .as_ref()
        .ok_or_else(|| PipelineError::NoEngine(concept.concept_id.clone()))?;

    // Step 2: Handle allocation
    let handle = state_registry.allocate().await;
    let handle_id = handle.id;
    info!(handle = %handle_id, "Pipeline: state handle allocated");

    // Step 3: Graph resolution (already done — concept is provided)

    // Step 4 + 5 + 6: Transaction + sandbox + execution
    let code = concept.code_blocks.join("\n");
    let timeout_ms = concept.timeout_ms;

    let execution = sandbox_pool.execute(engine, &code, handle_id, host_imports, timeout_ms, None);

    // Step 7: Commit or rollback
    let committed = if execution.success {
        // Attempt to record execution in TypeDB
        match record_execution(db, concept, &execution) {
            Ok(()) => {
                info!(concept_id = %concept.concept_id, "Pipeline: transaction committed");
                true
            }
            Err(e) => {
                warn!(error = %e, "Pipeline: failed to record execution, but script succeeded");
                false
            }
        }
    } else {
        error!(
            concept_id = %concept.concept_id,
            error = execution.error.as_deref().unwrap_or("unknown"),
            "Pipeline: execution failed, rolling back"
        );
        false
    };

    // Step 8: Cleanup — handle is dropped automatically (RAII)
    // The `handle` goes out of scope here, triggering StateHandleGuard::drop()

    Ok(PipelineResult {
        handle_id,
        execution,
        committed,
    })
}

/// Records a successful execution event in TypeDB.
fn record_execution(
    _db: &mut TypeDbRouter,
    concept: &OkfConcept,
    result: &ExecutionResult,
) -> Result<(), DbError> {
    // For Phase 1, we log the execution. Full mem-gecko episode generation
    // happens when the mem-gecko extension is wired in.
    info!(
        concept_id = %concept.concept_id,
        duration_ms = result.duration_ms,
        engine = ?result.engine,
        "Execution recorded"
    );
    Ok(())
}
