//! OpenAI-compatible chat completions streaming (Groq, DeepSeek, OpenAI, OpenRouter).
//!
//! Shared SSE parser and HTTP client for providers that expose
//! `/v1/chat/completions` with the same request/response shape.

use std::pin::Pin;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

use super::provider::{CompletionConfig, LLMProvider, RateLimit};

#[derive(Debug, Clone)]
pub struct OpenAiCompatConfig {
    pub provider_name: &'static str,
    pub base_url: String,
    pub default_model: &'static str,
    pub context_window: usize,
    pub requests_per_minute: u32,
    pub tokens_per_minute: u32,
    pub request_timeout: Duration,
    /// Some providers (DeepSeek) require a browser-like User-Agent for Cloudflare.
    pub user_agent: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct CompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<usize>,
    temperature: f32,
}

#[derive(Debug, Deserialize)]
struct StreamDelta {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
}

#[derive(Debug, Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
struct NonStreamMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NonStreamChoice {
    message: NonStreamMessage,
}

#[derive(Debug, Deserialize)]
struct NonStreamCompletion {
    choices: Vec<NonStreamChoice>,
}

pub struct OpenAiCompatProvider {
    api_key: SecretString,
    model: String,
    client: reqwest::Client,
    config: OpenAiCompatConfig,
}

impl OpenAiCompatProvider {
    pub fn new(api_key: SecretString, config: OpenAiCompatConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .context("Failed to build OpenAI-compatible HTTP client")?;

        Ok(Self {
            api_key,
            model: config.default_model.to_string(),
            client,
            config,
        })
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.config.base_url = base_url.into();
        self
    }

    fn build_request(&self, prompt: &str, config: &CompletionConfig) -> CompletionRequest {
        CompletionRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            stream: config.stream,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
        }
    }

    pub fn parse_sse_line(line: &str) -> Option<String> {
        let data = line.strip_prefix("data: ")?;
        if data.trim() == "[DONE]" {
            return None;
        }
        let chunk: StreamChunk = serde_json::from_str(data).ok()?;
        chunk
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.delta.content)
    }

    fn is_rate_limit_error(body: &Value) -> bool {
        body.get("error")
            .and_then(|e| e.get("type"))
            .and_then(|t| t.as_str())
            .map(|t| t.contains("rate_limit"))
            .unwrap_or(false)
    }
}

#[async_trait]
impl LLMProvider for OpenAiCompatProvider {
    async fn complete_stream(
        &self,
        prompt: String,
        config: CompletionConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let body = self.build_request(&prompt, &config);

        let mut req = self
            .client
            .post(&self.config.base_url)
            .header(
                AUTHORIZATION,
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header(CONTENT_TYPE, "application/json");

        if let Some(ua) = self.config.user_agent {
            req = req.header(USER_AGENT, ua);
        }

        let response = req
            .json(&body)
            .send()
            .await
            .with_context(|| format!("{} API request failed", self.config.provider_name))?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(10);

            warn!(
                provider = self.config.provider_name,
                retry_after_secs = retry_after,
                "rate limit (429) — caller should honour Retry-After"
            );

            bail!("rate_limit:{retry_after}");
        }

        if !status.is_success() {
            let err_body: Value = response
                .json()
                .await
                .unwrap_or_else(|_| serde_json::json!({"error": {"message": "unknown"}}));

            if Self::is_rate_limit_error(&err_body) {
                bail!("rate_limit:10");
            }

            bail!(
                "{} API error {status}: {}",
                self.config.provider_name,
                err_body
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown")
            );
        }

        if !config.stream {
            let completion: NonStreamCompletion = response
                .json()
                .await
                .context("non-streaming response decode failed")?;
            let content = completion
                .choices
                .into_iter()
                .next()
                .and_then(|c| c.message.content)
                .unwrap_or_default();
            let stream = futures::stream::once(async move { Ok(content) });
            return Ok(Box::pin(stream));
        }

        let byte_stream = response.bytes_stream();
        let provider_name = self.config.provider_name;
        let line_stream = byte_stream
            .map(|chunk| chunk.context("stream read error"))
            .flat_map(|chunk_result| {
                let lines: Vec<Result<String>> = match chunk_result {
                    Ok(bytes) => String::from_utf8_lossy(&bytes)
                        .lines()
                        .filter(|l| !l.is_empty())
                        .map(|l| Ok(l.to_string()))
                        .collect(),
                    Err(e) => vec![Err(e)],
                };
                futures::stream::iter(lines)
            })
            .filter_map(move |line_result| async move {
                match line_result {
                    Err(e) => Some(Err(e)),
                    Ok(line) => {
                        #[cfg(debug_assertions)]
                        debug!(provider = provider_name, line = %line, "openai-compat sse line");

                        Self::parse_sse_line(&line).map(Ok)
                    }
                }
            });

        Ok(Box::pin(line_stream))
    }

    fn name(&self) -> &str {
        self.config.provider_name
    }

    fn is_available(&self) -> bool {
        !self.api_key.expose_secret().is_empty()
    }

    fn context_window(&self) -> usize {
        self.config.context_window
    }

    fn rate_limit(&self) -> RateLimit {
        RateLimit {
            requests_per_minute: self.config.requests_per_minute,
            tokens_per_minute: self.config.tokens_per_minute,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> OpenAiCompatConfig {
        OpenAiCompatConfig {
            provider_name: "test",
            base_url: "http://localhost/v1/chat/completions".to_string(),
            default_model: "test-model",
            context_window: 128_000,
            requests_per_minute: 24,
            tokens_per_minute: 24_000,
            request_timeout: Duration::from_secs(30),
            user_agent: None,
        }
    }

    #[test]
    fn parse_sse_line_extracts_token() {
        let line = r#"data: {"choices":[{"delta":{"content":"Hello"}}]}"#;
        assert_eq!(
            OpenAiCompatProvider::parse_sse_line(line),
            Some("Hello".to_string())
        );
    }

    #[test]
    fn parse_sse_line_done_returns_none() {
        assert_eq!(OpenAiCompatProvider::parse_sse_line("data: [DONE]"), None);
    }

    #[test]
    fn is_available_false_for_empty_key() {
        let provider =
            OpenAiCompatProvider::new(SecretString::new("".into()), test_config()).unwrap();
        assert!(!provider.is_available());
    }

    #[test]
    fn is_available_true_for_nonempty_key() {
        let provider =
            OpenAiCompatProvider::new(SecretString::new("test-key".into()), test_config()).unwrap();
        assert!(provider.is_available());
    }
}
