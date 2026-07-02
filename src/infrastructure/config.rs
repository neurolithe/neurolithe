use serde::Deserialize;

/// Top-level runtime configuration for NeuroLithe V2.
///
/// V2 splits memory into two independent SQLite stores — a decaying short-term
/// store (`stm`) and a permanent long-term knowledge tree (`ltm`) — each with
/// its own path and vector dimension. The remaining sections wire the Kafka
/// feeder, the Pithos client, and the background schedulers. See
/// `IMPLEMENTATION-V2.md` slice 1 and `V2-DESIGN.md` §8.
#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub llm: LlmConfig,
    pub stm: StoreConfig,
    pub ltm: StoreConfig,
    pub kafka: KafkaConfig,
    pub pithos: PithosConfig,
    pub sweep: SweepConfig,
    pub metrics: MetricsConfig,
    pub feeder: FeederConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LlmConfig {
    pub provider: LlmProvider,
    pub model: String,
    pub embedding_model: String,
    pub base_url: Option<String>,
    /// Provider used for embeddings, independent of the chat `provider`.
    ///
    /// Claude has no embeddings endpoint, so when `provider` is `anthropic`
    /// this MUST point at a provider that does — e.g. `gemini` for Google
    /// `text-embedding-004`, or `custom` for a local Ollama/OpenAI-compatible
    /// endpoint. When unset it falls back to `provider` (single-provider mode,
    /// the historical behaviour).
    #[serde(default)]
    pub embedding_provider: Option<LlmProvider>,
    /// Base URL for the embedding provider (only used by `openai`/`custom`).
    /// Falls back to `base_url` when unset.
    #[serde(default)]
    pub embedding_base_url: Option<String>,
    /// GCP project for the `vertex` embedding provider (Vertex AI). Auth comes
    /// from the service-account key at `GOOGLE_APPLICATION_CREDENTIALS`.
    #[serde(default)]
    pub embedding_project: Option<String>,
    /// Vertex AI region for the `vertex` embedding provider, e.g. `us-central1`
    /// (drives data residency). `global` uses the unprefixed host. Defaults to
    /// `us-central1` when unset.
    #[serde(default)]
    pub embedding_location: Option<String>,
}

impl LlmConfig {
    /// The provider actually used for embeddings: `embedding_provider` when set,
    /// otherwise the chat `provider`.
    pub fn effective_embedding_provider(&self) -> &LlmProvider {
        self.embedding_provider.as_ref().unwrap_or(&self.provider)
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    Openai,
    Gemini,
    Anthropic,
    Custom,
    /// Google Vertex AI (service-account auth via `GOOGLE_APPLICATION_CREDENTIALS`).
    /// Used for embeddings, sharing Cadmus's Gemini/Vertex access.
    Vertex,
}

/// Configuration for one of the two SQLite memory stores (STM or LTM).
///
/// `vector_dimension` is locked at DB init — changing it on an existing file
/// breaks the `sqlite-vec` virtual table, so the DB must be rebuilt. The two
/// stores carry independent dimensions: STM and LTM can use different embedding
/// models (e.g. STM 1536, LTM 768 for local `nomic-embed-text`).
#[derive(Debug, Deserialize, Clone)]
pub struct StoreConfig {
    pub vector_dimension: usize,
    /// SQLite file path; `None` means an in-memory DB (used by tests).
    pub path: Option<String>,
}

/// Kafka connection settings for the feeder, command consumer, and metrics
/// publisher (rdkafka). Topics themselves are created out-of-band by
/// `../kafka/create-topics.sh` (slice 0); no auto-create.
#[derive(Debug, Deserialize, Clone)]
pub struct KafkaConfig {
    pub brokers: String,
    pub group_id: String,
}

/// Pithos archive-service connection — the feeder fetches `pt://` document text
/// over HTTP to distill meaning before writing to the stores. `token` is the
/// Pithos bearer token (read access); empty means unauthenticated (tests/local).
#[derive(Debug, Deserialize, Clone)]
pub struct PithosConfig {
    pub base_url: String,
    #[serde(default)]
    pub token: String,
}

/// How often the background STM decay sweep runs (`run_decay_sweep`, wired in
/// slice 2). The sweep applies one half-life day of decay per pass, so the
/// default cadence is daily.
#[derive(Debug, Deserialize, Clone)]
pub struct SweepConfig {
    pub interval_secs: u64,
}

/// How often the CT-scan snapshot is published to `memory.metrics` (slice 9).
#[derive(Debug, Deserialize, Clone)]
pub struct MetricsConfig {
    pub interval_secs: u64,
}

/// Whether the Kafka feeder (`document.completed` → dual-write) is active.
/// Lets the daemon run reset/introspection without consuming documents.
#[derive(Debug, Deserialize, Clone)]
pub struct FeederConfig {
    pub enabled: bool,
}

impl AppConfig {
    pub fn load() -> anyhow::Result<Self> {
        // Load .env file if it exists
        let _ = dotenvy::dotenv();

        Self::build(std::path::Path::new("neurolithe.toml").exists())
    }

