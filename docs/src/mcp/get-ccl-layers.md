# Tool: get_ccl_layers

Retrieve all registered Cognitive Context Layers (CCL) available to the tenant.

## Input Schema

```json
{
  "tenant_id": "string (optional, default: 'default')"
}
```

## Output

Returns an array of `CclDefinition` objects. Note that internal IDs and tenant_id are omitted from the agent's view.

```json
[
  {
    "name": "reality",
    "description": "Base truth facts"
  },
  {
    "name": "dream",
    "description": "Fictional or theoretical states"
  }
]
```

## Behavior

Queries and retrieves all CCL definitions previously registered or auto-generated for the tenant. Agents can use this tool to inspect available context layers before committing memories or conducting counterfactual queries.
