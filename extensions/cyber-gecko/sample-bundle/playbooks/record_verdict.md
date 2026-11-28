---
type: playbook
title: Record an alert verdict
description: Records an analyst's verdict on an alert from a detection, which precision counts.
engine: quickjs
scopes: [cyber-gecko:disposition, mem:rests_on]
tags: [cyber, detection, triage]
---

# Record an alert verdict

Records the analyst's `verdict` on an alert from the detection `logic_id`. The verdict is
one of `true-positive`, `false-positive` or `benign-true`. `evidence`, if given, lists
memory ids that back the verdict.

`assumes`, if given, lists the beliefs the verdict takes for granted, usually the claim
that the detection works. Withdrawing one of them then reports this verdict in its blast
radius. `assumes` must be a non-empty list of distinct ids; anything else is refused before
the verdict is recorded. An id that is missing, or not visible to the caller, is refused
only after the verdict is recorded: the run fails, but the verdict stands, and the error
names its `belief_id` so it can be withdrawn or kept.

```javascript
const { logic_id, verdict, evidence, assumes } = gecko.args;
if (assumes !== undefined) {
  if (!Array.isArray(assumes) || assumes.length === 0) {
    throw new TypeError("assumes must be a non-empty array of belief ids");
  }
  assumes.forEach((id, i) => {
    if (typeof id !== "string" || id === "") {
      throw new TypeError(`assumes[${i}] must be a non-empty belief id`);
    }
    if (assumes.indexOf(id) !== i) {
      throw new TypeError(`assumes names ${id} more than once`);
    }
  });
}
const recorded = gecko.callExtension("cyber-gecko", "disposition", { logic_id, verdict, evidence });
if (assumes !== undefined) {
  try {
    mem.restsOn(recorded.belief_id, assumes);
  } catch (e) {
    throw new Error(`recorded verdict ${recorded.belief_id} but could not record what it assumes: ${e.message}`);
  }
}
assumes === undefined ? recorded : { ...recorded, assumes };
```
