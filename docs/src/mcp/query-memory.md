# Tool: query_memory

Search the long-term knowledge graph for relevant historical context. Returns token-optimized results with 1-hop connections and temporal bounds.

## Input Schema

```json
{
  "query": "string (required)",
  "time_filter": {
    "after": "YYYY-MM-DD (optional)",
    "before": "YYYY-MM-DD (optional)"
  },
  "ccl_filter": ["string"] (optional, default: ['reality']),
  "tenant_id": "string (optional, default: 'default')"
}
```

## Output

Returns an array of `MemoryResult` objects (token-optimized — no internal IDs or scores):

```json
[
  {
    "fact": "Alice works at Google",
    "ccl": "reality",
    "last_updated": "2026-02-20T10:30:00",
    "connections": [
      {
        "relation": "WORKS_AT",
        "entity": "Google",
        "ccl": "reality",
        "valid_from": "2021-01-01",
        "valid_until": null
      }
    ]
  }
]
```

## Behavior

1. Embeds the query into a vector
2. Runs **hybrid search**: vector cosine distance + FTS5 BM25
3. Expands results with **1-hop graph traversal** (respecting temporal bounds on edges)
4. Applies **temporal filtering** on node creation date if `time_filter` is provided
5. **Boosts relevance** of all accessed nodes back to 1.0 (reading resets decay)
6. Returns token-optimized output (no `node_id`, `relevance_score`, or other internal fields)
