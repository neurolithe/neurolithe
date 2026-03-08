use crate::domain::models::{MemoryNode, MemoryResult, TenantId, TimeFilter};
use crate::domain::ports::{LlmClient, MemoryRepository};
use anyhow::Result;
use std::sync::Arc;

pub struct RetrievalService {
    llm_client: Arc<dyn LlmClient>,
    memory_repo: Arc<dyn MemoryRepository>,
}

impl RetrievalService {
    pub fn new(llm_client: Arc<dyn LlmClient>, memory_repo: Arc<dyn MemoryRepository>) -> Self {
        Self {
            llm_client,
            memory_repo,
        }
    }

    /// Retrieve relevant memory context with graph traversal and temporal filtering
    /// Returns token-optimized MemoryResult (no internal IDs/scores)
    pub async fn query(
        &self,
        tenant_id: &TenantId,
        raw_query: &str,
        time_filter: &TimeFilter,
        ccl_filter: &[String],
    ) -> Result<Vec<MemoryResult>> {
        // 1. Generate embedding for the raw query string
        let embedding = self.llm_client.embed_text(raw_query).await?;

        // 2. Perform hybrid search with graph traversal + temporal filtering
        let results =
            self.memory_repo
                .query_with_graph(raw_query, &embedding, tenant_id, time_filter, ccl_filter, 10)?;

        Ok(results)
    }

    /// Legacy: simple hybrid search without graph traversal (for backward compatibility)
    pub async fn query_simple(
        &self,
        tenant_id: &TenantId,
        raw_query: &str,
    ) -> Result<Vec<MemoryNode>> {
        let embedding = self.llm_client.embed_text(raw_query).await?;
        let results = self
            .memory_repo
            .hybrid_search(raw_query, &embedding, tenant_id, 10)?;
        Ok(results)
    }
}
