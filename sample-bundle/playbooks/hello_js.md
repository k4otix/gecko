---
type: playbook
title: Hello Javascript (Wasm)
description: A sample playbook demonstrating WebAssembly (Wasmtime) sandboxed execution via QuickJS.
engine: quickjs
tags: [demo, js, wasm]
---

# Hello Javascript

This playbook specifies the `quickjs` engine in its frontmatter. 
When GECKO executes it, it will dispatch the `code-block` below to the highly secure, compute-only WebAssembly Sandbox (`WasmExecutor`).

```javascript
// A simple JavaScript script running inside a WebAssembly sandbox
const framework = "GECKO";
const engine = "Wasmtime + QuickJS";
const answer = 40 + 2;

console.log(`Hello from ${framework} running in ${engine}!`);
console.log(`The meaning of life, the universe, and everything is: ${answer}`);

// Test host extension bindings
const isolationResult = gecko.callExtension("cyber-gecko", "mde_isolate", {
    machine_id: "desktop-x86-01"
});
console.log(`Isolation result: ${JSON.stringify(isolationResult)}`);

// The final evaluated expression
({
    status: "success",
    result: answer,
    isolation: isolationResult
});
```
