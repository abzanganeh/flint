//! Supabase session sync (design doc §Module 6).
//!
//! Syncs local SQLite session data to Supabase after a clean `ENDED`
//! transition. Network failures are non-fatal — the local SQLite copy is
//! always the source of truth.
//!
//! ## RLS contract
//! All Supabase tables have RLS enabled. Every write here goes through the
//! user's JWT (`AuthToken.access_token`), which Supabase GoTrue injects as
//! the `auth.uid()`. The Postgres policies verify ownership automatically.

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;
use tracing::{info, warn};
use uuid::Uuid;

use crate::interfaces::auth::AuthToken;
use crate::session::persistence::{RecoveryData, SessionPersistence};

const SYNC_TIMEOUT_SECS: u64 = 15;

// ──────────────────────────────────────────────────────────────────────────────
// Request bodies
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct UpsertSessionRow<'a> {
    id: &'a str,
    name: &'a str,
    #[serde(rename = "type")]
    session_type: &'a str,
    domain: &'a str,
    status: &'a str,
}

#[derive(Serialize)]
struct InsertTranscriptRow<'a> {
    id: &'a str,
    session_id: &'a str,
    speaker: &'a str,
    content: &'a str,
    timestamp: &'a str,
}

#[derive(Serialize)]
struct InsertResponseRow<'a> {
    id: &'a str,
    session_id: &'a str,
    #[serde(rename = "type")]
    response_type: &'a str,
    content: &'a str,
    confidence: &'a str,
}

// ──────────────────────────────────────────────────────────────────────────────
// Sync implementation
// ──────────────────────────────────────────────────────────────────────────────

/// Supabase session sync client.
pub struct SupabaseSessionSync {
    client: Client,
    base_url: String,
    anon_key: SecretString,
}

impl SupabaseSessionSync {
    pub fn new(base_url: String, anon_key: String) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(SYNC_TIMEOUT_SECS))
            .build()
            .context("Failed to create HTTP client for session sync")?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            anon_key: SecretString::new(anon_key),
        })
    }

    fn rest_url(&self, table: &str) -> String {
        format!("{}/rest/v1/{}", self.base_url, table)
    }

    fn headers(&self, token: &str) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(
            "apikey",
            self.anon_key
                .expose_secret()
                .parse()
                .expect("valid apikey"),
        );
        h.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {token}").parse().expect("valid auth"),
        );
        h.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        // Prefer=resolution=merge-duplicates so upserts are idempotent.
        h.insert("Prefer", "resolution=merge-duplicates".parse().unwrap());
        h
    }

    /// Sync one completed session from SQLite to Supabase.
    ///
    /// Loads the full session data from `persistence`, then POSTs each table
    /// in order. A partial sync (e.g. transcripts written but responses not)
    /// is safe: Supabase upserts are idempotent, so re-running on the next
    /// app launch will complete the sync.
    ///
    /// Returns `Ok(())` on success. Network errors are logged and returned
    /// so the caller can decide whether to retry or accept partial sync.
    pub async fn sync_session(
        &self,
        session_id: Uuid,
        token: &AuthToken,
        persistence: &SessionPersistence,
    ) -> Result<()> {
        let data: RecoveryData = match persistence.load_session_for_recovery(session_id)? {
            Some(d) => d,
            None => {
                warn!(session_id = %session_id, "no local data to sync");
                return Ok(());
            }
        };

        let sid = session_id.to_string();
        let bearer = token.access_token.expose_secret();
        let headers = self.headers(bearer);

        // 1. Upsert the session row.
        let session_row = UpsertSessionRow {
            id: &sid,
            name: "Interview Session",
            session_type: "interview",
            domain: "software engineering",
            status: "ended",
        };
        self.client
            .post(self.rest_url("sessions"))
            .headers(headers.clone())
            .json(&[&session_row])
            .send()
            .await
            .context("upsert session row")?
            .error_for_status()
            .context("session upsert HTTP error")?;

        info!(session_id = %sid, "session row synced");

        // 2. Insert transcript chunks (idempotent — Supabase ignores duplicate PKs).
        if !data.transcript_chunks.is_empty() {
            // Pre-build owned ID strings to avoid temporary lifetime issues.
            let chunk_ids: Vec<String> = data.transcript_chunks.iter().map(|c| c.id.to_string()).collect();
            let rows: Vec<InsertTranscriptRow<'_>> = data
                .transcript_chunks
                .iter()
                .zip(chunk_ids.iter())
                .map(|(c, id)| InsertTranscriptRow {
                    id,
                    session_id: &sid,
                    speaker: &c.speaker,
                    content: &c.text,
                    timestamp: "now()",
                })
                .collect();

            self.client
                .post(self.rest_url("transcripts"))
                .headers(headers.clone())
                .json(&rows)
                .send()
                .await
                .context("insert transcripts")?
                .error_for_status()
                .context("transcripts HTTP error")?;

            info!(
                session_id = %sid,
                count = rows.len(),
                "transcript chunks synced"
            );
        }

        // 3. Insert AI responses.
        if !data.responses.is_empty() {
            let response_ids: Vec<String> = data.responses.iter().map(|r| r.id.to_string()).collect();
            let rows: Vec<InsertResponseRow<'_>> = data
                .responses
                .iter()
                .zip(response_ids.iter())
                .map(|(r, id)| InsertResponseRow {
                    id,
                    session_id: &sid,
                    response_type: r.response_type.as_str(),
                    content: &r.content,
                    confidence: "medium",
                })
                .collect();

            self.client
                .post(self.rest_url("responses"))
                .headers(headers.clone())
                .json(&rows)
                .send()
                .await
                .context("insert responses")?
                .error_for_status()
                .context("responses HTTP error")?;

            info!(
                session_id = %sid,
                count = rows.len(),
                "responses synced"
            );
        }

        info!(session_id = %sid, "session sync complete");
        Ok(())
    }
}
