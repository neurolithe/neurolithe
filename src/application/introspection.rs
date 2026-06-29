//! Introspection — the on-demand "brain CT scan" behind the read-only MCP
//! tools (slice 10). Reuses the monitoring snapshot and the LTM retrieval
//! navigation; reference-returning where relevant (`dataId` + provenance).

use crate::application::ltm_retrieval::{LeafRef, LtmRetrieval, MapNode, NodeView};
use crate::application::monitoring::{MemoryMetrics, MonitoringService, RuntimeStats};
use crate::domain::ltm::LtmRepository;
use crate::domain::ports::{MemoryRepository, StmNodeSummary};
use anyhow::Result;
use serde::Serialize;
use std::sync::Arc;

/// A node with its immediate graph context (parents/children/document leaves).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeDetail {
    pub node: NodeView,
    pub parents: Vec<NodeView>,
    pub children: Vec<NodeView>,
    pub leaves: Vec<LeafRef>,
}

/// Where a document lives in the brain: its LTM leaf + ancestor branch, and how
/// many STM facts still carry it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraceResult {
    pub data_id: String,
    pub ltm_leaf: Option<NodeView>,
    pub ltm_branch: Vec<NodeView>,
    pub stm_node_count: i64,
}

/// A compact health summary (a subset of the full metrics).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HealthView {
    pub stm_active_nodes: i64,
    pub stm_archived_nodes: i64,
    pub ltm_tree_nodes: i64,
    pub ltm_orphan_leaves: i64,
    pub stm_db_bytes: i64,
    pub ltm_db_bytes: i64,
    pub feeder_lag: i64,
    pub feeder_errors: u64,
}

/// Read-only introspection over both stores.
pub struct IntrospectionService {
    stm: Arc<dyn MemoryRepository>,
    ltm: Arc<dyn LtmRepository>,
    monitoring: MonitoringService,
    retrieval: LtmRetrieval,
}

impl IntrospectionService {
    pub fn new(stm: Arc<dyn MemoryRepository>, ltm: Arc<dyn LtmRepository>) -> Self {
        Self {
            monitoring: MonitoringService::new(stm.clone(), ltm.clone()),
            retrieval: LtmRetrieval::new(ltm.clone()),
            stm,
            ltm,
        }
    }

    /// Full metrics snapshot. Runtime fields are unknown on the on-demand path
    /// (the daemon's metrics publisher carries the live runtime stats).
    pub fn memory_stats(&self) -> Result<MemoryMetrics> {
        self.monitoring.snapshot(&RuntimeStats::unknown())
    }

    /// STM facts, most-relevant first, optionally filtered by status.
    pub fn stm_list(&self, limit: usize, status: Option<&str>) -> Result<Vec<StmNodeSummary>> {
        self.stm.list_node_summaries(limit, status)
    }

    /// Top `depth` concept layers of the tree.
    pub fn ltm_map(&self, depth: usize) -> Result<Vec<MapNode>> {
        self.retrieval.map(depth)
    }

    /// One LTM node with its parents, children, and document leaves.
    pub fn inspect_node(&self, id: i64) -> Result<Option<NodeDetail>> {
        let Some(node) = self.ltm.get_node(id)? else {
            return Ok(None);
        };
        let parents = self
            .ltm
            .get_parents(id)?
            .iter()
            .map(NodeView::from)
            .collect();
        let children = self
            .ltm
            .get_children(id)?
            .iter()
            .map(NodeView::from)
            .collect();
        let leaves = self
            .ltm
            .get_child_leaves(id)?
            .into_iter()
            .map(|l| LeafRef {
                data_id: l.data_id,
                provenance: l.provenance,
            })
            .collect();
        Ok(Some(NodeDetail {
            node: NodeView::from(&node),
            parents,
            children,
            leaves,
        }))
    }

    /// A branch of the tree from `node` down to `depth`.
    pub fn subtree(&self, node: i64, depth: usize) -> Result<Option<MapNode>> {
        self.retrieval.subtree(node, depth)
    }

    /// Locate a `dataId` in both stores: its LTM leaf + ancestor branch, and the
    /// count of STM facts still carrying it.
    pub fn trace_data_id(&self, data_id: &str) -> Result<TraceResult> {
        let leaf = self.ltm.get_node_by_data_id(data_id)?;
        let (ltm_leaf, ltm_branch) = match &leaf {
            Some(leaf) => (
                Some(NodeView::from(leaf)),
                self.retrieval
                    .ancestors(leaf.id.expect("stored node has id"))?,
            ),
            None => (None, Vec::new()),
        };
        Ok(TraceResult {
            data_id: data_id.to_string(),
            ltm_leaf,
            ltm_branch,
            stm_node_count: self.stm.count_by_data_id(data_id)?,
        })
    }

