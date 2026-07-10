---
type: playbook
title: Recall then derive (epistemic memory)
description: Demonstrates the mem.* sandbox namespace — recall prior memories, then derive a new belief that is auto-linked to what it was built from (A5.7).
engine: quickjs
scopes: [mem:recall, mem:derive]
tags: [demo, js, wasm, memory]
---

# Recall then derive

This playbook shows GECKO's epistemic-memory surface inside the WebAssembly
sandbox. The `scopes` frontmatter grants exactly two capabilities —
`mem:recall` and `mem:derive` — and the sandbox **default-denies** every host
call that is not listed (S3), so a program can only reach the memory functions it
was explicitly granted.

The `mem.*` namespace is ergonomic sugar over the host memory bridge:

- `mem.recall(query, { maxChunks })` returns an array of prior memory chunks
  (`{ id, text, score }`) most relevant to `query`, seeded by the semantic index.
- `mem.derive(text, evidenceIds)` records a new **belief** synthesized from that
  evidence. Because the recall and the derive run in the **same** doc-run, GECKO
  automatically stamps the new belief with retrieval-provenance and links it back
  to the memories it was built from (the A5.7 correlation) — you don't wire that
  up yourself.

The host stamps run identity (`run_id`/`actor`/`source`) itself; the sandbox
cannot forge or supply it.

```javascript
// Surface prior memories relevant to the investigation.
const chunks = mem.recall("lateral movement", { maxChunks: 5 });
console.log(`recalled ${chunks.length} prior memories`);
for (const c of chunks) {
    console.log(`  - ${c.id} (score ${c.score.toFixed(3)}): ${c.text}`);
}

// Derive a new belief FROM that evidence. Passing the recalled ids as evidence is
// what lets GECKO auto-link the belief to the retrieval (A5.7). On an empty store
// recall returns [], and the belief is simply recorded without evidence.
const belief = mem.derive(
    "the host was likely compromised via SMB lateral movement",
    chunks.map((c) => c.id),
);
console.log(`derived belief ${belief.id}`);

// The final evaluated expression becomes the run's structured output.
({
    recalled: chunks.length,
    belief: belief.id,
});
```
