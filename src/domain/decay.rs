use crate::domain::models::MemoryNode;

pub struct DecayEngine {
    pub half_life_days: f64,
}

impl DecayEngine {
    pub fn new(half_life_days: f64) -> Self {
        Self { half_life_days }
    }

    /// Calculate the decayed relevance score
    pub fn calculate_decay(&self, current_score: f64, days_elapsed: f64) -> f64 {
        // formula: score = current_score * (0.5 ^ (days_elapsed / half_life))
        current_score * 0.5f64.powf(days_elapsed / self.half_life_days)
    }

    /// Apply decay to a specific node, returning the modified node
    /// If score drops below threshold (e.g. 0.1), status becomes 'archived'
    pub fn apply_to_node(&self, mut node: MemoryNode, days_elapsed: f64) -> MemoryNode {
        let new_score = self.calculate_decay(node.relevance_score, days_elapsed);
        node.relevance_score = new_score;

        if new_score < 0.1 && node.status == "active" {
            node.status = "archived".into();
        }

        node
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::TenantId;
    use serde_json::json;

    #[test]
    fn test_decay_calculation() {
        let engine = DecayEngine::new(7.0); // 7 day half-life

        let score = engine.calculate_decay(1.0, 7.0);
        assert!((score - 0.5).abs() < 0.001);

        let score_14 = engine.calculate_decay(1.0, 14.0);
        assert!((score_14 - 0.25).abs() < 0.001);
    }

    #[test]
    fn test_node_archiving() {
        let engine = DecayEngine::new(7.0);

        let node = MemoryNode {
            id: None,
            tenant_id: TenantId("t1".into()),
            source_episode_id: Some(1),
            payload: json!({}),
            status: "active".into(),
            ccl: "reality".into(),
            is_explicit: false,
            support_count: 1,
            relevance_score: 0.15,
        };

        // 7 days later, 0.15 becomes 0.075, which is < 0.1
        let decayed = engine.apply_to_node(node, 7.0);
        assert_eq!(decayed.status, "archived");
        assert!(decayed.relevance_score < 0.1);
    }
}
