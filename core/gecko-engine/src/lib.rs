//! # gecko-engine
//!
//! The domain-agnostic core of the GECKO framework.
//!
//! Provides OKF parsing, TypeDB transaction routing, sandboxed script execution,
//! RAII state management, and the extension trait for pluggable domain logic.

pub mod okf;
pub mod db;
pub mod sandbox;
pub mod state;
pub mod syncer;
pub mod extension;
pub mod pipeline;
