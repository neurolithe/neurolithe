# Changelog

All notable changes to NeuroLithe are documented here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/); versions follow SemVer.

## [0.1.2] — 2026-07-03

A large release: NeuroLithe grows from a single decaying store into a **dual
memory** brain (decaying STM + permanent LTM), gains a **daemon** mode with a
Kafka-native memory API, and a **connected working-memory graph** so an agent
can reconstruct what it just did.

### Added

- **V2 dual-memory architecture** — two independent SQLite stores: a decaying
  **STM** fact engine and a permanent, non-decaying **LTM** knowledge tree
  (concept hierarchy with document leaves). The forgetting curve can never touch
  LTM. Each store locks its own vector dimension at init.
- **Daemon run mode** — `neurolithe daemon`: a Kafka feeder consumes
  `document.completed` and dual-writes each item into STM + LTM; a **bus memory
  API** answers `memory.query` → `memory.result` and applies `memory.command`
  (remember / forget / soft+hard reset); a `memory.metrics` snapshot + read-only
  introspection tools give a "CT scan" of the brain. Dockerized
  (`docker-compose.yml`).
- **Working memory (situational awareness)** — a connected **session graph**:
  the agent's recent *turns* linked by `about` edges to the documents/entities
  they touched, with a **focus** so follow-ups ("what is *its* id?") resolve
  from context. New `working` CCL with its own short half-life; recency-first
  recall.
- **Configurable, splittable LLM** — chat and embeddings can use different
  providers: OpenAI, Google (Gemini / Vertex AI `text-embedding-004`), Anthropic
  Claude, or fully local/offline via Ollama (`nomic-embed-text`).
- **Reference-returning LTM recall** — results carry `dataId` + provenance so the
  caller can fetch originals.
- `CHANGELOG.md`.

### Changed

- STM and LTM are now **distinct regimes** (previously STM "compressed into"
  LTM). LTM is permanent; STM decays.
- **Decay is real-elapsed and per-layer** — a note decays by its true age since
  last touched (not a fixed one-day-per-sweep pass); reads reset the clock. A
  sweep or restart no longer wipes freshly-written memory. `working` notes fade
  in minutes–hours, durable facts over days.
- The **working-memory map is pure context recency** (no vector search) —
  semantic/knowledge recall stays behind the explicit `memory.query` tool.
- **MCP config must pass `args: ["mcp"]`.** Running the binary with no subcommand
  now starts the **daemon**.

### Fixed

- **Installers** wrote a V1 config (`[database]`, fixed 1536 dim) that mismatched
  the V2 stores' dimension → now write valid `[stm]` / `[ltm]` sections.
- **Install one-liner** pointed at a nonexistent `main` branch (404) → `master`.
- LTM recall could return empty (documents indexed in a separate `vec_leaves`
  table now searched on recall).
- `stm_map` semantic enrichment dredged unrelated documents/other threads into
  the situational map → scoped out.

## [0.1.1] — 2026-03-08

- Hybrid vector + FTS retrieval, adaptive forgetting curve, Cognitive Context
  Layers, MCP server over STDIO, one-line installers, release binaries.

## [0.1.0] — 2026-02-24

- First release.

[0.1.2]: https://github.com/neurolithe/neurolithe/releases/tag/v0.1.2
[0.1.1]: https://github.com/neurolithe/neurolithe/releases/tag/v0.1.1
[0.1.0]: https://github.com/neurolithe/neurolithe/releases/tag/v0.1.0
