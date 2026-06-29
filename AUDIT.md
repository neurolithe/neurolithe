# NeuroLithe Code Audit

**Audit date:** 2026-06-20  
**Auditor:** Claude Sonnet 4.6  
**Version audited:** 0.1.1 (Cargo.toml)  
**Scope:** All 23 source files + neurolithe.toml config

---

## 1. Project Overview & File Tree

NeuroLithe is an embedded hybrid Graph-Vector memory database for AI agents, exposed as an MCP server over STDIO (JSON-RPC 2.0). It combines short-term dialogue compression with a long-term fact store backed by SQLite, `sqlite-vec` (ANN search), and FTS5 (BM25 keyword search).

```
neurolithe/
├── Cargo.toml                              # Package manifest, all dependencies
├── neurolithe.toml                         # Runtime config (LLM + DB settings)
└── src/
    ├── main.rs                             # Entry point: wires config → DB → LLM → app → MCP loop
    ├── domain/
    │   ├── mod.rs                          # Re-exports: cognition, decay, models, ports
    │   ├── models.rs                       # Pure data types: TenantId, SessionId, MemoryNode,
    │   │                                   # Edge, Episode, CclDefinition, MemoryResult, etc.
    │   ├── ports.rs                        # Trait definitions: MemoryRepository + LlmClient;
    │   │                                   # also ExtractedFact / ExtractedRelationship DTOs
    │   ├── decay.rs                        # DecayEngine: exponential half-life math, node archiving
    │   └── cognition/
    │       ├── mod.rs                      # Re-exports: conflict_resolver
    │       └── conflict_resolver.rs        # ConflictResolver: Tri-Modal adaptation logic
    ├── infrastructure/
    │   ├── mod.rs                          # Re-exports: config, database, llm, repository, schema
    │   ├── config.rs                       # AppConfig, LlmConfig, DatabaseConfig; loads neurolithe.toml
    │   ├── database.rs                     # init_db(): rusqlite connection + sqlite-vec extension init
    │   ├── schema.rs                       # init_schema(): all DDL — 4 regular + 2 virtual tables
    │   ├── repository.rs                   # SqliteMemoryRepository: full MemoryRepository impl
    │   └── llm.rs                          # LlmClient impls: OpenAiClient, GeminiClient, AnthropicClient
    ├── application/
    │   ├── mod.rs                          # Re-exports: app, retrieval, session_manager, sleep
    │   ├── app.rs                          # NeurolitheApp: main facade; wires all services
    │   ├── session_manager.rs              # SessionManager: STM buffer, token counting, compression
    │   ├── sleep.rs                        # SleepWorker: fact extraction + conflict resolution pipeline
    │   └── retrieval.rs                    # RetrievalService: orchestrates hybrid + graph query
    └── interfaces/
        ├── mod.rs                          # Re-exports: mcp_server, mcp_types
        ├── mcp_server.rs                   # McpServer: JSON-RPC 2.0 event loop + tool dispatch
        └── mcp_types.rs                    # JsonRpcRequest/Response, McpToolResult, McpContent
```

---

## 2. Dependencies (Cargo.toml Analysis)

| Crate | Version | Purpose |
|---|---|---|
| `anyhow` | 1.0.102 | Ergonomic error handling and propagation throughout all layers |
| `async-trait` | 0.1.89 | Enables `async fn` in traits (`LlmClient`) |
| `config` | 0.15.19 | Multi-source config loading (TOML file + env vars) |
| `dotenvy` | 0.15.7 | Auto-loads `.env` file at startup |
| `libsqlite3-sys` | 0.36.0 | Low-level SQLite bindings (transitive dep of rusqlite + sqlite-vec) |
| `reqwest` | 0.13.2 | Async HTTP client for all LLM API calls; features: `json`, `rustls` (TLS) |
| `rusqlite` | 0.38.0 | SQLite interface; feature `bundled` compiles SQLite in — no system lib needed |
| `serde` | 1.0.228 | Serialization framework; feature `derive` |
| `serde_json` | 1.0.149 | JSON encode/decode for payloads, MCP protocol, API responses |
| `sqlite-vec` | 0.1.6 | sqlite-vec extension loaded at runtime for ANN vector search |
| `thiserror` | 2.0.18 | Imported but **not used anywhere in the codebase** (no custom error types defined) |
| `tokio` | 1.49.0 | Async runtime; feature `full` (all sub-runtimes) |

**Dev dependencies:**

| Crate | Version | Purpose |
|---|---|---|
| `tempfile` | 3.25.0 | Temporary files/dirs for integration tests (not yet used — tests use in-memory SQLite) |

**Notes:**
- `thiserror` is a dead dependency — it is listed in Cargo.toml but no custom error enums use it. All error handling uses `anyhow` directly.
- `reqwest` with `rustls` means no dependency on the system's OpenSSL, which is good for portability.
- `rusqlite`'s `bundled` feature significantly increases binary size and build time but removes the system SQLite version constraint.
- Rust edition is `2024`, which requires a recent compiler (1.85+).

---

## 3. Domain Layer — What's Implemented

### `domain/models.rs`

All types are `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]` unless noted.

**`TenantId(pub String)`** — Newtype wrapper around a tenant identifier string. Used as a key for all data isolation.

**`SessionId(pub String)`** — Newtype wrapper for conversation session identifiers.

**`CclDefinition`** — Represents a registered Cognitive Context Layer:
- `id: Option<i64>` — DB row ID (None before insertion)
- `tenant_id: TenantId`
- `name: String` — e.g. `"reality"`, `"dream"`
- `description: String` — LLM-generated or user-provided description

