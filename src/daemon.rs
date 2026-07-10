//! Daemon — the composition root that assembles and runs NeuroLithe V2.
//!
//! Everything holds a `rusqlite::Connection` (`!Sync`) and every loop is
//! `?Send`, so the daemon runs them all on a single thread via a `LocalSet`:
//! cooperative, no parallel access to the SQLite connections. The loops —
//! MCP (stdio), the Kafka feeder, the `memory.command` consumer (reset +
//! remember/forget writes), the `memory.query` → `memory.result` query door,
//! and the decay + metrics schedulers — run concurrently until Ctrl-C.

use crate::application::app::NeurolitheApp;
use crate::application::ingestion::IngestionService;
use crate::application::introspection::IntrospectionService;
use crate::application::ltm_retrieval::LtmRetrieval;
use crate::application::monitoring::{FeederStats, MonitoringService};
use crate::application::query_service::QueryService;
use crate::application::reset_service::ResetService;
use crate::application::retrieval::RetrievalService;
use crate::application::scheduler::{PeriodicTask, SweepTask, run_periodic};
use crate::application::write_service::WriteService;
use crate::domain::ltm::LtmRepository;
use crate::domain::ports::{ArtifactStore, LlmClient, MemoryRepository};
use crate::infrastructure::config::{AppConfig, LlmProvider};
use crate::infrastructure::database::{MemoryStores, init_stores};
use crate::infrastructure::llm::create_llm_client;
use crate::infrastructure::ltm_repository::SqliteLtmRepository;
use crate::infrastructure::metrics_publisher::MetricsPublisher;
use crate::infrastructure::pithos_client::PithosClient;
use crate::infrastructure::repository::SqliteMemoryRepository;
use crate::interfaces::command_consumer::{
    CommandConsumer, ConsumerRewind, FeederRewind, NoopRewind, NoopSpineEmbedder, SpineEmbedder,
};
use crate::interfaces::kafka_feeder::KafkaFeeder;
use crate::interfaces::mcp_server::McpServer;
use crate::interfaces::query_consumer::QueryConsumer;
use anyhow::{Result, bail};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

/// Tenant under which feeder-ingested documents are stored in STM — the one
/// shared default so the query doors read what the feeder wrote.
use crate::domain::models::DEFAULT_TENANT as FEEDER_TENANT;

/// Periodic metrics snapshot -> `memory.metrics`. A composition-root task tying
/// the monitoring snapshot to the publisher + live feeder stats (so it can
/// depend on both the application and infrastructure layers).
struct MetricsTask {
    monitoring: MonitoringService,
    publisher: MetricsPublisher,
    stats: Arc<FeederStats>,
}

#[async_trait(?Send)]
impl PeriodicTask for MetricsTask {
    async fn run_once(&self) {
        // Session count is wired when the SessionManager exposes it; 0 for now.
        let runtime = self.stats.to_runtime(0);
        match self.monitoring.snapshot(&runtime) {
            Ok(metrics) => {
                if let Err(e) = self.publisher.publish(&metrics).await {
                    eprintln!("[neurolithe] metrics publish failed: {e}");
                }
            }
            Err(e) => eprintln!("[neurolithe] metrics snapshot failed: {e}"),
        }
    }
}

/// Re-embeds the curated spine after a hard reset (composition-root wiring of
/// the application `embed_spine_concepts` step). Mirrors `MetricsTask`: it bridges
/// the interface-layer `SpineEmbedder` port to the application + infra it needs.
struct DaemonSpineEmbedder {
    ltm: Arc<dyn LtmRepository>,
    llm: Arc<dyn LlmClient>,
    dim: usize,
}

#[async_trait(?Send)]
impl SpineEmbedder for DaemonSpineEmbedder {
    async fn embed_spine(&self) -> Result<()> {
        let n =
            crate::application::ltm_placement::embed_spine_concepts(&self.ltm, &self.llm, self.dim)
                .await?;
        if n > 0 {
            eprintln!("[neurolithe] re-embedded {n} spine concept(s) after hard reset");
        }
        Ok(())
    }
}

fn resolve_api_key(provider: &LlmProvider) -> String {
    let primary = match provider {
        LlmProvider::Openai | LlmProvider::Custom => "OPENAI_API_KEY",
        LlmProvider::Gemini => "GEMINI_API_KEY",
        LlmProvider::Anthropic => "ANTHROPIC_API_KEY",
        // Vertex authenticates via gcp_auth (GOOGLE_APPLICATION_CREDENTIALS), not
        // an API key; the returned value is unused by VertexClient.
        LlmProvider::Vertex => return "vertex_uses_gcp_auth".to_string(),
    };
    std::env::var(primary)
        .or_else(|_| std::env::var("NEUROLITHE_API_KEY"))
        .unwrap_or_else(|_| "dummy_key".to_string())
}

