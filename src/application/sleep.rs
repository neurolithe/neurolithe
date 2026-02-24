use crate::domain::ports::{MemoryRepository, LlmClient};
use crate::domain::decay::DecayEngine;
use crate::domain::cognition::conflict_resolver::{ConflictResolver, AdaptationResult};
use std::sync::Arc;
use anyhow::Result;

pub struct SleepWorker {
    memory_repo: Arc<dyn MemoryRepository>,
    llm_client: Arc<dyn LlmClient>,
    decay_engine: DecayEngine,
    conflict_resolver: ConflictResolver,
}

impl SleepWorker {
    pub fn new(memory_repo: Arc<dyn MemoryRepository>, llm_client: Arc<dyn LlmClient>, half_life_days: f64) -> Self {
        Self {
            memory_repo,
            llm_client,
            decay_engine: DecayEngine::new(half_life_days),
            conflict_resolver: ConflictResolver::new(),
        }
    }

    /// Triggers the background decay process across the database
    pub async fn run_decay_sweep(&self) -> Result<()> {
        self.memory_repo.sweep_decay(&self.decay_engine)?;
        Ok(())
    }

    /// Processes un-extracted episodes using the full Sleep pipeline:
    /// 1. Extract facts (with relationships + temporal bounds)
    /// 2. For each fact, run Tri-Modal Conflict Resolution
    /// 3. Create edges for any extracted relationships
    pub async fn process_episode(&self, episode: &crate::domain::models::Episode) -> Result<()> {
        let extracted_facts = self.llm_client.extract_facts(&episode.raw_dialogue).await?;

        for fact in extracted_facts {
            let embedding = self.llm_client.embed_text(&fact.fact).await?;
            let payload = serde_json::json!({
                "fact": fact.fact,
                "tags": fact.tags
            });

            // Tri-Modal Conflict Resolution
            let source_node_id = match self.conflict_resolver.resolve(
                &self.memory_repo,
                &embedding,
                &episode.tenant_id,
                &payload,
            )? {
                AdaptationResult::Assimilated(existing_id) => {
                    // Fact already exists, support was boosted
                    existing_id
                }
                AdaptationResult::AccommodatedModify(existing_id) => {
                    // Similar fact was updated with merged payload
                    existing_id
                }
                AdaptationResult::AccommodateCreate => {
                    // No match — create a new node
                    let node = crate::domain::models::MemoryNode {
                        id: None,
                        tenant_id: episode.tenant_id.clone(),
                        source_episode_id: episode.id,
                        payload: payload.clone(),
                        status: "active".into(),
                        is_explicit: false,
                        support_count: 1,
                        relevance_score: 1.0,
                    };
                    self.memory_repo.store_node(&node, &embedding)?
                }
            };

            // Create edges for any extracted relationships
            for rel in &fact.relationships {
                let target_embedding = self.llm_client.embed_text(&rel.target_entity).await?;
                let target_payload = serde_json::json!({
                    "fact": rel.target_entity,
                    "tags": ["entity"]
                });

                // Also resolve target entities through conflict resolution
                let target_node_id = match self.conflict_resolver.resolve(
                    &self.memory_repo,
                    &target_embedding,
                    &episode.tenant_id,
                    &target_payload,
                )? {
                    AdaptationResult::Assimilated(id) | AdaptationResult::AccommodatedModify(id) => id,
                    AdaptationResult::AccommodateCreate => {
                        let target_node = crate::domain::models::MemoryNode {
                            id: None,
                            tenant_id: episode.tenant_id.clone(),
                            source_episode_id: episode.id,
                            payload: target_payload,
                            status: "active".into(),
                            is_explicit: false,
                            support_count: 1,
                            relevance_score: 1.0,
                        };
                        self.memory_repo.store_node(&target_node, &target_embedding)?
                    }
                };

                let edge = crate::domain::models::Edge {
                    source_id: source_node_id,
                    target_id: target_node_id,
                    relation: rel.relation.clone(),
                    valid_from: rel.valid_from.clone(),
                    valid_until: rel.valid_until.clone(),
                    weight: 1.0,
                };
                self.memory_repo.store_edge(&edge)?;
            }
        }

        Ok(())
    }
}