**`Episode`** — Append-only ground-truth dialogue log entry:
- `id: Option<i64>` — DB row ID
- `tenant_id: TenantId`
- `session_id: SessionId`
- `raw_dialogue: String` — The verbatim message text
- `ccl: String` — Layer at time of capture
- `created_at: Option<String>` — Set by DB DEFAULT

**`MemoryNode`** — A structured fact in the knowledge graph:
- `id: Option<i64>` — DB row ID
- `tenant_id: TenantId`
- `source_episode_id: Option<i64>` — FK to originating episode; None for explicit facts
- `payload: serde_json::Value` — JSON blob, expected shape: `{"fact": "...", "tags": [...]}`
- `status: String` — `"active"` or `"archived"`
- `ccl: String` — Cognitive Context Layer
- `is_explicit: bool` — `true` if written directly via `store_memory`, `false` if Sleep-extracted
- `support_count: i32` — How many times this fact has been reinforced
- `relevance_score: f64` — 0.0–1.0; decays over time, resets to 1.0 on read

**`Edge`** — A directed relationship between two nodes with temporal bounds:
- `source_id: i64`, `target_id: i64` — NOT wrapped in Option; must be valid node IDs
- `relation: String` — e.g. `"WORKS_AT"`, `"parent_of"`
- `ccl: String`
- `valid_from: Option<String>`, `valid_until: Option<String>` — ISO date strings or null
- `weight: f64` — Always stored as 1.0; not yet used for ranking

**`TimeFilter`** — `#[derive(Default)]`:
- `after: Option<String>`, `before: Option<String>` — ISO date strings

**`MemoryResult`** — Token-optimized query output (no internal IDs or raw scores):
- `fact: String`
- `ccl: String`
- `last_updated: String` — The `updated_at` timestamp from the DB row
- `connections: Vec<MemoryConnection>` — 1-hop neighbors

**`MemoryConnection`** — A single 1-hop graph neighbor in query results:
- `relation: String`
- `entity: String` — The `fact` text from the neighbor node's payload
- `ccl: String`
- `valid_from: Option<String>`, `valid_until: Option<String>`

**Tests:** One test (`test_model_serialization`) verifies round-trip JSON serialization of `MemoryNode`. Passes.

---

### `domain/ports.rs`

**`MemoryRepository` trait** — All methods are synchronous (no `async`). This means the SQLite implementation runs on the calling thread; the async boundary sits at the application layer.

| Method | Signature | Purpose |
|---|---|---|
| `store_ccl_definition` | `(&self, def) -> Result<()>` | Upsert a CCL layer definition |
| `get_ccl_definitions` | `(&self, tenant_id) -> Result<Vec<CclDefinition>>` | List all CCL layers for a tenant |
| `store_episode` | `(&self, ep) -> Result<i64>` | Append a raw dialogue episode; returns new row ID |
| `store_node` | `(&self, node, embedding) -> Result<i64>` | Insert a fact node + its embedding atomically |
| `store_edge` | `(&self, edge) -> Result<()>` | Insert a relationship edge |
| `hybrid_search` | `(&self, text, embedding, tenant_id, limit) -> Result<Vec<MemoryNode>>` | Combined vector + BM25 search, returns raw nodes |
| `query_with_graph` | `(&self, text, embedding, tenant_id, time_filter, ccl_filter, limit) -> Result<Vec<MemoryResult>>` | Full pipeline: hybrid search + graph expansion + temporal/CCL filter + relevance boost |
| `boost_relevance` | `(&self, node_ids) -> Result<()>` | Reset `relevance_score` to 1.0 for given IDs (reading = reinforcement) |
| `find_similar_nodes` | `(&self, embedding, tenant_id, threshold, limit) -> Result<Vec<MemoryNode>>` | ANN search with distance threshold; used by conflict resolver |
| `update_node_support` | `(&self, node_id, new_payload) -> Result<()>` | Increment `support_count`, reset `relevance_score`, optionally merge payload |
| `delete_tenant` | `(&self, tenant_id) -> Result<()>` | Purge all data for a tenant |
| `export_tenant` | `(&self, tenant_id) -> Result<String>` | Serialize all node payloads to JSON |
| `sweep_decay` | `(&self, engine) -> Result<()>` | Apply decay math across all active nodes |

**`LlmClient` trait** — All methods `async`, via `#[async_trait]`:

| Method | Signature | Purpose |
|---|---|---|
| `extract_facts` | `(&self, dialogue, valid_ccls) -> Result<Vec<ExtractedFact>>` | LLM extracts structured facts from raw dialogue |
| `generate_ccl_description` | `(&self, ccl_name, context) -> Result<String>` | LLM generates a description for a new CCL |
| `embed_text` | `(&self, text) -> Result<Vec<f32>>` | Returns float vector (dimension matches DB config) |
| `compress_context` | `(&self, messages) -> Result<String>` | LLM compresses dialogue history into a dense summary |

**Support types:**

- `ExtractedFact` — LLM output per extracted statement: `fact: String`, `ccl: String` (defaults to `"reality"` via serde), `tags: Vec<String>`, `relationships: Vec<ExtractedRelationship>`
- `ExtractedRelationship` — Per relationship: `target_entity: String`, `relation: String`, `ccl: String`, `valid_from: Option<String>`, `valid_until: Option<String>`

---

### `domain/decay.rs`

**`DecayEngine`** — Holds one field: `pub half_life_days: f64`.

**`calculate_decay(current_score, days_elapsed) -> f64`:**
```rust
current_score * 0.5f64.powf(days_elapsed / self.half_life_days)
```
Standard exponential half-life formula. At `days_elapsed == half_life_days`, score halves exactly. Mathematically correct.

**`apply_to_node(node, days_elapsed) -> MemoryNode`:**
Calls `calculate_decay`, updates `relevance_score`, then archives if `new_score < 0.1` and status is `"active"`.

