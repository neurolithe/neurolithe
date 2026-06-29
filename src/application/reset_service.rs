//! Reset — the disposable-brain operations.
//!
//! Soft reset wipes STM only (junky working memory → clean slate). Hard reset
//! wipes BOTH stores and re-seeds the curated spine; the caller then rewinds
//! the feeder so `document.completed` replays and LTM (+ STM) is re-derived.
//! Nothing is lost — every leaf is a `dataId` pointer back to Ledger/Pithos.
//! Both are destructive; hard reset is guarded by a confirmation token.

use crate::domain::ltm::LtmRepository;
use crate::domain::ports::MemoryRepository;
use anyhow::{Result, bail};
use serde::Deserialize;
use std::sync::Arc;

/// A command on the `memory.command` topic. Internally tagged on `command`:
/// `{"command":"reset_soft"}` / `{"command":"reset_hard","confirm":"<token>"}`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum MemoryCommand {
    ResetSoft,
    ResetHard { confirm: String },
}

impl MemoryCommand {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

/// Performs soft/hard resets across the two stores. Store wipes only — the
/// feeder offset rewind for hard reset is the consumer's job (slice 11).
pub struct ResetService {
    stm: Arc<dyn MemoryRepository>,
    ltm: Arc<dyn LtmRepository>,
    confirm_token: String,
}

impl ResetService {
    pub fn new(
        stm: Arc<dyn MemoryRepository>,
        ltm: Arc<dyn LtmRepository>,
        confirm_token: impl Into<String>,
    ) -> Self {
        Self {
            stm,
            ltm,
            confirm_token: confirm_token.into(),
        }
    }

    /// Wipe STM only; LTM untouched.
    pub fn soft_reset(&self) -> Result<()> {
        self.stm.reset_store()
    }

    /// Wipe BOTH stores and re-seed the curated spine. Requires the confirmation
    /// token. The caller rewinds the feeder afterward to relearn from the bus.
    pub fn hard_reset(&self, confirm: &str) -> Result<()> {
        if confirm != self.confirm_token {
            bail!("hard reset refused: confirmation token mismatch");
        }
        self.stm.reset_store()?;
        self.ltm.reset_store()?;
        // Restore the curated backbone; the feeder regrows the leaves.
        self.ltm.seed_spine()?;
        Ok(())
    }

    /// Dispatch a parsed command. Returns whether a hard reset occurred (so the
    /// caller knows to rewind the feeder).
    pub fn apply(&self, command: &MemoryCommand) -> Result<ResetKind> {
        match command {
            MemoryCommand::ResetSoft => {
                self.soft_reset()?;
                Ok(ResetKind::Soft)
            }
            MemoryCommand::ResetHard { confirm } => {
                self.hard_reset(confirm)?;
                Ok(ResetKind::Hard)
            }
        }
    }
}

/// Which reset ran — Hard signals the caller to rewind the feeder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetKind {
    Soft,
    Hard,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ingestion::{DocumentCompleted, IngestionService, PageRef};
    use crate::domain::models::{CclDefinition, TenantId};
    use crate::domain::ports::{ArtifactStore, ExtractedFact, FetchOutcome, LlmClient};
    use crate::infrastructure::database::init_db;
    use crate::infrastructure::ltm_repository::SqliteLtmRepository;
    use crate::infrastructure::repository::SqliteMemoryRepository;
    use crate::infrastructure::schema::{init_ltm_schema, init_schema};
    use async_trait::async_trait;

    const DIM: usize = 8;
    const TOKEN: &str = "CONFIRM-WIPE";

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

    struct StubArtifacts;
    #[async_trait]
    impl ArtifactStore for StubArtifacts {
        async fn fetch_text(&self, uri: &str) -> Result<FetchOutcome> {
            Ok(FetchOutcome::Found(format!("text of {uri}")))
        }
    }

    fn event(id: &str) -> DocumentCompleted {
        DocumentCompleted {
            group_id: Some(id.into()),
            data_id: None,
            pages: vec![PageRef {
                page_index: Some(0),
                status: Some("ok".into()),
                text_uri: Some("pt://archive/p0/text".into()),
                tags: vec![],
            }],
        }
    }

