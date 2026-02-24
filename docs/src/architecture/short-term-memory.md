# Short-Term Memory

The Short-Term Memory (STM) system acts as an **Active Context Manager** that eliminates LLM context window bloat while preserving critical information.

## The Push & Learn Flow

1. **Push** — The AI agent pushes a new message via `push_dialogue`
2. **Archive** — The raw dialogue is permanently saved to the `episodes` table (ground truth)
3. **Buffer** — The message is added to the session's in-memory buffer
4. **Token Check** — The service checks the current token count
5. **Compress** — If tokens exceed the threshold, old messages are summarized into a dense summary via LLM
6. **Learn** — The message is queued for background fact extraction (Sleep Pipeline)
7. **Return** — An optimized `ContextWindow` is returned:

```json
{
  "summary": "User discussed Rust programming and memory safety...",
  "recent_messages": ["What about borrowing rules?", "..."],
  "relevant_facts": [
    {
      "fact": "User is learning Rust",
      "last_updated": "2026-02-23",
      "connections": [
        {"relation": "INTERESTED_IN", "entity": "Memory Safety", "valid_from": null, "valid_until": null}
      ]
    }
  ]
}
```

## Ground-Truth Preservation

Before any raw dialogue is summarized and flushed from the active buffer, it is permanently saved to the `episodes` table. This ensures the original context is never lost, allowing for re-derivation if the AI makes a flawed extraction.

## Configuration

The session manager uses these defaults:

- **Token threshold:** ~4000 tokens before compression triggers
- **Keep recent:** 10 most recent messages are kept raw
- **Token estimation:** ~4 characters per token (GPT heuristic)
