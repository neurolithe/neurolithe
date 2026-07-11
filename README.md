<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="content/image/neurolithe-logo-full.png">
    <img width="500" alt="NeuroLithe" src="content/image/neurolithe-logo-full.png">
  </picture>
</p>

<p align="center">
  <b>A fast, embedded, and efficient contextual memory database for AI agents.</b>
</p>

<p align="center">
  <a href="https://github.com/neurolithe/neurolithe/actions"><img src="https://img.shields.io/github/actions/workflow/status/neurolithe/neurolithe/ci.yml?branch=master&color=cyan" alt="Build Status"></a>
  <a href="https://github.com/neurolithe/neurolithe/releases"><img src="https://img.shields.io/github/v/release/neurolithe/neurolithe?color=cyan" alt="Release"></a>
  <a href="https://github.com/neurolithe/neurolithe/blob/master/LICENSE"><img src="https://img.shields.io/badge/License-MIT-cyan.svg" alt="License"></a>
  <a href="https://docs.neurolithe.com"><img src="https://img.shields.io/badge/docs-neurolithe.com-cyan.svg" alt="Docs"></a>
  <a href="https://neurolithe.com"><img src="https://img.shields.io/badge/website-neurolithe.com-cyan.svg" alt="Website"></a>
</p>

**NeuroLithe** is built in 🦀 Rust to solve the **context memory problem** for AI agents. It gives an agent **two distinct memory regimes** — a fast, *decaying* **short-term memory** (STM) for "what's happening right now," and a permanent, non-decaying **long-term memory** (LTM) knowledge tree — both searchable by meaning (`sqlite-vec`) and by keyword (FTS5), so an agent recalls the right context without drowning the prompt in full history.

Run it two ways: as an embedded **MCP server** over STDIO (drop-in agent memory), or as a long-running **daemon** that feeds itself from a Kafka event stream and answers memory queries over the bus.

<p align="center">
<strong><a href="#-quick-start">Quick Start</a> • <a href="#-features">Features</a> • <a href="#-tech-stack">Tech Stack</a> • <a href="#-contributing">Contributing</a> • <a href="https://docs.neurolithe.com">Documentation</a></strong>
</p>

## 🚀 Quick Start

### 1. Installation

**macOS / Linux:**

```bash
curl -fsSL https://raw.githubusercontent.com/neurolithe/neurolithe/master/install.sh | bash
```

> [!NOTE]
> This automatically downloads the latest binary (macOS Intel/Apple-Silicon, Linux x86_64), creates config files, prompts for your LLM API key, and provides a ready-to-use MCP configuration snippet.

> [!NOTE]
> The published binaries (macOS Intel/Apple-Silicon, Linux x86_64, **Windows**)
> are the **standalone** build — just the embedded stores + MCP server, no Kafka.
> Nothing to install or run besides the binary.

<details>
<summary><b>Install from source</b></summary>
<br>

