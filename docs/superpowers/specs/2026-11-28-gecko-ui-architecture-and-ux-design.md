# GECKO UI — Architecture, Repository Strategy, and UX Design Brief

**Status:** Draft / Request for Second Opinion  
**Date:** 2026-11-28  
**Scope:** Designing a web-first (local-first alpha, SaaS-ready, optional Tauri v2 desktop) UI on top of the GECKO knowledge-graph, epistemic-memory, and cyber-detection framework.

---

## 1. Executive Summary

We are designing a graphical workbench (**working name: GECKO Studio**) on top of **GECKO** (*Graph Execution of Contextual Knowledge Objects*). Rather than treating GECKO as a generic node-and-edge graph viewer, the UI is designed around GECKO's **three distinct semantic layers**:

1. **Authored Knowledge & Execution (OKF Core):** Markdown + YAML frontmatter bundles, concept hierarchies, wikilinks, external citations, and capability-gated JavaScript playbooks executed inside a Wasmtime WebAssembly sandbox.
2. **Epistemic Memory Substrate (`mem-gecko`):** Always-on bitemporal graph of `belief`s, `episode`s, `anomaly` contradiction hubs, `derivation` chains, `rests-on` assumption trees (with retraction blast-radius), retrieval provenance (`retrieval-event` / `surfaced` / `informs-synthesis`), and optional in-process HNSW + Candle semantic recall.
3. **Domain Extensions (`cyber-gecko`):** STIX 2.1 / MITRE ATT&CK (v18–20.1 detection model) ingestion, sensor/environment telemetry definitions, live coverage-gap analysis, blinded-detection diagnostics, greedy log-source remediation planning, and alert-disposition precision tracking.

### Core Decisions Made So Far

| Decision Axis | Choice | Rationale |
| --- | --- | --- |
| **Deployment Model** | **Web-first SPA (local-first alpha, SaaS-ready) + optional Tauri v2 thick client** | Single web UI works via local `gecko-server`, wrapped in Tauri v2 for native desktop ergonomics (file/folder pickers, TypeDB lifecycle), and deploys cleanly to the cloud later. |
| **Repository Split** | **Two repositories:** pure-Rust engine + runtime + HTTP/SSE API in `gecko`; React/TS SPA + Tauri wrapper in a sibling repo (e.g., `gecko-studio`) | Keeps `gecko` 100% pure Rust (clean Cargo/CI/audit/`deny.toml`/release cadence), exposes a reusable HTTP/OpenAPI contract for agents/scripts/UIs, and preserves future product/licensing flexibility. |
| **Read vs. Write Scope** | **Include write & playbook execution early** | Exploring static data is only half of GECKO; running playbooks, syncing bundles/STIX/sensors, claiming detections, recording verdicts, and retracting beliefs with blast-radius previews showcase GECKO's real value. |
| **Persona & Lens Strategy** | **Single shell with three Adaptive Lenses** (Knowledge, Memory, Cyber) | Gives OKF a solid foundation, treats `mem-gecko` as an always-on agentic-memory debugger without needing a new plugin yet, and makes `cyber-gecko` the flagship vertical—while letting Cyber + Memory converge in Agentic SOC workflows. |
| **Visual Aesthetic** | **Dual-mode token system:** *Editorial Light* (Knowledge/OKF) & *Tactical Dark* (Cyber/SOC) + density toggle | Matches cognitive load to the task: calm prose clarity for knowledge management, high-contrast compact density for ATT&CK matrices and SOC triage. |

---

## 2. Grounding in GECKO's Existing Architecture

Any UI architecture must respect GECKO's real runtime and security invariants:

- **Native Rust + External Graph DB:** GECKO is a Rust 2024 workspace backed by **TypeDB 3.15** over native gRPC (`typedb-driver`), **Wasmtime** (running a prebuilt Boa JS guest at `core/gecko-engine/src/sandbox/quickjs.wasm`), and an optional local **HNSW + Candle (`bge-large-en-v1.5`)** semantic index (`gecko-semantic-index`). It cannot be compiled purely to browser-side WASM.
- **Strict Extension Isolation:** `core/*` never imports `extensions/*`, and `extensions/*` (`cyber-gecko`) depend *only* on `core/gecko-extension-api`—never `gecko-engine` or `typedb-driver` (CI-enforced by `scripts/check-extension-deps.sh`). Only the **Assembler** (`gecko-bin` today) composes core + extensions.
- **Database-Per-Bundle / Domain Model:** One bundle per TypeDB database by default, though multiple bundles may share a database with `--database`. ATT&CK domains (Enterprise, Mobile, ICS) cannot share a database because they ship overlapping STIX objects; separate domains live in separate databases.
- **S3 Default-Deny Execution:** Playbooks only reach host functions explicitly granted in their frontmatter `scopes` (e.g., `mem:recall`, `cyber-gecko:claim`) and are bounded by `timeout-ms` and memory limits.
- **Host-Mediated Epistemic Provenance:** Every belief write flows exclusively through `EpistemicWriter` / `BeliefEdgeWriter` stamped with a host-minted `RunContext` (`run_id`, `actor`, `occurred_at`, `source`). Sandbox code cannot forge identity.
- **Visibility & Entrenchment Guards:** Writes act only on beliefs visible to the actor (`private`, `team`, `shared`). Probabilistic derivations (`llm-synthesis`, `confidence < 1.0`) cannot supersede higher-entrenchment beliefs.
- **Retire-Not-Delete Guard:** Re-syncing a bundle that drops a concept referenced by a belief edge **retires** that concept (`retired-at`) instead of deleting it, and refuses syncs that attempt to mutate the belief tier.
- **Self-Documenting Live Schema:** Types, roles, and TypeDB functions carry `@doc("...")` annotations stored in TypeDB itself, enabling runtime schema introspection.

