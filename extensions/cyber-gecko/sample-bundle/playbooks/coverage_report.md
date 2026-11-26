---
type: playbook
title: Coverage report
description: Summarizes ATT&CK coverage, from the undetected techniques and coverage by tactic to the blind detections and the log sources to collect next.
engine: quickjs
scopes: [cyber-gecko:coverage_gaps, cyber-gecko:blinded, cyber-gecko:recommend_sources]
tags: [cyber, coverage]
---

# Coverage report

Combines three reads into one summary:

- `undetected`: the live techniques no current claim detects (`coverage_gaps`);
- `by_tactic`: how many live techniques serve each tactic, and how many of those are
  undetected;
- `blind`: the claimed detections that cannot run on some platform you declared,
  because no sensor there collects what they need (`blinded`);
- `recommended`: the log sources to start collecting, most useful first
  (`recommend_sources`).

```javascript
const gaps = gecko.callExtension("cyber-gecko", "coverage_gaps", {});
const blinded = gecko.callExtension("cyber-gecko", "blinded", {});
const plan = gecko.callExtension("cyber-gecko", "recommend_sources", {});

const sorted = (ids) => [...new Set(ids)].sort();
({
    undetected: sorted(gaps.undetected_techniques.map((t) => t.attack_id ?? t.concept_id)),
    by_tactic: gaps.by_tactic.map(({ attack_id, live, undetected }) => ({
        attack_id,
        live,
        undetected,
    })),
    blind: sorted(
        blinded.blinded_detections
            .filter((d) => d.blind_platforms.length > 0)
            .map((d) => d.concept_id),
    ),
    recommended: plan.steps.flatMap((s) => s.add.map((a) => a.log_source)),
});
```
