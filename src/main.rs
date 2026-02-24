pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod interfaces;

use tokio::runtime::Runtime;
use crate::interfaces::mcp_server::McpServer;
use crate::infrastructure::config::{AppConfig, LlmProvider};
use crate::infrastructure::llm::create_llm_client;

fn main() -> anyhow::Result<()> {
    // 1. Load configuration
    let config = AppConfig::load()?;

    // 2. Initialize Database with Configured Vector Dimension 
    let conn = crate::infrastructure::database::init_db(config.database.path.as_ref())?;
    crate::infrastructure::schema::init_schema(&conn, config.database.vector_dimension)?;
    
    let db_repo: std::sync::Arc<dyn crate::domain::ports::MemoryRepository> = 
        std::sync::Arc::new(crate::infrastructure::repository::SqliteMemoryRepository::new(conn));
        
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

    let app = std::sync::Arc::new(crate::application::app::NeurolitheApp::new(db_repo, llm_client, 7.0));

    // We create the Tokio runtime here since `main` is not async
    // and we want to spawn our MCP event loop.
    let rt = Runtime::new()?;
    
    rt.block_on(async {
        let server = McpServer::new(app);
        server.run_stdio().await
    })
}