    /// Build the config from defaults → optional `neurolithe.toml` → env vars.
    ///
    /// `include_file` is split out so tests can exercise the pure
    /// defaults/env path without a stray `neurolithe.toml` in the CWD skewing
    /// the result.
    fn build(include_file: bool) -> anyhow::Result<Self> {
        let mut builder = config::Config::builder()
            .set_default("llm.provider", "openai")?
            .set_default("llm.model", "gpt-4o-mini")?
            .set_default("llm.embedding_model", "text-embedding-3-small")?
            // STM keeps the historical default dimension (1536, OpenAI).
            .set_default("stm.vector_dimension", 1536)?
            .set_default("stm.path", "neurolithe-stm.sqlite")?
            // LTM defaults to 768 (local nomic-embed-text) per the offline goal;
            // the embedding provider stays configurable (decided at deploy time).
            .set_default("ltm.vector_dimension", 768)?
            .set_default("ltm.path", "neurolithe-ltm.sqlite")?
            .set_default("kafka.brokers", "localhost:9092")?
            .set_default("kafka.group_id", "neurolithe")?
            .set_default("pithos.base_url", "http://192.168.4.48:8080")?
            .set_default("pithos.token", "")?
            // Daily decay sweep (sweep applies one half-life day per pass).
            .set_default("sweep.interval_secs", 86_400_i64)?
            .set_default("metrics.interval_secs", 60_i64)?
            .set_default("feeder.enabled", true)?;

        // If neurolithe.toml exists, load it
        if include_file && std::path::Path::new("neurolithe.toml").exists() {
            builder = builder.add_source(config::File::with_name("neurolithe.toml"));
        }

        // Environment variables override file config (e.g. NEUROLITHE__LLM__PROVIDER=gemini)
        builder =
            builder.add_source(config::Environment::with_prefix("NEUROLITHE").separator("__"));

        let config = builder.build()?;
        let app_config: AppConfig = config.try_deserialize()?;

        Ok(app_config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Config parses with the V2 defaults (independent STM 1536 / LTM 768
    /// dimensions, Kafka/Pithos/scheduler sections) and env vars override a
    /// single store's dimension in isolation without touching the other.
    ///
    /// Defaults and env-override are asserted in one test on purpose: the env
    /// path mutates process-global vars, and merging avoids a race with a
    /// separate parallel defaults test.
    #[test]
    fn test_defaults_and_env_override() {
        // --- pure defaults (build(false) ignores any neurolithe.toml) ---
        let cfg = AppConfig::build(false).expect("config should load from defaults");

        assert_eq!(cfg.stm.vector_dimension, 1536);
        assert_eq!(cfg.ltm.vector_dimension, 768);
        assert_eq!(cfg.stm.path.as_deref(), Some("neurolithe-stm.sqlite"));
        assert_eq!(cfg.ltm.path.as_deref(), Some("neurolithe-ltm.sqlite"));
        assert_eq!(cfg.kafka.brokers, "localhost:9092");
        assert_eq!(cfg.kafka.group_id, "neurolithe");
        assert_eq!(cfg.pithos.base_url, "http://192.168.4.48:8080");
        assert_eq!(cfg.sweep.interval_secs, 86_400);
        assert_eq!(cfg.metrics.interval_secs, 60);
        assert!(cfg.feeder.enabled);
        // STM and LTM dimensions are independent — changing one never implies
        // the other.
        assert_ne!(cfg.stm.vector_dimension, cfg.ltm.vector_dimension);

        // --- env override of a single store's dimension ---
        // SAFETY: single-threaded test; vars are removed before returning.
        unsafe {
            std::env::set_var("NEUROLITHE__LTM__VECTOR_DIMENSION", "1024");
            std::env::set_var("NEUROLITHE__FEEDER__ENABLED", "false");
        }

        let cfg = AppConfig::build(false).expect("config should load with env overrides");

        // LTM override applied; STM untouched (dimensions are independent).
        assert_eq!(cfg.ltm.vector_dimension, 1024);
        assert_eq!(cfg.stm.vector_dimension, 1536);
        assert!(!cfg.feeder.enabled);

        unsafe {
            std::env::remove_var("NEUROLITHE__LTM__VECTOR_DIMENSION");
            std::env::remove_var("NEUROLITHE__FEEDER__ENABLED");
        }
    }
}