Ensure you have [Rust](https://rustup.rs/) installed:

```bash
git clone https://github.com/neurolithe/neurolithe.git
cd neurolithe
cargo install --path .                    # standalone MCP server (default)
cargo install --path . --features kafka   # + the Kafka event-driven daemon
```

The default build needs no Kafka/`librdkafka` toolchain — only a C compiler for
bundled SQLite. Add `--features kafka` only if you want the daemon.

</details>

### 2. Connect to Your AI Agent

Add NeuroLithe to your MCP client (*Claude Desktop, Cursor, etc.*). The install script generates exactly this config for you at `~/.neurolithe/mcp-config.json` — simply copy and paste it into your client configurations.

```json
{
  "mcpServers": {
    "neurolithe": {
      "command": "~/.neurolithe/bin/neurolithe",
      "args": ["mcp"],
      "env": {
        "NEUROLITHE_API_KEY": "your-api-key"
      }
    }
  }
}
```

> [!NOTE]
> The `mcp` subcommand runs the **MCP server** over STDIO — this is the default,
> standalone mode. The event-driven **daemon** is an opt-in build
> (`--features kafka`) — see [Run modes](#-run-modes).

## 🔀 Run modes

NeuroLithe is **standalone by default** — the MCP server needs no Kafka:

```bash
neurolithe mcp      # MCP server over STDIO (embedded agent memory) — always available
neurolithe daemon   # long-running: MCP + Kafka feeder + bus memory API + scheduler
                    #   (only in builds compiled with --features kafka)
```

- **`mcp`** — the drop-in option for an MCP client (Claude Desktop, Cursor, …).
  No Kafka, no broker; just `[llm]` + `[stm]` + `[ltm]` config. This is what the
  published binaries do.
- **`daemon`** — the event-driven brain: consumes `document.completed`, distills
  each item into STM + LTM, answers `memory.query`→`memory.result`, applies
  `memory.command` (remember/forget/reset), and publishes a `memory.metrics`
  CT-scan. Needs a Kafka broker (see [`docker-compose.yml`](docker-compose.yml))
  and a build with `--features kafka`.

> A standalone build runs only `neurolithe mcp`; invoking it without `mcp` prints
> a hint. A `--features kafka` build starts the daemon when run with no subcommand.

## ✨ Features

- 🧠 **Dual-memory architecture (V2):** two independent SQLite stores, so the two problems don't fight each other:
  - **Short-Term Memory (STM)** — a fast, *decaying* fact engine for recent, relevant context. Facts fade on a half-life curve unless reinforced.
  - **Long-Term Memory (LTM)** — a permanent, *non-decaying* knowledge tree. Documents/notes are placed as leaves under a growing concept hierarchy; the forgetting curve never touches it.
- 🔎 **Hybrid retrieval:** semantic vector search (`sqlite-vec`) + BM25 keyword search (FTS5) + 1-hop graph traversal, natively in SQL. LTM recall is *reference-returning* (`dataId` + provenance) so you can fetch originals.
- 🧭 **Working memory (situational awareness):** a connected **session graph** — the agent's recent *turns* linked by `about` edges to the documents/entities they touched, with a **focus** so follow-ups like "what is *its* id?" resolve from context, not a fuzzy re-search.
- ⏱️ **Adaptive forgetting curve:** real-elapsed, **per-layer** exponential decay — situational notes fade in minutes-to-hours, durable facts over days; reads reinforce (reset the clock). A sweep or restart never wipes fresh memory.
- 🌌 **Cognitive Context Layers (CCL):** segregate memories by conceptual layer (`reality`, `working`, `dream`, `simulation`, …) to prevent cross-talk and enable counterfactual reasoning.
- 🔌 **Two run modes:** an embedded **MCP server** over STDIO (drop-in agent memory) *or* a long-running **daemon** that feeds itself from Kafka (`document.completed`) and serves reads/writes over the bus (`memory.query` / `memory.command`). See [Run modes](#-run-modes).
- 🩻 **Introspection ("CT scan"):** read-only tools + a `memory.metrics` snapshot to see exactly what the brain holds (STM/LTM counts, sizes, layers).
- 🛠️ **Bring your own LLM:** chat + embeddings are configurable and can differ — OpenAI, Google (Gemini / Vertex `text-embedding-004`), Anthropic Claude, or fully **local/offline** via Ollama (`nomic-embed-text`). Keys via env / `.env`.
- 🗄️ **Zero external infra (MCP mode):** runs locally as an embedded database — nothing to manage. (The daemon mode adds Kafka when you want the event-driven pipeline.)

## 🛠️ Tech Stack

NeuroLithe is built for speed, safety, and conciseness using modern technologies:

- **Language:** [Rust](https://www.rust-lang.org/) — Ensuring memory safety, high performance, and fearless concurrency.
- **Database:** [SQLite](https://sqlite.org/) + `rusqlite` — Fast, file-based SQL database optimized with WAL mode.
- **Vector Search:** `sqlite-vec` & FTS5 — Powering hybrid search (semantic vector embeddings + BM25 full-text search) natively in SQL.
- **Async Runtime:** `tokio` — Handling concurrent operations efficiently.
- **LLM Integration:** `reqwest` & `serde` — Provider-agnostic clients for OpenAI, Google (Gemini / Vertex AI), Anthropic Claude, and local OpenAI-compatible endpoints (Ollama). Chat and embedding providers are configured independently.
- **Event Backbone (daemon, optional):** `rdkafka` behind the `kafka` feature — consumes `document.completed` and serves a request/reply memory API over Kafka. Not compiled into the standalone build.
- **Protocol:** [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) — Operating seamlessly as an intelligent MCP server over standard input/output (STDIO).

## 🤝 Contributing

We welcome contributions! NeuroLithe is built using **Domain-Driven Design (DDD)**. To keep the project clean, scalable, and testable, contributors must adhere to this architectural pattern.

<details>
<summary><b>View Project Architecture Overview</b></summary>
<br>

- **`src/domain/`**: The core of the application. Contains business models, logic (e.g., decay math), and interfaces (`ports`). Zero external networking or database logic belongs here.
- **`src/infrastructure/`**: Concrete implementations of the `ports`. This is where `rusqlite` database connections, `reqwest` LLM clients, and raw SQL schemas live.
- **`src/application/`**: Use cases and orchestrators (like `RetrievalService` or `SleepWorker`). This layer wires the domain and infrastructure together.
- **`src/interfaces/`**: The outer boundary. Contains the MCP server, JSON-RPC parsing, and STDIO handlers.

</details>

### Contribution Guidelines

1. **Test Early, Test Often:** We expect comprehensive unit tests within your modules alongside integration tests targeting the database. Include tests *in the same PR* as your feature.
2. **Feature Branches:** Never commit directly to `master`/`main`. Create a descriptive branch from the latest root:

   ```bash
   git checkout -b feature/your-feature-name
   ```

3. **Pull Requests:** Open a Pull Request outlining *what* changed and *why*. Ensure all tests pass (`cargo test`) before requesting a review.

## 📝 License

MIT License
