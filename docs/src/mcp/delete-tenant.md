# Tool: delete_tenant

Delete all memory nodes, edges, episodes, and embeddings for a specific tenant.

## Input Schema

```json
{
  "tenant_id": "string (optional, default: 'default')"
}
```

## Output

```json
{
  "content": [{"type": "text", "text": "Successfully deleted all data for tenant default"}],
  "isError": false
}
```

## Behavior

Executes a transactional deletion across all tables:

1. Deletes vector embeddings from `vec_nodes`
2. Deletes edges from `edges`
3. Deletes nodes from `nodes`
4. Deletes episodes from `episodes`

All operations are wrapped in a single SQLite transaction for atomicity.
