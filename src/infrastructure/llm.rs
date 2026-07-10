use crate::domain::ports::{ExtractedFact, LlmClient};
use crate::infrastructure::config::{LlmConfig, LlmProvider};
use anyhow::{Result, anyhow};
use reqwest::Client;
use serde_json::json;
use std::sync::Arc;

/// Build the LLM client, delegating chat and embeddings to (possibly) different
/// providers. Chat uses `config.provider`/`config.model`/`config.base_url`;
/// embeddings use `config.effective_embedding_provider()` /
/// `config.embedding_model` / `config.embedding_base_url` (falling back to
/// `base_url`). This lets chat run on Claude (which has no embeddings endpoint)
/// while embeddings run on Vertex/Google/OpenAI/local — see `SplitLlmClient`.
pub fn create_llm_client(
    config: &LlmConfig,
    chat_api_key: String,
    embed_api_key: String,
) -> Arc<dyn LlmClient> {
    let chat = build_chat_client(config, chat_api_key);
    let embed = build_embed_client(config, embed_api_key);
    Arc::new(SplitLlmClient { chat, embed })
}

/// Default Vertex AI region when `embedding_location` is unset. `us-central1`
/// serves `text-embedding-004`; the `global` endpoint does not host it.
const DEFAULT_VERTEX_LOCATION: &str = "us-central1";

fn build_chat_client(config: &LlmConfig, api_key: String) -> Arc<dyn LlmClient> {
    match config.provider {
        LlmProvider::Openai | LlmProvider::Custom => Arc::new(OpenAiClient::new(
            api_key,
            config.model.clone(),
            config.embedding_model.clone(),
            config.base_url.clone(),
        )),
        LlmProvider::Gemini => Arc::new(GeminiClient::new(
            api_key,
            config.model.clone(),
            config.embedding_model.clone(),
        )),
        LlmProvider::Anthropic => Arc::new(AnthropicClient::new(api_key, config.model.clone())),
        LlmProvider::Vertex => Arc::new(vertex_client(config, config.model.clone())),
    }
}

fn build_embed_client(config: &LlmConfig, api_key: String) -> Arc<dyn LlmClient> {
    let base_url = config
        .embedding_base_url
        .clone()
        .or_else(|| config.base_url.clone());
    match config.effective_embedding_provider() {
        LlmProvider::Openai | LlmProvider::Custom => Arc::new(OpenAiClient::new(
            api_key,
            config.model.clone(),
            config.embedding_model.clone(),
            base_url,
        )),
        LlmProvider::Gemini => Arc::new(GeminiClient::new(
            api_key,
            config.model.clone(),
            config.embedding_model.clone(),
        )),
        LlmProvider::Anthropic => Arc::new(AnthropicClient::new(api_key, config.model.clone())),
        LlmProvider::Vertex => Arc::new(vertex_client(config, config.embedding_model.clone())),
    }
}

/// Construct a `VertexClient` from the config's project/location, defaulting the
/// region to `us-central1`.
fn vertex_client(config: &LlmConfig, model: String) -> VertexClient {
    VertexClient::new(
        model,
        config.embedding_project.clone().unwrap_or_default(),
        config
            .embedding_location
            .clone()
            .unwrap_or_else(|| DEFAULT_VERTEX_LOCATION.to_string()),
    )
}

/// Routes chat/reasoning calls to one provider and embeddings to another.
///
/// Required because Claude has no embeddings endpoint: the reasoning half runs
/// on Anthropic while `embed_text` is served by Google/OpenAI/a local model.
/// When both halves resolve to the same provider the two inner clients are
/// simply equivalent, so single-provider setups behave exactly as before.
struct SplitLlmClient {
    chat: Arc<dyn LlmClient>,
    embed: Arc<dyn LlmClient>,
}

#[async_trait::async_trait]
impl LlmClient for SplitLlmClient {
    async fn extract_facts(
        &self,
        dialogue: &str,
        valid_ccls: &[crate::domain::models::CclDefinition],
    ) -> Result<Vec<ExtractedFact>> {
        self.chat.extract_facts(dialogue, valid_ccls).await
    }

    async fn generate_ccl_description(&self, ccl_name: &str, context: &str) -> Result<String> {
        self.chat.generate_ccl_description(ccl_name, context).await
    }

    async fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        self.embed.embed_text(text).await
    }

    async fn compress_context(&self, messages: &str) -> Result<String> {
        self.chat.compress_context(messages).await
    }
}

