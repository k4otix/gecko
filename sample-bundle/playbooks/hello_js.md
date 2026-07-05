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

// Perform some basic JS calculations
const data = [1, 2, 3, 4, 5];
const sum = data.reduce((a, b) => a + b, 0);
console.log(`The sum of array [1,2,3,4,5] is ${sum}`);

// The final evaluated expression
({
    status: "success",
    result: answer,
    sum: sum
});
```
