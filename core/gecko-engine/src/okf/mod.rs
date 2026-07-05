//! OKF (Open Knowledge Format) compliance layer.
//!
//! Parses markdown files with YAML frontmatter into typed structures,
//! extracts links and code blocks, and assembles bundles.

pub mod types;
pub mod parser;
pub mod linker;