---

## 3. Technology Stack & Rejected Alternatives

### Recommended Stack

| Layer | Technology | Why |
| --- | --- | --- |
| **Backend Runtime** | Rust (`gecko-runtime` lib + `gecko-server` via `axum` + `tokio`) | Direct in-process calls to `gecko-engine`, `mem-gecko`, `cyber-gecko`, and `gecko-semantic-index` with zero CLI-scraping or serialization impedance. |
| **API Contract** | REST JSON + **Server-Sent Events (SSE)** + **OpenAPI 3.1 (`utoipa`)** | Typed, versioned boundary across the two repos; `openapi-typescript` generates exact TS types and client bindings automatically. SSE cleanly streams sync progress and playbook `console.log` output without bidirectional WebSocket complexity (WebSockets can be added later if live collaborative editing or server-push subscriptions demand it). |
| **Frontend SPA** | **React 19 + TypeScript + Vite** | Strongest ecosystem for high-density tables, WebGL graphs, DAG editors, Markdown renderers, and code/JSON editors. |
| **Styling & Primitives** | **Tailwind CSS + Radix UI** (or shadcn-style unstyled primitives) | Full control over semantic CSS design tokens for Editorial Light ↔ Tactical Dark and Comfortable ↔ Compact density modes. |
| **Large Graph Rendering** | **Sigma.js (Graphology)** *(or Cytoscape.js)* | WebGL-accelerated rendering for multi-hundred/thousand-node STIX/OKF ego-graphs without DOM/SVG stalls. |
| **Structured DAG Rendering** | **React Flow** | Ideal for directed provenance trees (`derivation`, `retrieval-event` → `surfaced`, `rests-on` blast-radius graphs) where custom rich HTML cards inside nodes matter more than physics simulation. |
| **Tables & Virtualization** | **TanStack Table + TanStack Virtual** | Instant filtering/sorting across 700+ ATT&CK techniques, thousands of STIX objects, or large belief ledgers. |
| **Desktop Packaging (Optional)** | **Tauri v2** (in `gecko-studio`) | Spawns `gecko-server` as a **sidecar binary** bound to an ephemeral `127.0.0.1:<port>` with a per-launch bearer token; Tauri IPC is strictly limited to OS superpowers (`pick_directory`, `pick_file`, `open_in_editor`, system tray / TypeDB lifecycle). |

### Alternatives Considered and Rejected

- **Pure browser-side WASM:** Not viable—cannot speak TypeDB native gRPC directly, cannot run Wasmtime/Candle natively, and cannot manage local TypeDB or bundle directories.
- **Pure Rust native GUI (`egui`, `iced`, `GPUI`, `Dioxus`):** Significantly higher engineering cost for rich Markdown + wikilinks, dense interactive ATT&CK heatmaps, WebGL graph exploration, virtualized data grids, and rapid UX iteration—and cannot double as a SaaS web app.
- **Electron:** Heavy RAM/binary footprint and forces a Node↔Rust bridge anyway, whereas `axum` + Tauri v2 embeds/sidecars Rust natively with a fraction of the overhead.
- **Single Monorepo (`ui/` inside `gecko`):** Faster for initial solo hacking, but mixes Node/npm/Tauri tooling into a lean, pure-Rust Cargo workspace, complicates CI/Dependabot/releases, and couples engine and UI lifecycles.

---

## 4. Repository & Crate Architecture

### Repository 1: `gecko` (Pure Rust — Engine, CLI, and API Server)

Currently, `gecko-bin/src/main.rs` (~1,630 lines) mixes `clap` CLI parsing and `println!` presentation with all Assembler orchestration (`GeckoConfig`, TypeDB `orchestrator`, `doctor`, `build_extensions`, `build_host_bridge`, sync/run flows).

