# Files

- [JavaScript Guest (Boa) and Playbook API](js-guest.md) - The prebuilt Boa JavaScript engine compiled to wasm32-wasip1 that runs every playbook — the globals it exposes to playbook code (console.log, gecko.callExtension, frozen gecko.args, mem.*), how the program's last expression becomes the run output, and how to rebuild and re-commit the guest binary.
- [WASM Sandbox Executor and Host ABI](wasm-executor.md) - The Wasmtime executor that runs every playbook — S1 resource limits and the epoch timeout, the virtual stdin/stdout/env WASI context with no filesystem or network, the two-import host-call ABI with its guest length caps, S3 default-deny scope checking, the lazily built SandboxPool, and how guest stdout becomes an ExecutionResult.
