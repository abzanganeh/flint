//! Web search trait for rehearsal prep-mode research (Phase 5.6).
//!
//! Implementations are swappable — Tavily is the v1 provider. Live-session
//! web fallback will reuse this trait in a later phase.

use anyhow::Result;
use async_trait::async_trait;

/// A single web search hit returned by a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Contract for prep-mode web research providers.
#[async_trait]
pub trait WebSearchProvider: Send + Sync {
    /// Run a web search for `query` and return up to `max_results` hits.
    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<WebSearchResult>>;

    /// Provider identifier for logging and health checks.
    fn name(&self) -> &str;
}