We refactor the Assembler layer into three clean crates while leaving `core/*` and `extensions/*` boundaries untouched:

```text
gecko/
├── core/                        # Unchanged: domain-agnostic core
│   ├── gecko-extension-api/
│   ├── gecko-engine/
│   ├── mem-gecko/
│   ├── gecko-semantic-index/
│   └── js-sandbox/
├── extensions/                  # Unchanged: depends ONLY on gecko-extension-api
│   └── cyber-gecko/             # (scripts/check-extension-deps.sh stays 100% intact)
├── gecko-runtime/               # NEW (lib): The Assembler library
│   ├── config & cache paths
│   ├── TypeDB orchestrator (up / down / status)
│   ├── doctor (returns structured report structs)
│   ├── extension registration + host-bridge builder
│   └── typed async services:
│       ├── databases & schema_init
│       ├── sync_okf / sync_stix / sync_sensors (structured diffs + progress channel)
│       ├── run_playbook (captured stdout + structured result + run_id artifacts)
│       ├── query_tql (read-only fetch/rows)
│       ├── memory services (list beliefs, recall, believed_at, provenance, blast_radius, writes)
│       └── cyber services (matrix, coverage_gaps, blinded, recommend_sources, precision, claim, disposition, stix neighborhood)
├── gecko-bin/                   # Existing (bin `gecko`): thin CLI over `gecko-runtime`
│   └── optional `gecko serve` subcommand (or delegate to `gecko-server`)
└── gecko-server/                # NEW (bin + lib): thin `axum` HTTP + SSE API over `gecko-runtime`
    ├── REST JSON + SSE endpoints
    ├── `utoipa` OpenAPI 3.1 schema export (`gecko-server --emit-openapi` / `/api/openapi.json`)
    ├── local-first security: bind `127.0.0.1` by default, optional bearer token, strict CORS
    └── future SaaS extension points: auth middleware, multi-tenant DB scoping, upload-based ingest
```

### Repository 2: `gecko-studio` (TypeScript Web SPA + Optional Tauri v2 Shell)

```text
gecko-studio/
├── web/                         # Pure browser SPA (Vite + React + TS + Tailwind)
│   ├── src/
│   │   ├── api/                 # Generated OpenAPI types + typed `GeckoClient` + SSE helpers
│   │   ├── theme/               # Semantic CSS tokens (Editorial Light / Tactical Dark + Density)
│   │   ├── shell/               # App rail, Top context bar, Cmd+K palette, TypeQL drawer, Modals
│   │   ├── lenses/
│   │   │   ├── knowledge/       # OKF bundle tree, Markdown reader, Playbook runner, Schema explorer
│   │   │   ├── memory/          # Belief ledger, As-Of scrubber, Provenance/Blast-Radius DAG
│   │   │   └── cyber/           # ATT&CK matrix, Blindness/ROI planner, Detection workbench, STIX graph
│   │   └── platform/            # Abstraction over web vs. Tauri native capabilities (file pickers, etc.)
└── desktop/                     # Optional Tauri v2 wrapper (independent Cargo.toml)
    ├── src-tauri/               # Launches `gecko-server` sidecar + native file dialogs + tray
    └── tauri.conf.json
```

### System Topology

```text
┌─────────────────────────────────────────────────────────────────────────┐
│  gecko-studio/web (React + TypeScript SPA)                              │
│  Runs in: Browser (local or SaaS) OR inside Tauri v2 WebView            │
│  Talks ONLY via typed `GeckoClient` (REST JSON + SSE)                   │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │ HTTP + SSE (127.0.0.1:<port> or https://)
┌───────────────────────────────────▼─────────────────────────────────────┐
│  gecko/gecko-server (Rust / axum)                                       │
│  • Local web mode: `gecko-server --port 7420` (or `gecko serve`)        │
│  • Desktop mode: spawned by Tauri v2 as sidecar w/ ephemeral port+token │
│  • Future SaaS mode: container behind reverse proxy + auth middleware   │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │ Direct Rust calls
┌───────────────────────────────────▼─────────────────────────────────────┐
│  gecko/gecko-runtime (Assembler Library)                                │
│  gecko-engine · mem-gecko · gecko-semantic-index · cyber-gecko          │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │ typedb-driver (gRPC)
                              [ TypeDB 3.15 ]
```

### Cross-Repo Dev Workflow
- `gecko-server` exposes `/api/openapi.json` (and a CLI flag to dump it statically).
- `gecko-studio` runs `npm run gen:api` to regenerate TypeScript request/response types from `openapi.json`, ensuring compile-time safety across repos.
- Local development simply runs `gecko-server` in one terminal and Vite (`npm run dev` with proxy to `localhost:7420`) in the other.

---

## 5. Proposed Backend API Surface (`gecko-server`)