/// Assemble and run the daemon. Returns when Ctrl-C is received.
pub async fn run(config: AppConfig) -> Result<()> {
    // The feeder writes one distilled embedding to BOTH stores, so they must
    // share the embedding dimension.
    if config.feeder.enabled && config.stm.vector_dimension != config.ltm.vector_dimension {
        bail!(
            "feeder enabled but stm.vector_dimension ({}) != ltm.vector_dimension ({}); \
             set them equal or disable the feeder",
            config.stm.vector_dimension,
            config.ltm.vector_dimension
        );
    }

    // --- stores ---
    let MemoryStores { stm, ltm } = init_stores(&config)?;
    let stm_repo: Arc<dyn MemoryRepository> = Arc::new(SqliteMemoryRepository::new(stm));
    let ltm_repo: Arc<dyn LtmRepository> = Arc::new(SqliteLtmRepository::new(ltm));
    ltm_repo.seed_spine()?;

    // --- external adapters ---
    // Chat and embeddings may use different providers (Claude has no embeddings
    // endpoint), so resolve a key for each.
    let llm: Arc<dyn LlmClient> = create_llm_client(
        &config.llm,
        resolve_api_key(&config.llm.provider),
        resolve_api_key(config.llm.effective_embedding_provider()),
    );
    let pithos: Arc<dyn ArtifactStore> = Arc::new(PithosClient::new(
        &config.pithos.base_url,
        &config.pithos.token,
    ));

    // Give the freshly-seeded spine (and any new branches) placement vectors, so
    // documents can be filed under a concept instead of all landing in the inbox
    // (field-report §3). Best-effort: a transient embedder outage must not block
    // startup — un-embedded concepts are simply retried on the next boot, and the
    // feeder inboxes documents until then.
    if config.feeder.enabled {
        match crate::application::ltm_placement::embed_spine_concepts(
            &ltm_repo,
            &llm,
            config.ltm.vector_dimension,
        )
        .await
        {
            Ok(0) => {}
            Ok(n) => eprintln!("[neurolithe] embedded {n} spine concept(s) for placement"),
            Err(e) => eprintln!("[neurolithe] spine embedding skipped (will retry next boot): {e}"),
        }
    }

    // --- services ---
    let app = Arc::new(NeurolitheApp::new(
        stm_repo.clone(),
        llm.clone(),
        config.decay.default_half_life_days,
        config.decay.working_half_life_days(),
    ));
    let introspection = Arc::new(IntrospectionService::new(
        stm_repo.clone(),
        ltm_repo.clone(),
    ));
    let ingestion = Arc::new(IngestionService::new(
        stm_repo.clone(),
        ltm_repo.clone(),
        llm.clone(),
        pithos.clone(),
        config.ltm.vector_dimension,
        FEEDER_TENANT,
    ));

    // Inbox gardener: re-home any inbox documents that now match a concept (e.g.
    // after a placement-threshold change or a new spine branch), using stored
    // embeddings — no replay, no LLM. Runs once at startup; idempotent.
    if config.feeder.enabled {
        match ingestion.garden_inbox() {
            Ok(0) => {}
            Ok(n) => {
                eprintln!("[neurolithe] inbox gardener re-homed {n} document(s) under concepts")
            }
            Err(e) => eprintln!("[neurolithe] inbox gardener failed: {e}"),
        }
    }

    let reset_token =
        std::env::var("NEUROLITHE_RESET_TOKEN").unwrap_or_else(|_| "RESET-DISABLED".to_string());
    let reset = Arc::new(ResetService::new(
        stm_repo.clone(),
        ltm_repo.clone(),
        reset_token,
    ));
    let write = Arc::new(WriteService::new(
        stm_repo.clone(),
        ltm_repo.clone(),
        ingestion.clone(),
        llm.clone(),
        config.stm.vector_dimension,
    ));
    let feeder_stats = Arc::new(FeederStats::default());

    // --- feeder (optional) + rewind handle for hard reset ---
    let feeder = if config.feeder.enabled {
        Some(Arc::new(KafkaFeeder::new(
            &config.kafka.brokers,
            &config.kafka.group_id,
            ingestion.clone(),
            feeder_stats.clone(),
        )?))
    } else {
        None
    };
    let rewind: Arc<dyn FeederRewind> = match &feeder {
        Some(f) => Arc::new(ConsumerRewind::new(f.consumer())),
        None => Arc::new(NoopRewind),
    };

    // After a hard reset the spine is re-seeded but un-embedded — re-embed it so
    // the replay files documents under concepts, not the inbox. No-op when the
    // feeder is off (nothing replays to place).
    let spine_embedder: Arc<dyn SpineEmbedder> = if config.feeder.enabled {
        Arc::new(DaemonSpineEmbedder {
            ltm: ltm_repo.clone(),
            llm: llm.clone(),
            dim: config.ltm.vector_dimension,
        })
    } else {
        Arc::new(NoopSpineEmbedder)
    };

    // --- command consumer (reset), distinct group id ---
    let command = Arc::new(CommandConsumer::new(
        &config.kafka.brokers,
        &format!("{}-cmd", config.kafka.group_id),
        reset,
        write,
        rewind,
        spine_embedder,
    )?);

    // --- query consumer (memory.query -> memory.result), distinct group id ---
    let query = if config.bus_query.enabled {
        let query_service = Arc::new(QueryService::new(
            RetrievalService::new(llm.clone(), stm_repo.clone()),
            LtmRetrieval::new(ltm_repo.clone()),
            llm.clone(),
        ));
        Some(Arc::new(QueryConsumer::new(
            &config.kafka.brokers,
            &format!("{}-query", config.kafka.group_id),
            query_service,
        )?))
    } else {
        None
    };

    // --- schedulers ---
    let sweep_task: Arc<dyn PeriodicTask> = Arc::new(SweepTask::new(app.clone()));
    let metrics_task: Arc<dyn PeriodicTask> = Arc::new(MetricsTask {
        monitoring: MonitoringService::new(stm_repo.clone(), ltm_repo.clone()),
        publisher: MetricsPublisher::new(&config.kafka.brokers)?,
        stats: feeder_stats.clone(),
    });
    let sweep_interval = Duration::from_secs(config.sweep.interval_secs);
    let metrics_interval = Duration::from_secs(config.metrics.interval_secs);

    // --- MCP server (stdio) ---
    // Its own QueryService instance (the bus door's was moved into QueryConsumer);
    // both share the same application recall logic, so the doors can't diverge.
    let mcp_query = QueryService::new(
        RetrievalService::new(llm.clone(), stm_repo.clone()),
        LtmRetrieval::new(ltm_repo.clone()),
        llm.clone(),
    );
    let server = Arc::new(McpServer::new(app, introspection, mcp_query));

    // --- spawn all loops on the LocalSet ---
    {
        let s = server.clone();
        tokio::task::spawn_local(async move {
            if let Err(e) = s.run_stdio().await {
                eprintln!("[neurolithe] MCP stdio server stopped: {e}");
            }
        });
    }
    if let Some(f) = feeder.clone() {
        tokio::task::spawn_local(async move {
            if let Err(e) = f.run().await {
                eprintln!("[neurolithe] feeder stopped: {e}");
            }
        });
    }
    {
        let c = command.clone();
        tokio::task::spawn_local(async move {
            if let Err(e) = c.run().await {
                eprintln!("[neurolithe] command consumer stopped: {e}");
            }
        });
    }
    if let Some(q) = query.clone() {
        tokio::task::spawn_local(async move {
            if let Err(e) = q.run().await {
                eprintln!("[neurolithe] query consumer stopped: {e}");
            }
        });
    }
    tokio::task::spawn_local(run_periodic(sweep_task, sweep_interval));
    tokio::task::spawn_local(run_periodic(metrics_task, metrics_interval));

    eprintln!(
        "[neurolithe] daemon running (feeder: {}, query door: {})",
        config.feeder.enabled, config.bus_query.enabled
    );
    shutdown_signal().await;
    eprintln!("[neurolithe] shutting down");
    Ok(())
}

