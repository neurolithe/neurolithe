# Quickstart Guide

Get NeuroLithe running in under 5 minutes.

## Installation

### One-Line Installer (Recommended)

**macOS / Linux:**

```bash
curl -fsSL https://raw.githubusercontent.com/neurolithe/neurolithe/main/install.sh | bash
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/neurolithe/neurolithe/main/install.ps1 | iex
```

This will automatically:

- Download the correct binary for your OS and architecture
- Create a default `neurolithe.toml` config
- Prompt for your API key
- Generate a ready-to-paste MCP config for Claude Desktop / Cursor
- Add `neurolithe` to your PATH

### From Source

```bash
git clone https://github.com/neurolithe/neurolithe.git
cd neurolithe
cargo build --release
```

The binary will be at `target/release/neurolithe`.

### From Crates.io (coming soon)

```bash
cargo install neurolithe
```

## Configuration

Create a `neurolithe.toml` file in your working directory:

```toml
[llm]
provider = "custom"
model = "openai/gpt-4o-mini"
embedding_model = "openai/text-embedding-3-small"
base_url = "https://openrouter.ai/api/v1"

[database]
path = "neurolithe.sqlite"
vector_dimension = 1536
```

Set your API key:

```bash
export NEUROLITHE_API_KEY="your-api-key-here"
```

## Connect to Your AI Agent

NeuroLithe communicates over **STDIO** using the Model Context Protocol (MCP). Add it to your MCP client config:

### Claude Desktop / Cursor

```json
{
  "mcpServers": {
    "neurolithe": {
      "command": "/path/to/neurolithe",
      "args": []
    }
  }
}
```

## Try It Out

Once connected, your AI agent can use these tools:

1. **Store a fact:** `store_memory({fact_text: "User prefers dark mode", tags: ["preference"]})`
2. **Query memory:** `query_memory({query: "What does the user prefer?"})`
3. **Push dialogue:** `push_dialogue({session_id: "chat-1", new_message: "I just moved to Berlin"})`

The system will automatically extract facts, build a knowledge graph, and return relevant context when queried.
