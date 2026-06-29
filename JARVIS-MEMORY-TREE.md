# NeuroLithe — Knowledge Tree (design notes, parked)

> **Status: PARKED design conversation** (captured 2026-06-26). This is *not* a spec — it's the record of a brainstorm about how NeuroLithe should structure JARVIS's long-term memory as a growing knowledge tree. **Resume here when we start the NeuroLithe work.** Related: [`CLAUDE.md`](CLAUDE.md) roadmap, [`AUDIT.md`](AUDIT.md) §8, and JARVIS [`../docs/adr-0005-knowledge-layer.md`](../docs/adr-0005-knowledge-layer.md) + [`../docs/components/memory-service.md`](../docs/components/memory-service.md).

## The idea (Reza's vision)

Structure the brain's memory as a **growing tree of *you*** — a personal knowledge hierarchy:

```
Reza
├── job
│   └── bestbuy
│       ├── project metro
│       └── improvement plan
│           ├── leadership            (also under self-improvement / learning)
│           └── system architecture
├── investment
├── self-improvement
└── learning
    └── leadership …
```

Branches subdivide as more information arrives; details accrete at the leaves.

**Why (the motivation):** an AI's context window is finite. With flat memory, once there's a lot of information it either won't fit, or parts become "invisible" and the AI can't focus. A **tree** fixes this: the AI reads the **top 2–3 layers to get the map** (a compact "table of contents" of the person), then **drills into only the branch it's focusing on** for detail — general awareness of what exists, with bounded, focused depth.

## Prior art (this is a real, current direction)

- **Ontology / concept hierarchy** — classic knowledge organization.
- **Hierarchical retrieval + progressive disclosure** — read summaries first, drill for detail.
- **RAPTOR** — builds a tree of recursive summaries so retrieval can pull from any level.
- **GraphRAG** (Microsoft) — knowledge graph + rolled-up "community" summaries you read first, then drill down.

It **fits NeuroLithe naturally**: NeuroLithe is already a **knowledge graph** (`nodes` + `edges`) and is the *meaning* / LTM layer (ADR-0005). A hierarchy is just nodes with parent/child edges — shaping the existing graph into a navigable tree, not adding a foreign concept.

## Refinements settled in this conversation

1. **Poly-hierarchy (DAG), not a strict tree.** A concept can have **several parents** — e.g. `leadership` sits under both *job → improvement plan* and *self-improvement / learning*. Pure trees force one home; real life is multi-parent. NeuroLithe's graph supports many parent edges natively. (Same many-parent shape already chosen for the Ledger tag vocabulary — not a coincidence.)
2. **Every node carries a short, rolling summary.** This is the mechanism behind "read the top layers = the map." Summaries **roll up** from children (hierarchical summarization); the AI maintains them as branches grow. Without summaries it's just a skeleton.
3. **Documents attach as leaves by reference.** A node links to the `dataId`s (and extracted facts) that belong to it; the **actual content stays in Ledger/Pithos** (the `dataId` foreign key). The tree organizes *meaning + pointers*, never copies.
4. **Retrieval = map → expand → drill, paired with vectors.** "Give me the top N layers" (the map) → "expand `job`" (children + summaries) → "drill into `project metro`" (details + documents). Vector search *locates* the right node; the tree supplies context **up** (ancestors, for framing) and **down** (children, for detail). Navigation + similarity together.

## The governing principle (the big one)

> **NeuroLithe is the fuzzy brain; Ledger (+ Aristotle's tags) is the exact record.** The brain holds *meaning* and may be approximate, wrong, reorganized, or forgetful. The record is ground truth, kept so we can always refer back to the real data.

**Corollary — the brain is disposable & rebuildable; the record is permanent.** Nothing irreplaceable lives in NeuroLithe (every node points back to a `dataId` → Ledger → Pithos), so the tree can grow, re-summarize, reorganize, or be **wiped and re-derived** with nothing lost. This de-risks letting the brain be aggressively fuzzy. (It's the same source-of-truth-vs-derived split used across JARVIS — now applied to the brain itself.)

**Therefore the tree and the tag map are independent by design** — not two copies of one taxonomy:
- **Aristotle's tags (in Ledger)** = exact, controlled labels on the source of truth — precise "find docs tagged `tax`" lookups.
- **NeuroLithe's tree** = the fuzzy semantic map of *you* — for the brain to navigate and reason.
- **Bridge = `dataId`:** when the fuzzy brain needs the actual data, it follows a node's `dataId` to the exact record. They never have to agree on structure.

## Growth

- **Curated spine + AI-grown leaves.** Reza defines the **main branches**; the AI grows everything below them as information arrives.
- **No hard limit** on growth — but it must grow **"the right way."**
- **"The right way" is the key parked question** (see below) — it's the heart of the design.

## Open questions (for the NeuroLithe phase)

- **★ Growth rules ("the right way") — the big one.** When does the AI create a *new* branch vs file under an existing one? When does a fat node **split**? When do two similar branches **merge**? How to avoid sprawl and keep summaries honest? (Possibly leverages NeuroLithe's existing conflict-resolver: assimilate / modify / create.)
- **Node model** — exact fields a node holds (name, summary, embedding, child edges, `dataId` links, facts) and how it maps onto NeuroLithe's existing `nodes` / `edges` / `vec_nodes` / `fts_nodes` tables.
- **Retrieval API** — the map/expand/drill calls + vector search; reference-returning results (`dataId` + provenance), the opposite of NeuroLithe's id-hiding `MemoryResult`.
- **Ingestion → placement** — how a freshly ingested document finds its branch(es); whether to use Aristotle's tags as placement hints.
- **STM vs LTM for the tree** — the spine is permanent LTM (decay-exempt, the forgetting curve must never touch it); transient leaf notes could ride STM and fade.
- **Embeddings** — local model + **dimension locked at DB init** (e.g. nomic, 768) — carried over from the ADR-0005 discussion.

## Next session — pick one to shape first

1. **The node model** (what a node holds; mapping onto the existing tables).
2. **Retrieval** (map → expand → drill + vector).

The **growth rules** stay parked until then.
