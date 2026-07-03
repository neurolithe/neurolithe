//! Ingestion — turn a parsed `document.completed` event into memory, written to
//! BOTH stores: a permanent LTM leaf and a decaying STM working-memory fact.
//!
//! This is the broker-independent core of the feeder (the rdkafka loop in
//! `interfaces::kafka_feeder` drives it). Fetch text from Pithos -> distill ->
//! place in LTM + resolve into STM. Tombstone -> forget in both.
//!
//! Dimension note: the feeder writes the same distilled embedding to both
//! stores, so it requires `stm.vector_dimension == ltm.vector_dimension`. A
//! mismatch surfaces as a loud store error rather than silent corruption.

use crate::application::distill::Distiller;
use crate::application::ltm_placement::{DocumentToPlace, LtmPlacement};
use crate::domain::cognition::conflict_resolver::{AdaptationResult, ConflictResolver};
use crate::domain::ltm::{LtmRepository, Provenance};
use crate::domain::models::{MemoryNode, TenantId};
use crate::domain::ports::{ArtifactStore, FetchOutcome, LlmClient, MemoryRepository};
use anyhow::{Result, anyhow};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

/// The enriched `document.completed` event (schemaVersion 2). A group event
/// carries `groupId` + a multi-page list; a single-item event carries `dataId`.
/// Extra fields (pageCount, stage, ts, …) are ignored.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DocumentCompleted {
    #[serde(rename = "groupId", default)]
    pub group_id: Option<String>,
    #[serde(rename = "dataId", default)]
    pub data_id: Option<String>,
    #[serde(default)]
    pub pages: Vec<PageRef>,
}

/// One page within a completed document: a `pt://` text pointer + its tags.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PageRef {
    #[serde(rename = "pageIndex", default)]
    pub page_index: Option<u32>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(rename = "textUri", default)]
    pub text_uri: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl DocumentCompleted {
    /// The document's archive id: the group id for a multi-page group, else the
    /// single item's data id. `None` for a structurally invalid event.
    pub fn document_id(&self) -> Option<&str> {
        self.group_id.as_deref().or(self.data_id.as_deref())
    }
}

/// What ingesting one document did.
#[derive(Debug, Clone, PartialEq)]
pub enum IngestOutcome {
    Ingested {
        data_id: String,
        leaf_node_id: i64,
        matched: bool,
    },
    /// No usable text (all pages missing/empty) — nothing learned, but the
    /// offset still advances.
    Skipped { data_id: String },
}

/// Dual-writes documents into LTM (placement) + STM (conflict resolver).
pub struct IngestionService {
    stm: Arc<dyn MemoryRepository>,
    placement: LtmPlacement,
    distiller: Distiller,
    artifacts: Arc<dyn ArtifactStore>,
    conflict_resolver: ConflictResolver,
    tenant: TenantId,
}

impl IngestionService {
    pub fn new(
        stm: Arc<dyn MemoryRepository>,
        ltm: Arc<dyn LtmRepository>,
        llm: Arc<dyn LlmClient>,
        artifacts: Arc<dyn ArtifactStore>,
        embedding_dim: usize,
        tenant: impl Into<String>,
    ) -> Self {
        Self {
            placement: LtmPlacement::new(ltm),
            distiller: Distiller::new(llm, embedding_dim),
            conflict_resolver: ConflictResolver::new(),
            stm,
            artifacts,
            tenant: TenantId(tenant.into()),
        }
    }

    /// Ingest one completed document: fetch its page text from Pithos, distill,
    /// then write a permanent LTM leaf and a decaying STM fact.
    pub async fn ingest(&self, event: &DocumentCompleted) -> Result<IngestOutcome> {
        let document_id = event
            .document_id()
            .ok_or_else(|| anyhow!("document.completed has neither groupId nor dataId"))?
            .to_string();

        // Fetch page texts in page order; tolerate missing pages. Collect tags.
        let mut pages = event.pages.clone();
        pages.sort_by_key(|p| p.page_index.unwrap_or(0));
        let mut texts: Vec<String> = Vec::new();
        let mut tags: Vec<String> = Vec::new();
        for page in &pages {
            for t in &page.tags {
                if !tags.contains(t) {
                    tags.push(t.clone());
                }
            }
            let Some(uri) = page.text_uri.as_deref() else {
                continue;
            };
            if let FetchOutcome::Found(text) = self.artifacts.fetch_text(uri).await? {
                texts.push(text);
            }
        }

        if texts.is_empty() {
            return Ok(IngestOutcome::Skipped {
                data_id: document_id,
            });
        }

        let distillate = self.distiller.distill(&texts.join("\n\n")).await?;

        // LTM: file a permanent leaf under the best concept (or inbox).
        let placement = self.placement.place(&DocumentToPlace {
            name: document_id.clone(),
            summary: distillate.summary.clone(),
            embedding: distillate.embedding.clone(),
            data_id: document_id.clone(),
            provenance: Provenance {
                source: "document.completed".into(),
                ingested_at: None,
                confidence: 1.0,
            },
        })?;

        // STM: a decaying working-memory fact, merged via the conflict resolver.
        let payload = json!({ "fact": distillate.summary, "tags": tags, "dataId": document_id });
        if let AdaptationResult::AccommodateCreate = self.conflict_resolver.resolve(
            &self.stm,
            &distillate.embedding,
            &self.tenant,
            &payload,
        )? {
            let node = MemoryNode {
                id: None,
                tenant_id: self.tenant.clone(),
                source_episode_id: None,
                payload,
                status: "active".into(),
                ccl: "reality".into(),
                is_explicit: true,
                support_count: 1,
                relevance_score: 1.0,
                context_key: None,
            };
            self.stm.store_node(&node, &distillate.embedding)?;
        }

        Ok(IngestOutcome::Ingested {
            data_id: document_id,
            leaf_node_id: placement.leaf_node_id,
            matched: placement.matched,
        })
    }

