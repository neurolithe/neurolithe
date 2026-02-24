# The Sleep Pipeline

Inspired by the **PISA framework** and **Cognitive Load Theory**, the Sleep Pipeline runs asynchronously to consolidate short-term dialogue into long-term structured knowledge.

## Pipeline Stages

### 1. Fact Extraction

When new messages arrive via `push_dialogue`, the background worker extracts structured facts using LLM:

```json
{
  "facts": [
    {
      "fact": "Alice works at Google since 2021",
      "tags": ["employment"],
      "relationships": [
        {
          "target_entity": "Google",
          "relation": "WORKS_AT",
          "valid_from": "2021-01-01",
          "valid_until": null
        }
      ]
    }
  ]
}
```

### 2. Conflict Resolution (Tri-Modal Adaptation)

Before creating a new node, the system checks for existing similar knowledge:

| Mode | Condition | Action |
|------|-----------|--------|
| **Assimilate** | Exact match found | Boost `support_count`, reset `relevance_score` to 1.0 |
| **Accommodate-Modify** | Similar match found | Merge payloads, boost support |
| **Accommodate-Create** | No match | Create new node |

This prevents duplicate facts and strengthens repeated knowledge.

### 3. Edge Creation

For each extracted relationship, the system creates an `Edge` record with temporal bounds (`valid_from`, `valid_until`). Target entities also go through conflict resolution.

### 4. Adaptive Forgetting (Decay)

A periodic decay sweep reduces `relevance_score` for all active nodes:

```
new_score = current_score × 0.5^(days_elapsed / half_life)
```

If `relevance_score` drops below **0.1**, the node's status is changed to `'archived'`, pruning it from the active graph while preserving it for potential recovery.
