use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use reqwest::{Client, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tracing::info;
use uuid::Uuid;

use crate::interfaces::auth::{AuthInterface, AuthToken, Plan, User};

const AUTH_TIMEOUT_SECS: u64 = 10;

/// Supabase GoTrue client. URL and anon key come from `tauri.conf.json` → `plugins.supabase`.
pub struct SupabaseAuth {
    client: Client,
    base_url: String,
    anon_key: SecretString,
}

impl SupabaseAuth {
    /// Build from Tauri plugin config + env-var overrides. Env vars
    /// `FLINT_SUPABASE_URL` / `FLINT_SUPABASE_ANON_KEY` take precedence so
    /// the committed `tauri.conf.json` can ship without secrets.
    pub fn from_tauri_plugins(
        plugins: &std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<Self> {
        let cfg = crate::supabase::resolve_supabase_config_required(plugins)?;
        Self::new(cfg.url, cfg.anon_key)
    }

    pub fn new(url: String, anon_key: String) -> Result<Self> {
        let base_url = url.trim_end_matches('/').to_string();
        if base_url.is_empty() {
            anyhow::bail!("Supabase URL is not configured");
        }
        if anon_key.is_empty() {
            anyhow::bail!("Supabase anon key is not configured");
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(AUTH_TIMEOUT_SECS))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            client,
            base_url,
            anon_key: SecretString::new(anon_key),
        })
    }

    pub(crate) fn public_base_url(&self) -> &str {
        &self.base_url
    }

    /// Exchange a PKCE authorization code from the OAuth deep-link callback.
    pub async fn exchange_pkce_code(
        &self,
        auth_code: &str,
        code_verifier: &str,
    ) -> Result<AuthToken> {
        let response = self
            .client
            .post(self.auth_url("/token?grant_type=pkce"))
            .headers(self.anon_headers())
            .json(&serde_json::json!({
                "auth_code": auth_code,
                "code_verifier": code_verifier,
            }))
            .send()
            .await
            .map_err(Self::map_transport_error)?;

        let status = response.status();
        if status.is_success() {
            let body: GoTrueTokenResponse = response
                .json()
                .await
                .map_err(|_| anyhow!("Authentication failed. Please try again."))?;
            info!(event = "oauth_login_success", provider = "google");
            return Self::token_from_response(body);
        }
        info!(event = "oauth_login_failed", provider = "google");
        Err(Self::map_response_error(status))
    }

    fn auth_url(&self, path: &str) -> String {
        format!("{}/auth/v1{}", self.base_url, path)
    }

    fn default_headers(&self, bearer: &str) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "apikey",
            self.anon_key
                .expose_secret()
                .parse()
                .expect("valid apikey header"),
        );
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {bearer}")
                .parse()
                .expect("valid authorization header"),
        );
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        headers
    }

    fn anon_headers(&self) -> reqwest::header::HeaderMap {
        self.default_headers(self.anon_key.expose_secret())
    }

    fn user_facing_error(status: StatusCode) -> String {
        match status.as_u16() {
            400 => "Invalid credentials".to_string(),
            429 => "Too many attempts, try again later".to_string(),
            500..=599 => {
                "Flint could not reach the auth service. Check your connection.".to_string()
            }
            _ => "Authentication failed. Please try again.".to_string(),
        }
    }

    fn map_response_error(status: StatusCode) -> anyhow::Error {
        anyhow!(Self::user_facing_error(status))
    }

    fn map_transport_error(err: reqwest::Error) -> anyhow::Error {
        if err.is_timeout() || err.is_connect() || err.is_request() {
            anyhow!("Flint could not reach the auth service. Check your connection.")
        } else {
            anyhow!("Authentication failed. Please try again.")
        }
    }

    /// Convert a GoTrue token JSON body into an in-memory [`AuthToken`].
    ///
    /// Takes ownership of `body` so the plaintext `access_token` and
    /// `refresh_token` heap allocations are *moved* into `SecretString` —
    /// not copied. `SecretString::Drop` then zeroes the buffer when the
    /// `AuthToken` (or the wrapping cache entry) is dropped. The only
    /// window where these strings exist outside a `SecretString` is the
    /// short scope between `response.json()` and this call — keep it that
    /// way: never `.clone()`, `format!`, or log `body.access_token` etc.
    fn token_from_response(body: GoTrueTokenResponse) -> Result<AuthToken> {
        let expires_at = Utc::now() + ChronoDuration::seconds(body.expires_in);
        Ok(AuthToken {
            access_token: SecretString::new(body.access_token),
            refresh_token: SecretString::new(body.refresh_token),
            expires_at,
        })
    }

    fn user_from_gotrue(user: GoTrueUser) -> Result<User> {
        let plan = parse_plan(&user);
        let email = user
            .email
            .filter(|e| !e.is_empty())
            .ok_or_else(|| anyhow!("Authentication failed. Please try again."))?;
        Ok(User {
            id: user.id,
            email,
            plan,
        })
    }

    async fn handle_response<T: for<'de> Deserialize<'de>>(
        response: reqwest::Response,
    ) -> Result<T> {
        let status = response.status();
        if status.is_success() {
            return response
                .json::<T>()
                .await
                .map_err(|_| anyhow!("Authentication failed. Please try again."));
        }
        Err(Self::map_response_error(status))
    }
}

#[derive(Debug, Deserialize)]
struct GoTrueTokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    /// Supabase includes user info in token responses; reserved for future use.
    #[serde(default)]
    #[allow(dead_code)]
    user: Option<GoTrueUser>,
}

