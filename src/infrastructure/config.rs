use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub llm: LlmConfig,
    pub database: DatabaseConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LlmConfig {
    pub provider: LlmProvider,
    pub model: String,
    pub embedding_model: String,
    pub base_url: Option<String>,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    Openai,
    Gemini,
    Anthropic,
    Custom,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub vector_dimension: usize,
    pub path: Option<String>,
}

impl AppConfig {
    pub fn load() -> anyhow::Result<Self> {
        // Load .env file if it exists
        let _ = dotenvy::dotenv();

        let mut builder = config::Config::builder()
            .set_default("llm.provider", "openai")?
            .set_default("llm.model", "gpt-4o-mini")?
            .set_default("llm.embedding_model", "text-embedding-3-small")?
            .set_default("database.vector_dimension", 1536)?;

        // If neurolithe.toml exists, load it
        if std::path::Path::new("neurolithe.toml").exists() {
            builder = builder.add_source(config::File::with_name("neurolithe.toml"));
        }

        // Environment variables override file config (e.g. NEUROLITHE__LLM__PROVIDER=gemini)
        builder = builder.add_source(config::Environment::with_prefix("NEUROLITHE").separator("__"));

        let config = builder.build()?;
        let app_config: AppConfig = config.try_deserialize()?;
        
        Ok(app_config)
    }
}
