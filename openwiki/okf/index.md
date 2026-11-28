# Files

- [OKF Bundles and the Parser](bundle-format-and-parser.md) - The OKF bundle layout GECKO ingests — Markdown files with YAML frontmatter under a directory with an optional bundle.json — and how the parser derives concept ids, GECKO frontmatter (engine, scopes, timeout-ms), one program per concept, links, citations and directory hierarchy.
- [Bundle Sync into TypeDB](graph-sync.md) - How sync_bundle writes a parsed OKF bundle into TypeDB in one guarded, parameterized write transaction — content-hash planning, in-place updates, collision and retype refusals, garbage collection with release and retirement, extension writes in the same transaction, and the belief-tier fingerprint that refuses any belief change.
