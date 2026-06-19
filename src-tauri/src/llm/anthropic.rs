//! Anthropic Messages API streaming provider (Phase 12.3).

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::header::{HeaderName, HeaderValue, CONTENT_TYPE};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

use super::provider::{CompletionConfig, LLMProvider, RateLimit};

const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MODEL: &str = "claude-3-5-haiku-20241022";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CONTEXT_WINDOW: usize = 200_000;
const RATE_LIMIT_RPM: u32 = 48;
const RATE_LIMIT_TPM: u32 = 48_000;

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: usize,
    messages: Vec<AnthropicMessage>,
    stream: bool,
    temperature: f32,
}

#[derive(Debug, Deserialize)]
struct ContentBlockDelta {
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<ContentBlockDelta>,
}

#[derive(Debug, Deserialize)]
struct NonStreamContentBlock {
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NonStreamResponse {
    content: Vec<NonStreamContentBlock>,
}

pub struct AnthropicProvider {
    api_key: SecretString,
    model: String,
    base_url: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: SecretString) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("Failed to build Anthropic HTTP client")?;

        Ok(Self {
            api_key,
            model: DEFAULT_MODEL.to_string(),
            base_url: ANTHROPIC_BASE_URL.to_string(),
            client,
        })
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn build_request(&self, prompt: &str, config: &CompletionConfig) -> AnthropicRequest {
        AnthropicRequest {
            model: self.model.clone(),
            max_tokens: config.max_tokens.unwrap_or(1024),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            stream: config.stream,
            temperature: config.temperature,
        }
    }

    fn parse_sse_line(line: &str) -> Option<String> {
        let data = line.strip_prefix("data: ")?;
        if data.trim().is_empty() {
            return None;
        }
        let event: StreamEvent = serde_json::from_str(data).ok()?;
        if event.event_type != "content_block_delta" {
            return None;
        }
        event.delta.and_then(|d| d.text)
    }
}

pub fn resolve_anthropic() -> Option<Arc<dyn LLMProvider>> {
    let api_key = crate::keychain::get_api_key("anthropic").ok()?;
    AnthropicProvider::new(api_key)
        .map(|p| Arc::new(p) as Arc<dyn LLMProvider>)
        .ok()
}

#[async_trait]
impl LLMProvider for AnthropicProvider {
    async fn complete_stream(
        &self,
        prompt: String,
        config: CompletionConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let body = self.build_request(&prompt, &config);

        let api_key_header = HeaderName::from_static("x-api-key");
        let version_header = HeaderName::from_static("anthropic-version");

        let response = self
            .client
            .post(&self.base_url)
            .header(api_key_header, self.api_key.expose_secret())
            .header(version_header, HeaderValue::from_static(ANTHROPIC_VERSION))
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .context("Anthropic API request failed")?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(10);
            warn!(retry_after_secs = retry_after, "Anthropic rate limit (429)");
            bail!("rate_limit:{retry_after}");
        }

        if !status.is_success() {
            let err_body: Value = response
                .json()
                .await
                .unwrap_or_else(|_| serde_json::json!({"error": {"message": "unknown"}}));
            bail!(
                "Anthropic API error {status}: {}",
                err_body
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown")
            );
        }

        if !config.stream {
            let completion: NonStreamResponse = response
                .json()
                .await
                .context("Anthropic non-streaming response decode failed")?;
            let content = completion
                .content
                .into_iter()
                .filter_map(|b| b.text)
                .collect::<Vec<_>>()
                .join("");
            let stream = futures::stream::once(async move { Ok(content) });
            return Ok(Box::pin(stream));
        }

        let byte_stream = response.bytes_stream();
        let line_stream = byte_stream
            .map(|chunk| chunk.context("Anthropic stream read error"))
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
                        debug!(line = %line, "anthropic sse line");

                        Self::parse_sse_line(&line).map(Ok)
                    }
                }
            });

        Ok(Box::pin(line_stream))
    }

    fn name(&self) -> &str {
        "anthropic"
    }

    fn is_available(&self) -> bool {
        !self.api_key.expose_secret().is_empty()
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
    fn parse_sse_delta_extracts_text() {
        let line = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#;
        assert_eq!(
            AnthropicProvider::parse_sse_line(line),
            Some("Hi".to_string())
        );
    }

    #[test]
    fn is_available_false_for_empty_key() {
        let provider = AnthropicProvider::new(SecretString::new("".into())).unwrap();
        assert!(!provider.is_available());
    }
}
