# NeuroLithe V2 — Dual-Memory Architecture (STM + LTM)

> **Design doc.** Defines NeuroLithe V2: two cooperating memory systems — a fast **Short-Term Memory (STM)** and a permanent **Long-Term Memory (LTM)** knowledge tree — plus a Kafka feeder, soft/hard reset, and a "brain CT scan" introspection surface. Build plan: [`IMPLEMENTATION-V2.md`](IMPLEMENTATION-V2.md). Prior context: [`CLAUDE.md`](CLAUDE.md) (as-built features), [`JARVIS-MEMORY-TREE.md`](JARVIS-MEMORY-TREE.md) (tree brainstorm), JARVIS [`../docs/adr-0005-knowledge-layer.md`](../docs/adr-0005-knowledge-layer.md), [`../docs/components/memory-service.md`](../docs/components/memory-service.md).

## 1. The two concepts

NeuroLithe holds **two distinct memory systems that help each other**, in **separate SQLite files** so each can be reset and evolve independently.

| | **STM — working memory** | **LTM — knowledge tree** |
|---|---|---|
| Purpose | Small, fast, "what's on my mind now" — the most relevant/recent facts | The permanent, structured map of *the person* — everything, organized |
| Speed | Fast (first stop) | Slower, richer (escalation) |
| Size | Bounded — self-prunes via decay | Grows without limit |
| Lifetime | Volatile — decays, archives, can be wiped freely | Permanent — never decays |
| Source | Agent interaction (`push_dialogue`) **+ every new document** (feeder seeds it) | The Kafka feeder (documents) on a curated spine |
| Structure | Flat-ish fact graph (nodes + 1-hop edges) | Poly-hierarchy DAG of concepts + `dataId` leaves + rolling summaries |
| DB file | `neurolithe-stm.sqlite` | `neurolithe-ltm.sqlite` |
| Reset | Wiped by **soft** and **hard** reset | Wiped by **hard** reset only |

