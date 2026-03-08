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

**NeuroLithe** is built in 🦀 Rust to solve the **context memory problem** for AI agents. By providing both strictly managed **short-term memory** (STM) and unbounded, highly-retrievable **long-term memory** (LTM) via hybrid vector/keyword search, it allows intelligent systems to recall past interactions without drowning the LLM prompt in full conversation history.

<p align="center">
<strong><a href="#-quick-start">Quick Start</a> • <a href="#-features">Features</a> • <a href="#-tech-stack">Tech Stack</a> • <a href="#-contributing">Contributing</a> • <a href="https://docs.neurolithe.com">Documentation</a></strong>
</p>

## 🚀 Quick Start

### 1. Installation

**macOS / Linux:**

```bash
curl -fsSL https://raw.githubusercontent.com/neurolithe/neurolithe/main/install.sh | bash
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/neurolithe/neurolithe/main/install.ps1 | iex
```

> [!NOTE]
> This automatically downloads the latest binary, creates config files, prompts for your LLM API key, and provides a ready-to-use MCP configuration snippet.

<details>
<summary><b>Install from source</b></summary>
<br>

Ensure you have [Rust](https://rustup.rs/) installed:

```bash
git clone https://github.com/neurolithe/neurolithe.git
cd neurolithe
cargo install --path .
```

</details>

### 2. Connect to Your AI Agent

Add NeuroLithe to your MCP client (*Claude Desktop, Cursor, etc.*). The install script generates exactly this config for you at `~/.neurolithe/mcp-config.json` — simply copy and paste it into your client configurations.

```json
{
  "mcpServers": {
    "neurolithe": {
      "command": "~/.neurolithe/bin/neurolithe",
      "args": [],
      "env": {
        "NEUROLITHE_API_KEY": "your-api-key"
      }
    }
  }
}
```

## ✨ Features

- 📉 **Lower LLM Costs:** Minimize token usage by passing only the most relevant, retrieved information to the LLM during communication.
- ⚡ **Increased Efficiency:** Give the AI exactly the right amount of context it needs to perform tasks effectively, avoiding context window limits and reducing hallucinations.
- 🧠 **Hybrid Storage Model:**
  - **Short-Term Memory (STM):** Tracks recent, relevant dialogue. High priority for task context.
  - **Long-Term Memory (LTM):** As STM fills up, old interactions are *compressed* into long-term factual nodes using SQLite + semantic vector embeddings (`sqlite-vec`).
- 🗄️ **Seamless Integration:** Runs locally as an embedded database, meaning zero external infrastructure to manage.
- ⏱️ **Adaptive Forgetting Curve:** Simulates human memory dynamics. Unused or unimportant factual nodes gradually decay over time unless reinforced.
- 🌌 **Cognitive Context Layers (CCL):** Segregate memories by conceptual layers (e.g., 'reality', 'dream', 'simulation') to prevent AI hallucination and enable advanced counterfactual reasoning during hybrid searches.

## 🛠️ Tech Stack

NeuroLithe is built for speed, safety, and conciseness using modern technologies:

- **Language:** [Rust](https://www.rust-lang.org/) — Ensuring memory safety, high performance, and fearless concurrency.
- **Database:** [SQLite](https://sqlite.org/) + `rusqlite` — Fast, file-based SQL database optimized with WAL mode.
- **Vector Search:** `sqlite-vec` & FTS5 — Powering hybrid search (semantic vector embeddings + BM25 full-text search) natively in SQL.
- **Async Runtime:** `tokio` — Handling concurrent operations efficiently.
- **LLM Integration:** `reqwest` & `serde` — For fast asynchronous communication with OpenAI to generate text embeddings and extract factual models.
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
