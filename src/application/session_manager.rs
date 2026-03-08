use crate::domain::models::{Episode, MemoryResult, SessionId, TenantId, TimeFilter};
use crate::domain::ports::{LlmClient, MemoryRepository};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The optimized context window returned by push_dialogue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextWindow {
    /// Dense summary of older messages that were compressed
    pub summary: Option<String>,
    /// The most recent raw messages still in the buffer
    pub recent_messages: Vec<String>,
    /// Relevant facts from the knowledge graph
    pub relevant_facts: Vec<MemoryResult>,
}

/// Per-session buffer entry
#[derive(Debug, Clone)]
struct SessionBuffer {
    messages: Vec<String>,
    summary: Option<String>,
    token_count: usize,
}

impl SessionBuffer {
    fn new() -> Self {
        Self {
            messages: Vec::new(),
            summary: None,
            token_count: 0,
        }
    }
}

/// Manages per-session message buffers with token counting and context compression.
/// Implements blueprint section 2.2 (Short-Term Memory / Context Compressor).
pub struct SessionManager {
    memory_repo: Arc<dyn MemoryRepository>,
    llm_client: Arc<dyn LlmClient>,
    /// Per-session buffers (session_id → buffer)
    sessions: Mutex<HashMap<String, SessionBuffer>>,
    /// Max token count before triggering compression
    token_threshold: usize,
    /// Number of recent messages to keep raw after compression
    keep_recent: usize,
}

impl SessionManager {
    pub fn new(
        memory_repo: Arc<dyn MemoryRepository>,
        llm_client: Arc<dyn LlmClient>,
        token_threshold: usize,
        keep_recent: usize,
    ) -> Self {
        Self {
            memory_repo,
            llm_client,
            sessions: Mutex::new(HashMap::new()),
            token_threshold,
            keep_recent,
        }
    }

    /// Rough token estimation: ~4 characters per token (GPT tokenizer heuristic)
    fn estimate_tokens(text: &str) -> usize {
        text.len() / 4
    }

    /// Push a new message to the session buffer.
    /// Returns the optimized context window:
    /// [Dense Summary] + [Most Recent Raw Messages] + [Relevant Graph Facts]
    pub async fn push_dialogue(
        &self,
        tenant_id: &TenantId,
        session_id: &SessionId,
        new_message: &str,
        ccl: &str,
    ) -> Result<ContextWindow> {
        let message_tokens = Self::estimate_tokens(new_message);

        // 1. Archive the raw dialogue as an episode (Ground-Truth Preservation)
        let episode = Episode {
            id: None,
            tenant_id: tenant_id.clone(),
            session_id: session_id.clone(),
            raw_dialogue: new_message.to_string(),
            ccl: ccl.to_string(),
            created_at: None,
        };
        let _ep_id = self.memory_repo.store_episode(&episode)?;

        // 2. Add to session buffer
        let (needs_compression, _buffer_snapshot) = {
            let mut sessions = self.sessions.lock().unwrap();
            let buffer = sessions
                .entry(session_id.0.clone())
                .or_insert_with(SessionBuffer::new);

            buffer.messages.push(new_message.to_string());
            buffer.token_count += message_tokens;

            (buffer.token_count > self.token_threshold, buffer.clone())
        };

        // 3. Compress if buffer exceeds threshold
        if needs_compression {
            self.compress_buffer(session_id).await?;
        }

        // 4. Get the current optimized state
        let (summary, recent_messages) = {
            let sessions = self.sessions.lock().unwrap();
            if let Some(buffer) = sessions.get(&session_id.0) {
                (buffer.summary.clone(), buffer.messages.clone())
            } else {
                (None, vec![new_message.to_string()])
            }
        };

        // 5. Retrieve relevant graph facts for the latest message
        let time_filter = TimeFilter::default();
        let embedding = self.llm_client.embed_text(new_message).await?;
        let ccl_filter = vec![ccl.to_string()];
        let relevant_facts = self
            .memory_repo
            .query_with_graph(
                new_message,
                &embedding,
                tenant_id,
                &time_filter,
                &ccl_filter,
                5,
            )
            .unwrap_or_default();

        // 6. Queue for background fact extraction (asynchronous learning)
        // In a full implementation, this would spawn a background task.
        // For now, we'll extract inline if there's a matching episode.

        Ok(ContextWindow {
            summary,
            recent_messages,
            relevant_facts,
        })
    }

    /// Compress the oldest messages in a session buffer into a dense summary
    async fn compress_buffer(&self, session_id: &SessionId) -> Result<()> {
        let messages_to_compress = {
            let sessions = self.sessions.lock().unwrap();
            let buffer = sessions.get(&session_id.0).unwrap();

            if buffer.messages.len() <= self.keep_recent {
                return Ok(());
            }

            let compress_count = buffer.messages.len() - self.keep_recent;
            buffer.messages[..compress_count].to_vec()
        };

        if messages_to_compress.is_empty() {
            return Ok(());
        }

        // Build the text to compress (include existing summary if any)
        let existing_summary = {
            let sessions = self.sessions.lock().unwrap();
            sessions.get(&session_id.0).and_then(|b| b.summary.clone())
        };

        let text_to_compress = if let Some(ref existing) = existing_summary {
            format!(
                "Previous summary: {}\n\nNew messages:\n{}",
                existing,
                messages_to_compress.join("\n")
            )
        } else {
            messages_to_compress.join("\n")
        };

        // Call LLM to compress
        let new_summary = self.llm_client.compress_context(&text_to_compress).await?;

        // Update the buffer: remove compressed messages, update summary
        {
            let mut sessions = self.sessions.lock().unwrap();
            let buffer = sessions.get_mut(&session_id.0).unwrap();

            let compress_count = buffer.messages.len().saturating_sub(self.keep_recent);
            buffer.messages.drain(0..compress_count);
            buffer.summary = Some(new_summary);

            // Recalculate token count
            buffer.token_count = buffer
                .messages
                .iter()
                .map(|m| Self::estimate_tokens(m))
                .sum();
            if let Some(ref s) = buffer.summary {
                buffer.token_count += Self::estimate_tokens(s);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_estimation() {
        assert_eq!(SessionManager::estimate_tokens("hello world"), 2); // 11 chars / 4 ≈ 2
        assert_eq!(SessionManager::estimate_tokens(""), 0);
    }

    #[test]
    fn test_context_window_serialization() {
        let ctx = ContextWindow {
            summary: Some("User discussed Rust programming.".into()),
            recent_messages: vec!["What about borrowing?".into()],
            relevant_facts: vec![],
        };
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(json.contains("borrowing"));
    }
}
