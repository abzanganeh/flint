//! Strategy B Phase 3 — metered credit client scaffold.
//!
//! v1 default: [`NoopCreditClient`] when `metered_credits_enabled` is off.
//! [`SmartResumeCreditClient`] calls Smart Resume credit routes when the flag
//! is on (dev / post-SSO); BYOK users skip credit calls in the orchestrator.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::StatusCode;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};
use uuid::Uuid;

/// Result of a successful debit against the Smart Resume ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeductResult {
    pub new_balance: i32,
    pub transaction_id: Option<String>,
}

pub type HoldId = Uuid;

#[async_trait]
pub trait BearerTokenSource: Send + Sync {
    async fn access_token(&self) -> Option<SecretString>;
}

/// Access token supplier — reads from OS keychain (same source as auth restore).
pub struct KeychainBearerSource;

#[async_trait]
impl BearerTokenSource for KeychainBearerSource {
    async fn access_token(&self) -> Option<SecretString> {
        crate::keychain::get_auth_token()
            .ok()
            .map(|token| token.access_token)
    }
}

#[async_trait]
pub trait CreditClient: Send + Sync {
    async fn get_balance(&self) -> Result<i32>;
    async fn deduct(&self, action: &str, session_id: &str) -> Result<DeductResult>;
    async fn hold(&self, session_id: &str, amount: i32) -> Result<HoldId>;
    async fn release_hold(&self, hold_id: HoldId) -> Result<()>;
}

/// v1 default — always succeeds; no remote calls.
pub struct NoopCreditClient;

#[async_trait]
impl CreditClient for NoopCreditClient {
    async fn get_balance(&self) -> Result<i32> {
        Ok(i32::MAX / 4)
    }

    async fn deduct(&self, action: &str, _session_id: &str) -> Result<DeductResult> {
        debug!(
            event = "credit_deduct_noop",
            action, "metered credits disabled"
        );
        Ok(DeductResult {
            new_balance: i32::MAX / 4,
            transaction_id: None,
        })
    }

    async fn hold(&self, session_id: &str, amount: i32) -> Result<HoldId> {
        debug!(
            event = "credit_hold_noop",
            session_id, amount, "metered credits disabled"
        );
        Ok(Uuid::nil())
    }

    async fn release_hold(&self, _hold_id: HoldId) -> Result<()> {
        Ok(())
    }
}

/// HTTP client for Smart Resume `/api/credits/*` (Strategy B §3.2 scaffold).
pub struct SmartResumeCreditClient {
    http: reqwest::Client,
    base_url: String,
    token_source: Arc<dyn BearerTokenSource>,
}

impl SmartResumeCreditClient {
    pub fn new(base_url: String, token_source: Arc<dyn BearerTokenSource>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("build Smart Resume credit HTTP client")?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            token_source,
        })
    }

    fn smart_resume_base_url() -> Result<String> {
        if let Ok(raw) = std::env::var("FLINT_SMART_RESUME_URL") {
            let trimmed = raw.trim().trim_end_matches('/').to_string();
            if !trimmed.is_empty() {
                return Ok(trimmed);
            }
        }
        #[cfg(debug_assertions)]
        {
            Ok("http://localhost:8000".to_string())
        }
        #[cfg(not(debug_assertions))]
        {
            anyhow::bail!("Smart Resume is not configured. Set FLINT_SMART_RESUME_URL.");
        }
    }

    pub fn from_env(token_source: Arc<dyn BearerTokenSource>) -> Result<Self> {
        Self::new(Self::smart_resume_base_url()?, token_source)
    }

    async fn bearer(&self) -> Result<SecretString> {
        self.token_source
            .access_token()
            .await
            .context("not logged in — sign in to use platform credits")
    }

    async fn authed_get(&self, path: &str) -> Result<reqwest::Response> {
        let token = self.bearer().await?;
        let url = format!("{}{path}", self.base_url);
        self.http
            .get(&url)
            .header("Authorization", format!("Bearer {}", token.expose_secret()))
            .send()
            .await
            .context("Smart Resume credit request failed")
    }

    async fn authed_post<T: Serialize>(&self, path: &str, body: &T) -> Result<reqwest::Response> {
        let token = self.bearer().await?;
        let url = format!("{}{path}", self.base_url);
        self.http
            .post(&url)
            .header("Authorization", format!("Bearer {}", token.expose_secret()))
            .json(body)
            .send()
            .await
            .context("Smart Resume credit request failed")
    }
}

