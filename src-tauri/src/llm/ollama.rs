//! Ollama local provider (design doc §27).
//!
//! Calls the Ollama REST API at `http://localhost:11434/api/chat`.
//! Used as the local fallback when cloud providers are unavailable.
//!
//! Context window: 4,096 tokens (conservative; real value depends on the
//! loaded model — llama3.1:8b supports 128K natively, but we budget for the
//! smallest common model).

use std::pin::Pin;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::provider::{CompletionConfig, LLMProvider, RateLimit};

// ──────────────────────────────────────────────────────────────────────────────
// Constants
// ──────────────────────────────────────────────────────────────────────────────

const OLLAMA_CHAT_URL: &str = "http://localhost:11434/api/chat";
const OLLAMA_HEALTH_URL: &str = "http://localhost:11434/";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

/// Default model for live response generation on Tier 2+ hardware.
const DEFAULT_MODEL: &str = "llama3.1:8b";

/// Conservative context window that covers the smallest useful model.
const CONTEXT_WINDOW: usize = 4_096;

/// Ollama runs locally so rate limiting is effectively uncapped.
const RATE_LIMIT_RPM: u32 = 120;
const RATE_LIMIT_TPM: u32 = 120_000;

// ──────────────────────────────────────────────────────────────────────────────
// API types
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct OlamaChatRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct OllamaStreamChunk {
    message: Option<OllamaMessage>,
    done: bool,
}

// ──────────────────────────────────────────────────────────────────────────────
// Provider
// ──────────────────────────────────────────────────────────────────────────────

pub struct OllamaProvider {
    model: String,
    client: reqwest::Client,
    health_client: reqwest::Client,
}

impl OllamaProvider {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("Failed to build Ollama HTTP client")?;

        let health_client = reqwest::Client::builder()
            .timeout(HEALTH_TIMEOUT)
            .build()
            .context("Failed to build Ollama health-check client")?;

        Ok(Self {
            model: DEFAULT_MODEL.to_string(),
            client,
            health_client,
        })
    }

    /// Override the model — useful when the orchestrator selects a tier-based
    /// model (e.g. llama3.2:3b on low-memory hardware).
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    fn build_request(&self, prompt: &str, stream: bool) -> OlamaChatRequest {
        OlamaChatRequest {
            model: self.model.clone(),
            messages: vec![OllamaMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            stream,
        }
    }

    fn parse_chunk(line: &str) -> Option<Result<String>> {
        if line.is_empty() {
            return None;
        }
        let chunk: OllamaStreamChunk = match serde_json::from_str(line) {
            Ok(c) => c,
            Err(e) => return Some(Err(anyhow!("Ollama parse error: {e} | line: {line}"))),
        };
        if chunk.done {
            return None;
        }
        let content = chunk.message?.content;
        if content.is_empty() {
            None
        } else {
            Some(Ok(content))
        }
    }
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new().expect("OllamaProvider::new should not fail with valid reqwest defaults")
    }
}

#[async_trait]
impl LLMProvider for OllamaProvider {
    async fn complete_stream(
        &self,
        prompt: String,
        config: CompletionConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let body = self.build_request(&prompt, config.stream);

        let response = self
            .client
            .post(OLLAMA_CHAT_URL)
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .context("Ollama API request failed — is Ollama running?")?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Ollama API error {status}: {text}"));
        }

        let byte_stream = response.bytes_stream();

        let token_stream = byte_stream
            .map(|chunk| chunk.context("Ollama stream read error"))
            .flat_map(|chunk_result| {
                let lines: Vec<Result<Option<String>>> = match chunk_result {
                    Ok(bytes) => String::from_utf8_lossy(&bytes)
                        .lines()
                        .map(|l| {
                            #[cfg(debug_assertions)]
                            debug!(line = %l, "ollama stream line");

                            Self::parse_chunk(l).transpose()
                        })
                        .collect(),
                    Err(e) => vec![Err(e)],
                };
                futures::stream::iter(lines)
            })
            .filter_map(|r| async move {
                match r {
                    Ok(Some(token)) => Some(Ok(token)),
                    Ok(None) => None,
                    Err(e) => Some(Err(e)),
                }
            });

        Ok(Box::pin(token_stream))
    }

    fn name(&self) -> &str {
        "ollama"
    }

    fn is_available(&self) -> bool {
        // Synchronous check not possible here; callers use `check_health()`.
        // Return true as optimistic default — the orchestrator will discover
        // failures during the first real inference call.
        true
    }

    fn context_window(&self) -> usize {
        CONTEXT_WINDOW
    }

    fn rate_limit(&self) -> RateLimit {
        RateLimit {
            requests_per_minute: RATE_LIMIT_RPM,
            tokens_per_minute: RATE_LIMIT_TPM,
        }
    }
}

impl OllamaProvider {
    /// Async health check — returns `true` if Ollama's root endpoint responds
    /// within the 2-second timeout.
    pub async fn check_health(&self) -> bool {
        self.health_client
            .get(OLLAMA_HEALTH_URL)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_chunk_extracts_content() {
        let line = r#"{"model":"llama3.1:8b","message":{"role":"assistant","content":"Hello"},"done":false}"#;
        let result = OllamaProvider::parse_chunk(line);
        assert!(matches!(result, Some(Ok(ref s)) if s == "Hello"));
    }

    #[test]
    fn parse_chunk_done_returns_none() {
        let line = r#"{"model":"llama3.1:8b","done":true}"#;
        assert!(OllamaProvider::parse_chunk(line).is_none());
    }

    #[test]
    fn parse_chunk_empty_line_returns_none() {
        assert!(OllamaProvider::parse_chunk("").is_none());
    }

    #[test]
    fn build_request_uses_configured_model() {
        let provider = OllamaProvider::new()
            .unwrap()
            .with_model("llama3.2:3b");
        let req = provider.build_request("Hello", true);
        assert_eq!(req.model, "llama3.2:3b");
        assert!(req.stream);
    }

    #[test]
    fn rate_limits_are_uncapped() {
        let provider = OllamaProvider::new().unwrap();
        let rl = provider.rate_limit();
        assert!(rl.requests_per_minute >= 60);
    }
}
