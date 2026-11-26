---
type: playbook
title: Withdraw a detection claim
description: Retracts a detection claim's belief, so its technique counts as undetected again.
engine: quickjs
scopes: [mem:retract]
tags: [cyber, detection, memory]
---

# Withdraw a detection claim

Retracts the belief `belief_id` that `playbooks/claim_detection` returned, giving a
`reason`. The claim stops counting: its technique returns to the coverage gaps, and the
detection leaves the blind list. The belief stays in the graph as `retracted`, along with
the reason and who retracted it.

```javascript
const { belief_id, reason } = gecko.args;
mem.retract(belief_id, reason);
```
