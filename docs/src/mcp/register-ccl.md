# Tool: register_ccl

Register a new Cognitive Context Layer (CCL).

## Input Schema

```json
{
  "name": "string (required)",
  "description": "string (required)",
  "tenant_id": "string (optional, default: 'default')"
}
```

## Output

```json
{
  "content": [{"type": "text", "text": "CCL layer registered successfully."}],
  "isError": false
}
```

## Behavior

Registers a new cognitive layer with a explicit semantic description. The background extraction pipeline passes all available CCLs and descriptions to the LLM, allowing it to correctly map elements in the context to the defined context layers.
