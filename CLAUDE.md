# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

NeuroLithe is a Rust-based embedded contextual memory database for AI agents. It combines short-term dialogue compression (STM) with a permanent long-term knowledge tree (LTM), both backed by SQLite (`sqlite-vec` + FTS5). It runs either as a one-shot **MCP server** over STDIO (JSON-RPC) or as a long-running **daemon** that additionally feeds itself from Kafka and exposes introspection.

> **Status: V2 dual-memory is BUILT.** The dual-store (STM + LTM) architecture from [`V2-DESIGN.md`](V2-DESIGN.md) is implemented end-to-end on branch `feature/v2-dual-memory` — slices 0–12 of [`IMPLEMENTATION-V2.md`](IMPLEMENTATION-V2.md) are complete (config split, decay sweep + soft reset, LTM schema/placement/retrieval, Pithos client + distillation, Kafka `document.completed` feeder, `memory.command` consumer + hard reset, metrics publisher, introspection MCP tools, daemon assembly, Dockerize/deploy), plus later `mcp` stdio mode and an OpenRouter LLM/embeddings switch. ~54 unit/integration tests. **Slice 13 (the Pharos CT-scan view) is also built** — it lives in `../pharos/admin/` (routes `/memory` + soft/hard reset), not in this crate. The one open item is **merging `feature/v2-dual-memory` into `master`** (still ~16 commits ahead). The sections below describe the built system; the "Roadmap" framing further down is retained as historical design context.

## Common Commands

```bash
cargo build                       # debug build
cargo build --release             # release binary
cargo run -- mcp                  # one-shot MCP server (reads JSON-RPC from stdin)
cargo run -- daemon               # long-running daemon (MCP + Kafka feeder + command consumer + scheduler)
cargo test                        # run all tests (~54)
cargo test --package neurolithe <name>   # run a single test
cargo fmt --all -- --check        # CI formatting check
cargo clippy --all-targets -- -D warnings   # CI lint (warnings = error)

# Container (slice 12)
docker compose up --build         # build + run jarvis-neurolithe, joined to jarvis-kafka_default
```

CI (`.github/workflows/ci.yml`) requires `cargo fmt`, `cargo clippy -D warnings`, and `cargo test` to all pass on every PR.

## Configuration

Runtime config is loaded by `infrastructure::config::AppConfig::load()` from (in precedence order):
1. `neurolithe.toml` in CWD (optional — see repo root for example)
2. Environment variables prefixed `NEUROLITHE__` with `__` separator (e.g. `NEUROLITHE__LLM__PROVIDER=gemini`)
3. Hard-coded defaults

LLM API keys come from environment: `OPENAI_API_KEY`, `GEMINI_API_KEY`, `ANTHROPIC_API_KEY`, or fallback `NEUROLITHE_API_KEY`. A `.env` file is auto-loaded via `dotenvy`. **Deploy note:** the current on-mini config (commit `ffe8767`, "Option A") points the OpenAI-compatible `custom` provider at **OpenRouter for both chat and embeddings**, superseding the earlier local-gemma pointer. The design target remains local nomic (Ollama, offline); if the deployed OpenRouter embedding model's dimension differs from `ltm.vector_dimension` (default 768), the `sqlite-vec` LTM table must be rebuilt (dimension is locked at DB init).

**V2 dual-store config (slice 1).** Config is split into two independent SQLite stores: `[stm]` (decaying engine, `path` = `neurolithe-stm.sqlite`, `vector_dimension` default 1536) and `[ltm]` (permanent tree, `path` = `neurolithe-ltm.sqlite`, `vector_dimension` default 768 for local nomic), plus `[kafka]`, `[pithos]`, `[sweep]`, `[metrics]`, `[feeder]`. `infrastructure::database::init_stores()` opens both (`MemoryStores`); STM runs its schema now, LTM schema lands in slice 3.

**Vector dimension is locked per store at DB init** (`stm.vector_dimension` / `ltm.vector_dimension`). Changing it on an existing store file will break the `sqlite-vec` table — that file must be deleted and rebuilt. The two dimensions are independent.

## Architecture (DDD layering — enforced)

