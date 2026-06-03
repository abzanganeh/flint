//! Session persistence trait (design doc §27). Supabase implementation follows in Phase 2.

#![allow(dead_code)]

use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

use super::auth::AuthToken;

/// Session creation parameters (stub until Phase 2).
#[derive(Debug, Clone)]
pub struct SessionConfig;

/// Full session record (stub).
#[derive(Debug, Clone)]
pub struct Session {
    pub id: Uuid,
}

/// Session list entry (stub).
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: Uuid,
    pub name: String,
}

/// Persisted session payload (stub).
#[derive(Debug, Clone)]
pub struct SessionData;

/// Exported session bundle (stub).
#[derive(Debug, Clone)]
pub struct SessionExport;

/// Cloud/local session storage contract.
#[async_trait]
pub trait SessionInterface: Send + Sync {
    async fn create(&self, token: &AuthToken, config: SessionConfig) -> Result<Session>;
    async fn get(&self, token: &AuthToken, session_id: Uuid) -> Result<Session>;
    async fn list(&self, token: &AuthToken) -> Result<Vec<SessionSummary>>;
    async fn save(&self, token: &AuthToken, data: SessionData) -> Result<()>;
    async fn promote(&self, token: &AuthToken, session_id: Uuid) -> Result<()>;
    async fn delete(&self, token: &AuthToken, session_id: Uuid) -> Result<()>;
    async fn export(&self, token: &AuthToken, session_id: Uuid) -> Result<SessionExport>;
}

const NOT_READY: &str = "Session sync is not available yet. Please try again after signing in.";

/// Placeholder until `supabase::session` is implemented.
pub struct StubSession;

#[async_trait]
impl SessionInterface for StubSession {
    async fn create(&self, _token: &AuthToken, _config: SessionConfig) -> Result<Session> {
        anyhow::bail!(NOT_READY)
    }

    async fn get(&self, _token: &AuthToken, _session_id: Uuid) -> Result<Session> {
        anyhow::bail!(NOT_READY)
    }

    async fn list(&self, _token: &AuthToken) -> Result<Vec<SessionSummary>> {
        anyhow::bail!(NOT_READY)
    }

    async fn save(&self, _token: &AuthToken, _data: SessionData) -> Result<()> {
        anyhow::bail!(NOT_READY)
    }

    async fn promote(&self, _token: &AuthToken, _session_id: Uuid) -> Result<()> {
        anyhow::bail!(NOT_READY)
    }

    async fn delete(&self, _token: &AuthToken, _session_id: Uuid) -> Result<()> {
        anyhow::bail!(NOT_READY)
    }

    async fn export(&self, _token: &AuthToken, _session_id: Uuid) -> Result<SessionExport> {
        anyhow::bail!(NOT_READY)
    }
}
