//! LLM provider trait and supporting types (design doc §27).
//!
//! Concrete implementations (Groq, OpenAI, Anthropic, Ollama) are wired up
//! in later phases. Adding a new provider = implement `LLMProvider` and
//! register it in the orchestrator — no core changes.

#![allow(dead_code)]

use std::pin::Pin;

use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;

// ──────────────────────────────────────────────────────────────────────────────
// Supporting types
// ──────────────────────────────────────────────────────────────────────────────

/// Configuration for a single completion call.
#[derive(Debug, Clone)]
pub struct CompletionConfig {
    /// Hard upper limit on generated tokens. None means provider default.
    pub max_tokens: Option<usize>,
    /// Sampling temperature. 0.0 = deterministic.
    pub temperature: f32,
    /// Whether to stream tokens (`complete_stream`) or collect them in one
    /// call (`complete`). Providers should honour this when the API supports
    /// it, but the trait always returns a stream — callers that don't need
    /// streaming use [`LLMProvider::complete`].
    pub stream: bool,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            max_tokens: None,
            temperature: 0.0,
            stream: true,
        }
    }
}

/// Token-bucket parameters for rate-limiting enforcement (§29).
/// Values are set to 80% of the documented provider free-tier limits.
#[derive(Debug, Clone, Copy)]
pub struct RateLimit {
    pub requests_per_minute: u32,
    pub tokens_per_minute: u32,
}

// ──────────────────────────────────────────────────────────────────────────────
// Trait
// ──────────────────────────────────────────────────────────────────────────────

/// Streaming LLM completion contract (design doc §27).
///
/// All provider implementations must be `Send + Sync` so they can be held
/// in `Arc` and shared across `tokio::spawn` tasks.
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Fire an inference and return a live token stream. Each item is a token
    /// fragment (may be empty). The stream terminates when the provider
    /// signals completion or an error occurs.
    async fn complete_stream(
        &self,
        prompt: String,
        config: CompletionConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>>;

    /// Convenience: collect the full stream into a single `String`.
    /// Used for non-streaming calls (e.g. digest extraction, compression).
    async fn complete(&self, prompt: String, config: CompletionConfig) -> Result<String> {
        use futures::StreamExt;
        let mut stream = self
            .complete_stream(
                prompt,
                CompletionConfig {
                    stream: false,
                    ..config
                },
            )
            .await?;
        let mut out = String::new();
        while let Some(token) = stream.next().await {
            out.push_str(&token?);
        }
        Ok(out)
    }

    /// Identifier used for prompt file lookup (`prompts/{module}/{name}.txt`).
    fn name(&self) -> &str;

    /// Whether this provider is reachable right now (used by health check).
    fn is_available(&self) -> bool;

    /// Approximate context window in tokens.
    fn context_window(&self) -> usize;

    /// Rate-limit parameters for this provider/tier.
    fn rate_limit(&self) -> RateLimit;
}

// ──────────────────────────────────────────────────────────────────────────────
// Stub provider (development only — replaced when real providers are wired up)
// ──────────────────────────────────────────────────────────────────────────────

/// Stand-in provider used at startup before any real LLM is configured.
///
/// Returns a minimal valid JSON digest so that `extract_digest` does not
/// fail with a parse error, giving the rest of the session pipeline a chance
/// to exercise. Replace with a concrete provider (Groq, Ollama, etc.) in
/// Phase 3.
pub struct StubLLMProvider;

#[async_trait]
impl LLMProvider for StubLLMProvider {
    async fn complete_stream(
        &self,
        _prompt: String,
        _config: CompletionConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let json = r#"{
  "role": "Unknown",
  "company": "Unknown",
  "domain": "software engineering",
  "key_skills": [],
  "seniority": "unknown",
  "likely_questions": [
    "Tell me about yourself",
    "What are your greatest strengths?",
    "Why do you want this role?",
    "Describe a challenging project you have worked on",
    "Where do you see yourself in five years?"
  ],
  "topics_to_avoid": []
}"#;
        let stream = futures::stream::once(async move { Ok(json.to_string()) });
        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "default"
    }

    fn is_available(&self) -> bool {
        false // signals to the health-check that no real provider is configured
    }

    fn context_window(&self) -> usize {
        4_096
    }

    fn rate_limit(&self) -> RateLimit {
        RateLimit {
            requests_per_minute: 0,
            tokens_per_minute: 0,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Test helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Mock provider that returns a fixed string for every completion call.
/// Useful in unit tests that need an `LLMProvider` without hitting a real API.
#[cfg(test)]
pub struct MockLLMProvider {
    pub response: String,
    pub provider_name: String,
}

#[cfg(test)]
#[async_trait]
impl LLMProvider for MockLLMProvider {
    async fn complete_stream(
        &self,
        _prompt: String,
        _config: CompletionConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let response = self.response.clone();
        let stream = futures::stream::once(async move { Ok(response) });
        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        &self.provider_name
    }
    fn is_available(&self) -> bool {
        true
    }
    fn context_window(&self) -> usize {
        128_000
    }
    fn rate_limit(&self) -> RateLimit {
        RateLimit {
            requests_per_minute: 60,
            tokens_per_minute: 6_000,
        }
    }
}
