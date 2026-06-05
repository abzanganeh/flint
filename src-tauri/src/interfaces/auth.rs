//! Auth provider trait and domain types (Section 27). Consumed by `supabase::auth` and Tauri commands.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use secrecy::SecretString;
use uuid::Uuid;

/// Authenticated user returned by auth providers.
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub plan: Plan,
}

/// Subscription tier for feature gating.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Plan {
    Free,
    Premium,
}

/// OAuth-style tokens from the auth provider. Secrets must never be logged or persisted as plain `String`.
#[derive(Clone)]
pub struct AuthToken {
    pub access_token: SecretString,
    pub refresh_token: SecretString,
    pub expires_at: DateTime<Utc>,
}

/// Authentication provider contract. Implementations (e.g. Supabase GoTrue) swap without touching callers.
#[async_trait]
pub trait AuthInterface: Send + Sync {
    async fn signup(&self, email: &str, password: &str) -> Result<User>;
    async fn login(&self, email: &str, password: &str) -> Result<AuthToken>;
    async fn logout(&self, token: &AuthToken) -> Result<()>;
    async fn refresh(&self, refresh_token: &str) -> Result<AuthToken>;
    async fn get_current_user(&self, token: &AuthToken) -> Result<User>;
    async fn delete_account(&self, token: &AuthToken) -> Result<()>;
}
