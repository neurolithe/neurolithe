//! Kafka feeder — consumes `document.completed` and drives ingestion.
//!
//! Backfills from the earliest offset (the topic is compacted, so this replays
//! the latest state per document). Routing follows ADR-0004: a tombstone (null
//! payload) forgets the document; a valid event is ingested; a structurally
//! bad event goes to `dlq.memory`; un-parseable bytes go to `parking.lot`. The
//! offset is committed only AFTER the write, so a crash re-processes rather than
//! drops.
//!
//! The decision logic ([`decide`]) is pure and unit-tested; the rdkafka loop
//! itself is exercised by a live-broker smoke test (deferred, like Chronos).

use crate::application::ingestion::{DocumentCompleted, IngestionService};
use crate::application::monitoring::FeederStats;
use anyhow::{Context, Result};
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer, StreamConsumer};
use rdkafka::message::{Header, Message, OwnedHeaders};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use std::sync::Arc;
use std::time::Duration;

/// How to handle one consumed message. Derived purely from its key + payload.
#[derive(Debug, PartialEq)]
pub enum FeedDecision {
    /// Tombstone (null payload): forget this document id (the message key).
    Forget(String),
    /// A valid event to ingest.
    Ingest(DocumentCompleted),
    /// Parsed but structurally invalid (no id) — route to `dlq.memory`.
    BadEvent(String),
    /// Un-parseable bytes — route to `parking.lot`.
    Park(String),
}

/// Current Unix time in seconds (for the last-ingest stat).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Classify a message by its key + payload (ADR-0004 routing). Pure.
pub fn decide(key: Option<&str>, payload: Option<&[u8]>) -> FeedDecision {
    match payload {
        // Null payload = compaction tombstone -> forget by key.
        None => match key {
            Some(k) if !k.is_empty() => FeedDecision::Forget(k.to_string()),
            _ => FeedDecision::Park("tombstone without a key".into()),
        },
        Some(bytes) => match serde_json::from_slice::<DocumentCompleted>(bytes) {
            Ok(event) if event.document_id().is_some() => FeedDecision::Ingest(event),
            Ok(_) => FeedDecision::BadEvent("event has neither groupId nor dataId".into()),
            Err(e) => FeedDecision::Park(format!("un-parseable document.completed: {e}")),
        },
    }
}

/// Number of ingest attempts before giving up to `dlq.memory` (ADR-0004
/// transient retry).
const INGEST_ATTEMPTS: u32 = 3;
const RETRY_BACKOFF: Duration = Duration::from_millis(500);

pub struct KafkaFeeder {
    consumer: Arc<StreamConsumer>,
    producer: FutureProducer,
    ingestion: Arc<IngestionService>,
    stats: Arc<FeederStats>,
    source_topic: String,
    dlq_topic: String,
    parking_topic: String,
    group_id: String,
}

impl KafkaFeeder {
    pub fn new(
        brokers: &str,
        group_id: &str,
        ingestion: Arc<IngestionService>,
        stats: Arc<FeederStats>,
    ) -> Result<Self> {
        let consumer: StreamConsumer = ClientConfig::new()
            .set("bootstrap.servers", brokers)
            .set("group.id", group_id)
            // Backfill from the start; we manage offsets ourselves.
            .set("enable.auto.commit", "false")
            .set("auto.offset.reset", "earliest")
            .create()
            .context("creating document.completed consumer")?;

        let producer: FutureProducer = ClientConfig::new()
            .set("bootstrap.servers", brokers)
            .create()
            .context("creating dlq/parking producer")?;

        Ok(Self {
            consumer: Arc::new(consumer),
            producer,
            ingestion,
            stats,
            source_topic: "document.completed".into(),
            dlq_topic: "dlq.memory".into(),
            parking_topic: "parking.lot".into(),
            group_id: group_id.into(),
        })
    }

    /// Shared handle to the consumer, so the command consumer can rewind it to
    /// earliest after a hard reset.
    pub fn consumer(&self) -> Arc<StreamConsumer> {
        self.consumer.clone()
    }

    /// Consume forever: classify -> act -> commit. Never returns under normal
    /// operation; the daemon (slice 11) owns its lifecycle.
    pub async fn run(&self) -> Result<()> {
        self.consumer
            .subscribe(&[&self.source_topic])
            .context("subscribing to document.completed")?;

        loop {
            match self.consumer.recv().await {
                Err(e) => eprintln!("[neurolithe] consumer error: {e}"),
                Ok(msg) => {
                    self.process(&msg).await;
                    // Commit AFTER the write so a crash re-processes, not drops.
                    if let Err(e) = self.consumer.commit_message(&msg, CommitMode::Async) {
                        eprintln!("[neurolithe] commit failed: {e}");
                    }
                }
            }
        }
    }

