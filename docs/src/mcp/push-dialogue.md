# Tool: push_dialogue

Push the latest conversation turn to Short-Term Memory. The service automatically compresses old context, extracts facts, and returns an optimized context window.

## Input Schema

```json
{
  "session_id": "string (required)",
  "new_message": "string (required)",
  "tenant_id": "string (optional, default: 'default')"
}
```

## Output

Returns a `ContextWindow` object:

```json
{
  "summary": "Dense summary of compressed older messages (null if no compression yet)",
  "recent_messages": ["Most recent raw messages still in buffer"],
  "relevant_facts": [
    {
      "fact": "User lives in Berlin",
      "last_updated": "2026-02-23T14:00:00",
      "connections": [
        {
          "relation": "LIVES_IN",
          "entity": "Berlin",
          "valid_from": "2026-01-15",
          "valid_until": null
        }
      ]
    }
  ]
}
```

## Behavior

1. Archives raw message as an `episode` (ground truth)
2. Adds to session buffer with token counting
3. If buffer exceeds ~4000 tokens, compresses oldest messages into a summary via LLM
4. Queues the message for background fact extraction (Sleep Pipeline)
5. Returns `[Summary] + [Recent Messages] + [Relevant Graph Facts]`