**Important limitation:** The decay sweep in `repository.rs::sweep_decay` always passes `days_elapsed = 1.0` (hardcoded). This means every decay run applies exactly 1 day of decay regardless of when `sweep_decay` was last called. There is no tracking of the last-decayed timestamp per node or globally. If `sweep_decay` is called multiple times per day, over-decay occurs; if called sporadically, under-decay occurs.

**Tests:** Two unit tests — verify half-life math (correct) and archiving threshold (correct).

---

### `domain/cognition/conflict_resolver.rs`

**`AdaptationResult` enum:**
- `Assimilated(i64)` — Exact duplicate found; support boosted
- `AccommodatedModify(i64)` — Similar fact found; payload merged, support boosted
- `AccommodateCreate` — No match; caller should create a new node

**`ConflictResolver`** — Two thresholds:
- `assimilation_threshold: f64 = 0.15` — "same fact" boundary
- `accommodation_threshold: f64 = 0.35` — "similar, update" boundary

**`resolve()` logic:**

1. Calls `find_similar_nodes(embedding, tenant_id, accommodation_threshold, limit=3)` — this returns nodes within cosine distance 0.35.
2. If empty → `AccommodateCreate`.
3. Takes the closest (`similar[0]`).
4. **Critical design issue:** The `assimilation_threshold` field (0.15) is **never actually used** in the comparison. The resolver cannot distinguish "within 0.15" from "within 0.35" because `find_similar_nodes` returns nodes without their distances — only the node struct. The comment even acknowledges this: _"We don't have the actual distance in MemoryNode, so we use position as heuristic."_
5. Instead, the decision falls back to exact string comparison: `if existing_fact == new_fact { Assimilate } else { AccommodateModify }`.

This means:
- Two facts that are semantically very similar but use different wording will always trigger `AccommodateModify` instead of `Assimilate`, even if they are within 0.15 distance.
- The `assimilation_threshold` field serves no actual purpose in the current code.

**Tag merging in AccommodateModify:** Uses `let Some(existing_tags) = ... && let Some(new_tags) = ...` — this requires Rust edition 2024 `let-chains` feature, which the project does use. Tags from both the existing and new payload are deduplicated and merged.

**Tests:** One test verifies `assimilation_threshold < accommodation_threshold`. No behavioral test.

---

## 4. Infrastructure Layer — What's Implemented

### `infrastructure/schema.rs`

Six objects created via `CREATE ... IF NOT EXISTS`:

**`ccl_registry`** (regular table):
```sql
id INTEGER PRIMARY KEY AUTOINCREMENT,
tenant_id TEXT NOT NULL,
name TEXT NOT NULL,
description TEXT NOT NULL,
UNIQUE(tenant_id, name)
```

**`episodes`** (regular table):
```sql
id INTEGER PRIMARY KEY AUTOINCREMENT,
tenant_id TEXT NOT NULL,
session_id TEXT NOT NULL,
raw_dialogue TEXT NOT NULL,
ccl TEXT DEFAULT 'reality',
created_at DATETIME DEFAULT CURRENT_TIMESTAMP
```

**`nodes`** (regular table):
```sql
id INTEGER PRIMARY KEY AUTOINCREMENT,
tenant_id TEXT NOT NULL,
source_episode_id INTEGER,
payload JSON NOT NULL,
status TEXT DEFAULT 'active',
ccl TEXT DEFAULT 'reality',
is_explicit BOOLEAN DEFAULT 0,
support_count INTEGER DEFAULT 1,
relevance_score REAL DEFAULT 1.0,
last_accessed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
FOREIGN KEY(source_episode_id) REFERENCES episodes(id)
```

**`edges`** (regular table):
```sql
source_id INTEGER,
target_id INTEGER,
relation TEXT NOT NULL,
ccl TEXT DEFAULT 'reality',
valid_from DATETIME,
valid_until DATETIME,
weight REAL DEFAULT 1.0,
FOREIGN KEY(source_id) REFERENCES nodes(id),
FOREIGN KEY(target_id) REFERENCES nodes(id)
```
Note: `edges` has no `PRIMARY KEY` and no UNIQUE constraint — duplicate edges can be inserted. There is also no `id` column, so it is impossible to reference or delete a specific edge row by ID.

**`vec_nodes`** (virtual table — sqlite-vec):
```sql
USING vec0(
    node_id INTEGER PRIMARY KEY,
    embedding float[{vector_dimension}]
)
```
Dimension is injected at schema creation time via string formatting. This is the ANN index.

**`fts_nodes`** (virtual table — FTS5):
```sql
USING fts5(
    payload,
    content='nodes',
    content_rowid='id'
)
```
Content table mode: the actual text lives in `nodes.payload`; FTS only stores the index. Sync is maintained by three triggers (`nodes_ai`, `nodes_ad`, `nodes_au`) for INSERT, DELETE, and UPDATE operations.

**No indexes** exist on `nodes.tenant_id`, `nodes.status`, `episodes.tenant_id`, or `edges.source_id` / `edges.target_id`. At scale, queries filtering by `tenant_id` will do full table scans.

**Tests:** One integration test validates all six objects are created successfully in an in-memory DB.

---

### `infrastructure/repository.rs`

`SqliteMemoryRepository` wraps a single `rusqlite::Connection`. Every `MemoryRepository` method is implemented. No stubs, no `todo!()`, no `unimplemented!()`.

**`store_node`** — Uses `unchecked_transaction` for atomicity between INSERT into `nodes` and INSERT into `vec_nodes`. The embedding is cast from `&[f32]` to `&[u8]` via raw pointer:
```rust
let embedding_bytes: &[u8] = unsafe {
    std::slice::from_raw_parts(
        embedding.as_ptr() as *const u8,
        std::mem::size_of_val(embedding),
    )
};
```
This is correct on all little-endian platforms (x86, ARM) but technically not portable to big-endian architectures. This same pattern is repeated in `hybrid_search`, `find_similar_nodes`, and `query_with_graph`.