    /// A compact health summary.
    pub fn health(&self) -> Result<HealthView> {
        let stm = self.stm.stm_stats()?;
        let ltm = self.ltm.ltm_stats()?;
        let runtime = RuntimeStats::unknown();
        Ok(HealthView {
            stm_active_nodes: stm.active_nodes,
            stm_archived_nodes: stm.archived_nodes,
            ltm_tree_nodes: ltm.tree_nodes,
            ltm_orphan_leaves: ltm.orphan_leaves,
            stm_db_bytes: stm.db_size_bytes,
            ltm_db_bytes: ltm.db_size_bytes,
            feeder_lag: runtime.feeder_lag,
            feeder_errors: runtime.feeder_errors,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ltm::{Leaf, Provenance, TreeEdge, TreeNode, TreeNodeKind};
    use crate::domain::models::{MemoryNode, TenantId};
    use crate::infrastructure::database::init_db;
    use crate::infrastructure::ltm_repository::SqliteLtmRepository;
    use crate::infrastructure::repository::SqliteMemoryRepository;
    use crate::infrastructure::schema::{init_ltm_schema, init_schema};
    use serde_json::json;

    const DIM: usize = 4;

    struct Fixture {
        svc: IntrospectionService,
        inbox: i64,
        leaf: i64,
    }

    /// A doc "doc_1" present in BOTH stores: an LTM leaf under the inbox, and an
    /// STM fact carrying its dataId.
    fn fixture() -> Fixture {
        let stm_conn = init_db(None as Option<&String>).unwrap();
        init_schema(&stm_conn, DIM).unwrap();
        let stm = Arc::new(SqliteMemoryRepository::new(stm_conn));
        stm.store_node(
            &MemoryNode {
                id: None,
                tenant_id: TenantId("jarvis".into()),
                source_episode_id: None,
                payload: json!({"fact": "metro doc", "dataId": "doc_1"}),
                status: "active".into(),
                ccl: "reality".into(),
                is_explicit: true,
                support_count: 1,
                relevance_score: 0.9,
            },
            &[0.5; DIM],
        )
        .unwrap();

        let ltm_conn = init_db(None as Option<&String>).unwrap();
        init_ltm_schema(&ltm_conn, DIM).unwrap();
        let ltm = Arc::new(SqliteLtmRepository::new(ltm_conn));
        ltm.seed_spine().unwrap();
        let inbox = ltm.get_inbox().unwrap().unwrap().id.unwrap();
        let leaf = ltm
            .create_node(
                &TreeNode::new("metro", "metro doc", TreeNodeKind::Leaf),
                None,
            )
            .unwrap();
        ltm.create_leaf(&Leaf {
            tree_node_id: leaf,
            data_id: "doc_1".into(),
            provenance: Provenance {
                source: "scanner".into(),
                ingested_at: None,
                confidence: 1.0,
            },
        })
        .unwrap();
        ltm.add_edge(&TreeEdge::new(inbox, leaf)).unwrap();

        let svc = IntrospectionService::new(
            stm as Arc<dyn MemoryRepository>,
            ltm as Arc<dyn LtmRepository>,
        );
        Fixture { svc, inbox, leaf }
    }

    #[test]
    fn test_memory_stats_and_health() {
        let f = fixture();
        let stats = f.svc.memory_stats().unwrap();
        assert_eq!(stats.stm_active_nodes, 1);
        assert_eq!(stats.ltm_leaves, 1);

        let health = f.svc.health().unwrap();
        assert_eq!(health.stm_active_nodes, 1);
        assert_eq!(health.ltm_orphan_leaves, 0);
        assert_eq!(health.feeder_lag, -1, "unknown on the on-demand path");
    }

    #[test]
    fn test_stm_list() {
        let f = fixture();
        let all = f.svc.stm_list(10, None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].fact, "metro doc");
        assert_eq!(all[0].data_id.as_deref(), Some("doc_1"));

        // Filtering by a status with no rows yields nothing.
        assert!(f.svc.stm_list(10, Some("archived")).unwrap().is_empty());
    }

    #[test]
    fn test_ltm_map_and_subtree_and_inspect() {
        let f = fixture();

        let map = f.svc.ltm_map(2).unwrap();
        assert_eq!(map.len(), 1, "single root");
        assert!(!map[0].children.is_empty());

        let sub = f.svc.subtree(f.inbox, 2).unwrap().expect("inbox exists");
        assert_eq!(sub.id, f.inbox);

        // inspect the inbox: the leaf is a child; it has no concept parents shown
        // beyond the root.
        let detail = f.svc.inspect_node(f.inbox).unwrap().expect("inbox exists");
        assert!(detail.children.iter().any(|c| c.id == f.leaf));
        assert_eq!(detail.leaves.len(), 1, "inbox holds the document leaf");
        assert_eq!(detail.leaves[0].data_id, "doc_1");

        assert!(f.svc.inspect_node(99999).unwrap().is_none());
    }

    /// trace_dataId finds the document in BOTH LTM (leaf + branch) and STM.
    #[test]
    fn test_trace_data_id_spans_both_stores() {
        let f = fixture();
        let trace = f.svc.trace_data_id("doc_1").unwrap();

        // LTM: the leaf and its ancestor branch (inbox -> root).
        assert!(trace.ltm_leaf.is_some());
        assert_eq!(trace.ltm_leaf.unwrap().id, f.leaf);
        assert!(trace.ltm_branch.iter().any(|n| n.id == f.inbox));
        // STM: one fact carries it.
        assert_eq!(trace.stm_node_count, 1);

        // An unknown id is absent everywhere.
        let missing = f.svc.trace_data_id("doc_x").unwrap();
        assert!(missing.ltm_leaf.is_none());
        assert_eq!(missing.stm_node_count, 0);
    }
}
