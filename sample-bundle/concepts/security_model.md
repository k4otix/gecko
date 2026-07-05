---
type: concept
title: Security and Isolation Model
description: Overview of the security guarantees and compute limits provided by GECKO playbooks.
tags: [security, sandbox, architecture]
---

# Security and Isolation Model

GECKO operates on a principle of least privilege. Playbooks are executed in strictly bounded environments:

1. **WebAssembly Sandboxing**: Playbooks executing via engines like QuickJS are compiled down to WebAssembly. The `WasmExecutor` provides a compute-only WASI environment, guaranteeing complete isolation from the host filesystem, network, and system clock.
2. **Epoch Interruption**: Wasmtime's epoch interruption enables the engine to preemptively abort runaway playbooks, protecting against infinite loops and resource exhaustion (DoS).
3. **Explicit Host Imports**: To interact with the outside world, a playbook must use predefined Host Imports that are selectively provided by domain-specific `GeckoExtensions`.