**`hybrid_search`** — Combines vector + FTS via CTE:
```sql
WITH hybrid_matches AS (
    SELECT node_id, distance as score FROM vec_nodes WHERE embedding MATCH ?1 AND k = 10
    UNION ALL
    SELECT rowid as node_id, rank as score FROM fts_nodes WHERE fts_nodes MATCH ?2
),
ranked_matches AS (
    SELECT node_id, SUM(score) as combined_score FROM hybrid_matches GROUP BY node_id ORDER BY combined_score LIMIT ?3
)
SELECT ... FROM nodes n JOIN ranked_matches rm ON n.id = rm.node_id WHERE n.tenant_id = ?4 AND n.status = 'active'
ORDER BY rm.combined_score ASC;
```
**Score direction mismatch:** Vector distance (lower = closer) and BM25 rank (more negative = better in FTS5 `rank`) are summed raw. This is a subtle but real correctness issue: the combination `SUM(distance + rank)` does not produce a meaningful combined score because the two values are on different scales with different directions. In practice, whichever dimension produces larger magnitude numbers dominates. Proper RRF (Reciprocal Rank Fusion) or score normalization is not implemented.

**`query_with_graph`** — The full pipeline: vector + BM25 → top-5 nodes → expand 1 hop in both directions via `edges` → filter by `tenant_id`, `status='active'`, temporal bounds, and CCL. After retrieval, calls `boost_relevance` on all returned node IDs. Then runs a second query per result node to collect 1-hop connections for the returned `MemoryResult`.

**CCL filter bug:** When `ccl_filter` is an empty slice `[]`, `serde_json::to_string([])` produces `"[]"` and `json_each("[]")` returns nothing, so zero results are returned. The MCP server defaults `ccl_filter` to `["reality"]` if omitted, which avoids this in practice, but callers passing an empty filter get no results silently.

**`boost_relevance`** — Builds a parameterized query with `?1`, `?2`, ... positional params for each node ID via string formatting. This is safe but could be done more cleanly.

**`delete_tenant`** — Uses `unchecked_transaction`. Deletes from `vec_nodes` first (referencing `nodes`), then `nodes`, then `episodes`. Does NOT delete from `ccl_registry` or `edges`. After deletion, edges whose `source_id` or `target_id` pointed to now-deleted nodes remain in the `edges` table as dangling references (foreign keys are ON, but the `edges` delete is missing).

**`export_tenant`** — Only exports node payloads. Episodes, edges, and CCL definitions are not included in the export. This means export is not a complete backup.

**`sweep_decay`** — Loads all active nodes, applies 1.0 days of decay per call (hardcoded), updates in a transaction. The `DecayEngine.apply_to_node()` method is not used here — instead the calculation is done inline, duplicating the logic.

**Tests (integration):** Three tests — `test_store_episode_and_node`, `test_hybrid_search`, `test_tenant_isolation_delete_and_export`. All use in-memory SQLite. The `tempfile` dev-dependency is not used anywhere.

---

### `infrastructure/database.rs`

```rust
pub fn init_db(path: Option<&impl AsRef<Path>>) -> rusqlite::Result<Connection>
```

Loads the `sqlite-vec` extension via `sqlite3_auto_extension` using `unsafe` transmute — this is the documented way to load `sqlite-vec` in Rust and is safe in practice.

Sets pragmas:
- `journal_mode = WAL` — Good for concurrent reads. However, NeuroLithe is single-process, so WAL's main benefit here is crash safety.
- `foreign_keys = ON` — Enforces FK constraints.

**Issue:** `foreign_keys = ON` must be set per connection in SQLite. If the connection is ever recreated (e.g., for testing), the pragma must be re-applied. The current code correctly applies it in `init_db`, so this is fine as long as one connection is used throughout the process lifetime.

**Tests:** One test verifies in-memory DB opens without error.

---

### `infrastructure/llm.rs`

Three `LlmClient` implementations:

**`OpenAiClient`** — Handles `provider = "openai"` and `provider = "custom"`. All four trait methods are fully implemented. Uses configurable `base_url` (defaults to `https://api.openai.com/v1`), allowing OpenRouter, Ollama, LM Studio, vLLM, etc. The `response_format: {"type": "json_object"}` field ensures JSON output from `extract_facts`.

**`GeminiClient`** — Handles `provider = "gemini"`. All four trait methods are implemented. Uses Gemini's native REST API (`generativelanguage.googleapis.com`). `embed_text` calls the `embedContent` endpoint. `extract_facts` uses `responseMimeType: "application/json"` for structured output.

**`AnthropicClient`** — Handles `provider = "anthropic"`. Three of four methods are implemented. **`embed_text` is explicitly broken:**
```rust
async fn embed_text(&self, _text: &str) -> Result<Vec<f32>> {
    Err(anyhow!("Anthropic does not offer a native embedding API. ..."))
}
```
This means selecting `provider = "anthropic"` will cause every `push_dialogue`, `store_memory`, and `query_memory` call to fail (all require embeddings). The workaround documented in `neurolithe.toml` is to use `provider = "custom"` with an OpenAI-compatible embedding endpoint alongside Anthropic for chat. But there is only one `LlmClient` instance — no split-provider support is implemented.

**API key handling:** The `HTTP-Referer` and `X-Title` headers in OpenAI client calls are OpenRouter-specific headers that will be silently ignored by real OpenAI endpoints. They are harmless but reveal the original design target.

**No retry logic, no timeout configuration, no rate-limit handling** in any LLM client.

---

### `infrastructure/config.rs`

**`AppConfig`:**
- `llm: LlmConfig`
- `database: DatabaseConfig`

