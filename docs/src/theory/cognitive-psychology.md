# Cognitive Psychology Behind NeuroLithe

NeuroLithe's architecture is grounded in established cognitive psychology research on how human memory works.

## Piaget's Schema Theory (Tri-Modal Adaptation)

**Jean Piaget's** framework for cognitive development describes how we integrate new information:

- **Assimilation** — New information fits existing mental models. In NeuroLithe: if a new fact matches an existing node, we simply boost its `support_count`, reinforcing the memory.
- **Accommodation** — Existing models must be modified to fit new information. In NeuroLithe: if a similar but different fact is found, we merge the payloads and update the existing node.
- **Creation** — Entirely new information requires new mental models. In NeuroLithe: if no match is found, a new node is created.

## Ebbinghaus Forgetting Curve

**Hermann Ebbinghaus** demonstrated that memories follow an exponential decay curve over time unless reinforced. NeuroLithe implements this directly:

```
relevance_score = initial_score × 0.5^(days / half_life)
```

Key insights applied:

- **Spaced repetition** — Each time a memory is accessed (`query_memory`), its relevance is reset to 1.0
- **Support reinforcement** — Facts that are repeatedly extracted from conversations get higher `support_count`, making them harder to forget
- **Graceful degradation** — Memories don't vanish; they become `archived` and can be recovered

## Cognitive Load Theory

**John Sweller's** work shows that working memory (analogous to LLM context windows) has strict capacity limits. NeuroLithe addresses this through:

- **Context compression** — Old messages are summarized into dense summaries
- **Token-optimized output** — Query results strip internal metadata, returning only pure context
- **Selective retrieval** — Hybrid search ensures only the most relevant memories are returned

## Memory Consolidation (Sleep)

The "Sleep Pipeline" is inspired by research on how the brain consolidates short-term memories into long-term storage during sleep:

- **Short-term → Long-term** — Raw dialogue is transformed into structured knowledge graphs
- **Background processing** — Extraction happens asynchronously, not blocking the conversation
- **Ground-truth preservation** — Original dialogue is always preserved (like episodic memory) while facts are abstracted (like semantic memory)