// ==========================================
// OpenAI Client (also handles custom URLs)
// ==========================================
pub struct OpenAiClient {
    client: Client,
    api_key: String,
    model: String,
    embedding_model: String,
    base_url: String,
}

impl OpenAiClient {
    pub fn new(
        api_key: String,
        model: String,
        embedding_model: String,
        base_url: Option<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model,
            embedding_model,
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for OpenAiClient {
    async fn extract_facts(
        &self,
        dialogue: &str,
        valid_ccls: &[crate::domain::models::CclDefinition],
    ) -> Result<Vec<ExtractedFact>> {
        let ccl_descriptions = valid_ccls
            .iter()
            .map(|c| format!("- '{}' ({})", c.name, c.description))
            .collect::<Vec<_>>()
            .join("\n");

        let system_prompt = format!("
            Extract independent factual statements from the user's dialogue.
            Only extract facts that represent long-term knowledge, preferences, or identifiers.
            For each fact, also extract any relationships to other entities with temporal bounds.
            The available Cognitive Context Layers (CCL) are:
{}
            You MUST assign a valid 'ccl' to each fact and relationship based on these definitions.
            Return format: {{\"facts\": [{{\"fact\": \"...\", \"ccl\": \"reality\", \"tags\": [...], \"relationships\": [{{\"target_entity\": \"...\", \"relation\": \"WORKS_AT\", \"ccl\": \"reality\", \"valid_from\": \"YYYY-MM-DD or null\", \"valid_until\": \"YYYY-MM-DD or null\"}}]}}]}}
            If no facts are present, return {{\"facts\": []}}.
            Output ONLY valid JSON.
        ", ccl_descriptions);

        let payload = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": dialogue}
            ],
            "response_format": {"type": "json_object"}
        });

        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://neurolithe.com")
            .header("X-Title", "NeuroLithe")
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("OpenAI API error: {}", error_text));
        }

        let resp_json: serde_json::Value = resp.json().await?;
        let content = resp_json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("{\"facts\": []}");

        let parsed: serde_json::Value = serde_json::from_str(content)?;
        let facts = serde_json::from_value(parsed["facts"].clone())?;

        Ok(facts)
    }

    async fn generate_ccl_description(&self, ccl_name: &str, context: &str) -> Result<String> {
        let system_prompt = "You are a helpful assistant. Generate a generic one-line description for the conceptual memory layer requested.";
        let user_prompt = format!(
            "Generate a generic one-line description for the conceptual memory layer '{}' based on the following context:\n{}",
            ccl_name, context
        );

        let payload = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt}
            ]
        });

        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://neurolithe.com")
            .header("X-Title", "NeuroLithe")
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("OpenAI API error: {}", error_text));
        }

        let resp_json: serde_json::Value = resp.json().await?;
        let description = resp_json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        Ok(description)
    }

    async fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        let payload = json!({
            "model": self.embedding_model,
            "input": text
        });

        let url = format!("{}/embeddings", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://neurolithe.com")
            .header("X-Title", "NeuroLithe")
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("OpenAI API error: {}", error_text));
        }

        let resp_json: serde_json::Value = resp.json().await?;
        let embedding: Vec<f32> =
            serde_json::from_value(resp_json["data"][0]["embedding"].clone())?;

        Ok(embedding)
    }

    async fn compress_context(&self, messages: &str) -> Result<String> {
        let system_prompt = "Compress the following conversation into a dense, factual summary. Preserve all key facts, decisions, and context. Remove filler and redundancy. Output only the summary text.";

        let payload = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": messages}
            ]
        });

        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://neurolithe.com")
            .header("X-Title", "NeuroLithe")
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("OpenAI API error: {}", error_text));
        }

        let resp_json: serde_json::Value = resp.json().await?;
        let summary = resp_json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        Ok(summary)
    }
}

// ==========================================
// Google Gemini Client
// ==========================================
pub struct GeminiClient {
    client: Client,
    api_key: String,
    model: String,
    embedding_model: String,
}