**`LlmConfig`:**
- `provider: LlmProvider` — Enum: `Openai | Gemini | Anthropic | Custom`
- `model: String` — Chat/generation model
- `embedding_model: String` — Embedding model (may be ignored for Anthropic)
- `base_url: Option<String>` — Custom API base URL (used by OpenAI + Custom clients)

**`DatabaseConfig`:**
- `vector_dimension: usize` — Default 1536 (OpenAI); must match embedding model output
- `path: Option<String>` — SQLite file path; None = in-memory

**Loading precedence (highest to lowest):**
1. Environment variables with prefix `NEUROLITHE__` and `__` separator (e.g. `NEUROLITHE__LLM__PROVIDER=gemini`)
2. `neurolithe.toml` in CWD (if it exists)
3. Hard-coded defaults

**API key resolution is not in AppConfig** — it is done ad-hoc in `main.rs` by checking `OPENAI_API_KEY`, `GEMINI_API_KEY`, `ANTHROPIC_API_KEY`, or falling back to `NEUROLITHE_API_KEY`. If none is set, the key defaults to the string `"dummy_key"` rather than failing with a clear error.

---

## 5. Application Layer — What's Implemented

### `application/app.rs`

`NeurolitheApp` is the public facade. Fields: `memory_repo`, `llm_client`, `retrieval_service`, `sleep_worker`, `session_manager` — all wired in `new()`.

**`unsafe impl Send for NeurolitheApp` and `unsafe impl Sync for NeurolitheApp`** — Necessary because `rusqlite::Connection` is not `Sync`. The comment says "repository serializes access via Mutex internally," but `SqliteMemoryRepository` does **not** use a `Mutex` — it holds a raw `Connection`. Access is serialized in practice because the Tokio runtime executes `.await` points cooperatively and all repository calls are synchronous (blocking), but this is not formally safe. If `NeurolitheApp` were ever used from multiple threads concurrently (e.g., via `Arc` passed to separate `tokio::spawn` tasks with actual parallelism), two threads could call into `SqliteMemoryRepository` simultaneously, racing on the `Connection`. The current single-threaded event loop in `run_stdio` avoids this, but the unsafe impl is technically unsound.

**Public methods:**

| Method | Async | What it does |
|---|---|---|
| `push_dialogue(tenant_id, session_id, new_message, ccl)` | yes | Calls `session_manager.push_dialogue` (stores episode, manages buffer, compresses if needed, queries facts). Then calls `sleep_worker.process_episode` inline (not background). |
| `store_memory(tenant_id, session_id, dialogue, ccl)` | yes | Stores episode directly, then calls `sleep_worker.process_episode`. Used as alternative to `push_dialogue` but not exposed as an MCP tool. |
| `store_explicit_fact(tenant_id, fact_text, tags, ccl)` | yes | Embeds text, creates a `MemoryNode` with `is_explicit: true`, stores directly — bypasses Sleep pipeline. |
| `query_memory(tenant_id, query, time_filter, ccl_filter)` | yes | Delegates to `retrieval_service.query`. |
| `register_ccl(tenant_id, name, description)` | yes | Stores a `CclDefinition`. |
| `get_ccl_layers(tenant_id)` | yes | Lists CCL definitions. |
| `delete_tenant(tenant_id)` | yes | Delegates to `memory_repo.delete_tenant`. |
| `export_tenant(tenant_id)` | yes | Delegates to `memory_repo.export_tenant`. |

**Note:** `register_ccl` and `get_ccl_layers` exist on `NeurolitheApp` but are not exposed as MCP tools. There is no way to call them from an MCP client.

**`push_dialogue` double-processes:** The method calls `session_manager.push_dialogue` which internally stores the episode, then creates a *new* `Episode` struct with `id: Some(0)` and calls `sleep_worker.process_episode` again. This means the episode is stored once (by `session_manager`) but `process_episode` is called twice (once inline in `push_dialogue`, and `session_manager` also notes the episode for "background extraction" that never happens). The `Episode { id: Some(0) }` passed to `sleep_worker` has a fake ID of 0.

---

### `application/session_manager.rs`

**`SessionBuffer`** — Per-session in-memory state: `messages: Vec<String>`, `summary: Option<String>`, `token_count: usize`.

**`SessionManager`** — Wraps sessions in a `Mutex<HashMap<String, SessionBuffer>>`. Fields: `token_threshold = 4000`, `keep_recent = 10`.

**Token estimation:**
```rust
fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}
```
Rough heuristic (~4 bytes per token). This significantly underestimates for languages other than English or ASCII.

**`push_dialogue` flow:**
1. Store episode in DB
2. Add message to in-memory buffer, increment token count
3. If `token_count > threshold`: call `compress_buffer`
4. After potential compression, read current buffer state
5. Embed the new message, call `query_with_graph` for relevant facts (using `ccl_filter = [ccl]`)
6. Return `ContextWindow { summary, recent_messages, relevant_facts }`

**`compress_buffer` flow:**
1. Take all messages except the last `keep_recent`
2. Prepend any existing summary to the text
3. Call `llm_client.compress_context` (blocking LLM call inside async)
4. Update buffer: drain compressed messages, store new summary, recalculate token count

**Sessions are in-memory only** — they are lost on process restart. There is no persistence or recovery of session state. Session buffers grow unboundedly in the `HashMap` — there is no eviction policy for old/stale sessions.

**Tests:** Two unit tests — token estimation arithmetic and `ContextWindow` serialization.

---

### `application/sleep.rs`

**`SleepWorker`** holds: `memory_repo`, `llm_client`, `decay_engine`, `conflict_resolver`.

