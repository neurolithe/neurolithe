use serde::{Deserialize, Serialize};

/// The default cognitive layer for knowledge facts (documents, extracted facts).
pub const REALITY_CCL: &str = "reality";

/// The working-memory layer: short-lived situational notes the agent leaves for
/// itself (STM-WORKING-MEMORY). Decays on a much shorter half-life than
/// `reality` so stale session context fades fast (see [`crate::domain::decay`]).
pub const WORKING_CCL: &str = "working";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantId(pub String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionId(pub String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CclDefinition {
    pub id: Option<i64>,
    pub tenant_id: TenantId,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    pub id: Option<i64>,
    pub tenant_id: TenantId,
    pub session_id: SessionId,
    pub raw_dialogue: String,
    pub ccl: String,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryNode {
    pub id: Option<i64>,
    pub tenant_id: TenantId,
    pub source_episode_id: Option<i64>,
    pub payload: serde_json::Value,

    // Cognitive Attributes
    pub status: String,
    pub ccl: String,
    pub is_explicit: bool,
    pub support_count: i32,
    pub relevance_score: f64,

    /// Working-memory context key (the *who/where* axis, orthogonal to `ccl`).
    /// Set only on situational notes so recency orientation can be scoped to one
    /// active thread/run-chain (STM-WORKING-MEMORY §5a). Knowledge-path writes
    /// (feeder, sleep pipeline, direct `store_memory`) leave it `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub source_id: i64,
    pub target_id: i64,
    pub relation: String,
    pub ccl: String,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub weight: f64,
}

/// Temporal filter for memory queries
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TimeFilter {
    pub after: Option<String>,
    pub before: Option<String>,
}

/// Token-optimized output for query_memory (no internal IDs/scores)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryResult {
    pub fact: String,
    pub ccl: String,
    pub last_updated: String,
    pub connections: Vec<MemoryConnection>,
}

/// A 1-hop connection returned in query results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConnection {
    pub relation: String,
    pub entity: String,
    pub ccl: String,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_model_serialization() {
        let node = MemoryNode {
            id: Some(1),
            tenant_id: TenantId("tenant-123".into()),
            source_episode_id: Some(42),
            payload: json!({"fact": "User is a programmer", "tags": ["profession"]}),
            status: "active".into(),
            ccl: "reality".into(),
            is_explicit: true,
            support_count: 1,
            relevance_score: 1.0,
            context_key: None,
        };

        let serialized = serde_json::to_string(&node).unwrap();
        assert!(serialized.contains("tenant-123"));

        let deserialized: MemoryNode = serde_json::from_str(&serialized).unwrap();
        assert_eq!(node, deserialized);
    }
}