impl GeminiClient {
    pub fn new(api_key: String, model: String, embedding_model: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model,
            embedding_model,
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for GeminiClient {
    async fn extract_facts(
        &self,
        dialogue: &str,
        valid_ccls: &[crate::domain::models::CclDefinition],
    ) -> Result<Vec<ExtractedFact>> {
        let ccl_descriptions = valid_ccls
            .iter()
            .map(|c| format!("- '{}' ({})", c.name, c.description))
            .collect::<Vec<_>>()
            .join("\n");

        let system_prompt = format!("
            Extract independent factual statements from the user's dialogue.
            Only extract facts that represent long-term knowledge, preferences, or identifiers.
            For each fact, also extract any relationships to other entities with temporal bounds.
            The available Cognitive Context Layers (CCL) are:
{}
            You MUST assign a valid 'ccl' to each fact and relationship based on these definitions.
            Return format: {{\"facts\": [{{\"fact\": \"...\", \"ccl\": \"reality\", \"tags\": [...], \"relationships\": [{{\"target_entity\": \"...\", \"relation\": \"WORKS_AT\", \"ccl\": \"reality\", \"valid_from\": \"YYYY-MM-DD or null\", \"valid_until\": \"YYYY-MM-DD or null\"}}]}}]}}
            If no facts are present, return {{\"facts\": []}}.
            Output ONLY valid JSON.
        ", ccl_descriptions);

        let payload = json!({
            "system_instruction": {
                "parts": [{"text": system_prompt}]
            },
            "contents": [{
                "parts": [{"text": dialogue}]
            }],
            "generationConfig": {
                "responseMimeType": "application/json"
            }
        });

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model, self.api_key
        );
        let resp = self.client.post(&url).json(&payload).send().await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Gemini API error: {}", error_text));
        }

        let resp_json: serde_json::Value = resp.json().await?;
        let content = resp_json["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .unwrap_or("{\"facts\": []}");

        let parsed: serde_json::Value = serde_json::from_str(content)?;
        let facts = serde_json::from_value(parsed["facts"].clone())?;

        Ok(facts)
    }

    async fn generate_ccl_description(&self, ccl_name: &str, context: &str) -> Result<String> {
        let system_prompt = "You are a helpful assistant. Generate a generic one-line description for the conceptual memory layer requested.";
        let user_prompt = format!(
            "Generate a generic one-line description for the conceptual memory layer '{}' based on the following context:\n{}",
            ccl_name, context
        );

        let payload = json!({
            "system_instruction": {
                "parts": [{"text": system_prompt}]
            },
            "contents": [{
                "parts": [{"text": user_prompt}]
            }]
        });

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model, self.api_key
        );
        let resp = self.client.post(&url).json(&payload).send().await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Gemini API error: {}", error_text));
        }

        let resp_json: serde_json::Value = resp.json().await?;
        let description = resp_json["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        Ok(description)
    }

    async fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        let payload = json!({
            "model": format!("models/{}", self.embedding_model),
            "content": {
                "parts": [{"text": text}]
            }
        });

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:embedContent?key={}",
            self.embedding_model, self.api_key
        );
        let resp = self.client.post(&url).json(&payload).send().await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Gemini API error: {}", error_text));
        }

        let resp_json: serde_json::Value = resp.json().await?;
        let embedding: Vec<f32> = serde_json::from_value(resp_json["embedding"]["values"].clone())?;

        Ok(embedding)
    }

    async fn compress_context(&self, messages: &str) -> Result<String> {
        let system_prompt = "Compress the following conversation into a dense, factual summary. Preserve all key facts, decisions, and context. Remove filler and redundancy. Output only the summary text.";

        let payload = json!({
            "system_instruction": {
                "parts": [{"text": system_prompt}]
            },
            "contents": [{
                "parts": [{"text": messages}]
            }]
        });

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model, self.api_key
        );
        let resp = self.client.post(&url).json(&payload).send().await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Gemini API error: {}", error_text));
        }

        let resp_json: serde_json::Value = resp.json().await?;
        let summary = resp_json["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        Ok(summary)
    }
}

// ==========================================
// Anthropic Client
// ==========================================

/// Return the text of the first `text` content block in an Anthropic Messages
/// response. Claude 4.6+ models (incl. Sonnet 5) can emit a `thinking` block at
/// index 0, so indexing `content[0].text` is unsafe — scan for the text block
/// instead. (We also disable thinking on these requests, but this keeps parsing
/// robust regardless of model/config.)
fn anthropic_first_text(resp_json: &serde_json::Value) -> String {
    resp_json["content"]
        .as_array()
        .and_then(|blocks| blocks.iter().find(|b| b["type"] == "text"))
        .and_then(|b| b["text"].as_str())
        .unwrap_or("")
        .to_string()
}

/// Output-token ceiling for **document summarization** (`compress_context`).
///
/// The old value (1024, shared with fact extraction) truncated long-document
/// summaries mid-word around ~2,500 chars — losing the decision-relevant tail
/// of multi-page reports (field-report §2). A summary is a bounded artifact, so
/// a generous cap (well within Sonnet's output limit) lets it complete; short
/// inputs stop early on their own and cost nothing extra.
const COMPRESS_MAX_TOKENS: u32 = 8192;

/// Output-token ceiling for the small structured calls (fact extraction, CCL
/// descriptions) whose responses are inherently short.
const SHORT_MAX_TOKENS: u32 = 1024;

pub struct AnthropicClient {
    client: Client,
    api_key: String,
    model: String,
}

impl AnthropicClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model,
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for AnthropicClient {
    async fn extract_facts(
        &self,
        dialogue: &str,
        valid_ccls: &[crate::domain::models::CclDefinition],
    ) -> Result<Vec<ExtractedFact>> {
        let ccl_descriptions = valid_ccls
            .iter()
            .map(|c| format!("- '{}' ({})", c.name, c.description))
            .collect::<Vec<_>>()
            .join("\n");

        let system_prompt = format!("
            Extract independent factual statements from the user's dialogue.
            Only extract facts that represent long-term knowledge, preferences, or identifiers.
            For each fact, also extract any relationships to other entities with temporal bounds.
            The available Cognitive Context Layers (CCL) are:
{}
            You MUST assign a valid 'ccl' to each fact and relationship based on these definitions.
            Return format: {{\"facts\": [{{\"fact\": \"...\", \"ccl\": \"reality\", \"tags\": [...], \"relationships\": [{{\"target_entity\": \"...\", \"relation\": \"WORKS_AT\", \"ccl\": \"reality\", \"valid_from\": \"YYYY-MM-DD or null\", \"valid_until\": \"YYYY-MM-DD or null\"}}]}}]}}
            If no facts are present, return {{\"facts\": []}}.
            Output ONLY valid JSON.
        ", ccl_descriptions);

        // Anthropic structure. Thinking is disabled so the response is a single
        // `text` block of JSON (Sonnet 5 otherwise runs adaptive thinking and
        // returns a thinking block first).
        let payload = json!({
            "model": self.model,
            "max_tokens": SHORT_MAX_TOKENS,
            "thinking": {"type": "disabled"},
            "system": system_prompt,
            "messages": [
                {"role": "user", "content": dialogue}
            ]
        });

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Anthropic API error: {}", error_text));
        }

        let resp_json: serde_json::Value = resp.json().await?;
        let text = anthropic_first_text(&resp_json);
        let content = if text.is_empty() {
            "{\"facts\": []}"
        } else {
            text.as_str()
        };

        // For Claude we might need to find JSON substring if it chatters
        let json_start = content.find('{').unwrap_or(0);
        let json_end = content.rfind('}').unwrap_or(content.len() - 1) + 1;
        let clean_json = &content[json_start..json_end];

        let parsed: serde_json::Value = serde_json::from_str(clean_json)?;
        let facts = serde_json::from_value(parsed["facts"].clone())?;

        Ok(facts)
    }

    async fn embed_text(&self, _text: &str) -> Result<Vec<f32>> {
        Err(anyhow!(
            "Anthropic does not offer a native embedding API. Please use OpenAI/Gemini or an OpenAI-compatible custom endpoint for embeddings."
        ))
    }

    async fn generate_ccl_description(&self, ccl_name: &str, context: &str) -> Result<String> {
        let system_prompt = "You are a helpful assistant. Generate a generic one-line description for the conceptual memory layer requested.";
        let user_prompt = format!(
            "Generate a generic one-line description for the conceptual memory layer '{}' based on the following context:\n{}",
            ccl_name, context
        );

        let payload = json!({
            "model": self.model,
            "max_tokens": SHORT_MAX_TOKENS,
            "thinking": {"type": "disabled"},
            "system": system_prompt,
            "messages": [
                {"role": "user", "content": user_prompt}
            ]
        });

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Anthropic API error: {}", error_text));
        }

        let resp_json: serde_json::Value = resp.json().await?;
        let content = anthropic_first_text(&resp_json).trim().to_string();
        Ok(content)
    }

    async fn compress_context(&self, messages: &str) -> Result<String> {
        let system_prompt = "Compress the following conversation into a dense, factual summary. Preserve all key facts, decisions, and context. Remove filler and redundancy. Output only the summary text.";

        let payload = json!({
            "model": self.model,
            "max_tokens": COMPRESS_MAX_TOKENS,
            "thinking": {"type": "disabled"},
            "system": system_prompt,
            "messages": [
                {"role": "user", "content": messages}
            ]
        });

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Anthropic API error: {}", error_text));
        }

        let resp_json: serde_json::Value = resp.json().await?;
        let summary = anthropic_first_text(&resp_json);
        Ok(summary)
    }
}

// ==========================================
// Google Vertex AI Client (embeddings)
// ==========================================

/// Vertex AI access-token scope (same broad scope Cadmus uses).
const VERTEX_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// Google Vertex AI client, used for embeddings so NeuroLithe can share
/// Cadmus's Gemini/Vertex access. Authenticates with a service-account key via
/// `gcp_auth` (reads `GOOGLE_APPLICATION_CREDENTIALS`), minting short-lived
/// bearer tokens — no API key. Chat methods are unimplemented (use
/// anthropic/gemini/custom for reasoning).
pub struct VertexClient {
    client: Client,
    model: String,
    project: String,
    location: String,
    /// Lazily-initialised token provider, cached so we don't re-read the
    /// service-account key on every embed (the token itself is cached by
    /// `gcp_auth` until near expiry).
    token_provider: tokio::sync::OnceCell<Arc<dyn gcp_auth::TokenProvider>>,
}

impl VertexClient {
    pub fn new(model: String, project: String, location: String) -> Self {
        Self {
            client: Client::new(),
            model,
            project,
            location,
            token_provider: tokio::sync::OnceCell::new(),
        }
    }

    /// The Vertex `:predict` embeddings endpoint. `global` uses the unprefixed
    /// host; any other location pins data residency to `{loc}-aiplatform...`
    /// (mirrors Cadmus's `generate_content_url`).
    fn predict_url(&self) -> String {
        let host = if self.location == "global" {
            "aiplatform.googleapis.com".to_string()
        } else {
            format!("{}-aiplatform.googleapis.com", self.location)
        };
        format!(
            "https://{host}/v1/projects/{proj}/locations/{loc}\
             /publishers/google/models/{model}:predict",
            proj = self.project,
            loc = self.location,
            model = self.model,
        )
    }

    /// Mint (or reuse a cached) short-lived Vertex access token from the
    /// service-account key at `GOOGLE_APPLICATION_CREDENTIALS`.
    async fn access_token(&self) -> Result<String> {
        let provider = self
            .token_provider
            .get_or_try_init(|| async {
                gcp_auth::provider().await.map_err(|e| {
                    anyhow!(
                        "Vertex auth init failed — is GOOGLE_APPLICATION_CREDENTIALS a valid \
                         service-account key? {e}"
                    )
                })
            })
            .await?;
        let token = provider
            .token(&[VERTEX_SCOPE])
            .await
            .map_err(|e| anyhow!("fetching Vertex access token: {e}"))?;
        Ok(token.as_str().to_string())
    }
}

#[async_trait::async_trait]
impl LlmClient for VertexClient {
    async fn extract_facts(
        &self,
        _dialogue: &str,
        _valid_ccls: &[crate::domain::models::CclDefinition],
    ) -> Result<Vec<ExtractedFact>> {
        Err(anyhow!(
            "Vertex client is embeddings-only in NeuroLithe; use anthropic/gemini/custom for chat."
        ))
    }

    async fn generate_ccl_description(&self, _ccl_name: &str, _context: &str) -> Result<String> {
        Err(anyhow!(
            "Vertex client is embeddings-only in NeuroLithe; use anthropic/gemini/custom for chat."
        ))
    }

    async fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        let token = self.access_token().await?;
        let payload = json!({ "instances": [{ "content": text }] });

        let resp = self
            .client
            .post(self.predict_url())
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Vertex embeddings error: {}", error_text));
        }

        let resp_json: serde_json::Value = resp.json().await?;
        let values = resp_json["predictions"][0]["embeddings"]["values"].clone();
        if values.is_null() {
            return Err(anyhow!(
                "Vertex embeddings: no values in response (unexpected shape): {}",
                resp_json
            ));
        }
        let embedding: Vec<f32> = serde_json::from_value(values)?;
        Ok(embedding)
    }

    async fn compress_context(&self, _messages: &str) -> Result<String> {
        Err(anyhow!(
            "Vertex client is embeddings-only in NeuroLithe; use anthropic/gemini/custom for chat."
        ))
    }
}