**`run_decay_sweep()`** — Delegates to `memory_repo.sweep_decay`. This is never called anywhere in the codebase. There is no scheduled timer, no background task, and no MCP tool to trigger it. Decay sweeps must be triggered externally or the feature is effectively disabled.

**`process_episode(episode)`** — The Sleep pipeline:
1. Load CCL definitions for tenant
2. Call `llm_client.extract_facts` on `episode.raw_dialogue`
3. For each `ExtractedFact`:
   a. If CCL not in known set: auto-register with LLM-generated description
   b. Embed the fact text
   c. Run `conflict_resolver.resolve` (Tri-Modal)
   d. On `AccommodateCreate`: insert new `MemoryNode`
   e. For each relationship: embed target entity, resolve target through conflict resolver, insert/find target node, create `Edge`
4. No return value except `Ok(())`

**Fully wired:** Conflict resolution, CCL auto-registration, edge creation, and embedding are all functional end-to-end. The pipeline is correctly implemented.

---

### `application/retrieval.rs`

**`RetrievalService`** — Two methods:

**`query(tenant_id, raw_query, time_filter, ccl_filter) -> Result<Vec<MemoryResult>>`**
1. Embeds `raw_query`
2. Calls `memory_repo.query_with_graph` with embedding + BM25 + temporal + CCL filter + 1-hop expansion
3. Returns `Vec<MemoryResult>` (token-optimized, no IDs)

**`query_simple(tenant_id, raw_query) -> Result<Vec<MemoryNode>>`** — Legacy path; calls `hybrid_search` without graph expansion. Not used by any current MCP tool.

The hybrid search is real — both vector ANN and BM25 run inside a single SQL CTE. Graph expansion also runs in the same query. The primary concern is the score-direction mismatch noted in section 4.

---

## 6. Interfaces Layer — MCP Tools

The MCP server handles three JSON-RPC method names: `initialize`, `tools/list`, and `tools/call`.

### Protocol compliance

- `initialize` → responds with `protocolVersion: "2024-11-05"` ✓
- `tools/list` → responds with tool schemas ✓
- `tools/call` → dispatches to tool handler ✓
- Notifications (JSON-RPC requests without `id`) → silently dropped ✓
- Unknown methods → returns error code -32601 ✓

### Implemented MCP Tools

**`push_dialogue`**
```json
{
  "name": "push_dialogue",
  "inputSchema": {
    "type": "object",
    "properties": {
      "session_id": { "type": "string" },
      "new_message": { "type": "string" },
      "tenant_id": { "type": "string" }
    },
    "required": ["session_id", "new_message"]
  }
}
```
Note: `ccl` parameter is handled in the server code (defaults to `"reality"`) but is **not declared in the `inputSchema`**. Callers cannot discover or pass it.

Returns: `McpToolResult` with JSON-serialized `ContextWindow`:
```json
{
  "summary": "string or null",
  "recent_messages": ["string", ...],
  "relevant_facts": [{ "fact": "...", "ccl": "...", "last_updated": "...", "connections": [...] }]
}
```

---

**`store_memory`**
```json
{
  "name": "store_memory",
  "inputSchema": {
    "type": "object",
    "properties": {
      "fact_text": { "type": "string" },
      "tags": { "type": "array", "items": { "type": "string" } },
      "tenant_id": { "type": "string" }
    },
    "required": ["fact_text"]
  }
}
```
Note: `ccl` parameter handled in code (defaults to `"reality"`) but not in schema.

Returns: Text confirmation `"Memory fact explicitly stored."` or error message.

---

**`query_memory`**
```json
{
  "name": "query_memory",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": { "type": "string" },
      "time_filter": {
        "type": "object",
        "properties": {
          "after": { "type": "string" },
          "before": { "type": "string" }
        }
      },
      "tenant_id": { "type": "string" }
    },
    "required": ["query"]
  }
}
```
Note: `ccl_filter` handled in code (defaults to `["reality"]`) but not declared in schema.

Returns: JSON-serialized `Vec<MemoryResult>`.

---

**`delete_tenant`**
```json
{
  "name": "delete_tenant",
  "inputSchema": {
    "type": "object",
    "properties": {
      "tenant_id": { "type": "string" }
    },
    "required": []
  }
}
```
Returns: Text confirmation or error.

---

**`export_tenant`**
```json
{
  "name": "export_tenant",
  "inputSchema": {
    "type": "object",
    "properties": {
      "tenant_id": { "type": "string" }
    },
    "required": []
  }
}
```
Returns: Pretty-printed JSON with `tenant_id` + `extracted_facts` array.

---

### Tools mentioned in CLAUDE.md but not implemented

- `register_ccl` — App method exists, not exposed as MCP tool
- `get_ccl_layers` — App method exists, not exposed as MCP tool
- `store_document` / `archive_item` — JARVIS planned feature; not implemented

---

## 7. What's Missing or Stubbed

### No `unimplemented!()` or `todo!()` macros

A search of all source files finds zero `unimplemented!()` or `todo!()` calls. All trait methods have real implementations.

### Dead / non-functional code

1. **`run_decay_sweep` is never called.** `SleepWorker::run_decay_sweep` exists and is correctly implemented, but nothing in the codebase calls it. Decay never actually runs unless triggered manually (which no MCP tool or scheduler currently does). The `DecayEngine` and `sweep_decay` implementations are fully correct but completely inert.

2. **`store_memory` app method is unreachable.** `NeurolitheApp::store_memory` (which stores episodes and runs the sleep pipeline) is not called by any MCP tool. The MCP tool named `store_memory` calls `store_explicit_fact` instead. The `store_memory` method is dead code.

3. **`query_simple` is unused.** `RetrievalService::query_simple` is a legacy hybrid search without graph traversal. Nothing calls it.

4. **`StoreMemoryParams` and `QueryMemoryParams` structs in `mcp_types.rs`** are defined but never used. The MCP server parses parameters directly from `serde_json::Value`.

