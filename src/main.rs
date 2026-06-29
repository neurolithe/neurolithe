#![allow(clippy::arc_with_non_send_sync)]
pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod interfaces;

use crate::infrastructure::config::{AppConfig, LlmProvider};
use crate::infrastructure::llm::create_llm_client;
use crate::interfaces::mcp_server::McpServer;
use tokio::runtime::Runtime;

fn main() -> anyhow::Result<()> {
    // 1. Load configuration
    let config = AppConfig::load()?;

    // 2. Initialize the two memory stores (STM decaying engine + permanent LTM).
    //    The full concurrent daemon (feeder + command consumer + schedulers)
    //    lands in slice 11; for now main serves the MCP door over both stores.
    let stores = crate::infrastructure::database::init_stores(&config)?;
    let crate::infrastructure::database::MemoryStores { stm, ltm } = stores;

    let stm_repo: std::sync::Arc<dyn crate::domain::ports::MemoryRepository> =
        std::sync::Arc::new(crate::infrastructure::repository::SqliteMemoryRepository::new(stm));
    let ltm_repo: std::sync::Arc<dyn crate::domain::ltm::LtmRepository> =
        std::sync::Arc::new(crate::infrastructure::ltm_repository::SqliteLtmRepository::new(ltm));
    // Ensure the curated spine exists (idempotent).
    ltm_repo.seed_spine()?;

    let introspection = std::sync::Arc::new(
        crate::application::introspection::IntrospectionService::new(
            stm_repo.clone(),
            ltm_repo.clone(),
        ),
    );

    // 3. Initialize LLM Client
    let api_key = match config.llm.provider {
        LlmProvider::Openai | LlmProvider::Custom => std::env::var("OPENAI_API_KEY")
            .or_else(|_| std::env::var("NEUROLITHE_API_KEY"))
            .unwrap_or_else(|_| "dummy_key".to_string()),
        LlmProvider::Gemini => std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("NEUROLITHE_API_KEY"))
            .unwrap_or_else(|_| "dummy_key".to_string()),
        LlmProvider::Anthropic => std::env::var("ANTHROPIC_API_KEY")
            .or_else(|_| std::env::var("NEUROLITHE_API_KEY"))
            .unwrap_or_else(|_| "dummy_key".to_string()),
    };

    let llm_client = create_llm_client(&config.llm, api_key);

    let app = std::sync::Arc::new(crate::application::app::NeurolitheApp::new(
        stm_repo, llm_client, 7.0,
    ));

    // We create the Tokio runtime here since `main` is not async
    // and we want to spawn our MCP event loop.
    let rt = Runtime::new()?;

    rt.block_on(async {
        let server = McpServer::new(app, introspection);
        server.run_stdio().await
    })
}