All database-scoped endpoints take `{db}` (and optional actor header `X-Gecko-Actor` in local mode, replaced by verified JWT principal in SaaS mode). Every mutating endpoint mints a fresh host `RunContext` and returns the `run_id` so the UI can link directly to what the operation created.

### System & Environment
- `GET  /api/v1/health` — Server version, enabled extensions (`mem`, `cyber`), embedder mode (`stub` vs. `candle`), server capabilities (e.g., `local_fs_ingest: bool`).
- `GET  /api/v1/doctor` — Structured `gecko doctor` diagnostics (`✓`/`⚠`/`✗` + remediation hints).
- `POST /api/v1/typedb/up` & `POST /api/v1/typedb/down` — Manage orchestrated TypeDB (returns clear mode guidance in `compose`/`external` modes).
- `GET  /api/v1/databases` — List TypeDB databases with metadata (detected schema capabilities: core / mem / cyber, bundle counts, whether ATT&CK & sensors are present).
- `POST /api/v1/databases/{db}/schema-init` — Idempotently apply core + enabled extension schemas & migrations.
- `DELETE /api/v1/databases/{db}` — Drop database (and clean up its HNSW index file).

### Knowledge & OKF (`core`)
- `GET  /api/v1/db/{db}/bundles` — Synced bundles (`bundle-name`, `bundle-path`, `title`, `description`, `synced-at`, concept counts, retired counts).
- `GET  /api/v1/db/{db}/concepts` — Filterable/searchable concept list (by bundle, `concept-type`, `tag`, `has_program`, `retired`).
- `GET  /api/v1/db/{db}/concepts/{*concept_id}` — Full concept detail: Markdown `body`, frontmatter/typed attributes, `code-block`s, `engine`, `scopes`, `timeout-ms`, outgoing/incoming `concept-link`s, `hierarchy` parent/children, external `citation`s, linked beliefs (`source-link`), and `retired-at`.
- `POST /api/v1/db/{db}/sync/okf` *(SSE stream)* — Sync an OKF bundle (local directory path in local/Tauri mode; uploaded archive in future SaaS mode). Streams progress and returns `{ added, updated, unchanged, deleted, retired }`.
- `POST /api/v1/db/{db}/playbooks/{*concept_id}/run` *(SSE stream or JSON)* — Execute a playbook in the Wasmtime sandbox with `{ args }`. Streams `console.log` lines, returns `{ run_id, handle_id, success, duration_ms, result, error, created_memory_ids }`.
- `GET  /api/v1/db/{db}/schema` — Live schema introspection (entities, relations, attributes, functions, and `@doc` descriptions).
- `POST /api/v1/db/{db}/query` — Execute read-only TypeQL `fetch` query; returns structured JSON rows/documents.

### Epistemic Memory (`mem-gecko` — always available)
- `GET  /api/v1/db/{db}/memory/beliefs` — Query/filter beliefs (`state`, `entrenchment`, `owner`, `visibility`, `min_confidence`, `as_of` timestamp).
- `GET  /api/v1/db/{db}/memory/beliefs/{*mem_id}` — Full belief card (state, validity window, confidence, entrenchment, salience/activation/access-count, owner/visibility, domain belief edges such as `technique-detection` / `alert-disposition`).
- `GET  /api/v1/db/{db}/memory/beliefs/{*mem_id}/provenance` — Upstream graph (`derivation` sources, `source-link` origins, `informs-synthesis` → `retrieval-event` → `surfaced` items with similarity scores & `was-used`).
- `GET  /api/v1/db/{db}/memory/beliefs/{*mem_id}/blast-radius` — Dry-run preview of all currently visible beliefs/items reachable via `rests-on` and `derivation` (used as a mandatory pre-flight preview before retraction).
- `GET  /api/v1/db/{db}/memory/anomalies` — Active contradiction hubs and their conflicting claims.
- `GET  /api/v1/db/{db}/memory/runs/{run_id}` — Everything observed, retrieved, derived, or retracted during a single `doc-run`.
- `POST /api/v1/db/{db}/memory/recall` — Run `EpistemicReader::recall` (present-state semantic/hybrid or `as_of` temporal path) with `{ query, max_chunks, as_of }`.
- `POST /api/v1/db/{db}/memory/episodes` — Record an episode (`observe`).
- `POST /api/v1/db/{db}/memory/beliefs` — Assert/derive a belief (`assert_belief`, with optional `evidence` and `method`).
- `POST /api/v1/db/{db}/memory/beliefs/{*mem_id}/supersede` — Supersede with entrenchment guard validation.
- `POST /api/v1/db/{db}/memory/beliefs/{*mem_id}/retract` — Retract belief with `reason`; returns `{ id, blast_radius }`.
- `POST /api/v1/db/{db}/memory/anomalies` — Contest two or more beliefs (`contest`).
- `POST /api/v1/db/{db}/memory/rests-on` — Declare explicit assumption edges (`rests_on`).

