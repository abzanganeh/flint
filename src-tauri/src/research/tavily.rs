//! Tavily Search API client for prep-mode web research.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tracing::warn;

use crate::interfaces::web_search::{WebSearchProvider, WebSearchResult};

const TAVILY_SEARCH_URL: &str = "https://api.tavily.com/search";
const SEARCH_TIMEOUT_SECS: u64 = 20;

#[derive(Debug, Deserialize)]
struct TavilyResponse {
    results: Vec<TavilyHit>,
}

#[derive(Debug, Deserialize)]
struct TavilyHit {
    title: String,
    url: String,
    content: String,
}

/// Tavily-backed web search for rehearsal prep.
pub struct TavilySearch {
    client: Client,
    api_key: SecretString,
}

impl TavilySearch {
    pub fn new(api_key: SecretString) -> Result<Self> {
        if api_key.expose_secret().trim().is_empty() {
            anyhow::bail!("Tavily API key must not be empty");
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(SEARCH_TIMEOUT_SECS))
            .build()
            .context("Failed to build HTTP client for Tavily")?;
        Ok(Self { client, api_key })
    }
}

#[async_trait]
impl WebSearchProvider for TavilySearch {
    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<WebSearchResult>> {
        let max_results = max_results.clamp(1, 10);
        let body = serde_json::json!({
            // Tavily authenticates via the request body, not an Authorization header.
            "api_key": self.api_key.expose_secret(),
            "query": query,
            "search_depth": "basic",
            "max_results": max_results,
            "include_answer": false,
        });

        let response = self
            .client
            .post(TAVILY_SEARCH_URL)
            .json(&body)
            .send()
            .await
            .context("Tavily search request failed")?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            warn!(status = %status, "Tavily search HTTP error");
            anyhow::bail!("Tavily search failed ({status}): {body_text}");
        }

        let parsed: TavilyResponse = response
            .json()
            .await
            .context("Failed to parse Tavily search response")?;

        Ok(parsed
            .results
            .into_iter()
            .map(|hit| WebSearchResult {
                title: hit.title,
                url: hit.url,
                snippet: hit.content,
            })
            .collect())
    }

    fn name(&self) -> &str {
        "tavily"
    }
}

/// Resolve a Tavily provider when a key is stored in the OS keychain.
pub fn resolve_tavily() -> Option<Arc<dyn WebSearchProvider>> {
    let key = crate::keychain::get_api_key("tavily").ok()?;
    TavilySearch::new(key)
        .ok()
        .map(|provider| Arc::new(provider) as Arc<dyn WebSearchProvider>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::SecretString;

    #[test]
    fn empty_api_key_rejected() {
        assert!(TavilySearch::new(SecretString::new("".into())).is_err());
        assert!(TavilySearch::new(SecretString::new("  ".into())).is_err());
    }

    #[test]
    fn tavily_response_deserializes() {
        let parsed: TavilyResponse = serde_json::from_value(serde_json::json!({
            "results": [{
                "title": "NVIDIA NIM",
                "url": "https://example.com/nim",
                "content": "NIM microservice."
            }]
        }))
        .unwrap();
        assert_eq!(parsed.results.len(), 1);
        assert_eq!(parsed.results[0].title, "NVIDIA NIM");
    }
}
