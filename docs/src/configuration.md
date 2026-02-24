# Configuration

NeuroLithe is configured via a `neurolithe.toml` file and environment variables.

## neurolithe.toml

```toml
[llm]
# Provider: "openai", "gemini", "anthropic", or "custom"
provider = "custom"

# Chat model for fact extraction and context compression
model = "openai/gpt-4o-mini"

# Embedding model for vector search
embedding_model = "openai/text-embedding-3-small"

# Base URL (required for "custom" provider)
base_url = "https://openrouter.ai/api/v1"

[database]
# Path to the SQLite database file
path = "neurolithe.sqlite"

# Embedding vector dimension (must match your model)
# OpenAI text-embedding-3-small: 1536
# Google text-embedding-004: 768
# Nomic nomic-embed-text: 768
vector_dimension = 1536
```

## Environment Variables

| Variable | Description |
|----------|------------|
| `NEUROLITHE_API_KEY` | Universal API key (works for any provider) |
| `OPENAI_API_KEY` | OpenAI-specific key (fallback) |
| `GEMINI_API_KEY` | Google Gemini-specific key (fallback) |
| `ANTHROPIC_API_KEY` | Anthropic-specific key (fallback) |

Environment variables can also override config file values using the `NEUROLITHE__` prefix with `__` as separator:

```bash
export NEUROLITHE__LLM__PROVIDER=gemini
export NEUROLITHE__DATABASE__PATH=/data/memory.sqlite
```

## Provider Examples

### OpenRouter (Recommended)

```toml
[llm]
provider = "custom"
model = "openai/gpt-4o-mini"
embedding_model = "openai/text-embedding-3-small"
base_url = "https://openrouter.ai/api/v1"
```

### Google Gemini

```toml
[llm]
provider = "gemini"
model = "gemini-2.5-flash"
embedding_model = "text-embedding-004"
```

### Local (Ollama / LM Studio)

```toml
[llm]
provider = "custom"
model = "meta-llama/Llama-3-8b-chat-hf"
embedding_model = "nomic-embed-text"
base_url = "http://localhost:11434/v1"
```

> ⚠️ **Important:** Changing `vector_dimension` on an existing database will cause errors. Delete the database file to resize.
