# Tool: store_memory

Explicitly store a crucial fact immediately, bypassing the background extraction pipeline.

## Input Schema

```json
{
  "fact_text": "string (required)",
  "tags": ["string"] ,
  "tenant_id": "string (optional, default: 'default')"
}
```

## Output

```json
{
  "content": [{"type": "text", "text": "Memory fact explicitly stored."}],
  "isError": false
}
```

## Behavior

1. Embeds the fact text into a vector via the configured LLM
2. Creates a `MemoryNode` with `is_explicit = true`
3. Stores the node and its embedding in the database
4. Does **not** run the Sleep Pipeline (fact is already structured)

## Use Cases

- Storing critical user preferences that shouldn't wait for extraction
- Correcting existing knowledge ("User now lives in Tokyo, not Berlin")
- Agent self-notes ("User prefers formal communication style")
