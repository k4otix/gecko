---
type: playbook
title: Claim a detection
description: Records that a detection detects an ATT&CK technique, as a belief with its confidence.
engine: quickjs
scopes: [cyber-gecko:claim]
tags: [cyber, detection]
---

# Claim a detection

Asserts that the detection `logic_id` (a concept id in this bundle) detects the ATT&CK
technique `technique_id` (an ATT&CK id such as `T1059.001`, not a concept id).

- `method` says how you know it: `human-assertion`, `external-tool`, and so on.
- `confidence` is in `[0.0, 1.0]`.
- `evidence`, optional, lists memory ids that back the claim.

The reply's `belief_id` is what `playbooks/withdraw_claim` takes to withdraw the claim.

```javascript
const { logic_id, technique_id, method, confidence, evidence } = gecko.args;
gecko.callExtension("cyber-gecko", "claim", {
    logic_id,
    technique_id,
    method,
    confidence,
    evidence,
});
```
