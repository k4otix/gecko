---
type: playbook
title: Detection precision
description: Computes a detection's true-positive precision from the verdicts recorded over a trailing window.
engine: quickjs
scopes: [cyber-gecko:precision]
tags: [cyber, detection, triage]
---

# Detection precision

Counts the verdicts recorded on the detection `logic_id` over the last `window` seconds,
an integer greater than 0. It returns the precision: true positives over true plus false
positives. `benign-true` counts as neither, and `precision` is `null` when both counts
are 0.

```javascript
const { logic_id, window } = gecko.args;
gecko.callExtension("cyber-gecko", "precision", { logic_id, window });
```
