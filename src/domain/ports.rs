use crate::domain::models::{Edge, Episode, MemoryNode, MemoryResult, TenantId, TimeFilter};
use anyhow::Result;

pub trait MemoryRepository {
    /// Store raw episodic dialogue
    fn store_episode(&self, episode: &Episode) -> Result<i64>;
    
    /// Store a structured fact (Node) along with its embedding
    fn store_node(&self, node: &MemoryNode, embedding: &[f32]) -> Result<i64>;

    /// Store a relationship edge between two nodes
    fn store_edge(&self, edge: &Edge) -> Result<()>;

    /// Search via hybrid (Vector + FTS) search
    fn hybrid_search(&self, query_text: &str, query_embedding: &[f32], tenant_id: &TenantId, limit: usize) -> Result<Vec<MemoryNode>>;
    
    /// Full hybrid search with 1-hop graph traversal and temporal filtering (blueprint spec)
    fn query_with_graph(&self, query_text: &str, query_embedding: &[f32], tenant_id: &TenantId, time_filter: &TimeFilter, limit: usize) -> Result<Vec<MemoryResult>>;

    /// Boost relevance score back to 1.0 on read (blueprint: reading resets decay)
    fn boost_relevance(&self, node_ids: &[i64]) -> Result<()>;

    /// Find nodes semantically similar to the given embedding (for conflict resolution)
    fn find_similar_nodes(&self, embedding: &[f32], tenant_id: &TenantId, threshold: f64, limit: usize) -> Result<Vec<MemoryNode>>;

    /// Update existing node by incrementing support_count and resetting relevance (for assimilation)
    fn update_node_support(&self, node_id: i64, new_payload: Option<&serde_json::Value>) -> Result<()>;
    
    /// Delete all data for a given tenant
    fn delete_tenant(&self, tenant_id: &TenantId) -> Result<()>;

    /// Export all data for a given tenant as structured JSON string
    fn export_tenant(&self, tenant_id: &TenantId) -> Result<String>;

    /// Apply decay sweep across all active memory nodes
    fn sweep_decay(&self, engine: &crate::domain::decay::DecayEngine) -> Result<()>;
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtractedFact {
    pub fact: String,
    pub tags: Vec<String>,
    #[serde(default)]
    pub relationships: Vec<ExtractedRelationship>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtractedRelationship {
    pub target_entity: String,
    pub relation: String,
    #[serde(default)]
    pub valid_from: Option<String>,
    #[serde(default)]
    pub valid_until: Option<String>,
}

#[async_trait::async_trait]
pub trait LlmClient {
    /// Extract factual statements from given raw dialogue
    async fn extract_facts(&self, dialogue: &str) -> Result<Vec<ExtractedFact>>;
    
    /// Generate a 1536d float vector for text 
    async fn embed_text(&self, text: &str) -> Result<Vec<f32>>;

    /// Compress/summarize old dialogue messages into a dense summary
    async fn compress_context(&self, messages: &str) -> Result<String>;
}
