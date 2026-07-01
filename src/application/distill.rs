//! Meaning extraction — distill a document's text into the form the memory
//! stores need: a concise summary + an embedding (+ forward-looking concept
//! hints). The feeder (slice 7) runs this on text fetched from Pithos.

use crate::domain::ports::LlmClient;
use anyhow::{Result, bail};
use std::sync::Arc;

/// The distilled meaning of a document.
#[derive(Debug, Clone, PartialEq)]
pub struct Distillate {
    /// Concise summary — the leaf's summary and roll-up input.
    pub summary: String,
    /// Candidate concept labels for placement. Reserved for smart growth
    /// (deferred); V2 placement uses the embedding, so this is empty for now.
    pub concept_hints: Vec<String>,
    /// Embedding of the summary, locked to the LTM vector dimension.
    pub embedding: Vec<f32>,
}

/// Distills document text using the local LLM (summary) + embedding model.
pub struct Distiller {
    llm: Arc<dyn LlmClient>,
    /// Expected embedding length (LTM vector dimension). A mismatch is a hard
    /// error — a wrong-dimension vector would corrupt `vec_ltm`.
    expected_dim: usize,
}

impl Distiller {
    pub fn new(llm: Arc<dyn LlmClient>, expected_dim: usize) -> Self {
        Self { llm, expected_dim }
    }

    /// Summarize `text`, then embed the summary. Errors if the embedding's
    /// length does not match the configured LTM dimension.
    pub async fn distill(&self, text: &str) -> Result<Distillate> {
        let summary = self.llm.compress_context(text).await?;
        let embedding = self.llm.embed_text(&summary).await?;

        if embedding.len() != self.expected_dim {
            bail!(
                "embedding dimension {} does not match LTM dimension {}",
                embedding.len(),
                self.expected_dim
            );
        }

        Ok(Distillate {
            summary,
            concept_hints: Vec::new(),
            embedding,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::CclDefinition;
    use crate::domain::ports::ExtractedFact;
    use async_trait::async_trait;

    const DIM: usize = 8;

    /// A stub LLM: summary is a fixed marker, embedding is a fixed-length vector
    /// (length controlled per-test to exercise the dimension check).
    struct StubLlm {
        embed_dim: usize,
    }

    #[async_trait]
    impl LlmClient for StubLlm {
        async fn extract_facts(
            &self,
            _dialogue: &str,
            _valid_ccls: &[CclDefinition],
        ) -> Result<Vec<ExtractedFact>> {
            Ok(vec![])
        }
        async fn generate_ccl_description(&self, _name: &str, _ctx: &str) -> Result<String> {
            Ok(String::new())
        }
        async fn embed_text(&self, _text: &str) -> Result<Vec<f32>> {
            Ok(vec![0.5; self.embed_dim])
        }
        async fn compress_context(&self, messages: &str) -> Result<String> {
            Ok(format!("SUMMARY OF: {messages}"))
        }
    }

    #[tokio::test]
    async fn test_distill_returns_summary_and_embedding() {
        let distiller = Distiller::new(Arc::new(StubLlm { embed_dim: DIM }), DIM);

        let out = distiller.distill("a long document body").await.unwrap();
        assert_eq!(out.summary, "SUMMARY OF: a long document body");
        assert_eq!(out.embedding.len(), DIM, "embedding is the configured dim");
        assert!(out.concept_hints.is_empty());
    }

    #[tokio::test]
    async fn test_distill_rejects_wrong_dimension() {
        // Model returns DIM+1; the distiller must refuse it (would corrupt vec_ltm).
        let distiller = Distiller::new(Arc::new(StubLlm { embed_dim: DIM + 1 }), DIM);
        assert!(distiller.distill("text").await.is_err());
    }
}