**How they cooperate (the agent's two-stage recall):**
1. Agent queries **STM first** — fast. Often enough.
2. If STM is thin, agent **escalates to LTM** — locate the right branch, read summaries, drill to documents. More time, more information.
3. A freshly scanned document is written to **both**: permanently filed in LTM, and seeded into STM as "recently arrived." Over time it fades from STM (decay) but stays in LTM forever.

**STM is (almost) already built.** Today's NeuroLithe — episodes, nodes/edges, hybrid search, conflict resolver, decay, boost, session buffer — **is the STM**. V2 keeps it, wires the decay sweep (currently inert), splits it into its own DB file, and adds reset. **LTM is the new part.**

## 2. STM — working memory (reuse, lightly changed)

Keep the existing engine essentially as-is; it already does exactly what STM needs:
- **Storage:** `episodes` (raw ground truth) + `nodes` (facts: payload, status, ccl, support_count, relevance_score, timestamps) + `edges` (typed, temporally-bounded) + `vec_nodes` (sqlite-vec) + `fts_nodes` (FTS5, trigger-synced).
- **Age-prioritization:** `DecayEngine` (exponential half-life, default 7d) → score fades; below 0.1 → `archived` (excluded from search). *V2 change: actually schedule the sweep — `run_decay_sweep` is implemented but never called today.*
- **Focus-prioritization:** `boost_relevance` (read resets score to 1.0 + stamps `last_accessed_at`) and `support_count` (repetition strengthens). Use it → it stays; ignore it → it fades.
- **Online learning:** the **Tri-Modal Conflict Resolver** (assimilate < 0.15 / accommodate-modify 0.15–0.35 / accommodate-create > 0.35) merges each new fact on arrival — no batch re-index. This is what keeps STM small and non-duplicative.
- **Session compression:** `SessionManager` rolling summary + recent-raw + relevant-facts.

**STM keeps decay; LTM must never decay** — this is the central reason they are separate stores, not one with a flag.

## 3. LTM — the knowledge tree (new)

A permanent, navigable poly-hierarchy of concepts with documents as leaves. Realizes the parked tree brainstorm.

### 3.1 Model (`neurolithe-ltm.sqlite`)
- **`tree_nodes`** — `id`, `name`, `summary` (rolling), `kind` (`spine` = curated backbone | `grown` = AI-created concept | `inbox` = unsorted holding | `leaf` = a document), `permanent` (always true), `created_at`, `updated_at`. Concept nodes carry an embedding (of their summary) in `vec_ltm`; summaries also indexed in `fts_ltm`.
- **`tree_edges`** — `parent_id`, `child_id`, `weight`. **Multi-parent (DAG)** — a concept can sit under several parents (e.g. `leadership` under both *job→improvement* and *self-improvement*).
- **`leaves`** — `tree_node_id` (the leaf node), `data_id` (FK → Ledger/Pithos), `provenance` (source, ingest time, confidence). The **actual content stays in Ledger/Pithos**; LTM stores meaning + the `dataId` pointer only.

### 3.2 Placement (V2 = simple; smart growth deferred)
On a new document: embed its summary, find the best-match concept node (vector similarity ≥ threshold) → attach the leaf there; if no good match → attach under **`inbox`** for later sorting. Roll the new summary up into ancestor summaries (simple condense). The **curated spine** (Reza's main branches: job, learning, investment, self-improvement, …) is seeded once; the AI grows leaves under it.

> **Deferred (★ growth rules):** when to split a fat node, merge similar branches, or spawn a new branch. V2 just does best-match-or-inbox. The hard "grow the right way" logic is a later version — see [`JARVIS-MEMORY-TREE.md`](JARVIS-MEMORY-TREE.md).

### 3.3 Retrieval — map → expand → drill (+ vector)
- **`ltm_map(depth)`** — top N layers = a compact "table of contents" of the person (the AI reads this first to get the lay of the land within a bounded token budget).
- **`ltm_expand(node)`** — a node's children + their summaries.
- **`ltm_drill(node)`** — a node's detail + its `dataId` leaves (with provenance).
- **`recall_ltm(query)`** — vector-locate the entry node, then assemble context **up** (ancestors for framing) and **down** (children/leaves for detail).
- **Reference-returning:** LTM results carry `dataId` + provenance (the opposite of STM's id-hiding `MemoryResult`), so the agent can fetch originals from Ledger → Pithos.

## 4. Ingestion — the Kafka feeder (feeds BOTH)

NeuroLithe becomes a **long-running daemon** (today it's a per-invocation stdio MCP server). The daemon runs, concurrently: the MCP door, the Kafka feeder, the command consumer, and a scheduler (decay sweep + metrics).

**Feeder consumes `document.completed`** (compacted → backfill by reading from earliest offset):
1. Parse event (`dataId`, `groupId`, per-page `textUri`/`tags`, `pt://` pointers).
2. Fetch text from **Pithos** via `pt://` — **tolerate missing** (bytes may be gone before a tombstone compacts).
3. **Distill meaning** with the local LLM → concise summary + concept hints + embedding (local model, e.g. nomic 768; offline).
4. **Write LTM** — create/locate the `dataId` leaf (summary + embedding + provenance), place per §3.2, roll up ancestor summaries. Permanent.
5. **Write STM** — insert a decaying working-memory fact for the new doc *through the conflict resolver* (so it dedups/merges), making recent ingests instantly visible to fast search. It will fade from STM via decay while persisting in LTM.
6. **Tombstone** (null value, ADR-0001 D11) → **forget** that `dataId` in **both** stores (remove STM node(s) + LTM leaf, re-roll affected summaries).

Error handling per **ADR-0004**: transient → retry; bad event → `dlq.memory`; un-parseable → `parking.lot`; commit offset **after** the write. (Supersedes/realizes task #57.)

## 5. Reset — soft & hard (the disposable brain)

Resets are how we exploit the principle *"the fuzzy brain is disposable; Ledger/Pithos is the permanent source of truth."*

- **Soft reset** — wipe **STM only** (`neurolithe-stm.sqlite`). LTM untouched. Use when working memory gets junky or you want a clean slate for a session.
- **Hard reset** — wipe **both** stores, then **rebuild LTM by replaying** `document.completed` from the earliest offset (relearn). Use when you change the logic, embeddings, or tree structure and want to re-derive from scratch. Nothing is lost — every leaf is a `dataId` pointer back to the record.

**Mechanism (bus-native):** a `memory.command` topic carries `reset_soft` / `reset_hard` (also exposed as guarded MCP/CLI tools). Hard reset = wipe both + reset the feeder consumer-group offset to earliest + resume. Both are destructive → require an explicit confirmation token; triggered from Pharos (admin).

## 6. Observability — the "brain CT scan"

Goal: see inside the brain to diagnose and improve it. Two tiers, both honoring the distributed principle (**Pharos never reads NeuroLithe's SQLite directly** — Ledger is the lone direct-DB exception).

- **Metrics stream → `memory.metrics`** (compacted topic, latest snapshot; published periodically). Carries: STM node/archived counts, decay histogram, avg relevance, session count; LTM tree-node count, max depth, leaf/document count, inbox/orphan count, edge count; feeder consumer **lag**, last backfill time, error counts, DB file sizes. **Pharos consumes** → dashboard KPIs + health.
- **Deep introspection → read-only MCP/A2A tools** (on-demand, the CT scan itself), called by Pharos:
  - `memory_stats` — the snapshot above.
  - `stm_list(limit, status)` — working-memory facts with score + age (watch what's hot vs fading).
  - `ltm_map(depth)` — the tree's top layers.
  - `inspect_node(id)` — summary, edges (parents/children), leaves (`dataId`s), age/decay (STM) or permanence (LTM), provenance.
  - `subtree(node, depth)` — a branch.
  - `trace_dataId(dataId)` — where a given document lives in the brain (LTM leaf/branch + STM presence).
  - `health` — consumer lag, last error, DB sizes, sweep status.

**Pharos** renders these as a Memory / CT-scan view — **built** in `../pharos/admin/` (`/memory`, with soft/hard reset).

## 7. Communication summary (bus + doors)

| Direction | Channel | Purpose |
|---|---|---|
| In | `document.completed` (Kafka, compacted) | Feeder learns; backfill from earliest; tombstone = forget |
| In | `memory.command` (Kafka) | `reset_soft` / `reset_hard` |
| In (fetch) | Pithos `pt://` (HTTP) | Pull document text to distill (tolerate missing) |
| Out | `memory.metrics` (Kafka, compacted) | Periodic snapshot → Pharos dashboard |
| Door | MCP / A2A (read tools) | Recall (STM/LTM) + introspection (CT scan) for the agentic core & Pharos |
| Errors | `dlq.memory`, `parking.lot` | ADR-0004 |

## 8. What's reused / changed / new

- **Reused (STM):** episodes/nodes/edges/vec/fts schema, `hybrid_search`, `query_with_graph`, `ConflictResolver`, `DecayEngine`, `boost_relevance`, `SessionManager`, `LlmClient`, MCP types.
- **Changed:** split DB into two files (config gains LTM path/dim, Kafka, Pithos, intervals); **wire the decay sweep** on a schedule; MCP server → persistent daemon with network transport + new tools; `main.rs` runs MCP + feeder + command-consumer + scheduler concurrently.
- **New:** LTM schema + repository + retrieval (map/expand/drill + vector) + placement + rolling summaries; Kafka feeder (dual-write + tombstones + backfill); Pithos client; `memory.command` consumer + hard/soft reset; `memory.metrics` publisher; introspection tools; **Pharos** CT-scan view.

## 9. DDD placement (preserve layering)

- **domain/** — add `ltm` models (`TreeNode`, `TreeEdge`, `Leaf`), `LtmRepository` port; keep STM domain as-is.
- **application/** — `ingestion` (feeder pipeline), `ltm_retrieval`, `reset_service`, `monitoring` (snapshot builder).
- **infrastructure/** — `ltm_repository` (SQLite), `kafka` (consumer/producer via rdkafka), `pithos_client`, `metrics_publisher`; config split.
- **interfaces/** — extend `mcp_server` (recall + nav + introspection + reset tools); `kafka_feeder` + `command_consumer` loops; daemon wiring in `main.rs`.

## 10. Open / deferred

- **★ Smart growth rules** (split/merge/new-branch) — V2 uses best-match-or-inbox; the hard logic is later (may reuse the conflict-resolver's assimilate/modify/create idea at the *branch* level).
- **STM↔LTM hydration** — when the agent focuses a branch, optionally pre-load its summaries into STM. V2 can ship a basic `hydrate(branch)`; smart policy later.
- **Agentic-core door** — MCP-over-network now; A2A for fast internal calls later.
- **Embeddings/summaries** — local model (nomic 768 for vectors; local LLM e.g. Gemma for summaries) to stay offline; dimension locked per DB at init.
