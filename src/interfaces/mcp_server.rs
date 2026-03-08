use crate::application::app::NeurolitheApp;
use crate::domain::models::TimeFilter;
use crate::interfaces::mcp_types::{JsonRpcRequest, JsonRpcResponse, McpToolResult};
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

pub struct McpServer {
    app: Arc<NeurolitheApp>,
}

impl McpServer {
    pub fn new(app: Arc<NeurolitheApp>) -> Self {
        Self { app }
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
                        .unwrap_or("default");
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
                        .unwrap_or("default");
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
                    let tenant_id = tool_args
                        .get("tenant_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default");
                    let query = tool_args
                        .get("query")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    // Parse optional time_filter
                    let time_filter = tool_args
                        .get("time_filter")
                        .and_then(|tf| serde_json::from_value::<TimeFilter>(tf.clone()).ok())
                        .unwrap_or_default();
                    let ccl_filter: Vec<String> = tool_args
                        .get("ccl_filter")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_else(|| vec!["reality".to_string()]);

                    match self.app.query_memory(tenant_id, query, &time_filter, &ccl_filter).await {
                        Ok(results) => {
                            let json_results =
                                serde_json::to_string(&results).unwrap_or("[]".into());
                            McpToolResult::ok(&json_results)
                        }
                        Err(e) => McpToolResult::err(format!("Query failed: {}", e)),
                    }
                }
                "delete_tenant" => {
                    let tenant_id = tool_args
                        .get("tenant_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default");
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
                        .unwrap_or("default");
                    match self.app.export_tenant(tenant_id).await {
                        Ok(json_export) => McpToolResult::ok(&json_export),
                        Err(e) => McpToolResult::err(format!("Export failed: {}", e)),
                    }
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
                    "version": "0.1.0"
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
                        "description": "Search the long-term knowledge graph for relevant historical context. Returns token-optimized results with 1-hop connections and temporal bounds.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": { "type": "string", "description": "The query to search for in memory" },
                                "time_filter": {
                                    "type": "object",
                                    "description": "Optional temporal boundaries",
                                    "properties": {
                                        "after": { "type": "string", "description": "Only return memories after this date (YYYY-MM-DD)" },
                                        "before": { "type": "string", "description": "Only return memories before this date (YYYY-MM-DD)" }
                                    }
                                },
                                "tenant_id": { "type": "string", "description": "Optional tenant ID. Defaults to 'default'." }
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
                    }
                ]
            });
            JsonRpcResponse::success(id, tools_list)
        } else {
            JsonRpcResponse::error(id, -32601, format!("Method not found: {}", req.method))
        }
    }
}
