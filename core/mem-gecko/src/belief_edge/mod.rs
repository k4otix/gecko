//! The registry of extension-declared relations that link a new belief to domain records.

mod registry;
mod scan;

pub use registry::{BeliefEdgeRegistry, ExtensionFragment, RegistryError};
