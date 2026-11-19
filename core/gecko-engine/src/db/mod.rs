//! TypeDB transaction routing and connection management.

pub mod graph_store;
pub mod router;
pub mod schema_apply;

/// The core schema text the engine applies before any extension fragment.
pub const CORE_SCHEMA: &str = include_str!("../../schema/core_schema.tql");

pub use graph_store::RouterGraphStore;
pub use router::{GraphHandle, WriteTransaction};
