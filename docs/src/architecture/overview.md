# System Overview

NeuroLithe follows a **Domain-Driven Design (DDD)** architecture in Rust:

```
src/
├── domain/                  # Pure business logic
│   ├── models/              # Node, Edge, Episode, TimeFilter
│   ├── ports/               # MemoryRepository, LlmClient traits
│   └── cognition/           # ConflictResolver
├── application/             # Use-case orchestration
│   ├── session_manager.rs   # STM Context Compressor
│   ├── sleep.rs             # Sleep Pipeline (Extract → Adapt → Decay)
│   ├── retrieval.rs         # Hybrid Search orchestration
│   └── app.rs               # Main application facade
├── infrastructure/          # External adapters
│   ├── database.rs          # rusqlite + sqlite-vec init
│   ├── schema.rs            # SQL schema + FTS5 triggers
│   ├── repository.rs        # SQLite MemoryRepository impl
│   ├── llm.rs               # OpenAI / Gemini / Anthropic clients
│   └── config.rs            # TOML + env config loading
└── interfaces/              # Delivery mechanism
    └── mcp_server.rs        # MCP JSON-RPC STDIO server
```

## Data Flow

```
Agent ──push_dialogue──→ SessionManager ──→ Episode (Ground Truth)
                              │                    │
                              ├──→ Buffer/Compress  │
                              │                    ▼
                              └──→ ContextWindow   SleepWorker
                                                    │
                                              ConflictResolver
                                                    │
                                              Nodes + Edges
                                                    │
                                              Vec + FTS Index
```

## SQLite Schema

The database uses 5 core tables:

| Table | Purpose |
|-------|---------|
| `episodes` | Ground-truth raw dialogue logs |
| `nodes` | Structured facts with cognitive attributes |
| `edges` | Relationships with temporal bounds |
| `vec_nodes` | sqlite-vec embeddings for semantic search |
| `fts_nodes` | FTS5 index for keyword search |