The four layers in `src/` must respect dependency direction: `interfaces → application → domain ← infrastructure`. Domain has zero networking/DB code.

- **`domain/`** — Pure business logic. `models.rs` (Node/Edge/Episode/CclDefinition), `ltm.rs` (LTM tree types: `TreeNode`/`TreeEdge`/`Leaf`, kinds `spine`/`grown`/`inbox`/`leaf`), `ports.rs` (`MemoryRepository`, `LtmRepository`, `LlmClient` traits), `decay.rs` (exponential half-life math — **STM only**), `cognition/conflict_resolver.rs` (Tri-Modal adaptation: Assimilate / AccommodateModify / AccommodateCreate based on cosine-distance thresholds 0.15 / 0.35).
- **`infrastructure/`** — Adapters implementing the ports. `repository.rs` (SQLite impl of `MemoryRepository`, STM store), `ltm_repository.rs` (SQLite impl of `LtmRepository`, the permanent tree store), `schema.rs` (STM DDL: `episodes`, `nodes`, `edges`, `vec_nodes`, `fts_nodes`; LTM DDL: `tree_nodes`, `tree_edges` multi-parent DAG, `leaves`, `vec_ltm`, `fts_ltm`), `llm.rs` (OpenAI/Gemini/Anthropic/Custom/local clients via `reqwest`), `database.rs` (opens **both** STM + LTM connections, rusqlite + sqlite-vec init), `pithos_client.rs` (HTTP client for `pt://` text, tolerates 404), `metrics_publisher.rs` (`memory.metrics` Kafka snapshot), `config.rs`.
- **`application/`** — Use-case orchestration. `app.rs` (`NeurolitheApp` facade), `session_manager.rs` (per-session STM buffer, ~4 chars/token heuristic, compresses over threshold), `sleep.rs` (`SleepWorker`: extract facts → resolve conflicts → store, plus decay sweeps), `retrieval.rs` (STM hybrid search), `distill.rs` (LLM meaning extraction → summary/concept-hints/embedding), `ltm_placement.rs` (best-match-or-`inbox` placement + rolling summaries + tombstone re-roll), `ltm_retrieval.rs` (`ltm_map`/`ltm_expand`/`ltm_drill`/`recall_ltm`, reference-returning), `ingestion.rs` (per-document dual-write: LTM leaf + STM node via `ConflictResolver`), `reset_service.rs` (soft = STM only, hard = both + feeder offset reset), `introspection.rs` (read-only CT-scan queries), `monitoring.rs` (metrics snapshot computation), `scheduler.rs` (drives decay sweep + metrics intervals).
- **`interfaces/`** — Delivery only. `mcp_server.rs` + `mcp_types.rs` run the JSON-RPC loop; tools exposed: `push_dialogue`, `store_memory`, `query_memory`, `delete_tenant`, `export_tenant`, plus read-only introspection `memory_stats`, `health`, `stm_list`, `ltm_map`, `inspect_node`, `subtree`, `trace_dataId` (plus implicit `register_ccl` / `get_ccl_layers` via app). `kafka_feeder.rs` consumes `document.completed` (backfill from earliest, tombstone = forget, ADR-0004 retry/`dlq.memory`/`parking.lot`). `command_consumer.rs` consumes `memory.command` (soft/hard reset, hard guarded by a confirmation token).
- **`daemon.rs` / `main.rs`** — `main.rs` dispatches the `mcp` and `daemon` subcommands. `daemon.rs` assembles the long-running process: MCP server + Kafka feeder + command consumer + scheduler concurrently, one process owning both DB connections, with graceful shutdown.

### Key concepts to understand before editing