#[derive(Debug, Deserialize)]
struct GoTrueUser {
    id: Uuid,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    app_metadata: serde_json::Value,
    #[serde(default)]
    user_metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct SignupResponse {
    #[serde(default)]
    user: Option<GoTrueUser>,
}

fn parse_plan(user: &GoTrueUser) -> Plan {
    let plan_str = user
        .app_metadata
        .get("plan")
        .and_then(|v| v.as_str())
        .or_else(|| user.user_metadata.get("plan").and_then(|v| v.as_str()));
    match plan_str.map(str::to_ascii_lowercase).as_deref() {
        Some("premium") => Plan::Premium,
        Some("basic") | Some("free") | None => Plan::Free,
        _ => Plan::Free,
    }
}

#[async_trait]
impl AuthInterface for SupabaseAuth {
    async fn signup(&self, email: &str, password: &str) -> Result<User> {
        let response = self
            .client
            .post(self.auth_url("/signup"))
            .headers(self.anon_headers())
            .json(&serde_json::json!({ "email": email, "password": password }))
            .send()
            .await
            .map_err(Self::map_transport_error)?;

        let status = response.status();
        if status.is_success() {
            let body: SignupResponse = response
                .json()
                .await
                .map_err(|_| anyhow!("Authentication failed. Please try again."))?;
            let user = body
                .user
                .ok_or_else(|| anyhow!("Authentication failed. Please try again."))?;
            return Self::user_from_gotrue(user);
        }
        Err(Self::map_response_error(status))
    }

    async fn login(&self, email: &str, password: &str) -> Result<AuthToken> {
        let response = self
            .client
            .post(self.auth_url("/token?grant_type=password"))
            .headers(self.anon_headers())
            .json(&serde_json::json!({ "email": email, "password": password }))
            .send()
            .await;

        match response {
            Ok(resp) if resp.status().is_success() => {
                info!(event = "login_success");
                let body: GoTrueTokenResponse = resp
                    .json()
                    .await
                    .map_err(|_| anyhow!("Authentication failed. Please try again."))?;
                Self::token_from_response(body)
            }
            Ok(resp) => {
                info!(event = "login_failed");
                Err(Self::map_response_error(resp.status()))
            }
            Err(err) => {
                info!(event = "login_failed");
                Err(Self::map_transport_error(err))
            }
        }
    }

    async fn logout(&self, token: &AuthToken) -> Result<()> {
        let response = self
            .client
            .post(self.auth_url("/logout"))
            .headers(self.default_headers(token.access_token.expose_secret()))
            .send()
            .await
            .map_err(Self::map_transport_error)?;

        let status = response.status();
        if status.is_success() {
            info!(event = "logout_success");
            Ok(())
        } else {
            info!(event = "logout_failed");
            Err(Self::map_response_error(status))
        }
    }

    async fn refresh(&self, refresh_token: &SecretString) -> Result<AuthToken> {
        let response = self
            .client
            .post(self.auth_url("/token?grant_type=refresh_token"))
            .headers(self.anon_headers())
            .json(&serde_json::json!({ "refresh_token": refresh_token.expose_secret() }))
            .send()
            .await
            .map_err(Self::map_transport_error)?;

        Self::handle_response::<GoTrueTokenResponse>(response)
            .await
            .and_then(Self::token_from_response)
    }

    async fn get_current_user(&self, token: &AuthToken) -> Result<User> {
        let response = self
            .client
            .get(self.auth_url("/user"))
            .headers(self.default_headers(token.access_token.expose_secret()))
            .send()
            .await
            .map_err(Self::map_transport_error)?;

        let status = response.status();
        if status.is_success() {
            let user: GoTrueUser = response
                .json()
                .await
                .map_err(|_| anyhow!("Authentication failed. Please try again."))?;
            return Self::user_from_gotrue(user);
        }
        Err(Self::map_response_error(status))
    }

    async fn delete_account(&self, token: &AuthToken) -> Result<()> {
        let response = self
            .client
            .delete(self.auth_url("/user"))
            .headers(self.default_headers(token.access_token.expose_secret()))
            .send()
            .await
            .map_err(Self::map_transport_error)?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(Self::map_response_error(status))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_facing_error_mapping() {
        assert_eq!(
            SupabaseAuth::user_facing_error(StatusCode::BAD_REQUEST),
            "Invalid credentials"
        );
        assert_eq!(
            SupabaseAuth::user_facing_error(StatusCode::TOO_MANY_REQUESTS),
            "Too many attempts, try again later"
        );
        assert_eq!(
            SupabaseAuth::user_facing_error(StatusCode::INTERNAL_SERVER_ERROR),
            "Flint could not reach the auth service. Check your connection."
        );
    }

    #[test]
    fn parse_plan_variants() {
        let premium = GoTrueUser {
            id: Uuid::new_v4(),
            email: Some("a@b.c".into()),
            app_metadata: serde_json::json!({ "plan": "premium" }),
            user_metadata: serde_json::Value::Null,
        };
        assert_eq!(parse_plan(&premium), Plan::Premium);

        let basic = GoTrueUser {
            id: Uuid::new_v4(),
            email: Some("a@b.c".into()),
            app_metadata: serde_json::json!({ "plan": "basic" }),
            user_metadata: serde_json::Value::Null,
        };
        assert_eq!(parse_plan(&basic), Plan::Free);
    }
}
