<p align="center">
  <img src="assets/gecko_logo.svg" alt="GECKO Logo" width="250" />
</p>

# GECKO: Graph Execution of Contextual Knowledge Objects

[![Rust](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org)
[![TypeDB](https://img.shields.io/badge/TypeDB-3.0-blue.svg)](https://typedb.com/)
[![WebAssembly](https://img.shields.io/badge/WebAssembly-Wasmtime-654FF0.svg)](https://wasmtime.dev/)
[![Build Status](https://github.com/k4otix/gecko/actions/workflows/ci.yml/badge.svg)](https://github.com/k4otix/gecko/actions/workflows/ci.yml)

At its core, **GECKO** is a strongly-typed semantic knowledge graph designed to underlie Open Knowledge Framework (OKF) objects. While GECKO acts as a high-performance framework for orchestrating execution and context-aware workflows, its foundational capability is ingesting human-readable documentation bundles and rigorously mapping them into an interconnected, queryable graph of domain knowledge.

Built in Rust and backed by TypeDB 3.0, GECKO bridges the gap between static domain documentation and automated execution, enabling complex, graph-traversal-driven capabilities in a secure, sandboxed environment.

## Use Cases

GECKO is designed to serve as the intelligence and orchestration layer for complex, domain-specific systems. Common use cases include:

*   **Workflow Orchestration:** Automate multi-step processes by executing playbooks that interact securely with host environments via WebAssembly or Rhai sandboxes.
*   **Knowledge Management:** Parse Markdown and YAML documentation bundles to build a dynamic, relational ontology of your organization's infrastructure, policies, and concepts.
*   **Intelligent Autonomous Agents:** Provide a semantic, contextual graph memory (`mem-gecko`) and a rigorous operational environment (`cyber-gecko`) for AI agents reasoning over complex environments.
*   **Extensible Domain Logic:** Compose custom capabilities using the Assembler pattern, binding new host functions without sacrificing security or rewriting the core engine.

## Core Architecture

GECKO is built around four fundamental components:

*   **Open Knowledge Format (OKF) Parser**: Ingests Markdown bundles augmented with YAML frontmatter, parsing hierarchical directories of domain concepts, playbooks, and relationships.
*   **Knowledge Graph Syncer**: Maps the parsed OKF concepts into a TypeDB 3.0 database. The syncer ensures idempotency using content hashes and accurately builds the entity-relation hierarchy (containment, concept links, hierarchy edges, and citations).
*   **Sandboxed Execution Engine**: Executes playbook code blocks in secure, isolated runtime environments. Powered by **Wasmtime** (for WebAssembly QuickJS modules) and **Rhai**, the sandbox provides deterministic execution limits, epoch timeouts, and safe host-function FFI bindings.
*   **The Assembler Pattern**: GECKO operates on a statically compiled extension model. Rather than relying on runtime dynamic linking or microservices over network boundaries, GECKO uses Rust traits to compose the core engine with domain-specific extensions at compile time, resulting in a single, highly optimized binary.

## Repository Structure

The project is divided into core abstractions, domain extensions, and the final assembler binary:

*   **`core/gecko-engine/`**: The core framework library. It contains the OKF parser, the TypeDB router and synchronization logic, the foundational TypeQL schema, and the secure execution sandboxes.
*   **`extensions/`**: Domain-specific crates that implement the `GeckoExtension` trait. Each extension can define its own TypeDB schema subtyping the core schema and inject specific host functions into the execution sandbox.
    *   `cyber-gecko`: Abstractions for cybersecurity operations, defining entities like networks, vulnerabilities, and host configurations.
    *   `mem-gecko`: Cognitive and contextual memory abstractions for agents and persistent context.
*   **`gecko-bin/`**: The final Assembler executable. It parses command-line arguments, instantiates the core engine, wires the selected extensions together, and dispatches the execution lifecycle.

## Getting Started

### Prerequisites

*   Rust toolchain (cargo)
*   TypeDB 3.x Server running locally or accessible via TypeDB Cloud

### Building & Installation

Compile and install the GECKO binary globally to your path:

```bash
cargo install --path gecko-bin
```

*(Alternatively, you can run commands directly using `cargo run -p gecko-bin -- <command>`)*

### Installation / First run (pre-built binaries)

For most users, a pre-built per-platform binary from [GitHub
Releases](https://github.com/k4otix/gecko/releases) is faster than compiling
from source. **The download is small (tens of MB): neither the ~1.3GB
embedding model nor TypeDB is ever bundled** — both are runtime assets fetched
separately, on your terms.

```bash
# 1. Install the binary (detects OS/arch, grabs the matching release asset)
curl -fsSL https://raw.githubusercontent.com/k4otix/gecko/main/scripts/install.sh | sh

# 2. Bring up the pinned TypeDB (downloaded once, run as a managed child process)
gecko up

# 3. Sync a bundle — records-only by default, no embedding model required
gecko sync sample-bundle
```

That's the whole basic path — it never touches the ~1.3GB embedding model.
**Semantic retrieval is a separate, explicit opt-in**: run `gecko model
fetch` to stage the model, then set `[semantic_index] enabled = true` and
`embedder = "bge-large-en-v1.5"` in `gecko.toml`.

A Homebrew formula template is available at `HomebrewFormula/gecko.rb` for
maintainers cutting a tap. For the full first-run walkthrough and the
**air-gapped / enterprise path** (pre-staged model + external TypeDB, zero
runtime downloads), see [`docs/INSTALL.md`](docs/INSTALL.md).

### Usage

The `gecko` CLI tool manages the full lifecycle of the knowledge graph and playbook execution. We have provided a `sample-bundle` to test the functionality.

**Write a default config** (optional)
Creates a `gecko.toml` with sensible defaults if one does not already exist (idempotent — never clobbers an existing file).
```bash
gecko init
```

**Initialize the Database**
Applies the core GECKO schema and any registered extension schemas to the TypeDB instance.
```bash
gecko schema-init
```

**Sync a Knowledge Bundle**
Parses an OKF-compliant directory bundle and synchronizes its concepts, hierarchies, and code blocks into the knowledge graph.
```bash
gecko sync sample-bundle
```

**Execute a Playbook**
Retrieves a synchronized playbook from the graph by its Concept ID and executes its script within the secure sandbox.
```bash
# Execute native Rhai script
gecko run sample-bundle:playbooks/hello_gecko

# Execute JavaScript via WebAssembly JS sandbox
gecko run sample-bundle:playbooks/hello_js
```

**Query the Graph**
Run ad-hoc TypeQL read queries to inspect the synchronized domain knowledge.
```bash
gecko query 'match $c isa concept, has concept-id $id; fetch {"id": $id};'
```

## Security Model

Security and isolation are foundational to GECKO's execution model. The `gecko-engine` employs bounded execution environments to prevent malicious or poorly written playbooks from compromising the host system.

*   **Step-Capped Evaluation**: Execution engines enforce strict operation and recursion limits to guarantee deterministic termination and prevent denial-of-service via infinite loops.
*   **State Isolation**: The RAII-based state registry issues ephemeral, concurrent state handles per playbook invocation, ensuring total memory isolation across parallel executions.
*   **Explicit Host Imports**: Playbooks cannot directly access the host filesystem or network. They must explicitly request capabilities through registered host imports provided by vetted extensions.

## Contributing

Contributions to the core engine or new domain extensions are welcome. When developing new extensions, ensure they adhere strictly to the `GeckoExtension` trait and do not introduce breaking changes to the core TypeQL schema hierarchy.
