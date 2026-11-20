# Retire, not delete (ledger O6) — short design

Approved by the user 2026-11-20. Stacked on `feat/belief-tier-guard` (PR #63),
whose pre-GC refusal this replaces.

## Intent

Today a sync refuses outright when it would garbage-collect a concept that a
belief edge references (`DbError::BeliefEdgeReferencesRemovedConcept`): renaming
a cited playbook, or an ATT&CK re-import that drops a claimed technique, blocks
every later sync of that bundle until the file comes back. Beliefs are
history and must keep their edges, but the bundle must be able to move on. The
sync should *retire* such a concept — keep the entity so belief edges stay
valid, strip what the bundle no longer says about it — and carry on.

**Goals.** A sync that drops a belief-referenced concept succeeds; the belief
and its edge are untouched; the concept is visibly retired; running it fails
clearly; restoring its file brings it back; once nothing references it, a
later sync deletes it as usual.

**Non-goals.** Recall does not flag or filter beliefs about retired concepts
(the user's call: "leave beliefs as they are"). Cyber's STIX `revoked`
attribute stays a separate, domain-level notion.

**Anti-goals.** Retirement must not change the belief tier (the fingerprint
guard must see nothing), must not re-type or re-insert the concept (that would
sever the very edges it protects), and must not let a retired concept collide
with its own returning file.

## Approaches

- **`retired-at` attribute on `concept` (chosen).** Additive schema change;
  the entity, its ID and its belief edges are untouched.
- *A `retired-concept` subtype* — rejected: TypeDB cannot change an instance's
  type, so retiring would mean delete + re-insert, which breaks belief edges.
- *Release the concept from its bundle* — rejected: an uncontained concept is a
  collision for every bundle, so its file could never return.

## Design

1. **Schema** (`core/gecko-engine/schema/core_schema.tql`): `attribute
   retired-at value datetime` and `concept owns retired-at @card(0..1)`, each
   with a greenfield `@doc`. Additive, so `schema-init` upgrades an existing
   database.
2. **Sync, step 5.** `belief_edge_references` still computes which GC-delete
   candidates a belief-tier relation references. Those concepts are *retired*;
   the rest are deleted as today. Retiring a concept:
   - removes every attribute except `concept-id` (as `clear_concept_content`);
   - deletes every core OKF relation it plays — hierarchy, concept-link and
     citation, in either direction — except its `containment`;
   - deletes every other relation it plays whose type is not in the belief-tier
     surface (cyber's typed relations), matching what deletion does to them;
   - sets `retired-at` to the sync time, and keeps containment in its bundle.
   Belief-tier relations are never touched, so the end-of-transaction
   fingerprint is unchanged.
3. **Later syncs.**
   - Still referenced and still absent: it stays retired. Its first
     `retired-at` is kept (not re-stamped), and no further writes are needed.
   - No longer referenced and still absent: deleted by the normal GC path.
   - Its file returns: it is an owned concept with no stored hash, so it takes
     the update path; `clear_concept_content` removes `retired-at` with the
     other attributes and the content is re-attached. It is live again.
   - A dropped concept another bundle also contains is released, not retired
     (only this bundle's containment goes), as today.
4. **Reporting.** `SyncResult` gains `concepts_retired`: concepts *this* sync
   newly retired (a concept already retired counts as neither retired nor
   deleted). The CLI prints it alongside the other counts and the completion
   log includes it. `DbError::BeliefEdgeReferencesRemovedConcept` is removed —
   nothing raises it.
5. **Run.** `gecko run` on a retired concept fails with ``concept `X` is
   retired (since <retired-at>)`` before the "no executable program" check.
6. **Docs.** SPEC-W5 §3.5 records that retirement replaces the pre-GC refusal
   (same date); the ledger marks O6 done; KNOWN-LIMITATIONS replaces the
   "Pre-GC refusal" bullet with a "Retired concepts" one (readers learn of a
   retirement only by looking at the concept; recall does not flag it);
   `sync_bundle`'s doc comment, CLAUDE.md's architecture line and
   `extensions/README.md` stop describing a refusal.
7. **Tests** (live TypeDB, `--test-threads=1`):
   - renaming a playbook a claim cites retires it: the sync succeeds, the claim
     and its edge survive, `retired-at` is set, content and outgoing links are
     gone, containment remains, `concepts_retired == 1`, and `run` refuses it;
   - a second sync keeps it retired with the same `retired-at` and
     `concepts_retired == 0`;
   - restoring the file un-retires it (`retired-at` absent, content back);
   - once the referencing edge is gone, the next sync deletes it;
   - an ATT&CK-shaped cyber re-import that drops a claimed technique retires it
     and removes its typed relations;
   - the existing refusal tests (`bundle_sync_test.rs`,
     `belief_tier_reconciliation_test.rs`) become retirement tests.

## Left to the builder

Query shapes, whether retire reuses `clear_concept_content` or a sibling, how
"already retired" is read (e.g. `StoredConcept` gaining `retired: bool`), and
the exact CLI wording of the retired count.