5. **`assimilation_threshold` field in `ConflictResolver`** is defined and documented but never used in any comparison.

6. **`thiserror` dependency** is declared in Cargo.toml but never used.

7. **`tempfile` dev-dependency** is declared but not used in any test.

8. **`Edge.weight`** is always stored as `1.0` and never read back or used in any ranking logic.

### Test coverage gaps

- No tests for `SleepWorker::process_episode` (the core pipeline)
- No tests for `ConflictResolver::resolve` behavior
- No tests for `SessionManager::push_dialogue` or `compress_buffer`
- No tests for `RetrievalService`
- No tests for `NeurolitheApp` methods
- No tests for `McpServer` / JSON-RPC parsing
- No tests for any LLM client (would require mocking or integration test)
- The `query_with_graph` method has no dedicated test
- `delete_tenant` edge-dangling bug has no test

---

## 8. JARVIS Integration Gap Analysis

### Feature 1: Permanent (non-decaying) memory

**What exists:** `MemoryNode.status` can be `"active"` or `"archived"`. `MemoryNode.is_explicit` marks directly-stored facts. The decay sweep already looks at `status = 'active'` to select candidates.

**What's absent:** No `decay_exempt` / `permanent` flag anywhere in the schema, models, or sweep logic. The sweep indiscriminately processes all active nodes — there is no skip path for any category. The schema has no column for this.

**Estimated complexity:** Small. Add a `permanent BOOLEAN DEFAULT 0` column to `nodes` schema (and migration path), propagate to `MemoryNode` model, add `AND permanent = 0` to the sweep SQL in `sweep_decay`. One or two hours of work.

---

### Feature 2: Document/item records (store_document MCP tool)

**What exists:** `MemoryNode.payload` is a free `serde_json::Value`, so it can hold arbitrary fields today. The `is_explicit` flag distinguishes direct writes. `source_episode_id` provides a provenance link.

**What's absent:** No document-specific schema fields (`dataId`, `title`, `date`, `type`, `artifact_uris`, `content`, `links`). No `store_document` MCP tool. The `payload` JSON shape is entirely unvalidated — storing a document there would work but with no type safety. No chunking model (see Feature 4).

**Estimated complexity:** Large. Requires: a new `documents` table (or a structured payload schema + Rust struct), `store_document` app method, MCP tool with schema, chunking pipeline integration, and artifact URI storage.

---

### Feature 3: Controlled tag vocabulary + tag map

**What exists:** Tags today are free strings inside `payload.tags` JSON array. The graph's `edges` table with `relation` and parent-child traversal is structurally ready to represent a tag taxonomy (tags as nodes, `parent_of` as edges). The 1-hop expansion in `query_with_graph` already traverses edges.

**What's absent:** No enforcement of a controlled vocabulary — any string is accepted. No tag normalization or synonym lookup. No API to manage the tag vocabulary. Tag taxonomy (parent-child links) would need to be bootstrapped as actual graph nodes and edges.

**Estimated complexity:** Medium. Tag nodes can be stored as regular `MemoryNode`s with a special CCL (e.g., `"taxonomy"`). Vocabulary enforcement needs a validation layer in the Sleep pipeline and `store_explicit_fact`. Synonym resolution needs a lookup step. The 1-hop graph expansion would naturally surface children, though only one hop deep.

---

### Feature 4: Chunking for long content

**What exists:** Nothing. Each `MemoryNode` stores one fact as a single embedding. The embedding is stored in `vec_nodes` as a single row per node.

**What's absent:** No chunking model (`chunkId`, `dataId`, `span`, chunk-to-document reference). No chunking pipeline that splits long documents into overlapping passages before embedding. No `chunk_nodes` table or schema.

**Estimated complexity:** Large. Requires: a new data model for chunks (new table or new `MemoryNode` subtype with `chunk_index`, `parent_document_id`, `text_span`), a chunking algorithm (fixed-window + overlap or sentence-aware), a new ingestion path that chunks before embedding, and a retrieval-time de-duplication step so chunks from the same document are grouped.

---

### Feature 5: Reference-returning retrieval (dataId + artifact URIs)

**What exists:** `MemoryResult` deliberately hides IDs. `MemoryNode.id` is an internal `i64` rowid. `export_tenant` can return all node payloads. The `payload` JSON field could theoretically carry a `dataId` field today.

**What's absent:** No `dataId` field in the schema or model. No artifact URI field. No retrieval mode that exposes references instead of inline text. No provenance tracking (`exact` vs `related` match type). No filter-by-type or filter-by-tag-with-child-expansion retrieval.

**Estimated complexity:** Medium (assuming Feature 2 is done first). Once `dataId` and `artifact_uris` exist in the schema, a new `query_references` app method + MCP tool can return them. The main work is schema design and adding a parallel retrieval path.

---

### Feature 6: Local Ollama embeddings (offline)

**What exists:** `OpenAiClient` already supports a configurable `base_url`. The `neurolithe.toml` documentation already shows an Ollama example:
```toml
provider = "custom"
model = "meta-llama/Llama-3-8b-chat-hf"
embedding_model = "nomic-embed-text"
base_url = "http://localhost:11434/v1"
```

**What's absent:** Nothing fundamental. The `custom` provider routes through `OpenAiClient` which calls `/embeddings` at the configured base URL. Ollama serves an OpenAI-compatible `/v1/embeddings` endpoint. The only required action is setting `vector_dimension = 768` in `neurolithe.toml` before first run (since `nomic-embed-text` outputs 768 dimensions, not 1536).

**Estimated complexity:** Small (essentially zero new code). It's a configuration choice. The caveat is the dimension lock — existing databases with 1536-dim vectors must be deleted and rebuilt.

---