#[derive(Debug, Deserialize)]
struct SrBalanceResponse {
    free: i32,
}

#[derive(Debug, Serialize)]
struct DeductRequest<'a> {
    action: &'a str,
    product: &'static str,
    session_id: &'a str,
}

#[derive(Debug, Deserialize)]
struct DeductResponse {
    balance: i32,
    transaction_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct HoldRequest<'a> {
    session_id: &'a str,
    amount: i32,
}

#[derive(Debug, Deserialize)]
struct HoldResponse {
    hold_id: String,
}

#[derive(Debug, Serialize)]
struct ReleaseHoldRequest {
    hold_id: String,
}

#[async_trait]
impl CreditClient for SmartResumeCreditClient {
    async fn get_balance(&self) -> Result<i32> {
        let resp = self.authed_get("/api/credits/balance").await?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            anyhow::bail!("Smart Resume rejected the access token");
        }
        if !resp.status().is_success() {
            warn!(
                event = "credit_balance_failed",
                status = %resp.status(),
                "Smart Resume balance check failed"
            );
            anyhow::bail!("credit balance unavailable");
        }
        let body: SrBalanceResponse = resp.json().await.context("parse balance response")?;
        Ok(body.free)
    }

    async fn deduct(&self, action: &str, session_id: &str) -> Result<DeductResult> {
        let body = DeductRequest {
            action,
            product: "career_flint",
            session_id,
        };
        let resp = self.authed_post("/api/credits/deduct", &body).await?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            anyhow::bail!("Smart Resume rejected the access token");
        }
        if !resp.status().is_success() {
            warn!(
                event = "credit_deduct_failed",
                status = %resp.status(),
                action,
                "Smart Resume deduct failed — non-fatal in scaffold"
            );
            anyhow::bail!("credit deduct failed");
        }
        let parsed: DeductResponse = resp.json().await.context("parse deduct response")?;
        Ok(DeductResult {
            new_balance: parsed.balance,
            transaction_id: parsed.transaction_id,
        })
    }

    async fn hold(&self, session_id: &str, amount: i32) -> Result<HoldId> {
        let body = HoldRequest { session_id, amount };
        let resp = self.authed_post("/api/credits/hold", &body).await?;
        if !resp.status().is_success() {
            anyhow::bail!("credit hold failed");
        }
        let parsed: HoldResponse = resp.json().await.context("parse hold response")?;
        Uuid::parse_str(&parsed.hold_id).context("parse hold_id uuid")
    }

    async fn release_hold(&self, hold_id: HoldId) -> Result<()> {
        let body = ReleaseHoldRequest {
            hold_id: hold_id.to_string(),
        };
        let resp = self.authed_post("/api/credits/release-hold", &body).await?;
        if !resp.status().is_success() {
            anyhow::bail!("credit release-hold failed");
        }
        Ok(())
    }
}

pub fn build_credit_client(token_source: Arc<dyn BearerTokenSource>) -> Arc<dyn CreditClient> {
    match SmartResumeCreditClient::from_env(token_source) {
        Ok(client) => Arc::new(client),
        Err(e) => {
            debug!(
                event = "credit_client_noop_fallback",
                error = %e,
                "Smart Resume URL unavailable — using NoopCreditClient"
            );
            Arc::new(NoopCreditClient)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_client_returns_generous_balance() {
        let client = NoopCreditClient;
        let balance = client.get_balance().await.expect("balance");
        assert!(balance > 0);
    }

    #[tokio::test]
    async fn noop_deduct_succeeds() {
        let client = NoopCreditClient;
        let result = client
            .deduct("rehearsal_turn", "sess-1")
            .await
            .expect("deduct");
        assert!(result.new_balance > 0);
    }
}