- **Episodes vs Nodes vs Edges.** Episodes are append-only ground-truth dialogue logs. Nodes are structured facts extracted by the Sleep pipeline (or written directly via `store_memory`). Edges are temporally-bounded relationships (`valid_from`/`valid_until`). All three are tenant-scoped via `tenant_id`.
- **Sleep pipeline** (`SleepWorker::process_episode`): LLM extracts `ExtractedFact`s with `relationships`, each fact + each relationship target goes through `ConflictResolver` (which calls `find_similar_nodes` and decides Assimilate/Modify/Create). Unknown CCLs are auto-registered with an LLM-generated description.
- **Cognitive Context Layers (CCL)** segregate memory by conceptual layer (`reality`, `dream`, `simulation`, …). Default is `"reality"`. Queries take a `ccl_filter`; nodes/edges store their own `ccl`. Layers are registered per-tenant in `ccl_registry`.
- **Adaptive forgetting.** `DecayEngine` applies `score * 0.5^(days/half_life)` (default half-life: 7 days, set in `main.rs`). Nodes with score < 0.1 flip to `archived`. Reads call `boost_relevance` to reset score to 1.0 — querying = reinforcement.
- **Hybrid retrieval.** `query_with_graph` combines vector similarity (`vec_nodes`) + BM25 keyword (`fts_nodes`), applies temporal + CCL filters, then expands 1-hop graph neighbors. Output (`MemoryResult`) is intentionally token-optimized — no internal IDs/scores leak to the LLM.
- **`unsafe impl Send/Sync for NeurolitheApp`** in `app.rs` is intentional: `rusqlite::Connection` isn't `Sync`, but the repository serializes access via `Mutex` internally. Be careful preserving this invariant when changing the repository layer.

## Contribution rules (from README)

- Never commit directly to `master`/`main` — use `feature/<name>` branches.
- Tests ship in the same PR as the feature. Domain logic gets unit tests; repository changes get integration tests against a real SQLite DB (use `tempfile` from dev-deps).

## Memory model — STM vs LTM (design intent)

NeuroLithe has **two distinct memory regimes** that must not be conflated:

- **Short-term / session memory (STM) — keep as-is.** Decay (`DecayEngine`) and the SleepWorker decay sweep (`run_decay_sweep` / `sweep_decay`) are *intentional, core* features here. STM is meant to stay **small**: unused facts fade via the half-life curve and flip to `archived` below the 0.1 threshold, with reads reinforcing via `boost_relevance`. This is the original NeuroLithe design (agent session memory) and these forgetting features are to be **kept**.
- **Long-term memory (LTM) — JARVIS, needs different logic.** JARVIS's permanent knowledge archive is a *different problem* and should NOT reuse the decay/sweep path. LTM likely lives in a **separate layer** (e.g. a dedicated CCL like `archive`, and/or a `decay_exempt`/`permanent` flag, possibly separate tables for document/chunk records). The forgetting curve must never touch LTM.

