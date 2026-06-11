//! OpenRouter streaming provider — OpenAI-compatible gateway (design doc §27).
//!
//! Used as cloud fallback when Groq is rate-limited or unavailable.
//! Default model matches Flint's Groq primary: Llama 3.3 70B Instruct.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

use super::provider::{CompletionConfig, LLMProvider, RateLimit};

const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const DEFAULT_MODEL: &str = "meta-llama/llama-3.3-70b-instruct";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CONTEXT_WINDOW: usize = 128_000;
const RATE_LIMIT_RPM: u32 = 60;
const RATE_LIMIT_TPM: u32 = 60_000;

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

pub struct OpenRouterProvider {
    api_key: SecretString,
    model: String,
    client: reqwest::Client,
}

impl OpenRouterProvider {
    pub fn new(api_key: SecretString) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("Failed to build OpenRouter HTTP client")?;

        Ok(Self {
            api_key,
            model: DEFAULT_MODEL.to_string(),
            client,
        })
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

    fn parse_sse_line(line: &str) -> Option<String> {
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
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_i64())
            .map(|c| c == 429)
            .unwrap_or(false)
            || body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(|m| m.to_lowercase().contains("rate"))
                .unwrap_or(false)
    }
}

/// Build an OpenRouter provider when a key is stored in the keychain.
pub fn resolve_openrouter() -> Option<Arc<dyn LLMProvider>> {
    let api_key = crate::keychain::get_api_key("openrouter").ok()?;
    OpenRouterProvider::new(api_key)
        .map(|p| Arc::new(p) as Arc<dyn LLMProvider>)
        .ok()
}

#[async_trait]
impl LLMProvider for OpenRouterProvider {
    async fn complete_stream(
        &self,
        prompt: String,
        config: CompletionConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let body = self.build_request(&prompt, &config);

        let response = self
            .client
            .post(OPENROUTER_BASE_URL)
            .header(
                AUTHORIZATION,
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header(CONTENT_TYPE, "application/json")
            .header("HTTP-Referer", "https://github.com/abzanganeh/flint")
            .header("X-Title", "Flint")
            .json(&body)
            .send()
            .await
            .context("OpenRouter API request failed")?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(10);
            warn!(
                retry_after_secs = retry_after,
                "OpenRouter rate limit (429)"
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

            let message = err_body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown");
            warn!(status = %status, message = %message, "OpenRouter API error");
            bail!("OpenRouter API error {status}: {message}");
        }

        if !config.stream {
            let completion: NonStreamCompletion = response
                .json()
                .await
                .context("OpenRouter non-streaming response decode failed")?;
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
        let line_stream = byte_stream
            .map(|chunk| chunk.context("OpenRouter stream read error"))
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
            .filter_map(|line_result| async move {
                match line_result {
                    Err(e) => Some(Err(e)),
                    Ok(line) => {
                        #[cfg(debug_assertions)]
                        debug!(line = %line, "openrouter sse line");
                        Self::parse_sse_line(&line).map(Ok)
                    }
                }
            });

        Ok(Box::pin(line_stream))
    }

    fn name(&self) -> &str {
        "openrouter"
    }

    fn is_available(&self) -> bool {
        !self.api_key.expose_secret().is_empty()
    }

    async fn health_check(&self) -> bool {
        self.is_available()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sse_line_extracts_token() {
        let line = r#"data: {"choices":[{"delta":{"content":"Hello"}}]}"#;
        assert_eq!(
            OpenRouterProvider::parse_sse_line(line),
            Some("Hello".to_string())
        );
    }

    #[test]
    fn is_available_requires_key() {
        let provider = OpenRouterProvider::new(SecretString::new("sk-or-test".into())).unwrap();
        assert!(provider.is_available());
    }
}
