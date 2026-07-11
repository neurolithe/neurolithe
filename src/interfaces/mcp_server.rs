use crate::application::app::NeurolitheApp;
use crate::application::introspection::{IntrospectionService, LeafPage};
use crate::application::query_service::{QueryRequest, QueryScope, QueryService};
use crate::domain::models::{DEFAULT_TENANT, TimeFilter};
use crate::interfaces::bus_query::flatten_recall;
use crate::interfaces::mcp_types::{JsonRpcRequest, JsonRpcResponse, McpToolResult};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Default recall breadth for the MCP query tools when the caller omits it.
const DEFAULT_K: usize = 10;

/// Render an introspection result as an MCP tool result (JSON text on success).
fn introspect_result<T: Serialize>(result: anyhow::Result<T>) -> McpToolResult {
    match result {
        Ok(value) => {
            McpToolResult::ok(serde_json::to_string(&value).unwrap_or_else(|_| "null".into()))
        }
        Err(e) => McpToolResult::err(format!("introspection failed: {e}")),
    }
}

pub struct McpServer {
    app: Arc<NeurolitheApp>,
    introspection: Arc<IntrospectionService>,
    /// The shared recall use-case — the SAME service the `memory.query` bus door
    /// runs on (design §7 "doors"). Both delivery adapters route through it so
    /// the two doors can never drift on tenant/scope semantics again (the cause
    /// of the field-report §1 empty-results bug).
    query: QueryService,
}

impl McpServer {
    pub fn new(
        app: Arc<NeurolitheApp>,
        introspection: Arc<IntrospectionService>,
        query: QueryService,
    ) -> Self {
        Self {
            app,
            introspection,
            query,
        }
    }