> ⚠️ **Terminology trap.** The existing architecture docs (`docs/src/architecture/long-term-memory.md`) already call the current nodes/edges/vec/FTS fact store "long-term memory." That is **not** the same thing as JARVIS LTM — that store still **decays** (its nodes carry `relevance_score` and are swept). So NeuroLithe has *three* layers to keep straight: session STM buffer (in-memory), the decaying "long-term" fact store (current docs' LTM), and the *new* permanent JARVIS archive (true non-decaying LTM, not yet built). When discussing LTM, always specify which.

**Historical note (resolved by V2).** The original audit (`AUDIT.md`, 2026-06-20, §8 JARVIS Gap Analysis) found the codebase was STM/decaying-store only — no permanent layer, no document model, no reference-returning retrieval, and `run_decay_sweep` never called. **V2 closed every one of those gaps:** the permanent LTM now lives in its own SQLite file (`ltm_repository.rs` + LTM schema) that the decay sweep physically cannot reach; documents flow in via the Kafka feeder + `ltm_placement` as tree leaves; `ltm_retrieval` is reference-returning (`dataId` + provenance); and the decay sweep is wired to `scheduler.rs`. Keep `AUDIT.md` for the historical gap analysis, but read it as "what V2 was built to fix," not current state.

## Roadmap — JARVIS integration (V2 built; historical context below)

> **✅ V2 BUILT.** [`V2-DESIGN.md`](V2-DESIGN.md) (dual memory: STM + LTM) + [`IMPLEMENTATION-V2.md`](IMPLEMENTATION-V2.md) (sliced TDD plan) are **implemented** on `feature/v2-dual-memory` (slices 0–12, plus later `mcp` stdio mode and an OpenRouter LLM/embeddings switch). V2 splits NeuroLithe into a fast **STM** (the decaying fact engine, own DB file) and a permanent **LTM knowledge tree** (own DB file), adds a **Kafka feeder** (`document.completed` → learns into both stores), **soft/hard reset**, and a **"brain CT scan"** introspection surface (`memory.metrics` topic + read-only MCP tools → Pharos). The remaining work is **merging `feature/v2-dual-memory` into `master`** — slice 13 (the Pharos CT-scan view: `memory.metrics` + introspection tools + reset buttons) is already built and committed in `../pharos/admin/`. The text below is retained as the design rationale.

NeuroLithe is being repurposed as the **brain of JARVIS**, a private, offline-first "second brain" that ingests and indexes *all*  information (scanned documents, email, WhatsApp, Telegram, tasks, files). JARVIS uses NeuroLithe not as short-lived agent memory but as a **permanent personal knowledge archive** — giving it a second nature alongside the existing decaying working memory.

Crucially, JARVIS LTM is **additive, not a replacement**: STM (decay + sweep) stays intact for session memory; LTM is built alongside it as a separate, non-decaying regime.

Full context: `../JARVIS/docs/overview.md` and `../JARVIS/docs/components/memory-service.md`.

> **Design brainstorm (parked):** [`JARVIS-MEMORY-TREE.md`](JARVIS-MEMORY-TREE.md) — structuring LTM as a growing **knowledge tree of the person** (poly-hierarchy + rolling summaries + `dataId` leaves; fuzzy-brain-vs-exact-record principle; curated spine + AI-grown leaves). Read this when we start the memory-tree work.

Planned features (mapped to current modules):

1. **Permanent (non-decaying) memory.** Archive items must be exempt from `DecayEngine` — never decay, never flip to `archived`. *(Naming clash to keep in mind: NeuroLithe's `archived` status = forgotten; JARVIS "archive" = permanently kept.)* Likely approach: a `permanent` / `decay_exempt` flag on nodes that the decay sweep (`SleepWorker` background pass) skips, and/or a dedicated CCL (e.g. `archive`) treated as decay-exempt. The forgetting curve must never touch these.

2. **Document/item records (richer than dialogue facts).** JARVIS items carry an external `dataId` (prefixed UUIDv7), title, date, type (`image`/`email`/`msg`/…), controlled **tags**, **artifact URIs** (image/text/pdf/summary held in JARVIS's archive service), `links`, and content. Needs a catalog-record model and an ingestion path for *documents* — not only `push_dialogue` / `store_memory` facts — e.g. a `store_document` / `archive_item` MCP tool.

3. **Controlled tag vocabulary + tag map.** Tags come from an approved list (no free text), with synonyms and **many-to-many parent links** (a tag can have several parents — e.g. `rrsp` under both `tax` and `retirement`); searching a tag includes its children. Maps naturally onto existing **graph edges + 1-hop expansion** (tags as nodes, `parent_of` as edges) plus a synonym/normalization layer and vocabulary enforcement.

4. **Chunking for long content.** Embed documents as **passages/chunks** `(chunkId, dataId, span, vector)`, not single fact-nodes — with a chunk→item reference.

5. **Reference-returning retrieval.** JARVIS retrieval must return **`dataId` + artifact URIs + provenance** (`exact` vs `related`) so the app can fetch originals — the opposite of the current token-optimized `MemoryResult` that deliberately hides IDs. Add a retrieval mode that surfaces references for human-in-the-loop selection, with filters for date / type / tag (with child expansion).

6. **Local embeddings (offline).** Point the embedding provider at a **local** model (Ollama, e.g. `nomic-embed-text`) to honor JARVIS's no-cloud-egress constraint. Choose the **vector dimension up front** (768 for nomic) — it's locked at DB init.

**Reuses well as-is:** hybrid vector + BM25 retrieval, graph edges (for tags), CCL layering, local-LLM config. **New work concentrates in:** decay-exemption, controlled tag vocabulary, document/chunk model, and reference-returning retrieval.
