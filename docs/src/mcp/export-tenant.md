# Tool: export_tenant

Export all memory data for a tenant as a JSON string for backup or migration.

## Input Schema

```json
{
  "tenant_id": "string (optional, default: 'default')"
}
```

## Output

Returns a JSON string containing all nodes for the tenant:

```json
{
  "content": [{"type": "text", "text": "[{\"id\":1, \"tenant_id\":\"default\", ...}]"}],
  "isError": false
}
```

## Use Cases

- Backing up all memory for a specific agent/user
- Migrating data between NeuroLithe instances
- Debugging memory state
