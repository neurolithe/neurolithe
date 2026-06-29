//! Metrics publisher — sends the CT-scan snapshot to `memory.metrics`.
//!
//! The topic is compacted and the snapshot is published under a single fixed
//! key, so the topic always holds exactly one record (the latest brain state),
//! never a growing log. Pharos consumes it for the dashboard.
//!
//! `render` (key + JSON bytes) is pure and unit-tested; the rdkafka send has a
//! deferred live-broker smoke test.

use crate::application::monitoring::MemoryMetrics;
use anyhow::{Context, Result};
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;

/// The single compaction key — one record on the topic, always the latest.
pub const METRICS_KEY: &str = "neurolithe";
const METRICS_TOPIC: &str = "memory.metrics";

/// The keyed message body for a snapshot: a fixed key + JSON bytes.
pub fn render(metrics: &MemoryMetrics) -> Result<(&'static str, Vec<u8>)> {
    let bytes = serde_json::to_vec(metrics).context("serializing memory.metrics snapshot")?;
    Ok((METRICS_KEY, bytes))
}

pub struct MetricsPublisher {
    producer: FutureProducer,
    topic: String,
}

impl MetricsPublisher {
    pub fn new(brokers: &str) -> Result<Self> {
        let producer: FutureProducer = ClientConfig::new()
            .set("bootstrap.servers", brokers)
            .create()
            .context("creating memory.metrics producer")?;
        Ok(Self {
            producer,
            topic: METRICS_TOPIC.into(),
        })
    }

    /// Publish one snapshot under the fixed compaction key.
    pub async fn publish(&self, metrics: &MemoryMetrics) -> Result<()> {
        let (key, payload) = render(metrics)?;
        let record = FutureRecord::to(&self.topic).key(key).payload(&payload);
        self.producer
            .send(record, Timeout::Never)
            .await
            .map_err(|(e, _)| e)
            .context("publishing memory.metrics")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::monitoring::MemoryMetrics;

    fn metrics() -> MemoryMetrics {
        MemoryMetrics {
            stm_active_nodes: 3,
            stm_archived_nodes: 1,
            stm_avg_relevance: 0.6,
            stm_decay_histogram: vec![0, 1, 0, 0, 2],
            stm_db_bytes: 4096,
            sessions: 1,
            ltm_tree_nodes: 7,
            ltm_leaves: 1,
            ltm_edges: 6,
            ltm_inbox_docs: 1,
            ltm_orphan_leaves: 0,
            ltm_max_depth: 2,
            ltm_db_bytes: 8192,
            feeder_lag: 0,
            feeder_errors: 0,
            last_backfill_unix: Some(1000),
        }
    }

    /// render uses the single fixed key (so compaction keeps one record) and
    /// round-trips the snapshot.
    #[test]
    fn test_render_is_one_fixed_key_message() {
        let (key, payload) = render(&metrics()).unwrap();
        assert_eq!(key, METRICS_KEY);
        // Same key every time -> no churn on the compacted topic.
        let (key2, _) = render(&metrics()).unwrap();
        assert_eq!(key, key2);

        let decoded: MemoryMetrics = serde_json::from_slice(&payload).unwrap();
        assert_eq!(decoded, metrics());
    }
}