## 9. Code Quality Notes

### Bugs

**B1 — `delete_tenant` leaks edges and CCL definitions:**
```rust
// delete_tenant only deletes vec_nodes, nodes, episodes
// Missing:
tx.execute("DELETE FROM edges WHERE source_id IN (SELECT id FROM nodes WHERE tenant_id = ?1) OR target_id IN (SELECT id FROM nodes WHERE tenant_id = ?1)", ...)?;
tx.execute("DELETE FROM ccl_registry WHERE tenant_id = ?1", ...)?;
```
After deleting a tenant, `edges` rows with references to the deleted nodes remain. Even with `foreign_keys = ON`, this works because the `nodes` delete happens and FK enforcement would fail — but actually the `vec_nodes` delete happens first (which removes nothing from `nodes`), then `nodes` is deleted. The FK constraint `edges(source_id) REFERENCES nodes(id)` means deleting `nodes` rows while `edges` references them should fail with a foreign key violation. This may cause `delete_tenant` to silently fail or return an error in practice.

**B2 — Decay sweep always uses `days_elapsed = 1.0` (hardcoded):**
```rust
let days_elapsed = 1.0; // repository.rs:481
```
No timestamp is tracked per node for last-decay time. If `sweep_decay` runs twice in one day, the node is double-decayed. If it runs after a week of inactivity, only one day of decay is applied. The `last_accessed_at` field exists on nodes but is not used for this purpose.

**B3 — `push_dialogue` processes an episode with fake ID 0:**
```rust
let episode = Episode {
    id: Some(0), // placeholder, already stored by session_manager
    ...
};
let _ = self.sleep_worker.process_episode(&episode).await;
```
`session_manager.push_dialogue` already stores the episode (getting the real ID). Then `push_dialogue` creates a new `Episode` with `id: Some(0)` and runs `process_episode` again. This (a) processes the same dialogue twice (double extraction), and (b) links extracted nodes to `source_episode_id = 0` which is not a real row. The `let _ = ...` also silently ignores errors from this second call.

**B4 — Hybrid score direction mismatch (false ranking):**
Vector distances from `sqlite-vec` are non-negative (0 = identical, higher = more distant). FTS5 `rank` values are negative (more negative = better match). Summing them gives a combined score where the signs work in opposite directions. The `ORDER BY combined_score ASC` favors low distance (good for vector) but high rank (bad for FTS5 since rank is negative). In practice the ranking may accidentally work because FTS5 rank is always negative, so it subtracts from vector distance and lower combined scores still win — but this is fragile and the semantics are wrong.

**B5 — `ccl_filter = []` silently returns no results:**
`json_each('[]')` in SQLite yields an empty set. Any `WHERE n.ccl IN (SELECT value FROM json_each(?7))` with an empty list matches nothing. There is no fallback or error.

**B6 — `assimilation_threshold` is inert:**
The `ConflictResolver` always uses `accommodation_threshold` as its only numeric distance filter. The `assimilation_threshold = 0.15` field is never applied. All matches within 0.35 distance that don't have identical fact text are treated as `AccommodateModify`, including very close semantic matches that should be `Assimilated`.

### Unsafe code

Two `unsafe` blocks in use:

1. `database.rs:init_db` — `sqlite3_auto_extension` transmute: standard pattern for `sqlite-vec`, acceptable.
2. `repository.rs` — Four identical casts of `&[f32]` to `&[u8]` via raw pointer. Correct on little-endian platforms; not portable to big-endian.
3. `app.rs` — `unsafe impl Send/Sync for NeurolitheApp` — technically unsound (see section 5). The `rusqlite::Connection` is not protected by a mutex; the safety relies on the single-threaded Tokio event loop never running two repository calls in parallel, which is true today but fragile.

### Design issues

**D1 — Blocking SQLite calls in async context.** All `MemoryRepository` methods are synchronous and called from `async` context without `tokio::task::spawn_blocking`. This blocks the Tokio executor thread during DB I/O. For a single-user MCP server this is fine, but it will degrade under any concurrency.

**D2 — Session buffers never evicted.** The in-memory `HashMap<String, SessionBuffer>` grows indefinitely. Long-running processes with many distinct `session_id`s will leak memory.

**D3 — LLM calls inside session lock.** In `session_manager.rs`, the `compress_buffer` method acquires the sessions `Mutex`, reads messages to compress, then releases it before calling the LLM. This is correct — the lock is not held across the async boundary. But the lock is re-acquired twice (once to read, once to write). This is safe but slightly inefficient.

**D4 — No error logging.** There is no tracing or logging framework (`log`, `tracing`, `env_logger`). Errors from `sleep_worker.process_episode` inside `push_dialogue` are discarded silently with `let _ = ...`. Debugging production issues requires running with a debugger or adding prints.

**D5 — Single SQLite connection is a bottleneck.** The entire system runs through one `rusqlite::Connection` owned by `SqliteMemoryRepository`. A connection pool (e.g., `r2d2-sqlite`) would be needed for any meaningful concurrency.

### Minor issues

- `neurolithe.toml` default `base_url` points to `openrouter.ai` while `LlmConfig::base_url` defaults to `openai.com` in code — the toml overrides the code default, but documentation could be clearer.
- `JsonRpcRequest.params` defaults to `Value::Null` via `#[serde(default)]`. Tool handlers use `.get("arguments")` on this Value — if `params` is Null, this returns None and arguments all default to empty strings / empty arrays. No validation error is returned.
- `main.rs` hardcodes `half_life_days = 7.0` in the `NeurolitheApp::new` call. This is not configurable via `neurolithe.toml` or environment variables.
- The `ccl` parameter is accepted by `push_dialogue` and `store_memory` MCP tools but not declared in their `inputSchema`, so MCP clients that read the schema will never send it.
