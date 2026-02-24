use crate::domain::models::TenantId;
use crate::domain::ports::MemoryRepository;
use std::sync::Arc;
use anyhow::Result;

/// The three adaptation modes from cognitive psychology:
/// - Assimilate: Fact already exists, boost its support
/// - AccommodateModify: Similar fact exists but with new info, update it
/// - AccommodateCreate: No match found, create new node
pub enum AdaptationResult {
    /// Existing node was reinforced (its support_count was incremented)
    Assimilated(i64),
    /// Existing node was modified with updated payload
    AccommodatedModify(i64),
    /// A brand new node should be created
    AccommodateCreate,
}

pub struct ConflictResolver {
    /// Cosine distance threshold: below this = "same fact"
    pub assimilation_threshold: f64,
    /// Cosine distance threshold: between assimilation and this = "similar, update"
    pub accommodation_threshold: f64,
}

impl ConflictResolver {
    pub fn new() -> Self {
        Self {
            assimilation_threshold: 0.15,  // very close match → same fact
            accommodation_threshold: 0.35, // somewhat close → related, update
        }
    }

    /// Determine how to adapt a new fact given existing knowledge
    pub fn resolve(
        &self,
        memory_repo: &Arc<dyn MemoryRepository>,
        embedding: &[f32],
        tenant_id: &TenantId,
        new_payload: &serde_json::Value,
    ) -> Result<AdaptationResult> {
        let similar = memory_repo.find_similar_nodes(
            embedding,
            tenant_id,
            self.accommodation_threshold,
            3,
        )?;

        if similar.is_empty() {
            return Ok(AdaptationResult::AccommodateCreate);
        }

        let closest = &similar[0];
        // We don't have the actual distance in MemoryNode, so we use position as heuristic:
        // If the closest match is within assimilation threshold, we assimilate.
        // Since find_similar_nodes already filters by accommodation_threshold,
        // the presence of results means they're <= accommodation_threshold.
        // We check if the fact text is highly similar by comparing payloads.
        
        let closest_id = closest.id.unwrap_or(0);
        let existing_fact = closest.payload.get("fact").and_then(|f| f.as_str()).unwrap_or("");
        let new_fact = new_payload.get("fact").and_then(|f| f.as_str()).unwrap_or("");

        // Simple heuristic: if the first result is very close and facts overlap significantly
        // Since sqlite-vec returns ordered by distance, if we have a match, the first is closest
        if existing_fact == new_fact {
            // Exact same fact → Assimilate (boost support)
            memory_repo.update_node_support(closest_id, None)?;
            return Ok(AdaptationResult::Assimilated(closest_id));
        }

        // Similar but different → Accommodate-Modify (update payload, boost support)
        // Merge: keep the new fact text but preserve existing tags
        let mut merged_payload = new_payload.clone();
        if let Some(existing_tags) = closest.payload.get("tags") {
            if let Some(new_tags) = merged_payload.get("tags").cloned() {
                let mut all_tags: Vec<serde_json::Value> = existing_tags.as_array().cloned().unwrap_or_default();
                if let Some(nt) = new_tags.as_array() {
                    for t in nt {
                        if !all_tags.contains(t) {
                            all_tags.push(t.clone());
                        }
                    }
                }
                merged_payload["tags"] = serde_json::Value::Array(all_tags);
            }
        }

        memory_repo.update_node_support(closest_id, Some(&merged_payload))?;
        Ok(AdaptationResult::AccommodatedModify(closest_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_thresholds() {
        let resolver = ConflictResolver::new();
        assert!(resolver.assimilation_threshold < resolver.accommodation_threshold);
    }
}