    async fn process(&self, msg: &rdkafka::message::BorrowedMessage<'_>) {
        let key = msg.key().and_then(|k| std::str::from_utf8(k).ok());
        match decide(key, msg.payload()) {
            FeedDecision::Forget(id) => {
                if let Err(e) = self.ingestion.forget(&id).await {
                    self.send_aside(msg, &self.dlq_topic, &format!("forget failed: {e}"))
                        .await;
                }
            }
            FeedDecision::Ingest(event) => match self.ingest_with_retry(&event).await {
                Ok(()) => self.stats.record_document(now_unix()),
                Err(e) => {
                    self.stats.record_error();
                    self.send_aside(msg, &self.dlq_topic, &format!("ingest failed: {e}"))
                        .await;
                }
            },
            FeedDecision::BadEvent(reason) => {
                self.send_aside(msg, &self.dlq_topic, &reason).await;
            }
            FeedDecision::Park(reason) => {
                self.send_aside(msg, &self.parking_topic, &reason).await;
            }
        }
    }

    /// Retry transient ingest failures a bounded number of times before the
    /// caller dead-letters (ADR-0004).
    async fn ingest_with_retry(&self, event: &DocumentCompleted) -> Result<()> {
        let mut last_err = None;
        for attempt in 1..=INGEST_ATTEMPTS {
            match self.ingestion.ingest(event).await {
                Ok(_) => return Ok(()),
                Err(e) => {
                    eprintln!("[neurolithe] ingest attempt {attempt} failed: {e}");
                    last_err = Some(e);
                    if attempt < INGEST_ATTEMPTS {
                        tokio::time::sleep(RETRY_BACKOFF).await;
                    }
                }
            }
        }
        Err(last_err.expect("loop ran at least once"))
    }

    /// Forward a message to a dead-letter / parking topic with context headers
    /// (ADR-0004 E3b). Failures here are only logged — we still commit and move
    /// on, since the alternative is wedging the whole feeder.
    async fn send_aside(
        &self,
        msg: &rdkafka::message::BorrowedMessage<'_>,
        topic: &str,
        reason: &str,
    ) {
        let partition = msg.partition().to_string();
        let offset = msg.offset().to_string();
        let headers = OwnedHeaders::new()
            .insert(Header {
                key: "x-reason",
                value: Some(reason),
            })
            .insert(Header {
                key: "x-consumer",
                value: Some(self.group_id.as_str()),
            })
            .insert(Header {
                key: "x-source-topic",
                value: Some(self.source_topic.as_str()),
            })
            .insert(Header {
                key: "x-source-partition",
                value: Some(partition.as_str()),
            })
            .insert(Header {
                key: "x-source-offset",
                value: Some(offset.as_str()),
            });

        let key = msg.key().unwrap_or_default();
        let payload = msg.payload().unwrap_or_default();
        let record = FutureRecord::to(topic)
            .key(key)
            .payload(payload)
            .headers(headers);

        if let Err((e, _)) = self.producer.send(record, Timeout::Never).await {
            eprintln!("[neurolithe] failed to forward to {topic}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tombstone_with_key_is_forget() {
        assert_eq!(
            decide(Some("grp_1"), None),
            FeedDecision::Forget("grp_1".into())
        );
    }

    #[test]
    fn test_tombstone_without_key_is_parked() {
        assert!(matches!(decide(None, None), FeedDecision::Park(_)));
        assert!(matches!(decide(Some(""), None), FeedDecision::Park(_)));
    }

    #[test]
    fn test_valid_event_is_ingest() {
        let json = br#"{"groupId":"grp_1","pageCount":1,"pages":[{"pageIndex":0,"textUri":"pt://archive/p/text","tags":["x"]}]}"#;
        match decide(Some("grp_1"), Some(json)) {
            FeedDecision::Ingest(e) => assert_eq!(e.document_id(), Some("grp_1")),
            other => panic!("expected Ingest, got {other:?}"),
        }
    }

    #[test]
    fn test_event_without_id_is_bad_event() {
        // Valid JSON, but no groupId/dataId -> dlq.memory.
        let json = br#"{"pageCount":1,"pages":[]}"#;
        assert!(matches!(
            decide(Some("k"), Some(json)),
            FeedDecision::BadEvent(_)
        ));
    }

    #[test]
    fn test_unparseable_is_parked() {
        // Not JSON at all -> parking.lot.
        assert!(matches!(
            decide(Some("k"), Some(b"\xff\x00 not json")),
            FeedDecision::Park(_)
        ));
    }
}