/// Run ONLY the MCP server over stdio — no feeder, consumers, or schedulers.
///
/// For a transient client session (e.g. Claude via `docker exec -i`) that shares
/// the live stores with the running daemon: reads (recall + CT-scan) run
/// concurrently under WAL; the occasional write waits on `busy_timeout`. Serves
/// until stdin closes.
pub async fn run_mcp(config: AppConfig) -> Result<()> {
    let MemoryStores { stm, ltm } = init_stores(&config)?;
    let stm_repo: Arc<dyn MemoryRepository> = Arc::new(SqliteMemoryRepository::new(stm));
    let ltm_repo: Arc<dyn LtmRepository> = Arc::new(SqliteLtmRepository::new(ltm));
    ltm_repo.seed_spine()?;

    let llm: Arc<dyn LlmClient> = create_llm_client(
        &config.llm,
        resolve_api_key(&config.llm.provider),
        resolve_api_key(config.llm.effective_embedding_provider()),
    );
    let app = Arc::new(NeurolitheApp::new(
        stm_repo.clone(),
        llm.clone(),
        config.decay.default_half_life_days,
        config.decay.working_half_life_days(),
    ));
    let introspection = Arc::new(IntrospectionService::new(
        stm_repo.clone(),
        ltm_repo.clone(),
    ));

    // The one-shot MCP session (the field-test path) gets the same recall
    // service as the daemon/bus door — STM hybrid search + reference-returning
    // LTM recall, defaulting to the JARVIS tenant.
    let query = QueryService::new(
        RetrievalService::new(llm.clone(), stm_repo),
        LtmRetrieval::new(ltm_repo),
        llm,
    );

    McpServer::new(app, introspection, query).run_stdio().await
}

/// Resolve on Ctrl-C (SIGINT) or, on Unix, SIGTERM — so `docker stop` shuts the
/// daemon down gracefully instead of waiting for SIGKILL.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
