# Adaptive Forgetting

NeuroLithe implements **Adaptive Forgetting** — a decay mechanism that mimics how human memory naturally fades over time, unless reinforced.

## How It Works

Every node has a `relevance_score` that starts at **1.0** and decays exponentially:

```
relevance_score = current_score × 0.5^(days_elapsed / half_life)
```

With a default **7-day half-life**:

- After 7 days without access: score drops to **0.5**
- After 14 days: **0.25**
- After 21 days: **0.125**
- After ~24 days: falls below **0.1** → node is **archived**

## Reinforcement

There are two ways a memory is reinforced:

1. **Read boost** — When `query_memory` retrieves a node, its `relevance_score` is reset to **1.0** and `last_accessed_at` is updated
2. **Support boost** — When the Sleep Pipeline encounters a fact that matches an existing node (Assimilation), it increments `support_count` and resets relevance

## Archiving vs. Deletion

Archived nodes are **not deleted**. They remain in the database with `status = 'archived'` and are:

- Excluded from active search results
- Available for recovery if needed
- Traceable back to their source `episode`

This design ensures no knowledge is permanently lost — it simply becomes dormant.
