<div align="center">
  <h1>🧠 NeuroLithe</h1>
  <p><strong>A fast, embedded, and efficient contextual memory database for AI agents.</strong></p>
  <p>🌐 <a href="https://neurolithe.com">neurolithe.com</a></p>
</div>

<hr/>

## 🎯 Goal of the Project

NeuroLithe is designed to solve the **context memory problem** for AI agents. By providing both **short-term and long-term memory**, it allows AI systems to recall past interactions without needing to inject the entire conversation history into the prompt.

**Key Benefits:**

- 📉 **Lower LLM Costs:** Minimize token usage by passing only the most relevant, retrieved information to the LLM during communication.
- ⚡ **Increased Efficiency:** Give the AI exactly the right amount of context it needs to perform tasks effectively, avoiding context window limits and reducing hallucinations.
- 🗄️ **Seamless Integration:** Runs locally as an embedded database, meaning zero external infrastructure to manage.

## 🛠️ Tech Stack

NeuroLithe is built for speed, safety, and conciseness using modern technologies:

- **Language:** [Rust](https://www.rust-lang.org/) — Ensuring memory safety, high performance, and fearless concurrency.
- **Database:** [SQLite](https://sqlite.org/) + `rusqlite` — Fast, file-based SQL database optimized with WAL mode.
- **Vector Search:** `sqlite-vec` & FTS5 — Powering hybrid search (semantic vector embeddings + BM25 full-text search) natively in SQL.
- **Async Runtime:** `tokio` — Handling concurrent operations efficiently.
- **LLM Integration:** `reqwest` & `serde` — For fast asynchronous communication with OpenAI to generate text embeddings and extract factual models.
- **Protocol:** [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) — Operating seamlessly as an intelligent MCP server over standard input/output (STDIO).

## 🚀 Quick Install

**macOS / Linux:**

```bash
curl -fsSL https://raw.githubusercontent.com/neurolithe/neurolithe/main/install.sh | bash
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/neurolithe/neurolithe/main/install.ps1 | iex
```

This automatically downloads the binary, creates config files, prompts for your API key, and generates a ready-to-paste MCP config.

### From Source

```bash
git clone https://github.com/neurolithe/neurolithe.git
cd neurolithe
cargo build --release
```

### Connect to Your AI Agent

Add NeuroLithe to your MCP client (Claude Desktop, Cursor, etc.):

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

> 💡 The install script generates this config at `~/.neurolithe/mcp-config.json` — just copy and paste it.

### Configuration

See `~/.neurolithe/neurolithe.toml` (or the [Configuration docs](https://docs.neurolithe.com/configuration.html)) for all options.

**📖 Full documentation:** [docs.neurolithe.com](https://docs.neurolithe.com)

## 🤝 Contributing

We welcome contributions! NeuroLithe is built using **Domain-Driven Design (DDD)**. To keep the project clean, scalable, and testable, contributors must adhere to this architectural pattern.

### Project Structure (DDD)

- **`src/domain/`**: The core of the application. Contains business models, logic (e.g., decay math), and interfaces (`ports`). Zero external networking or database logic belongs here.
- **`src/infrastructure/`**: Concrete implementations of the `ports`. This is where `rusqlite` database connections, `reqwest` LLM clients, and raw SQL schemas live.
- **`src/application/`**: Use cases and orchestrators (like `RetrievalService` or `SleepWorker`). This layer wires the domain and infrastructure together.
- **`src/interfaces/`**: The outer boundary. Contains the MCP server, JSON-RPC parsing, and STDIO handlers.

### Contribution Guidelines

1. **Test Early, Test Often:** We expect comprehensive unit tests within your modules alongside integration tests targeting the database. Include tests *in the same PR* as your feature.
2. **Feature Branches:** Never commit directly to `master`/`main`. Create a descriptive branch from the latest root:

   ```bash
   git checkout -b feature/your-feature-name
   # or
   git checkout -b fix/issue-description
   ```

3. **Pull Requests:**
   - Fork the repository.
   - Push your feature branch to your fork.
   - Open a Pull Request outlining *what* changed and *why*. Ensure all tests pass (`cargo test`) before requesting a review.

## 📝 License

MIT License