    /// Build a [`QueryRequest`] from MCP tool arguments, applying the shared
    /// defaults (tenant = [`DEFAULT_TENANT`], `reality` layer, breadth
    /// [`DEFAULT_K`]). `scope` is fixed per tool. Keeps both query tools on one
    /// parse path so their defaults never diverge.
    fn query_request(&self, args: &Value, scope: QueryScope) -> QueryRequest {
        let tenant = args
            .get("tenant_id")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_TENANT)
            .to_string();
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let time_filter = args
            .get("time_filter")
            .and_then(|tf| serde_json::from_value::<TimeFilter>(tf.clone()).ok())
            .unwrap_or_default();
        let ccl: Vec<String> = args
            .get("ccl_filter")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let k = args
            .get("k")
            .and_then(|v| v.as_u64())
            .map(|k| k as usize)
            .unwrap_or(DEFAULT_K);
        QueryRequest {
            scope,
            tenant,
            query,
            k,
            time_filter,
            ccl,
            context_key: None,
        }
    }

    pub async fn run_stdio(&self) -> anyhow::Result<()> {
        let stdin = io::stdin();
        let mut stdout = io::stdout();
        let mut reader = BufReader::new(stdin).lines();

        while let Some(line) = reader.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }

            let response_opt = match serde_json::from_str::<JsonRpcRequest>(&line) {
                Ok(req) => {
                    if req.id.is_none() {
                        continue;
                    }
                    Some(self.handle_request(req).await)
                }
                Err(e) => Some(JsonRpcResponse::error(
                    Value::Null,
                    -32700,
                    format!("Parse error: {}", e),
                )),
            };

            if let Some(response) = response_opt {
                let response_str = serde_json::to_string(&response)?;
                stdout.write_all(response_str.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
        }

        Ok(())
    }

    async fn handle_request(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let id = req.id.clone().unwrap_or(Value::Null);

        if req.method == "tools/call" {
            let tool_name = req
                .params
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            let tool_args = req.params.get("arguments").cloned().unwrap_or(Value::Null);

            let result = match tool_name {
                "store_memory" => {
                    // Blueprint: explicit fact storage (bypasses Sleep pipeline)
                    let tenant_id = tool_args
                        .get("tenant_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or(DEFAULT_TENANT);
                    let fact_text = tool_args
                        .get("fact_text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let tags: Vec<String> = tool_args
                        .get("tags")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let ccl = tool_args
                        .get("ccl")
                        .and_then(|v| v.as_str())
                        .unwrap_or("reality");

                    match self
                        .app
                        .store_explicit_fact(tenant_id, fact_text, &tags, ccl)
                        .await
                    {
                        Ok(_) => McpToolResult::ok("Memory fact explicitly stored."),
                        Err(e) => McpToolResult::err(format!("Failed to store memory: {}", e)),
                    }
                }
                "push_dialogue" => {
                    // Flow 1: Push dialogue to STM, compress, return optimized context
                    let tenant_id = tool_args
                        .get("tenant_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or(DEFAULT_TENANT);
                    let session_id = tool_args
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default");
                    let new_message = tool_args
                        .get("new_message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let ccl = tool_args
                        .get("ccl")
                        .and_then(|v| v.as_str())
                        .unwrap_or("reality");

                    match self
                        .app
                        .push_dialogue(tenant_id, session_id, new_message, ccl)
                        .await
                    {
                        Ok(context_window) => {
                            let json_ctx =
                                serde_json::to_string(&context_window).unwrap_or("{}".into());
                            McpToolResult::ok(&json_ctx)
                        }
                        Err(e) => McpToolResult::err(format!("Failed to process dialogue: {}", e)),
                    }
                }
                "query_memory" => {
                    // STM recall over the shared QueryService (same path as the
                    // bus door). Defaults to the JARVIS tenant the feeder writes.
                    let req = self.query_request(&tool_args, QueryScope::Stm);
                    match self.query.execute(&req).await {
                        Ok(outcome) => {
                            let json_results =
                                serde_json::to_string(&outcome.stm).unwrap_or("[]".into());
                            McpToolResult::ok(&json_results)
                        }
                        Err(e) => McpToolResult::err(format!("Query failed: {}", e)),
                    }
                }
                "recall_ltm" => {
                    // Reference-returning search of the permanent archive: locate
                    // the nearest concept/document and surface its `dataId` +
                    // provenance so the caller can fetch the original (design §3.3).
                    let req = self.query_request(&tool_args, QueryScope::Ltm);
                    match self.query.execute(&req).await {
                        Ok(outcome) => {
                            let entries: Vec<_> =
                                outcome.ltm.iter().flat_map(flatten_recall).collect();
                            let json = serde_json::to_string(&entries).unwrap_or("[]".into());
                            McpToolResult::ok(&json)
                        }
                        Err(e) => McpToolResult::err(format!("LTM recall failed: {}", e)),
                    }
                }
                "delete_tenant" => {
                    let tenant_id = tool_args
                        .get("tenant_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or(DEFAULT_TENANT);
                    match self.app.delete_tenant(tenant_id).await {
                        Ok(_) => McpToolResult::ok(format!(
                            "Successfully deleted all data for tenant {}",
                            tenant_id
                        )),
                        Err(e) => McpToolResult::err(format!("Deletion failed: {}", e)),
                    }
                }
                "export_tenant" => {
                    let tenant_id = tool_args
                        .get("tenant_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or(DEFAULT_TENANT);
                    match self.app.export_tenant(tenant_id).await {
                        Ok(json_export) => McpToolResult::ok(&json_export),
                        Err(e) => McpToolResult::err(format!("Export failed: {}", e)),
                    }
                }
                // --- read-only introspection (CT scan) ---
                "memory_stats" => introspect_result(self.introspection.memory_stats()),
                "health" => introspect_result(self.introspection.health()),
                "placement_debug" => {
                    let sample = tool_args
                        .get("sample")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(30) as usize;
                    introspect_result(self.introspection.placement_debug(sample))
                }
                "stm_list" => {
                    let limit = tool_args
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(20) as usize;
                    let offset = tool_args
                        .get("offset")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    let status = tool_args.get("status").and_then(|v| v.as_str());
                    let contains = tool_args.get("contains").and_then(|v| v.as_str());
                    introspect_result(self.introspection.stm_list(limit, offset, status, contains))
                }
                "ltm_map" => {
                    let depth =
                        tool_args.get("depth").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
                    introspect_result(self.introspection.ltm_map(depth))
                }
                "inspect_node" => match tool_args.get("id").and_then(|v| v.as_i64()) {
                    Some(node_id) => {
                        let page = LeafPage {
                            child_limit: tool_args
                                .get("child_limit")
                                .and_then(|v| v.as_u64())
                                .map(|v| v as usize),
                            child_offset: tool_args
                                .get("child_offset")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0) as usize,
                            summary_max_chars: tool_args
                                .get("summary_max_chars")
                                .and_then(|v| v.as_u64())
                                .map(|v| v as usize),
                        };
                        introspect_result(self.introspection.inspect_node(node_id, page))
                    }
                    None => McpToolResult::err("inspect_node requires an integer 'id'"),
                },
                "subtree" => match tool_args.get("node").and_then(|v| v.as_i64()) {
                    Some(node_id) => {
                        let depth =
                            tool_args.get("depth").and_then(|v| v.as_u64()).unwrap_or(2) as usize;
                        introspect_result(self.introspection.subtree(node_id, depth))
                    }
                    None => McpToolResult::err("subtree requires an integer 'node'"),
                },
                "trace_dataId" => {
                    let data_id = tool_args
                        .get("dataId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    introspect_result(self.introspection.trace_data_id(data_id))
                }
                _ => McpToolResult::err(format!("Unknown tool: {}", tool_name)),
            };

            JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
        } else if req.method == "initialize" {
            let init_result = serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {
                        "listChanged": true
                    }
                },
                "serverInfo": {
                    "name": "NeuroLithe",
                    "version": env!("CARGO_PKG_VERSION")
                }
            });
            JsonRpcResponse::success(id, init_result)
        } else if req.method == "tools/list" {
            let tools_list = serde_json::json!({
                "tools": [
                    {
                        "name": "push_dialogue",
                        "description": "Push the latest conversation turn to Short-Term Memory. The service automatically extracts facts and stores them in long-term memory.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "session_id": { "type": "string", "description": "The session ID for the conversation" },
                                "new_message": { "type": "string", "description": "The new dialogue message to process" },
                                "tenant_id": { "type": "string", "description": "Optional tenant ID. Defaults to 'default'." }
                            },
                            "required": ["session_id", "new_message"]
                        }
                    },
                    {
                        "name": "store_memory",
                        "description": "Explicitly store a crucial fact immediately, bypassing the background extraction pipeline.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "fact_text": { "type": "string", "description": "The factual statement to store" },
                                "tags": { "type": "array", "items": { "type": "string" }, "description": "Tags for categorizing the fact" },
                                "tenant_id": { "type": "string", "description": "Optional tenant ID. Defaults to 'default'." }
                            },
                            "required": ["fact_text"]
                        }
                    },
                    {
                        "name": "query_memory",
                        "description": "Search SHORT-TERM working memory (recent/decaying facts) for relevant context. Hybrid keyword + semantic search; returns token-optimized facts with 1-hop connections and temporal bounds. For the permanent archive of documents, use recall_ltm.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": { "type": "string", "description": "The query to search for in memory" },
                                "k": { "type": "integer", "description": "Max facts to return. Defaults to 10." },
                                "time_filter": {
                                    "type": "object",
                                    "description": "Optional temporal boundaries",
                                    "properties": {
                                        "after": { "type": "string", "description": "Only return memories after this date (YYYY-MM-DD)" },
                                        "before": { "type": "string", "description": "Only return memories before this date (YYYY-MM-DD)" }
                                    }
                                },
                                "tenant_id": { "type": "string", "description": "Optional tenant ID. Defaults to the JARVIS tenant ('jarvis')." }
                            },
                            "required": ["query"]
                        }
                    },
                    {
                        "name": "recall_ltm",
                        "description": "Search the PERMANENT long-term archive (all ingested documents) by meaning. Reference-returning: each hit carries the document's dataId + provenance + ancestor concepts, so you can fetch the original. This is the primary tool for finding a scanned document, receipt, letter, or report.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": { "type": "string", "description": "What to look for in the archive (a phrase, topic, merchant, or document description)." },
                                "tenant_id": { "type": "string", "description": "Optional tenant ID. Defaults to the JARVIS tenant ('jarvis')." }
                            },
                            "required": ["query"]
                        }
                    },
                    {
                        "name": "delete_tenant",
                        "description": "Delete all memory nodes, edges, and episodes for a specific tenant.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "tenant_id": { "type": "string", "description": "Optional tenant ID. Defaults to 'default'." }
                            },
                            "required": []
                        }
                    },
                    {
                        "name": "export_tenant",
                        "description": "Export all memory data for a tenant as a JSON string.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "tenant_id": { "type": "string", "description": "Optional tenant ID. Defaults to 'default'." }
                            },
                            "required": []
                        }
                    },
                    {
                        "name": "memory_stats",
                        "description": "CT scan: full metrics snapshot of both memory stores (STM counts/decay histogram, LTM tree size/depth/inbox, DB sizes).",
                        "inputSchema": { "type": "object", "properties": {}, "required": [] }
                    },
                    {
                        "name": "health",
                        "description": "Compact health summary: STM/LTM counts, orphan leaves, DB sizes, feeder lag (feeder_lag = -1 means unknown on this on-demand path; live lag is on the memory.metrics stream).",
                        "inputSchema": { "type": "object", "properties": {}, "required": [] }
                    },
                    {
                        "name": "placement_debug",
                        "description": "Placement calibration: for a sample of document leaves, the distance to their nearest concept (threshold-free). Used to tune the placement threshold to real embedding distances.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "sample": { "type": "integer", "description": "Number of leaves to probe. Defaults to 30." }
                            },
                            "required": []
                        }
                    },
                    {
                        "name": "stm_list",
                        "description": "List STM working-memory facts (most-relevant first) with score, status, and age. Supports pagination and a keyword filter so you can find facts without pulling the whole store.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "limit": { "type": "integer", "description": "Max facts to return. Defaults to 20." },
                                "offset": { "type": "integer", "description": "How many facts to skip (pagination). Defaults to 0." },
                                "status": { "type": "string", "description": "Optional filter: 'active' or 'archived'." },
                                "contains": { "type": "string", "description": "Optional case-insensitive substring the fact text must contain." }
                            },
                            "required": []
                        }
                    },
                    {
                        "name": "ltm_map",
                        "description": "The top N concept layers of the long-term knowledge tree (a compact table of contents).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "depth": { "type": "integer", "description": "Number of layers from the roots. Defaults to 3." }
                            },
                            "required": []
                        }
                    },
                    {
                        "name": "inspect_node",
                        "description": "Inspect one LTM node: its summary, parents, children, and document leaves (dataIds + provenance). Children/leaves are paged (child_limit/child_offset) and summaries capped (summary_max_chars); child_count/leaf_count report the totals.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "integer", "description": "The LTM node id." },
                                "child_limit": { "type": "integer", "description": "Max children/leaves to return. Defaults to 50." },
                                "child_offset": { "type": "integer", "description": "How many children/leaves to skip (pagination). Defaults to 0." },
                                "summary_max_chars": { "type": "integer", "description": "Cap each summary to this many chars (0 = full). Defaults to 200." }
                            },
                            "required": ["id"]
                        }
                    },
                    {
                        "name": "subtree",
                        "description": "A branch of the LTM tree from a node down to a given depth (concepts only).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "node": { "type": "integer", "description": "The LTM node id to start from." },
                                "depth": { "type": "integer", "description": "Number of layers. Defaults to 2." }
                            },
                            "required": ["node"]
                        }
                    },
                    {
                        "name": "trace_dataId",
                        "description": "Locate a document by dataId across the brain: its LTM leaf + ancestor branch, and how many STM facts carry it.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "dataId": { "type": "string", "description": "The document's dataId (Ledger/Pithos reference)." }
                            },
                            "required": ["dataId"]
                        }
                    }
                ]
            });
            JsonRpcResponse::success(id, tools_list)
        } else {
            JsonRpcResponse::error(id, -32601, format!("Method not found: {}", req.method))
        }
    }
}
