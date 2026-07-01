# NeuroLithe V2 — Build Plan (IMPLEMENTATION-V2.md)

> Step-by-step plan to build the dual-memory (STM + LTM) NeuroLithe. Architecture: [`V2-DESIGN.md`](V2-DESIGN.md). Build pattern, non-negotiable: **small slice → unit test → review (security → performance → style) → next slice.** Each slice compiles and its tests pass before the next starts.
>
> Stack: **Rust + SQLite** (two DB files), **sqlite-vec** + **FTS5**, **rdkafka** (feeder + command consumer + metrics producer), local LLM + embeddings (Ollama, offline) — *deployed config currently uses OpenRouter for both (commit `ffe8767`)*, Pithos HTTP client, Docker Compose on the Mac mini joined to `jarvis-kafka_default`. Error handling per **ADR-0004**.

## Guardrails
- **STM keeps decay; LTM must never decay.** Enforced by separate DB files — never share a decay path.
- **Pharos reads NeuroLithe only via the bus (`memory.metrics`) or the MCP/A2A door — never its SQLite.** (Ledger's direct-DB read stays the lone exception.)
- **Reuse the existing STM engine** (conflict resolver, decay, boost, hybrid search, session manager) — V2 changes how it's wired, not its logic.
- **Reset is destructive** — guard `reset_hard` behind an explicit confirmation token.

---

## Slice 0 — Kafka topics *(prerequisite, outside the crate)*
Add to `../kafka/create-topics.sh`: `memory.command` (short retention, `$D7`), `memory.metrics` (**compacted**, `cleanup.policy=compact` — latest snapshot), `dlq.memory` (`$D14`). (`document.completed` already exists, compacted.)
**Done when:** topics present; `memory.metrics` compacted; no auto-create.

## Slice 1 — Config + dual-DB split
`AppConfig` gains: `stm.path`, `ltm.path`, `ltm.vector_dimension`, `kafka.{brokers,group_id}`, `pithos.base_url`, `sweep.interval`, `metrics.interval`, `feeder.enabled`. Init **two** connections; run STM schema on one, LTM schema (slice 3) on the other.
**Unit tests:** config parses with defaults + env override; both DB files init independently; changing one dimension doesn't touch the other.

## Slice 2 — Wire STM decay sweep + soft reset
Schedule `run_decay_sweep` on `sweep.interval` (it exists but is never called today). Implement **soft reset** = wipe STM store only (drop+recreate `neurolithe-stm.sqlite`), LTM untouched.
**Unit tests:** sweep decays active nodes and archives < 0.1; boost still resets on read; soft reset empties STM, LTM rows survive.

## Slice 3 — LTM schema + repository
DDL in `neurolithe-ltm.sqlite`: `tree_nodes` (id, name, summary, kind[`spine`/`grown`/`inbox`/`leaf`], permanent, timestamps), `tree_edges` (parent_id, child_id, weight — **multi-parent DAG**), `leaves` (tree_node_id, data_id, provenance), `vec_ltm` (sqlite-vec, ltm dimension), `fts_ltm` (FTS5 on summary, trigger-synced). `LtmRepository` port + SQLite impl: CRUD nodes/edges/leaves, get-children, get-parents, get-by-dataId. Seed the curated **spine** on first init.
**Unit tests (temp DB):** create node/edge incl. a node with **two parents**; leaf links to a data_id; migrations idempotent; spine seeded once.

## Slice 4 — LTM placement + rolling summary (simple)
Given `(summary, embedding, dataId, provenance)`: find best-match concept (vector ≥ threshold) → attach leaf; else attach under `inbox`. Roll the summary up into ancestor summaries (condense). Tombstone path: remove a `dataId`'s leaf + re-roll affected ancestors.
**Unit tests:** high-similarity → under the match; low-similarity → `inbox`; ancestor summary updates after attach; remove-by-dataId deletes the leaf and re-rolls. *(Smart split/merge/new-branch is explicitly out of scope — deferred.)*

## Slice 5 — LTM retrieval (map / expand / drill + vector)
`ltm_map(depth)` (top layers), `ltm_expand(node)` (children + summaries), `ltm_drill(node)` (detail + `dataId` leaves + provenance), `recall_ltm(query)` (vector entry → context up+down). Results are **reference-returning** (`dataId` + provenance).
**Unit tests:** map returns only the top `depth` layers; expand returns direct children; drill returns leaves with `dataId`; vector entry lands on the seeded node for a matching query.

## Slice 6 — Pithos client + meaning extraction
HTTP client to fetch `pt://` text (**tolerate 404/missing** → skip gracefully). LLM distill → `{summary, concept_hints, embedding}` via the local model.
**Unit tests (mock LLM + mock Pithos):** distill returns summary + embedding of the configured dimension; missing fetch returns a typed "skipped" outcome, not an error crash.

## Slice 7 — Kafka feeder (document.completed → dual-write)
rdkafka consumer on `document.completed`; **backfill from earliest**. Per document: fetch (slice 6) → distill → **LTM write** (slice 4) **+ STM write** through the `ConflictResolver` (dedups/merges). Tombstone → forget in **both** stores. ADR-0004: transient retry → `dlq.memory` on bad event → `parking.lot` on un-parseable → commit offset **after** write.
**Integration tests (mock/embedded broker):** a `document.completed` event lands a leaf in LTM **and** a node in STM; a tombstone removes it from both; malformed → parking, offset still advances; replay from earliest re-ingests.

## Slice 8 — `memory.command` consumer + hard reset
Consume `memory.command`: `reset_soft` (slice 2); `reset_hard` = wipe **both** stores + reset the feeder consumer-group offset to **earliest** + resume (relearn). Guard `reset_hard` with a confirmation token.
**Integration tests:** `reset_soft` empties STM only; `reset_hard` empties both, then the feeder backfill repopulates LTM (+ STM) from replayed `document.completed`.

## Slice 9 — Metrics publisher (`memory.metrics`)
Periodic snapshot → publish (compacted, fixed key). Fields: STM node/archived counts, decay histogram, avg relevance, sessions; LTM tree-node count, max depth, leaf/doc count, inbox/orphan count, edges; feeder **lag**, last backfill, error counts, DB file sizes.
**Unit tests:** snapshot fields computed correctly on a seeded STM+LTM; publish is one keyed message (no churn).

## Slice 10 — Introspection MCP tools (CT scan, read-only)
Add tools: `memory_stats`, `stm_list(limit,status)`, `ltm_map(depth)`, `inspect_node(id)`, `subtree(node,depth)`, `trace_dataId(dataId)`, `health`. All read-only; reference-returning where relevant.
**Unit tests:** each returns the expected shape on seeded data; `trace_dataId` finds a doc in both LTM (leaf/branch) and STM (presence).

## Slice 11 — Daemon assembly + MCP network transport
`main.rs` becomes a long-running daemon running concurrently: MCP server (network transport + existing stdio), Kafka feeder (slice 7), command consumer (slice 8), scheduler (sweep slice 2 + metrics slice 9). Graceful shutdown; one process owns both DB connections (serialize access as today).
**Tests:** smoke — daemon boots, ingests one test doc end-to-end, serves `recall_stm` + `recall_ltm`, publishes a metrics snapshot.

## Slice 12 — Dockerize + deploy
Multi-stage Dockerfile; `docker-compose.yml` joining `jarvis-kafka_default`, both SQLite files on a named volume, env wired (Kafka, Pithos URL, local-LLM endpoint), README run/verify. Deploy `jarvis-neurolithe` on the mini. Smoke: scan a real doc → appears in the brain (CT scan shows the leaf + STM node); soft then hard reset behave.
**Done when:** container runs, backfills existing `document.completed`, serves recall + introspection, resets work.

## Slice 13 — Pharos: Memory / CT-scan view *(separate, in `pharos/admin/`)* ✅ BUILT
Built in `../pharos/admin/` (routes `/memory` + `/memory/reset/{soft,hard}`). Consume `memory.metrics` for KPIs + health; call the introspection tools for tree snapshot, node inspector, decay histogram, `dataId` trace; **reset buttons** publish `memory.command` (with a confirmation dialog). Bus + door only — **no DB peeking.**
**Done when:** the operator can watch the brain (sizes, lag, decay), browse the tree, trace a document, and trigger soft/hard reset from the console. ✅ — shipped.

---

## Build order / dependencies
- **0 → 1** first. Then STM track (**2**) and LTM track (**3 → 4 → 5**) can proceed in parallel.
- **6** (Pithos+distill) feeds **7** (feeder), which needs **4** (LTM write) + STM. **7** realizes/supersedes task #57.
- **8** (reset) needs **2** + **7** (hard reset replays the feeder). **9/10** (observability) need **2/3** present. **11** assembles everything; **12** ships it; **13** is the Pharos face (after **9/10**).

## Confirm before coding
The four V2 forks are settled (separate DB files; Pharos CT-scan; simple placement, defer growth rules; feeder → both). Open items deliberately deferred: smart growth rules (★), STM↔LTM hydration policy, A2A door. Flag if any should move into V2 scope before slice 3.