    /// Tombstone: forget a document from BOTH stores.
    pub async fn forget(&self, document_id: &str) -> Result<()> {
        self.placement.forget(document_id)?;
        self.stm.delete_nodes_by_data_id(document_id)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::CclDefinition;
    use crate::domain::ports::ExtractedFact;
    use crate::infrastructure::database::init_db;
    use crate::infrastructure::ltm_repository::SqliteLtmRepository;
    use crate::infrastructure::repository::SqliteMemoryRepository;
    use crate::infrastructure::schema::{init_ltm_schema, init_schema};
    use async_trait::async_trait;

    const DIM: usize = 8;

    struct StubLlm;
    #[async_trait]
    impl LlmClient for StubLlm {
        async fn extract_facts(
            &self,
            _d: &str,
            _c: &[CclDefinition],
        ) -> Result<Vec<ExtractedFact>> {
            Ok(vec![])
        }
        async fn generate_ccl_description(&self, _n: &str, _c: &str) -> Result<String> {
            Ok(String::new())
        }
        async fn embed_text(&self, _t: &str) -> Result<Vec<f32>> {
            Ok(vec![0.25; DIM])
        }
        async fn compress_context(&self, m: &str) -> Result<String> {
            Ok(format!("summary[{}]", m.len()))
        }
    }

    /// Returns canned text for any URI, or always-missing.
    struct StubArtifacts {
        missing: bool,
    }
    #[async_trait]
    impl ArtifactStore for StubArtifacts {
        async fn fetch_text(&self, uri: &str) -> Result<FetchOutcome> {
            if self.missing {
                Ok(FetchOutcome::Missing)
            } else {
                Ok(FetchOutcome::Found(format!("text of {uri}")))
            }
        }
    }

    fn event(group_id: &str) -> DocumentCompleted {
        DocumentCompleted {
            group_id: Some(group_id.into()),
            data_id: None,
            pages: vec![PageRef {
                page_index: Some(0),
                status: Some("ok".into()),
                text_uri: Some("pt://archive/p0/text".into()),
                tags: vec!["letter".into()],
            }],
        }
    }

    struct Harness {
        svc: IngestionService,
        stm: Arc<SqliteMemoryRepository>,
        ltm: Arc<SqliteLtmRepository>,
        tenant: TenantId,
    }

    fn harness(missing: bool) -> Harness {
        let stm_conn = init_db(None as Option<&String>).unwrap();
        init_schema(&stm_conn, DIM).unwrap();
        let stm = Arc::new(SqliteMemoryRepository::new(stm_conn));

        let ltm_conn = init_db(None as Option<&String>).unwrap();
        init_ltm_schema(&ltm_conn, DIM).unwrap();
        let ltm = Arc::new(SqliteLtmRepository::new(ltm_conn));
        ltm.seed_spine().unwrap(); // provides the inbox fallback

        let svc = IngestionService::new(
            stm.clone() as Arc<dyn MemoryRepository>,
            ltm.clone() as Arc<dyn LtmRepository>,
            Arc::new(StubLlm),
            Arc::new(StubArtifacts { missing }),
            DIM,
            "jarvis",
        );
        Harness {
            svc,
            stm,
            ltm,
            tenant: TenantId("jarvis".into()),
        }
    }

    /// A document lands a leaf in LTM AND a node in STM; a tombstone removes it
    /// from both.
    #[tokio::test]
    async fn test_ingest_dual_writes_then_tombstone_forgets_both() {
        let h = harness(false);

        let outcome = h.svc.ingest(&event("grp_1")).await.unwrap();
        match outcome {
            IngestOutcome::Ingested { data_id, .. } => assert_eq!(data_id, "grp_1"),
            other => panic!("expected Ingested, got {other:?}"),
        }

        // LTM: the leaf exists, found by dataId.
        assert!(h.ltm.get_node_by_data_id("grp_1").unwrap().is_some());
        // STM: a fact carrying the dataId exists.
        let export = h.stm.export_tenant(&h.tenant).unwrap();
        assert!(export.contains("grp_1"), "STM has the doc fact");

        // Tombstone -> gone from both.
        h.svc.forget("grp_1").await.unwrap();
        assert!(h.ltm.get_node_by_data_id("grp_1").unwrap().is_none());
        let export = h.stm.export_tenant(&h.tenant).unwrap();
        assert!(!export.contains("grp_1"), "STM fact removed");
    }

    /// All pages missing from Pithos -> Skipped, nothing written.
    #[tokio::test]
    async fn test_all_pages_missing_is_skipped() {
        let h = harness(true);

        let outcome = h.svc.ingest(&event("grp_2")).await.unwrap();
        assert_eq!(
            outcome,
            IngestOutcome::Skipped {
                data_id: "grp_2".into()
            }
        );
        assert!(h.ltm.get_node_by_data_id("grp_2").unwrap().is_none());
    }

    /// An event with neither groupId nor dataId is a bad event (the feeder
    /// routes it to dlq.memory).
    #[tokio::test]
    async fn test_event_without_id_errors() {
        let h = harness(false);
        let bad = DocumentCompleted {
            group_id: None,
            data_id: None,
            pages: vec![],
        };
        assert!(h.svc.ingest(&bad).await.is_err());
    }
}