### Cyber-Gecko (`cyber` feature)
- `POST /api/v1/db/{db}/cyber/sync-stix` *(SSE stream)* — Ingest STIX 2.1 / ATT&CK JSON (path or upload) under `--bundle-name`; surfaces domain-collision guidance cleanly.
- `POST /api/v1/db/{db}/cyber/sync-sensors` — Ingest `sensors.yaml` (path or raw YAML text from the UI editor); returns sync summary + unused log source / unused platform notices.
- `GET  /api/v1/db/{db}/cyber/sensors` — Current `monitored-environment` platforms, deployed `sensor`s, and `stale_sensor_components`.
- `GET  /api/v1/db/{db}/cyber/matrix` — Aggregated ATT&CK matrix payload: live tactics (ordered), techniques & sub-techniques, and per-technique operational status (`covered` | `blind` | `unassessed` | `undetected`), active claim count, and platform breakdowns—optionally filtered by `platform` or overlaid with an `intrusion-set` / `campaign` / `malware` / `tool` ID.
- `GET  /api/v1/db/{db}/cyber/blinded` — Full `blinded` report (per-claim blind platforms, missing log sources, data components, unassessed platforms, stale sensor components).
- `GET  /api/v1/db/{db}/cyber/recommend-sources` — Greedy `recommend_sources` plan (`steps` with `add`, `channels`, `unblinds`, `cumulative_unblinded`, plus `unfixable`).
- `GET  /api/v1/db/{db}/cyber/detections` & `GET /api/v1/db/{db}/cyber/detections/{*logic_id}` — Detections, their claims, platform analytic requirement checklists, recent alert dispositions, and computed `precision` over configurable windows.
- `POST /api/v1/db/{db}/cyber/claim` — Assert a `technique-detection` belief (`logic_id`, `technique_id`, `method`, `confidence`, `evidence`).
- `POST /api/v1/db/{db}/cyber/disposition` — Record an `alert-disposition` belief (`logic_id`, `verdict`, `evidence`, optional `assumes` → `rests-on`).
- `GET  /api/v1/db/{db}/cyber/stix` — Searchable catalog of STIX objects (`intrusion-set`, `threat-actor`, `campaign`, `malware`, `tool`, `indicator`, `observable`, `course-of-action`, `detection-strategy`, `analytic`, `data-component`).
- `GET  /api/v1/db/{db}/cyber/stix/{*concept_id}/neighborhood` — Typed 1–2 hop ego-graph (`uses`, `attributed-to`, `indicates`, `observed-as`, `mitigates`, `targets`, `strategy-detects`, `strategy-analytic`, `analytic-requirement`, `subtechnique-of`, `revoked-by`) for clean visual pivoting without hairballs.

---

## 6. UX Architecture & Screen Design

### Design Philosophy: Structured Views First, Purposeful Graphs Second
Knowledge-graph UIs fail when they dump 1,500 nodes into a force-directed hairball. GECKO Studio leads with **strongly-typed domain views** (document reader, ATT&CK matrix, remediation table, belief ledger) and deploys interactive graph canvases where topology genuinely clarifies meaning (1–2 hop ego-graphs, provenance/derivation DAGs, and retraction blast-radius trees).

### Global Application Shell

```text
┌────┬──────────────────────────────────────────────────────────────────────────┐
│    │ [DB: attack ▾]  [Actor: human:eddie ▾]  [● TypeDB 3.15]   [⌘K Search]  ◐ │
├────┼──────────────────────────────────────────────────────────────────────────┤
│ K  │                                                                          │
│ M  │                       ACTIVE LENS WORKSPACE                              │
│ C  │                 (Split panes / Matrix / Workbench)                       │
│    │                                                                          │
│ ── │                                                                          │
│ +  │  Sync / Ingest                                                           │
│ >_ │  TypeQL Drawer (collapsible bottom panel)                                │
└────┴──────────────────────────────────────────────────────────────────────────┘
  K = Knowledge Lens (OKF)   M = Memory Lens (mem-gecko)   C = Cyber Lens
```

- **Database Switcher:** Reflects GECKO's database-per-domain architecture (e.g., `gecko-sample`, `attack`, `attack-mobile`). Automatically lights up the **Cyber Lens** when a database contains cyber schema/STIX data, while keeping **Knowledge** and **Memory** always available.
- **Active Actor Pill:** Makes GECKO's `RunContext` and multi-agent visibility (`private` / `team` / `shared`) tangible and testable right from the header.
- **TypeDB Status Pill:** Shows live `doctor` status; click to run `gecko up` / `gecko down` or inspect health checks (embedder, model cache, HNSW index status).
- **`⌘K` Command Palette:** Instant jump to concepts, ATT&CK IDs (`T1059.001`), STIX actors (`APT29`), beliefs (`mem/bel/...`), quick actions (`Sync STIX`, `Run Playbook`), and live **semantic recall** when `semantic_index` is enabled.

