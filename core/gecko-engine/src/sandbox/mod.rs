//! Script sandbox engine.
//!
//! All synced code is untrusted and runs inside one WebAssembly sandbox boundary
//! (wasmtime + a QuickJS guest). MCP/API/DB/TypeQL access is exposed as
//! scope-gated host capabilities, not as separate engines.

pub mod engine;
pub mod mem_host;
pub mod wasm_executor;
pub mod wasm_pool;
