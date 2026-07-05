//! Polyglot script sandbox engine.
//!
//! Provides trait-based abstraction over scripting runtimes (Rhai, QuickJS).
//! Phase 1: native embedding. Phase 2: Wasm isolation via wasmtime.

pub mod engine;
pub mod rhai_executor;
pub mod wasm_executor;
pub mod wasm_pool;