---

### Lens 1: Knowledge Lens (OKF Foundation)
*Default Theme: Editorial Light · Comfortable Density*

1. **Bundle & Concept Navigator (Left Sidebar):**
   - Hierarchical tree grouped by synced bundle (`bundle-name`) and OKF path/hierarchy.
   - Filter chips by `concept-type`, `tag`, `Executable Playbooks`, and `Retired` (concepts kept alive because beliefs still reference them, rendered with a clear `Retired` badge and timestamp).
2. **Concept Reader & Context Inspector (Main + Right Rail):**
   - Clean, editorial Markdown rendering with clickable internal `concept-link`s (hover preview popover!), external `citation` cards, and structured frontmatter metadata.
   - Right Inspector shows: Incoming/Outgoing links, a focused 1-hop local graph widget, and any **beliefs/memories** grounded in this document (`source-link`).
3. **Interactive Playbook Runner (Embedded on Executable Concepts):**
   - Security header displaying `engine`, `timeout-ms`, and every granted **S3 capability scope** as an explicit pill (e.g., `cyber-gecko:claim`, `mem:recall`) so users immediately understand what the sandbox can and cannot touch.
   - Split runner panel: JSON `gecko.args` editor (with validation & saved arg presets) → **Run** button → live streaming `console.log` + structured JSON/Table result + execution duration + clickable list of beliefs/episodes created in that `run_id`.
4. **Live Schema Explorer:**
   - Visualizes TypeDB's active type hierarchy (`core` → `mem` → `cyber`) directly from the live database, displaying every entity, relation, role, cardinality, and `@doc` annotation.

---

### Lens 2: Memory Lens (Agentic Memory / `mem-gecko`)
*Theme: Adaptive (works great in either Editorial Light or Tactical Dark)*

Positioned as **"Chrome DevTools / Flight Recorder for AI Agent Memory"**—requiring **no new plugin today** because `mem-gecko` is already built in:

1. **Belief Ledger + As-Of Time Scrubber:**
   - Virtualized table of beliefs with columns for Claim Text, State (`asserted` / `superseded` / `retracted` / `contested`), Entrenchment (`axiom` → `llm`), Confidence bar, Derivation Method (`type-join` vs. `human-assertion` vs. `llm-synthesis`), Owner/Visibility, and Validity Window.
   - **As-Of Time Scrubber:** Toggle from *Present* to a timestamp slider/picker (`EpistemicReader::believed_at`) to inspect exact historical belief state across supersessions and retractions (clearly noting that temporal recall bypasses the HNSW index per invariant 8).
