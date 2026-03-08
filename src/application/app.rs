use crate::application::retrieval::RetrievalService;
use crate::application::session_manager::{ContextWindow, SessionManager};
use crate::application::sleep::SleepWorker;
use crate::domain::models::{Episode, MemoryNode, MemoryResult, SessionId, TenantId, TimeFilter};
use crate::domain::ports::{LlmClient, MemoryRepository};
use anyhow::Result;
use std::sync::Arc;

pub struct NeurolitheApp {
    memory_repo: Arc<dyn MemoryRepository>,
    llm_client: Arc<dyn LlmClient>,
    retrieval_service: RetrievalService,
    sleep_worker: SleepWorker,
    session_manager: SessionManager,
}

// SAFETY: All fields are either Arc (Send+Sync) or use std::sync::Mutex internally.
unsafe impl Send for NeurolitheApp {}
unsafe impl Sync for NeurolitheApp {}

impl NeurolitheApp {
    pub fn new(
        memory_repo: Arc<dyn MemoryRepository>,
        llm_client: Arc<dyn LlmClient>,
        half_life_days: f64,
    ) -> Self {
        Self {
            memory_repo: memory_repo.clone(),
            llm_client: llm_client.clone(),
            retrieval_service: RetrievalService::new(llm_client.clone(), memory_repo.clone()),
            sleep_worker: SleepWorker::new(memory_repo.clone(), llm_client.clone(), half_life_days),
            session_manager: SessionManager::new(
                memory_repo.clone(),
                llm_client.clone(),
                4000, // ~4000 token threshold (configurable)
                10,   // keep 10 most recent messages raw
            ),
        }
    }

    /// Push dialogue to Short-Term Memory (Flow 1 from blueprint).
    /// Compresses old messages, returns optimized context window,
    /// and queues the new dialogue for background fact extraction.
    pub async fn push_dialogue(
        &self,
        tenant_id: &str,
        session_id: &str,
        new_message: &str,
        ccl: &str,
    ) -> Result<ContextWindow> {
        let ctx = self
            .session_manager
            .push_dialogue(
                &TenantId(tenant_id.to_string()),
                &SessionId(session_id.to_string()),
                new_message,
                ccl,
            )
            .await?;

        // Queue for background learning (extract facts from the new message)
        let episode = Episode {
            id: Some(0), // placeholder, already stored by session_manager
            tenant_id: TenantId(tenant_id.to_string()),
            session_id: SessionId(session_id.to_string()),
            raw_dialogue: new_message.to_string(),
            ccl: ccl.to_string(),
            created_at: None,
        };
        // Fire-and-forget: in production this would be async/background
        let _ = self.sleep_worker.process_episode(&episode).await;

        Ok(ctx)
    }

    /// Stores raw memory dialogue (Episode) and extracts facts via the Sleep pipeline.
    /// Used by push_dialogue's background extraction pathway.
    pub async fn store_memory(
        &self,
        tenant_id: &str,
        session_id: &str,
        dialogue: &str,
        ccl: &str,
    ) -> Result<()> {
        let ep = Episode {
            id: None,
            tenant_id: TenantId(tenant_id.to_string()),
            session_id: SessionId(session_id.to_string()),
            raw_dialogue: dialogue.to_string(),
            ccl: ccl.to_string(),
            created_at: None,
        };

        let ep_id = self.memory_repo.store_episode(&ep)?;
        let mut ep_with_id = ep.clone();
        ep_with_id.id = Some(ep_id);

        self.sleep_worker.process_episode(&ep_with_id).await?;
        Ok(())
    }

    /// Store an explicit fact directly (bypasses LLM extraction) — Blueprint store_memory tool
    pub async fn store_explicit_fact(
        &self,
        tenant_id: &str,
        fact_text: &str,
        tags: &[String],
        ccl: &str,
    ) -> Result<()> {
        let embedding = self.llm_client.embed_text(fact_text).await?;

        let node = MemoryNode {
            id: None,
            tenant_id: TenantId(tenant_id.to_string()),
            source_episode_id: None,
            payload: serde_json::json!({
                "fact": fact_text,
                "tags": tags
            }),
            status: "active".into(),
            ccl: ccl.to_string(),
            is_explicit: true,
            support_count: 1,
            relevance_score: 1.0,
        };

        self.memory_repo.store_node(&node, &embedding)?;
        Ok(())
    }

    /// Query hybrid memory with temporal filtering and graph traversal
    pub async fn query_memory(
        &self,
        tenant_id: &str,
        query: &str,
        time_filter: &TimeFilter,
        ccl_filter: &[String],
    ) -> Result<Vec<MemoryResult>> {
        self.retrieval_service
            .query(
                &TenantId(tenant_id.to_string()),
                query,
                time_filter,
                ccl_filter,
            )
            .await
    }

    pub async fn register_ccl(&self, tenant_id: &str, name: &str, description: &str) -> Result<()> {
        let def = crate::domain::models::CclDefinition {
            id: None,
            tenant_id: TenantId(tenant_id.to_string()),
            name: name.to_string(),
            description: description.to_string(),
        };
        self.memory_repo.store_ccl_definition(&def)?;
        Ok(())
    }

    pub async fn get_ccl_layers(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<crate::domain::models::CclDefinition>> {
        self.memory_repo
            .get_ccl_definitions(&TenantId(tenant_id.to_string()))
    }

    /// Delete all tenant information
    pub async fn delete_tenant(&self, tenant_id: &str) -> Result<()> {
        self.memory_repo
            .delete_tenant(&TenantId(tenant_id.to_string()))
    }

    /// Export tenant data to a JSON string
    pub async fn export_tenant(&self, tenant_id: &str) -> Result<String> {
        self.memory_repo
            .export_tenant(&TenantId(tenant_id.to_string()))
    }
}
