# Log-source recommendation — short design

2026-11-22. This is a follow-on to S2 (`2026-11-21-attack-detection-model-design.md`). The
user ranked `blinded`'s purposes as: which platform you are blind on, then which sensor to
add, then that the detection is unreliable. This design builds the second.

## Intent

`blinded` says where a claimed detection cannot see its evidence. This design adds the next
step: what to start collecting, in what order, to fix the most of those detections.

The user chose:

- **ATT&CK's own log sources.** The recommendation names log sources by the names ATT&CK
  analytics require. A catalog of products is a non-goal.
- **Approach A.** A new `recommend_sources` host import, with the greedy loop in Rust.

**Goals.**

- An ordered list of steps. Each step says which log sources to add, on which platform, and
  which claims become able to run.
- The steps are deterministic for a given graph.
- Every claim the list says it fixes really runs once the steps up to that point are in
  place.

**Non-goals.**

- A product or vendor catalog.
- Costs or weights per log source.
- Optimal, solver-based selection. Greedy is the method. A solver is considered only if
  real-data lists look poor.
- Technique-wide coverage. The target is the user's asserted claims, as in `blinded`.
- Recommending a fallback data component. Recommendations name log sources only.

**Anti-goals.**

- **Never claim to fix what a step cannot fix.** A zero-requirement analytic, or a platform
  ATT&CK gives no analytic for, is listed as unfixable with its reason. It never appears as
  a step.
- **Never return an empty list for a graph the check cannot judge.** The preconditions and
  errors are the same as `blinded`'s: no sensor file, or no detection strategies.
- **Never recommend a platform the user did not declare.**

## Approaches

- **A. A new `recommend_sources` host import, with the greedy loop in Rust (chosen).**
  - It reads the blind (claim, platform) pairs and each blind analytic's missing log sources
    from the graph. The loop itself is a pure Rust function.
- *B. A section in `blinded`'s output.* Rejected: every `blinded` call would pay for the
  planning, and it mixes "what's wrong" with "what to do".
- *Pure TypeQL.* Rejected: TypeQL cannot express the loop that picks a step, applies it,
  and recomputes.

## Design

**Surface.**

- `cyber-gecko:recommend_sources` joins `DETECT_FNS`. It is gated by the S3 scope
  `cyber-gecko:recommend_sources` like the other detect imports.
- It takes no arguments.

**Inputs from the graph.**

- **Blind pairs.** Every (claim, platform) pair that `blinded` would report as blind,
  using the same rule: a platform where any analytic is covered is not blind.
- **Gap sets.** For each live analytic on a blind pair's platform, its set of missing
  log-source keys on that platform (`analytic-gaps`). The display name and ATT&CK channels
  come with each key.
- **Unfixable pairs.** An analytic with no requirement on a live data component cannot run,
  whatever is added. If every analytic of a blind pair is like that, the pair is unfixable.
- **Unassessed pairs.** `blinded`'s unassessed platforms, which have no analytic, are
  unfixable too.

**Greedy loop.** This is a pure function over those inputs.

- **A candidate** is one analytic's remaining gap set on one platform. The unit is the
  whole set, because adding part of it fixes nothing.
- **Each round:**
  1. For every candidate, compute the number of blind pairs it would fix, given the sources
     already added. A pair is fixed when any one of its analytics has every gap added.
  2. Pick the candidate with the highest ratio of pairs fixed to new log sources needed.
     Ratios are compared by integer cross-multiplication.
  3. Break ties by more pairs fixed, then by platform, then by the sorted source keys.
- **Applying a step.** Add the step's sources and record the step. Remaining gap sets
  shrink accordingly.
- **Stop** when no candidate fixes a pair.

**Output.**

```json
{
  "steps": [
    {
      "platform": "Windows",
      "add": [{ "log_source": "…", "key": "…", "channels": ["…"] }],
      "unblinds": [{ "concept_id": "…", "technique": "T…", "technique_concept_id": "…" }],
      "cumulative_unblinded": 3
    }
  ],
  "unfixable": [
    { "concept_id": "…", "technique": "T…", "technique_concept_id": "…",
      "platform": "…", "reason": "no-requirements" | "no-analytic" }
  ]
}
```

**Tests.**

- **Unit tests** of the pure loop, covering:
  - the stall case, where two sources are each useless alone;
  - shared sources across analytics;
  - several analytics on one platform;
  - tie-breaking.
- **proptest properties** over random inputs:
  - every pair listed under a step is fixed by the sources added up to that step;
  - no step's added sources are already present;
  - a pair is never listed twice;
  - every blind pair is either fixed by some step or has no candidate that could fix it.
- **One integration test** over the existing ATT&CK test fixtures
  (`gecko-bin/tests/common/attack.rs`) with a sensor file. It runs the import end to end,
  including its error preconditions.

## Left to the builder

- How the reads are shared with `blinded`, such as factoring out the blind-pair read.
- Output field order and naming beyond the shape above.
- Where the pure loop lives, for example `src/detect/recommend.rs`.
- Whether proptest enters as a dev-dependency of cyber-gecko only.
