//! Command consumer — drives soft/hard reset from the `memory.command` topic.
//!
//! Parses each command and dispatches to [`ResetService`]. On a hard reset it
//! then rewinds the feeder (via [`FeederRewind`]) so `document.completed`
//! replays and the stores are re-derived. The command channel is short-retention
//! (not a replay source); each command is processed once.
//!
//! Like the feeder loop, the rdkafka plumbing here has a deferred live-broker
//! smoke test — the reset logic itself is unit-tested in `reset_service`.

use crate::application::reset_service::{MemoryCommand, ResetKind, ResetService};
use crate::application::write_service::WriteService;
use anyhow::{Context, Result};
use async_trait::async_trait;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer, StreamConsumer};
use rdkafka::message::Message;
use rdkafka::util::Timeout;
use rdkafka::{Offset, TopicPartitionList};
use std::sync::Arc;
use std::time::Duration;

/// Rewinds the feeder's consumer group to the earliest offset, so a hard reset
/// replays `document.completed` and re-derives LTM (+ STM). Implemented by the
/// daemon, which owns the feeder consumer (slice 11).
#[async_trait]
pub trait FeederRewind: Send + Sync {
    async fn rewind_to_earliest(&self) -> Result<()>;
}

/// Rewinds a shared feeder consumer to the beginning of its assigned partitions.
pub struct ConsumerRewind {
    consumer: Arc<StreamConsumer>,
}

impl ConsumerRewind {
    pub fn new(consumer: Arc<StreamConsumer>) -> Self {
        Self { consumer }
    }
}

#[async_trait]
impl FeederRewind for ConsumerRewind {
    async fn rewind_to_earliest(&self) -> Result<()> {
        let assignment = self
            .consumer
            .assignment()
            .context("reading feeder assignment")?;
        let mut tpl = TopicPartitionList::new();
        for elem in assignment.elements() {
            tpl.add_partition_offset(elem.topic(), elem.partition(), Offset::Beginning)
                .context("building rewind offsets")?;
        }
        self.consumer
            .seek_partitions(tpl, Timeout::After(Duration::from_secs(5)))
            .context("seeking feeder to earliest")?;
        Ok(())
    }
}

/// No-op rewind for when the feeder is disabled (hard reset still wipes stores).
pub struct NoopRewind;

#[async_trait]
impl FeederRewind for NoopRewind {
    async fn rewind_to_earliest(&self) -> Result<()> {
        Ok(())
    }
}

pub struct CommandConsumer {
    consumer: StreamConsumer,
    reset: Arc<ResetService>,
    write: Arc<WriteService>,
    rewind: Arc<dyn FeederRewind>,
    topic: String,
}

impl CommandConsumer {
    pub fn new(
        brokers: &str,
        group_id: &str,
        reset: Arc<ResetService>,
        write: Arc<WriteService>,
        rewind: Arc<dyn FeederRewind>,
    ) -> Result<Self> {
        let consumer: StreamConsumer = ClientConfig::new()
            .set("bootstrap.servers", brokers)
            .set("group.id", group_id)
            .set("enable.auto.commit", "false")
            .set("auto.offset.reset", "earliest")
            .create()
            .context("creating memory.command consumer")?;

        Ok(Self {
            consumer,
            reset,
            write,
            rewind,
            topic: "memory.command".into(),
        })
    }

    /// Consume forever: parse -> apply -> (rewind on hard) -> commit.
    pub async fn run(&self) -> Result<()> {
        self.consumer
            .subscribe(&[&self.topic])
            .context("subscribing to memory.command")?;

        loop {
            match self.consumer.recv().await {
                Err(e) => eprintln!("[neurolithe] command consumer error: {e}"),
                Ok(msg) => {
                    self.process(&msg).await;
                    if let Err(e) = self.consumer.commit_message(&msg, CommitMode::Async) {
                        eprintln!("[neurolithe] command commit failed: {e}");
                    }
                }
            }
        }
    }

    async fn process(&self, msg: &rdkafka::message::BorrowedMessage<'_>) {
        let Some(payload) = msg.payload() else {
            return; // null command — nothing to do
        };
        let command = match MemoryCommand::parse(payload) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[neurolithe] invalid memory.command: {e}");
                return;
            }
        };

        // Route by variant: resets go to ResetService (rewinding the feeder on a
        // hard reset), writes go to the idempotent WriteService.
        match &command {
            MemoryCommand::ResetSoft | MemoryCommand::ResetHard { .. } => {
                match self.reset.apply(&command) {
                    // Hard reset wiped both stores — relearn from the bus.
                    Ok(ResetKind::Hard) => {
                        if let Err(e) = self.rewind.rewind_to_earliest().await {
                            eprintln!("[neurolithe] feeder rewind after hard reset failed: {e}");
                        }
                    }
                    Ok(ResetKind::Soft) => {}
                    // e.g. confirmation token mismatch — refuse and keep running.
                    Err(e) => eprintln!("[neurolithe] reset refused: {e}"),
                }
            }
            MemoryCommand::Remember(_) | MemoryCommand::Forget(_) => {
                // A write failure is logged, not retried in-loop; redelivery
                // re-applies (the commandId isn't marked until success).
                if let Err(e) = self.write.handle(&command).await {
                    eprintln!("[neurolithe] memory write failed: {e}");
                }
            }
        }
    }
}
