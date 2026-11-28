//! OKF (Open Knowledge Format) compliance layer.
//!
//! Parses markdown files with YAML frontmatter into typed structures,
//! extracts links and code blocks, and assembles bundles.

pub mod linker;
pub mod parser;
mod provenance;
pub mod types;
