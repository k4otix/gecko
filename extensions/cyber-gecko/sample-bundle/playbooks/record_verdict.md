---
type: playbook
title: Record an alert verdict
description: Records an analyst's verdict on an alert from a detection, which precision counts.
engine: quickjs
scopes: [cyber-gecko:disposition]
tags: [cyber, detection, triage]
---

# Record an alert verdict

Records the analyst's `verdict` on an alert from the detection `logic_id`. The verdict is
one of `true-positive`, `false-positive` or `benign-true`. `evidence`, if given, lists
memory ids that back the verdict.

```javascript
const { logic_id, verdict, evidence } = gecko.args;
gecko.callExtension("cyber-gecko", "disposition", { logic_id, verdict, evidence });
```
