# Agent Instructions for Gecko

This file contains customizations and rules that AI agents should follow when working in this repository.

## Development Workflow & Coding Routine
- **Formatting**: Always run `cargo fmt --all` as part of your regular coding routine before ending your turn or committing code. Ensure the codebase stays cleanly formatted.
- **Verification**: Always run `cargo clippy --workspace` and `cargo test --workspace` after making changes to verify correctness and idiomatic Rust style.

## Rust Development Guidelines
- **Expertise**: Act as an expert Rust developer, fluent in idiomatic Rust, safe programming, and software architecture.
- **Quality First**: Care deeply about quality, readability, and maintainability. Ensure essential code paths are covered by tests.
- **Pedantic Quality**: Be mindful of pedantic clippy warnings (e.g., redundant closures, collapsed if blocks, missing Default derives) and write idiomatic Rust.
