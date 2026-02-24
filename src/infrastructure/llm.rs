use crate::domain::ports::{ExtractedFact, LlmClient};
use crate::infrastructure::config::{LlmConfig, LlmProvider};
use anyhow::{Result, anyhow};
use reqwest::Client;
use serde_json::json;
use std::sync::Arc;

pub fn create_llm_client(config: &LlmConfig, api_key: String) -> Arc<dyn LlmClient> {
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
    async fn extract_facts(&self, dialogue: &str) -> Result<Vec<ExtractedFact>> {
        let system_prompt = "
            Extract independent factual statements from the user's dialogue.
            Only extract facts that represent long-term knowledge, preferences, or identifiers.
            For each fact, also extract any relationships to other entities with temporal bounds.
            Return format: {\"facts\": [{\"fact\": \"...\", \"tags\": [...], \"relationships\": [{\"target_entity\": \"...\", \"relation\": \"WORKS_AT\", \"valid_from\": \"YYYY-MM-DD or null\", \"valid_until\": \"YYYY-MM-DD or null\"}]}]}
            Example: {\"facts\": [{\"fact\": \"Alice works at Google since 2021\", \"tags\": [\"employment\"], \"relationships\": [{\"target_entity\": \"Google\", \"relation\": \"WORKS_AT\", \"valid_from\": \"2021-01-01\", \"valid_until\": null}]}]}
            If no facts are present, return {\"facts\": []}.
            Output ONLY valid JSON.
        ";

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
    async fn extract_facts(&self, dialogue: &str) -> Result<Vec<ExtractedFact>> {
        let system_prompt = "
            Extract independent factual statements from the user's dialogue.
            Only extract facts that represent long-term knowledge, preferences, or identifiers.
            For each fact, also extract any relationships to other entities with temporal bounds.
            Return format: {\"facts\": [{\"fact\": \"...\", \"tags\": [...], \"relationships\": [{\"target_entity\": \"...\", \"relation\": \"WORKS_AT\", \"valid_from\": \"YYYY-MM-DD or null\", \"valid_until\": \"YYYY-MM-DD or null\"}]}]}
            Example: {\"facts\": [{\"fact\": \"Alice works at Google since 2021\", \"tags\": [\"employment\"], \"relationships\": [{\"target_entity\": \"Google\", \"relation\": \"WORKS_AT\", \"valid_from\": \"2021-01-01\", \"valid_until\": null}]}]}
            If no facts are present, return {\"facts\": []}.
            Output ONLY valid JSON.
        ";

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
    async fn extract_facts(&self, dialogue: &str) -> Result<Vec<ExtractedFact>> {
        let system_prompt = "
            Extract independent factual statements from the user's dialogue.
            Only extract facts that represent long-term knowledge, preferences, or identifiers.
            For each fact, also extract any relationships to other entities with temporal bounds.
            Return format: {\"facts\": [{\"fact\": \"...\", \"tags\": [...], \"relationships\": [{\"target_entity\": \"...\", \"relation\": \"WORKS_AT\", \"valid_from\": \"YYYY-MM-DD or null\", \"valid_until\": \"YYYY-MM-DD or null\"}]}]}
            Example: {\"facts\": [{\"fact\": \"Alice works at Google since 2021\", \"tags\": [\"employment\"], \"relationships\": [{\"target_entity\": \"Google\", \"relation\": \"WORKS_AT\", \"valid_from\": \"2021-01-01\", \"valid_until\": null}]}]}
            If no facts are present, return {\"facts\": []}.
            Output ONLY valid JSON.
        ";

        // Anthropic structure
        let payload = json!({
            "model": self.model,
            "max_tokens": 1024,
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
        let content = resp_json["content"][0]["text"]
            .as_str()
            .unwrap_or("{\"facts\": []}");

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

    async fn compress_context(&self, messages: &str) -> Result<String> {
        let system_prompt = "Compress the following conversation into a dense, factual summary. Preserve all key facts, decisions, and context. Remove filler and redundancy. Output only the summary text.";

        let payload = json!({
            "model": self.model,
            "max_tokens": 1024,
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
        let summary = resp_json["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        Ok(summary)
    }
}