2. **Provenance & Blast-Radius Studio (React Flow DAG):**
   - Selecting any belief opens a structured horizontal DAG:
     - **Left (Upstream — Why we believe this):** `derivation` sources, `source-link` OKF concepts/runs, and **Retrieval Provenance** (`informs-synthesis` → `retrieval-event` → `surfaced` chunks with cosine `similarity-score` and `was-used` indicator, visually distinct from logical derivation to preserve GECKO's episodic firewall).
     - **Center:** Selected belief card (with action buttons: **Supersede**, **Contest**, **Add `rests-on`**, **Retract**).
     - **Right (Downstream — What breaks if this fails):** Every item connected via `rests-on` or downstream `derivation` (`blast_radius`). Clicking **Retract** highlights the entire downstream blast radius in warning red before confirmation.
3. **Contradiction (`anomaly`) Queue & Semantic Recall Sandbox:**
   - Dedicated queue for contested claims grouped under `anomaly` hubs.
   - Interactive recall tester: enter a natural-language query + `maxChunks` + optional `as_of` to inspect ranked chunks and scores.
4. **Future Agentic Adapter Compatibility:**
   - When an external agent integration (e.g., MCP server or LangGraph/Claude trace collector) is eventually built, its `doc-run`s, `execution-episode`s, `pivot` moves, and `prediction-resolution` calibration scores will populate this lens automatically with zero schema redesign.

---

### Lens 3: Cyber Lens (`cyber-gecko` — Flagship Vertical)
*Default Theme: Tactical Dark · Compact Density*

1. **Live ATT&CK Coverage Matrix (Home View of Cyber Lens):**
   - Columns for every live ATT&CK Tactic (in kill-chain order), rows for Techniques with expandable Sub-techniques.
   - Each cell reflects live graph reality from `coverage_gaps` + `blinded`:
     - **Covered (Emerald):** Asserted claim exists and sensors satisfy at least one live analytic on declared platforms.
     - **Blind (Amber/Crimson):** Asserted claim exists, but analytics cannot run on ≥1 declared platform due to missing log sources.
     - **Unassessed (Slate/Dashed):** Claim exists, but ATT&CK has no live analytic for declared platforms (or technique is retired/deprecated).
     - **Undetected Gap (Neutral/Muted):** Live technique with no asserted detection claim.
   - **High-Signal Controls:**
     - **Platform Filter** (`All`, `Windows`, `Linux`, `macOS`, etc.).
     - **Threat Overlay Dropdown:** Select any `intrusion-set` (e.g., APT29), `campaign`, `malware`, or `tool` to spotlight only the techniques it `uses`—instantly answering *"Where are our detection gaps and blind spots against this specific adversary?"*
     - **Tactic Summary Strip:** Live coverage percentage and blind count per tactic.
2. **Telemetry Blindness & Remediation Planner:**
   - Powered directly by `blinded` + `recommend_sources`.
   - Top visualization: **Cumulative Unblinded ROI Chart**—each step on the X-axis adds a recommended log source (`WinEventLog:Security`, etc.) and plots how many `(claim, platform)` pairs become unblinded.
   - Ranked Action Table: Step #, Platform, Log Source Name, required ATT&CK **Channels** (`EventCode=4688`, `EventCode=4103`), and expandable list of unblinded detections.
   - Separate collapsible card for **Unfixable Claims** (`no-requirements` / `no-analytic`) and **Stale Sensor Components**.
   - **Integrated Sensor Manager:** View deployed sensors by platform, edit `sensors.yaml` in-app with live syntax validation, sync via `sync-sensors`, and immediately watch blind cells flip to covered.
3. **Detection Engineering Workbench (Slide-Over Drawer / Full Detail View):**
   - Combines all three layers for one detection (e.g., `detections/encoded_powershell`):
     - Rendered OKF detection documentation + frontmatter tags.
     - Active **Technique Claims** (showing `method`, `confidence`, `entrenchment` badge—making human vs. LLM claims unmistakable).
     - **Platform Analytic Checklist:** Strategy → Analytic (`ANxxxx`) → Data Component → Required Log Source → Covered by Sensor ✓ / Missing ✗.
     - **Precision & Alert Dispositions:** Trailing precision gauge (`24h` / `7d` / `30d` / custom window), TP/FP/Benign-True breakdown, and one-click **Record Verdict** modal (with optional `assumes` link to the active detection claim!).
     - **One-Click Actions:** *Claim Technique*, *Record Verdict*, *Withdraw Claim* (opens the **Blast-Radius Preview Modal** listing every verdict resting on that claim before calling `mem.retract`).
4. **STIX Threat Graph Explorer:**
   - Searchable STIX catalog (Groups, Threat Actors, Campaigns, Malware, Tools, Indicators, Observables/IOCs, Mitigations).
   - Selecting any entity opens a clean **WebGL Ego-Graph** (Sigma.js) with typed edge toggles (`uses`, `attributed-to`, `indicates`, `observed-as`, `mitigates`, `targets`, `revoked-by`) and an inspector panel—allowing controlled 1-hop expansion ("Expand neighbors") rather than an unreadable full-DB dump.
5. **Guided Ingestion Modal (Global `+ Sync` Action):**
   - Tabbed ingestion for **OKF Bundle**, **STIX 2.1 JSON**, and **Sensors YAML** with database selection, `--bundle-name` input, live SSE progress, and rich structured results (added/updated/retired counts, unused log-source notices, and clear ATT&CK domain-collision guidance).

---

## 7. Visual Design & Dual-Theme System

Both aesthetics share one underlying component architecture and layout grammar, separated strictly via CSS custom properties:

| Token Group | Editorial Light (Knowledge / OKF default) | Tactical Dark (Cyber / SOC default) |
| --- | --- | --- |
| **Canvas / Surfaces** | Warm alabaster (`#FAF9F6`), crisp white cards (`#FFFFFF`) | Deep obsidian (`#0B0E14`), elevated slate (`#121722`) |
| **Borders** | Subtle warm hairline (`#E6E4DD`) | Crisp low-contrast slate (`#222B3B`) |
| **Typography** | Humanist sans + generous 68ch Markdown measure | Tight geometric sans + prominent tabular monospace for IDs (`T1059.001`, `AN0002`, `mem/bel/...`) |
| **Semantic Status** | Soft tinted pills for tags, scopes, retired states | High-legibility status hues: Emerald (covered), Amber (blind), Rose (retracted/contested), Cyan (semantic/LLM) |
| **Density** | Comfortable (default) | Compact (default) |

- Users can choose **Auto by Lens** (Editorial Light in Knowledge, Tactical Dark in Cyber/SOC) or **pin Light or Dark globally**, with an independent **Comfortable / Compact** density toggle.

---

## 8. Phased Implementation Roadmap

### Phase 0 — Engine API Foundation (in `gecko` repo)
- Extract `gecko-runtime` library crate from `gecko-bin` without altering CLI behavior (keep all existing integration tests green).
- Stand up `gecko-server` (`axum` + `utoipa`) with core health/doctor, database, OKF browse/sync, playbook run (SSE), and TypeQL endpoints.
- Add CI check that verifies `gecko-server` compiles cleanly and exports a valid `openapi.json`.

### Phase 1 — `gecko-studio` Shell + Knowledge Lens (in `gecko-studio` repo)
- Scaffold Vite + React 19 + TypeScript + Tailwind + Radix + generated OpenAPI `GeckoClient`.
- Build Global App Shell (DB switcher, Actor selector, TypeDB `up`/`down` & `doctor` popover, `⌘K` palette, dual theme system, TypeQL drawer).
- Deliver **Knowledge Lens**: OKF bundle sync, hierarchy tree, Markdown concept reader with live `concept-link`s, and the **Interactive Playbook Runner** (with S3 scopes + streaming logs + structured output).

### Phase 2 — Cyber-Gecko Flagship Workbench (both repos)
- Add `/cyber/*` endpoints to `gecko-runtime` / `gecko-server` (`sync-stix`, `sync-sensors`, aggregated `/matrix`, `blinded`, `recommend-sources`, `detections`, `claim`, `disposition`, `precision`).
- Build **Cyber Lens**: Live ATT&CK Coverage Matrix with Platform filter & Threat Actor overlay, Blindness & Log-Source ROI Planner, Sensor YAML editor/syncer, and Detection Workbench (Claim / Verdict / Precision / Blast-Radius Withdraw).

### Phase 3 — Memory Lens, STIX Graph & Tauri Desktop (both repos)
- Add `/memory/*` endpoints (`beliefs`, `provenance`, `blast-radius`, `anomalies`, `recall`, write mutations) and `/cyber/stix/*/neighborhood`.
- Build **Memory Lens** (Belief Ledger, As-Of Time Scrubber, React Flow Provenance/Blast-Radius DAG, Recall Sandbox).
- Build **STIX Neighborhood Ego-Graph** (Sigma.js).
- Wrap `gecko-studio/web` in **Tauri v2** (`gecko-studio/desktop`) with `gecko-server` as a bundled sidecar and native OS file/folder pickers.

---

## 9. Open Questions & Tradeoffs for Second-Opinion Review

If you are sharing this document for architectural/UX feedback, these are the sharpest decisions worth pressure-testing:

1. **Two repos from day 1 vs. deferring the split:** Does the clean Rust-only boundary of `gecko` outweigh the minor friction of two-repo commits during rapid alpha API iteration? (Mitigation: OpenAPI codegen + side-by-side local checkout.)
2. **Tauri integration strategy — sidecar `gecko-server` vs. embedded Rust:** Running `gecko-server` as a Tauri sidecar keeps `gecko`'s `Cargo.lock` completely free of Tauri/WebKit crates and guarantees 100% behavioral parity with web/SaaS mode. Is there any reason to embed `gecko-runtime` directly in the Tauri binary instead?
3. **Server-side aggregation vs. client-side TypeQL:** The proposed `/cyber/matrix` and `/memory/beliefs/{id}/provenance` endpoints perform specialized multi-query aggregation in Rust rather than making the browser assemble raw TypeQL queries. Does this hit the right balance between API cleanliness and extensibility for future GECKO extensions?
4. **Live reactivity when external actors mutate TypeDB:** If a CLI command or an external AI agent writes beliefs or syncs a bundle while `gecko-studio` is open, TypeDB does not push change notifications natively. Should v1 rely on manual refresh + mutation-triggered React Query invalidation, or add lightweight polling / a server-side event stream for server-originated writes?
5. **Local filesystem security for `gecko-server`:** Endpoints that accept local filesystem paths (for OKF bundle directories / STIX files) are essential for local-first alpha ergonomics, but must be strictly gated (e.g., disabled by default unless bound to `127.0.0.1` + explicit `--allow-local-fs` flag, replaced by file/archive uploads in SaaS mode). Is that boundary strict enough?
6. **Multi-database / cross-ATT&CK-domain UX:** Because Enterprise, Mobile, and ICS ATT&CK must live in separate TypeDB databases, the UI scopes lenses to one active database at a time. Should we anticipate a future "Workspace" abstraction that groups multiple databases, or keep 1 active DB = 1 workspace for simplicity?
7. **Direct UI writes vs. playbook-only writes:** GECKO's CLI performs cyber claims/verdicts by executing playbooks (`gecko run playbooks/claim_detection`), whereas the proposed UI offers *both* direct structured buttons (calling `gecko-runtime` with a host-minted `RunContext`) *and* the Playbook Runner. Should UI actions invoke direct runtime services, or optionally generate/run standard playbooks under the hood so every mutation follows the exact same audit trail?