    struct Harness {
        reset: ResetService,
        ingest: IngestionService,
        stm: Arc<SqliteMemoryRepository>,
        ltm: Arc<SqliteLtmRepository>,
        tenant: TenantId,
    }

    fn harness() -> Harness {
        let stm_conn = init_db(None as Option<&String>).unwrap();
        init_schema(&stm_conn, DIM).unwrap();
        let stm = Arc::new(SqliteMemoryRepository::new(stm_conn));

        let ltm_conn = init_db(None as Option<&String>).unwrap();
        init_ltm_schema(&ltm_conn, DIM).unwrap();
        let ltm = Arc::new(SqliteLtmRepository::new(ltm_conn));
        ltm.seed_spine().unwrap();

        let ingest = IngestionService::new(
            stm.clone() as Arc<dyn MemoryRepository>,
            ltm.clone() as Arc<dyn LtmRepository>,
            Arc::new(StubLlm),
            Arc::new(StubArtifacts),
            DIM,
            "jarvis",
        );
        let reset = ResetService::new(
            stm.clone() as Arc<dyn MemoryRepository>,
            ltm.clone() as Arc<dyn LtmRepository>,
            TOKEN,
        );
        Harness {
            reset,
            ingest,
            stm,
            ltm,
            tenant: TenantId("jarvis".into()),
        }
    }

    fn stm_has(h: &Harness, id: &str) -> bool {
        h.stm.export_tenant(&h.tenant).unwrap().contains(id)
    }

    #[test]
    fn test_command_parsing() {
        assert_eq!(
            MemoryCommand::parse(br#"{"command":"reset_soft"}"#).unwrap(),
            MemoryCommand::ResetSoft
        );
        assert_eq!(
            MemoryCommand::parse(br#"{"command":"reset_hard","confirm":"x"}"#).unwrap(),
            MemoryCommand::ResetHard {
                confirm: "x".into()
            }
        );
        assert!(MemoryCommand::parse(b"not json").is_err());
    }

    /// Soft reset empties STM only; the LTM leaf survives.
    #[tokio::test]
    async fn test_soft_reset_empties_stm_only() {
        let h = harness();
        h.ingest.ingest(&event("grp_1")).await.unwrap();
        assert!(stm_has(&h, "grp_1"));
        assert!(h.ltm.get_node_by_data_id("grp_1").unwrap().is_some());

        h.reset.apply(&MemoryCommand::ResetSoft).unwrap();

        assert!(!stm_has(&h, "grp_1"), "STM wiped");
        assert!(
            h.ltm.get_node_by_data_id("grp_1").unwrap().is_some(),
            "LTM survives soft reset"
        );
    }

    /// Hard reset empties both stores (spine restored), then a replayed
    /// document re-populates LTM + STM.
    #[tokio::test]
    async fn test_hard_reset_empties_both_then_replay_repopulates() {
        let h = harness();
        h.ingest.ingest(&event("grp_1")).await.unwrap();

        let kind = h
            .reset
            .apply(&MemoryCommand::ResetHard {
                confirm: TOKEN.into(),
            })
            .unwrap();
        assert_eq!(kind, ResetKind::Hard);

        // Both emptied of the document; the curated spine is back.
        assert!(!stm_has(&h, "grp_1"));
        assert!(h.ltm.get_node_by_data_id("grp_1").unwrap().is_none());
        assert!(!h.ltm.get_roots().unwrap().is_empty(), "spine re-seeded");

        // Replay (feeder backfill) re-derives both.
        h.ingest.ingest(&event("grp_1")).await.unwrap();
        assert!(stm_has(&h, "grp_1"));
        assert!(h.ltm.get_node_by_data_id("grp_1").unwrap().is_some());
    }

    /// Hard reset with the wrong token is refused and changes nothing.
    #[tokio::test]
    async fn test_hard_reset_wrong_token_refused() {
        let h = harness();
        h.ingest.ingest(&event("grp_1")).await.unwrap();

        let result = h.reset.hard_reset("WRONG");
        assert!(result.is_err(), "wrong token refused");

        // Nothing was wiped.
        assert!(stm_has(&h, "grp_1"));
        assert!(h.ltm.get_node_by_data_id("grp_1").unwrap().is_some());
    }
}
