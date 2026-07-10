# Contributing to GECKO

Thanks for your interest in GECKO. Contributions are welcome — but please read this first so your expectations match reality.

## What this project is

GECKO is a **solo hobby and research project**, maintained by one person in spare time. It is shared in the hope that it is useful and interesting, not as a supported product.

Practically, that means:

- **Support is best-effort.** Issues and pull requests are read when time allows. There is no SLA, no roadmap commitment, and no guarantee that anything will be reviewed, merged, or fixed on any particular timeline — or at all.
- **Direction is opinionated.** This is a research vehicle for specific ideas. A change may be declined simply because it doesn't fit where the project is going, even if it's well made.
- **Breaking changes happen.** APIs, the TypeQL schema, and internals can change without notice while the project is pre-1.0.

None of this is meant to discourage you — it's just honesty so nobody's time is wasted.

## Ways to contribute

- **Bug reports** — a clear description, what you expected, what happened, and the smallest steps to reproduce are worth a lot.
- **Small, focused pull requests** — bug fixes, docs, and self-contained improvements are easiest to review and most likely to land.
- **Larger changes** — please open an issue to discuss the idea *before* investing significant effort, so we can check it fits the project's direction before you write the code.

## Before you open a pull request

- Keep the change focused; unrelated cleanups are best as separate PRs.
- Make sure the checks pass locally:
  ```bash
  just check          # fmt + clippy + fast tests (the pre-push gate)
  ```
  For anything touching TypeDB or the memory substrate, also run the integration tier:
  ```bash
  just test-it        # Tier 2 — real TypeDB, stub embedder
  ```
- Follow the conventions in [`CLAUDE.md`](CLAUDE.md) — notably: keep comments greenfield (describe current behavior, not history), parameterize all TypeQL, and treat the semantic index as a rebuildable accelerator, never a source of truth.
- If you change the WASM JS guest (`core/js-sandbox/src/main.rs`), rebuild and commit the guest binary:
  ```bash
  just build-guest
  ```
- New domain extensions should implement the `GeckoExtension` trait (`core/gecko-extension-api`) and must not make breaking changes to the core TypeQL schema hierarchy.

## Licensing of contributions

GECKO is licensed under the [Apache License 2.0](LICENSE). By submitting a contribution, you agree that it is provided under the same license, per section 5 of that license.

Thanks again for taking a look.
