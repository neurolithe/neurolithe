# Changelog

All notable changes to NeuroLithe are documented here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/); versions follow SemVer.

## [0.2.0] — 2026-07-10

A reliability release driven by real-world MCP testing: search actually works
now, and NeuroLithe is **standalone by default** — install the binary, run
`neurolithe mcp`, and its MCP server is ready with no Kafka or broker.

### Changed

- **Standalone by default; Kafka is opt-in.** The default build is just the
  embedded stores + the MCP server — **no `rdkafka`/librdkafka**, so it compiles
  and installs cleanly on every platform (fixing the release build that failed on
  the Kafka dependency, and restoring the prebuilt **Windows** binary). The full
  JARVIS daemon (feeder + `memory.command`/`memory.query` + metrics) is now
  behind a `kafka` cargo feature (`cargo build --release --features kafka`); the
  Docker image enables it. `neurolithe mcp` is the standalone entry point.

### Fixed

- **`query_memory` results carried no `data_id` (round 2).** Search hits now
  include the archive `data_id` (both the MCP `MemoryResult` and the bus
  `StmEntry`), so an agent goes straight from a hit to fetching the source
  instead of a second `stm_list` scan — closing the search → trace → fetch loop.
- **Ranking ignored match quality (round 2).** Results were ordered by decay
  `relevance_score`, so once reads reinforced several facts to 1.0 the best hit
  no longer ranked first (a reinforced but weaker keyword match outranked a
  stronger one). Now ranked by the hybrid vector+keyword score — direct matches
  first, graph neighbours after, relevance only as a tiebreak.
- **`query_memory` returned nothing over MCP (tenant mismatch).** The feeder
  ingests under tenant `jarvis`, but the MCP door defaulted queries to `default`,
  so the tenant filter dropped every row. The MCP door now routes through the
  **same `QueryService`** as the `memory.query` bus door and both default to one
  shared `DEFAULT_TENANT` constant, so they can't drift again.
- **Hybrid keyword search failed to zero.** The raw query was passed straight to
  FTS5 (implicit-AND; syntax error on punctuation). It's now sanitized into an
  OR of quoted terms, so search degrades to keyword-any instead of erroring or
  over-matching; the keyword leg is skipped (vector-only) when there are no terms.
- **Document summaries truncated ~2,500 chars.** Anthropic `compress_context`
  capped output at 1024 tokens; raised to 8192 so long-document summaries
  complete instead of cutting off mid-word.
- **Every document landed in the inbox.** The spine was seeded with no
  embeddings, so placement could never match a concept. The daemon now embeds
  each concept from its curated identity at startup, and seeding is **additive**
  with generic personal-life branches (health, home, vehicles, insurance,
  family, admin, …). The placement distance threshold was then **tuned from
  measured data** (new `placement_debug` CT-scan tool): real document→concept
  distances cluster ~1.05 (cosine ~0.4) for normalized `text-embedding-004`, so
  the threshold is 1.10 — earlier guesses (0.5, 0.85) sat below the whole
  distribution and filed 100% to the inbox.
- **Inbox gardener.** A startup pass re-homes inbox documents that now match a
  concept, using their **stored** embeddings — so a threshold change or a new
  branch re-files existing docs with **no replay and no LLM calls** (idempotent;
  the ambiguous tail stays in the inbox). Also exposed as `garden_inbox`.
- **Hard reset silently re-broke placement.** `hard_reset` re-seeds the spine but
  seeding leaves concepts un-embedded, so a post-reset replay filed everything
  back into the inbox. The command consumer now **re-embeds the spine after a
  hard reset** (before rewinding the feeder), so a reset → replay rebuilds a
  correctly-filed tree.
- **Personal data removed.** The seed root node is renamed from a personal name
  to the generic `"root"`, and all test fixtures use fictional data — the crate
  is reusable and carries no PII (it is a public repository).
- **Rolling summaries were degenerate.** A container's summary was one child's
  truncated text (which propagated to the root). Container summaries now describe
  the collection (`"N items: title; title; …"`).

### Added

- **`placement_debug` CT-scan tool** — reports, for a sample of document leaves, the distance to their nearest concept, so the placement threshold can be tuned to real embedding distances instead of guessed.
- **`recall_ltm` MCP tool** — reference-returning search of the permanent archive
  (dataId + provenance + ancestor concepts), the primary way to find a document.
- **Leaf titles + `ingested_at`** — leaves get a human title from the summary's
  first line (not the raw dataId), and `provenance.ingested_at` is populated from
  the leaf's creation time.
- **CT-scan ergonomics** — `stm_list` gains `offset` pagination and a `contains`
  substring filter; `inspect_node` pages children/leaves (`child_limit`/
  `child_offset`), caps summaries (`summary_max_chars`), and reports
  `child_count`/`leaf_count`. `feeder_lag: -1` is documented as "unknown".

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

[0.2.0]: https://github.com/rezangit/neurolithe/releases/tag/v0.2.0
[0.1.2]: https://github.com/rezangit/neurolithe/releases/tag/v0.1.2
[0.1.1]: https://github.com/rezangit/neurolithe/releases/tag/v0.1.1
[0.1.0]: https://github.com/rezangit/neurolithe/releases/tag/v0.1.0
