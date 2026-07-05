//! # gecko-engine
//!
//! The domain-agnostic core of the GECKO framework.
//!
//! Provides OKF parsing, TypeDB transaction routing, sandboxed script execution,
//! RAII state management, and the extension trait for pluggable domain logic.

pub mod db;
pub mod extension;
pub mod okf;
pub mod pipeline;
pub mod sandbox;
pub mod state;
pub mod syncer;
