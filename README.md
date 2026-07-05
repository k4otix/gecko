# GECKO: Graph Execution of Contextual Knowledge Objects

[![Build Status](https://github.com/k4otix/gecko/actions/workflows/rust.yml/badge.svg)](https://github.com/k4otix/gecko/actions/workflows/rust.yml)
[![Rust](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org)
[![TypeDB](https://img.shields.io/badge/TypeDB-3.0-blue.svg)](https://typedb.com/)
[![WebAssembly](https://img.shields.io/badge/WebAssembly-Wasmtime-654FF0.svg)](https://wasmtime.dev/)

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

### Usage

The `gecko` CLI tool manages the full lifecycle of the knowledge graph and playbook execution. We have provided a `sample-bundle` to test the functionality.

**Initialize the Database**
Applies the core GECKO schema and any registered extension schemas to the TypeDB instance.
```bash
gecko init
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
