//! Write-through SQLite persistence for session state, transcripts, and
//! responses.
//!
//! Reference: design doc §25 (State Persistence), `.cursor/rules` flint-data
//! §"Local SQLite" and §"Migration Rules".
//!
//! ## Crash recovery insurance
//!
//! [`write_transcript_chunk`](SessionPersistence::write_transcript_chunk) and
//! [`write_response`](SessionPersistence::write_response) are called on
//! **every** new chunk / response during a live session — not just at session
//! end. On startup, [`load_session_for_recovery`](SessionPersistence::load_session_for_recovery)
//! checks for sessions left in `LIVE`, `ENDING`, or `CRASHED` state; if any
//! are found the orchestrator offers recovery.
//!
//! ## WAL mode
//!
//! `PRAGMA journal_mode = WAL` is set at connection open and verified. This
//! is a hard requirement — DELETE journal mode is never used.

#![allow(dead_code)]

use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use rusqlite::{params, OptionalExtension};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::session::state::{SessionState, StatePersister};

// ──────────────────────────────────────────────────────────────────────────────
// Domain types
// ──────────────────────────────────────────────────────────────────────────────

/// A single chunk of live transcript.
#[derive(Debug, Clone)]
pub struct TranscriptChunk {
    pub id: Uuid,
    pub session_id: Uuid,
    /// `"System"` (interviewer audio) or `"Microphone"` (user's voice).
    pub speaker: String,
    pub text: String,
    /// Wall-clock offset from session start in milliseconds.
    pub timestamp_ms: i64,
}

/// Type of AI response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseType {
    Directional,
    Depth,
    Clarifying,
}

impl ResponseType {
    pub fn as_str(self) -> &'static str {
        match self {
            ResponseType::Directional => "directional",
            ResponseType::Depth => "depth",
            ResponseType::Clarifying => "clarifying",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "directional" => Some(Self::Directional),
            "depth" => Some(Self::Depth),
            "clarifying" => Some(Self::Clarifying),
            _ => None,
        }
    }
}

/// An AI-generated response persisted during a live session.
#[derive(Debug, Clone)]
pub struct Response {
    pub id: Uuid,
    pub session_id: Uuid,
    pub response_type: ResponseType,
    pub content: String,
    /// Confidence score in [0.0, 1.0] (design doc §21).
    pub confidence: f32,
}

/// Data required to resume a session after an unexpected termination.
#[derive(Debug, Clone)]
pub struct RecoveryData {
    pub session_id: Uuid,
    pub state: SessionState,
    pub transcript_chunks: Vec<TranscriptChunk>,
    pub responses: Vec<Response>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Schema
// ──────────────────────────────────────────────────────────────────────────────

/// Schema version stored in `PRAGMA user_version`. Increment when adding
/// columns or tables; the migration runner applies deltas sequentially.
const SCHEMA_VERSION: u32 = 3;

fn run_migrations(conn: &rusqlite::Connection) -> Result<()> {
    let current: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .context("read user_version")?;

    if current < 1 {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS sessions (
                id         TEXT    PRIMARY KEY,
                state      TEXT    NOT NULL DEFAULT 'IDLE',
                created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );

            CREATE TABLE IF NOT EXISTS session_state_transitions (
                id             INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id     TEXT    NOT NULL,
                state          TEXT    NOT NULL,
                transitioned_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );

            CREATE TABLE IF NOT EXISTS transcript_chunks (
                id           TEXT    PRIMARY KEY,
                session_id   TEXT    NOT NULL,
                speaker      TEXT    NOT NULL,
                text         TEXT    NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                created_at   INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE INDEX IF NOT EXISTS idx_tc_session ON transcript_chunks(session_id, created_at);

            CREATE TABLE IF NOT EXISTS responses (
                id            TEXT    PRIMARY KEY,
                session_id    TEXT    NOT NULL,
                response_type TEXT    NOT NULL,
                content       TEXT    NOT NULL,
                confidence    REAL    NOT NULL DEFAULT 0.0,
                created_at    INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE INDEX IF NOT EXISTS idx_resp_session ON responses(session_id, created_at);

            PRAGMA user_version = 1;
            ",
        )
        .context("schema migration v1")?;
        info!("sqlite schema migrated to version 1");
    }

    if current < 2 {
        // Add promotion tracking and explicit 30-day expiry. sessions rows created
        // in v1 get `promoted = 0` and expire 30 days from their `created_at`.
        conn.execute_batch(
            "
            ALTER TABLE sessions ADD COLUMN promoted  INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE sessions ADD COLUMN expires_at INTEGER NOT NULL
                DEFAULT (strftime('%s','now') + 2592000);

            PRAGMA user_version = 2;
            ",
        )
        .context("schema migration v2")?;
        info!("sqlite schema migrated to version 2");
    }

    if current < 3 {
        conn.execute_batch(
            "
            ALTER TABLE sessions ADD COLUMN name         TEXT NOT NULL DEFAULT '';
            ALTER TABLE sessions ADD COLUMN session_type TEXT NOT NULL DEFAULT 'interview';
            ALTER TABLE sessions ADD COLUMN domain       TEXT NOT NULL DEFAULT '';

            PRAGMA user_version = 3;
            ",
        )
        .context("schema migration v3")?;
        info!("sqlite schema migrated to version 3");
    }

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Persistence
// ──────────────────────────────────────────────────────────────────────────────

/// Write-through SQLite persistence for a Flint session.
///
/// `rusqlite::Connection` is `Send` but not `Sync`; wrapping it in `Mutex`
/// makes `SessionPersistence: Send + Sync`, which is required by
/// [`StatePersister`] and `Arc` sharing.
pub struct SessionPersistence {
    db: Mutex<rusqlite::Connection>,
}

impl SessionPersistence {
    /// Open (or create) the local session database at `db_path`.
    /// Pass `":memory:"` for ephemeral use in tests.
    pub fn new(db_path: &str) -> Result<Self> {
        let conn =
            rusqlite::Connection::open(db_path).context("failed to open session database")?;

        // WAL mode: mandatory (never DELETE mode).
        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .context("set journal_mode WAL")?;
        if mode != "wal" {
            warn!(mode = %mode, "WAL mode not active (expected for :memory: in tests)");
        }

        // Foreign-key enforcement.
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .context("enable foreign keys")?;

        run_migrations(&conn)?;

        Ok(Self {
            db: Mutex::new(conn),
        })
    }

    // ── State ────────────────────────────────────────────────────────────────

    /// Persist initial session row with metadata. Called once at session
    /// creation time from `create_session`. Separate from state transitions
    /// because it carries name/type/domain that don't change after creation.
    pub fn create_session_row(
        &self,
        session_id: Uuid,
        name: &str,
        session_type: &str,
        domain: &str,
    ) -> Result<()> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        conn.execute(
            "INSERT INTO sessions (id, state, name, session_type, domain)
             VALUES (?1, 'CONFIGURING', ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 session_type = excluded.session_type,
                 domain = excluded.domain,
                 updated_at = strftime('%s','now')",
            params![sid, name, session_type, domain],
        )
        .context("create session row with metadata")?;
        debug!(session_id = %session_id, name = %name, "session row created");
        Ok(())
    }

    /// Persist a state transition. Creates the session row if it doesn't exist
    /// (UPSERT), updates `sessions.state`, and appends to the audit log.
    ///
    /// Called by the [`StatePersister`] impl on every successful transition.
    pub fn write_state_transition(&self, session_id: Uuid, state: &SessionState) -> Result<()> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        let state_str = state.as_str();

        // Upsert the session row.
        conn.execute(
            "INSERT INTO sessions (id, state) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET
                 state = excluded.state,
                 updated_at = strftime('%s','now')",
            params![sid, state_str],
        )
        .context("upsert sessions row")?;

        // Append to the full transition audit log.
        conn.execute(
            "INSERT INTO session_state_transitions (session_id, state) VALUES (?1, ?2)",
            params![sid, state_str],
        )
        .context("insert session_state_transitions row")?;

        debug!(session_id = %session_id, state = %state_str, "state transition persisted");
        Ok(())
    }

    // ── Transcript ───────────────────────────────────────────────────────────

    /// Persist a transcript chunk. Called on EVERY chunk during a live session
    /// — this is the crash-recovery insurance. Never batch or defer.
    pub fn write_transcript_chunk(&self, chunk: &TranscriptChunk) -> Result<()> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        conn.execute(
            "INSERT OR IGNORE INTO transcript_chunks
                 (id, session_id, speaker, text, timestamp_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                chunk.id.to_string(),
                chunk.session_id.to_string(),
                chunk.speaker,
                chunk.text,
                chunk.timestamp_ms,
            ],
        )
        .context("insert transcript chunk")?;

        debug!(
            session_id = %chunk.session_id,
            chunk_id   = %chunk.id,
            speaker    = %chunk.speaker,
            "transcript chunk persisted",
        );
        Ok(())
    }

    // ── Responses ────────────────────────────────────────────────────────────

    /// Persist an AI response. Called on EVERY response during a live session.
    /// Never batch or defer.
    pub fn write_response(&self, response: &Response) -> Result<()> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        conn.execute(
            "INSERT OR IGNORE INTO responses
                 (id, session_id, response_type, content, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                response.id.to_string(),
                response.session_id.to_string(),
                response.response_type.as_str(),
                response.content,
                response.confidence as f64,
            ],
        )
        .context("insert response")?;

        debug!(
            session_id    = %response.session_id,
            response_id   = %response.id,
            response_type = %response.response_type.as_str(),
            "response persisted",
        );
        Ok(())
    }

    // ── Recovery ─────────────────────────────────────────────────────────────

    /// Return recovery data for the most recently incomplete session, or
    /// `None` if no session is in `LIVE`, `ENDING`, or `CRASHED` state.
    ///
    /// Called on app startup. If `Some(data)` is returned the orchestrator
    /// offers the user the option to resume or discard the session.
    pub fn load_session_for_recovery(&self, session_id: Uuid) -> Result<Option<RecoveryData>> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();

        // Find the session if it is in a recoverable state.
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT id, state FROM sessions
                 WHERE id = ?1
                   AND state IN ('LIVE', 'ENDING', 'CRASHED')
                 LIMIT 1",
                params![sid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .context("query session for recovery")?;

        let (row_id, state_str) = match row {
            None => return Ok(None),
            Some(r) => r,
        };

        let recovered_id = Uuid::parse_str(&row_id).context("parse session_id")?;
        let state = parse_session_state(&state_str)?;

        // Load transcript chunks.
        let mut stmt = conn
            .prepare(
                "SELECT id, speaker, text, timestamp_ms
                 FROM transcript_chunks
                 WHERE session_id = ?1
                 ORDER BY created_at ASC",
            )
            .context("prepare transcript query")?;

        let transcript_chunks = stmt
            .query_map(params![sid], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .context("query transcript chunks")?
            .map(|row| {
                let (id, speaker, text, ts) = row.context("read transcript row")?;
                Ok(TranscriptChunk {
                    id: Uuid::parse_str(&id).context("parse chunk uuid")?,
                    session_id: recovered_id,
                    speaker,
                    text,
                    timestamp_ms: ts,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        // Load responses.
        let mut stmt = conn
            .prepare(
                "SELECT id, response_type, content, confidence
                 FROM responses
                 WHERE session_id = ?1
                 ORDER BY created_at ASC",
            )
            .context("prepare responses query")?;

        let responses = stmt
            .query_map(params![sid], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, f64>(3)?,
                ))
            })
            .context("query responses")?
            .map(|row| {
                let (id, rt, content, conf) = row.context("read response row")?;
                let response_type = ResponseType::from_str(&rt)
                    .ok_or_else(|| anyhow::anyhow!("unknown response_type: {rt}"))?;
                Ok(Response {
                    id: Uuid::parse_str(&id).context("parse response uuid")?,
                    session_id: recovered_id,
                    response_type,
                    content,
                    confidence: conf as f32,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        info!(
            session_id = %recovered_id,
            state = %state_str,
            transcript_count = transcript_chunks.len(),
            response_count = responses.len(),
            "session loaded for crash recovery",
        );

        Ok(Some(RecoveryData {
            session_id: recovered_id,
            state,
            transcript_chunks,
            responses,
        }))
    }

    /// Load transcript chunks and responses for any session, regardless of
    /// its current state. Used by Supabase sync (after ENDED) and by
    /// `generate_session_summary`.
    ///
    /// Unlike `load_session_for_recovery`, this does NOT filter by state.
    pub fn load_session_data(&self, session_id: Uuid) -> Result<Option<RecoveryData>> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();

        let state_str: Option<String> = conn
            .query_row(
                "SELECT state FROM sessions WHERE id = ?1 LIMIT 1",
                params![sid],
                |r| r.get(0),
            )
            .optional()
            .context("query session by id")?;

        let state_str = match state_str {
            None => return Ok(None),
            Some(s) => s,
        };

        let state = parse_session_state(&state_str)?;

        let mut stmt = conn
            .prepare(
                "SELECT id, speaker, text, timestamp_ms
                 FROM transcript_chunks
                 WHERE session_id = ?1
                 ORDER BY created_at ASC",
            )
            .context("prepare transcript query (load_session_data)")?;

        let transcript_chunks = stmt
            .query_map(params![sid], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .context("query transcript chunks (load_session_data)")?
            .map(|row| {
                let (id, speaker, text, ts) = row.context("read transcript row")?;
                Ok(TranscriptChunk {
                    id: Uuid::parse_str(&id).context("parse chunk uuid")?,
                    session_id,
                    speaker,
                    text,
                    timestamp_ms: ts,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let mut stmt = conn
            .prepare(
                "SELECT id, response_type, content, confidence
                 FROM responses
                 WHERE session_id = ?1
                 ORDER BY created_at ASC",
            )
            .context("prepare responses query (load_session_data)")?;

        let responses = stmt
            .query_map(params![sid], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, f64>(3)?,
                ))
            })
            .context("query responses (load_session_data)")?
            .map(|row| {
                let (id, rt, content, conf) = row.context("read response row")?;
                let response_type = ResponseType::from_str(&rt)
                    .ok_or_else(|| anyhow::anyhow!("unknown response_type: {rt}"))?;
                Ok(Response {
                    id: Uuid::parse_str(&id).context("parse response uuid")?,
                    session_id,
                    response_type,
                    content,
                    confidence: conf as f32,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Some(RecoveryData {
            session_id,
            state,
            transcript_chunks,
            responses,
        }))
    }

    /// Find any incomplete session (LIVE/ENDING/CRASHED) regardless of ID.
    ///
    /// Used on app startup to detect crash-interrupted sessions proactively.
    pub fn find_incomplete_session(&self) -> Result<Option<Uuid>> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let row: Option<String> = conn
            .query_row(
                "SELECT id FROM sessions
                 WHERE state IN ('LIVE', 'ENDING', 'CRASHED')
                 ORDER BY updated_at DESC
                 LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .context("find incomplete session")?;

        row.map(|s| Uuid::parse_str(&s).context("parse session uuid from DB"))
            .transpose()
    }

    /// List all sessions in the database, most recent first.
    ///
    /// Returns lightweight rows suitable for the SessionList screen — no
    /// transcript or response data is loaded.
    pub fn list_sessions(&self) -> Result<Vec<crate::dto::SessionSummaryDto>> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let now = chrono::Utc::now().timestamp();

        let mut stmt = conn
            .prepare(
                "SELECT id, state, created_at, expires_at, promoted, name, session_type, domain
                 FROM sessions
                 ORDER BY created_at DESC",
            )
            .context("prepare list_sessions")?;

        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                ))
            })
            .context("query list_sessions")?
            .map(|row| {
                let (id, state, created_at, expires_at, promoted, name, session_type, domain) =
                    row.context("read sessions row")?;
                Ok(crate::dto::SessionSummaryDto {
                    id,
                    state,
                    created_at,
                    expires_in_secs: expires_at - now,
                    promoted: promoted != 0,
                    name,
                    session_type,
                    domain,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(rows)
    }

    /// Mark a session as promoted so it is exempt from the 30-day auto-expiry.
    pub fn promote_session(&self, session_id: Uuid) -> Result<()> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        conn.execute(
            "UPDATE sessions SET promoted = 1 WHERE id = ?1",
            params![sid],
        )
        .context("promote session")?;
        debug!(session_id = %session_id, "session promoted");
        Ok(())
    }

    /// Delete all persisted data for `session_id`.
    ///
    /// Called after a session is successfully ended and synced, or when the
    /// user discards a recovery.
    pub fn clear_session(&self, session_id: Uuid) -> Result<()> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();

        conn.execute(
            "DELETE FROM transcript_chunks WHERE session_id = ?1",
            params![sid],
        )
        .context("delete transcript chunks")?;
        conn.execute("DELETE FROM responses WHERE session_id = ?1", params![sid])
            .context("delete responses")?;
        conn.execute(
            "DELETE FROM session_state_transitions WHERE session_id = ?1",
            params![sid],
        )
        .context("delete state transitions")?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![sid])
            .context("delete session")?;

        debug!(session_id = %session_id, "session data cleared");
        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// StatePersister implementation
// ──────────────────────────────────────────────────────────────────────────────

impl StatePersister for SessionPersistence {
    fn write_state_transition(&self, session_id: Uuid, state: SessionState) -> Result<()> {
        // Delegate to the inherent method (same logic, just copy semantics).
        SessionPersistence::write_state_transition(self, session_id, &state)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

fn parse_session_state(s: &str) -> Result<SessionState> {
    match s {
        "IDLE" => Ok(SessionState::Idle),
        "CONFIGURING" => Ok(SessionState::Configuring),
        "INGESTING" => Ok(SessionState::Ingesting),
        "DIGEST_REVIEW" => Ok(SessionState::DigestReview),
        "PRE_WARMING" => Ok(SessionState::PreWarming),
        "REHEARSING" => Ok(SessionState::Rehearsing),
        "READY" => Ok(SessionState::Ready),
        "LIVE" => Ok(SessionState::Live),
        "PAUSED" => Ok(SessionState::Paused),
        "ENDING" => Ok(SessionState::Ending),
        "ENDED" => Ok(SessionState::Ended),
        "CRASHED" => Ok(SessionState::Crashed),
        "RECOVERING" => Ok(SessionState::Recovering),
        other => bail!("unknown session state in database: {other:?}"),
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn new_db() -> SessionPersistence {
        SessionPersistence::new(":memory:").expect("open :memory: db")
    }

    fn sample_chunk(session_id: Uuid, ts: i64, text: &str) -> TranscriptChunk {
        TranscriptChunk {
            id: Uuid::new_v4(),
            session_id,
            speaker: "System".to_string(),
            text: text.to_string(),
            timestamp_ms: ts,
        }
    }

    fn sample_response(session_id: Uuid) -> Response {
        Response {
            id: Uuid::new_v4(),
            session_id,
            response_type: ResponseType::Directional,
            content: "A concise directional answer.".to_string(),
            confidence: 0.85,
        }
    }

    // ── State persistence ────────────────────────────────────────────────────

    #[test]
    fn test_write_state_transition_creates_session_row() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.write_state_transition(sid, &SessionState::Configuring)
            .unwrap();

        let conn = db.db.lock().unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM sessions WHERE id = ?1",
                params![sid.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "CONFIGURING");
    }

    #[test]
    fn test_write_state_transition_updates_existing_row() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.write_state_transition(sid, &SessionState::Configuring)
            .unwrap();
        db.write_state_transition(sid, &SessionState::Ingesting)
            .unwrap();
        db.write_state_transition(sid, &SessionState::DigestReview)
            .unwrap();

        let conn = db.db.lock().unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM sessions WHERE id = ?1",
                params![sid.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "DIGEST_REVIEW");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_state_transitions WHERE session_id = ?1",
                params![sid.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 3, "all 3 transitions must be in the audit log");
    }

    #[test]
    fn test_state_persister_trait_impl() {
        let db = new_db();
        let sid = Uuid::new_v4();
        let persister: &dyn StatePersister = &db;
        persister
            .write_state_transition(sid, SessionState::Ready)
            .unwrap();

        let conn = db.db.lock().unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM sessions WHERE id = ?1",
                params![sid.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "READY");
    }

    // ── Transcript chunks ────────────────────────────────────────────────────

    #[test]
    fn test_write_transcript_chunk_persists() {
        let db = new_db();
        let sid = Uuid::new_v4();
        let chunk = sample_chunk(sid, 1000, "Tell me about yourself.");
        db.write_transcript_chunk(&chunk).unwrap();

        let conn = db.db.lock().unwrap();
        let text: String = conn
            .query_row(
                "SELECT text FROM transcript_chunks WHERE id = ?1",
                params![chunk.id.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(text, "Tell me about yourself.");
    }

    #[test]
    fn test_write_multiple_chunks() {
        let db = new_db();
        let sid = Uuid::new_v4();
        for i in 0..3 {
            db.write_transcript_chunk(&sample_chunk(sid, i * 1000, &format!("chunk {i}")))
                .unwrap();
        }
        let conn = db.db.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transcript_chunks WHERE session_id = ?1",
                params![sid.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 3);
    }

    // ── Responses ────────────────────────────────────────────────────────────

    #[test]
    fn test_write_response_persists() {
        let db = new_db();
        let sid = Uuid::new_v4();
        let resp = sample_response(sid);
        db.write_response(&resp).unwrap();

        let conn = db.db.lock().unwrap();
        let content: String = conn
            .query_row(
                "SELECT content FROM responses WHERE id = ?1",
                params![resp.id.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(content, "A concise directional answer.");
    }

    // ── Crash simulation ─────────────────────────────────────────────────────

    /// Simulates a crash: write 3 chunks, drop the connection, re-open the DB,
    /// and assert all 3 chunks are still present.
    #[test]
    fn test_crash_simulation_data_survives() {
        use std::path::PathBuf;

        let dir = std::env::temp_dir();
        let db_path: PathBuf = dir.join(format!("flint_crash_test_{}.sqlite", Uuid::new_v4()));
        let db_path_str = db_path.to_str().unwrap();

        let session_id = Uuid::new_v4();
        {
            let db = SessionPersistence::new(db_path_str).unwrap();
            db.write_state_transition(session_id, &SessionState::Live)
                .unwrap();
            for i in 0..3 {
                db.write_transcript_chunk(&sample_chunk(
                    session_id,
                    i * 500,
                    &format!("chunk {i}"),
                ))
                .unwrap();
            }
            // db is dropped here — simulating a crash (connection closes).
        }

        // Re-open and verify.
        let db2 = SessionPersistence::new(db_path_str).unwrap();
        let conn = db2.db.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transcript_chunks WHERE session_id = ?1",
                params![session_id.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 3, "all 3 chunks must survive a connection drop");

        // Clean up temp file.
        drop(conn);
        let _ = std::fs::remove_file(db_path);
    }

    // ── Recovery ─────────────────────────────────────────────────────────────

    #[test]
    fn test_load_session_for_recovery_returns_none_for_clean_session() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.write_state_transition(sid, &SessionState::Ended)
            .unwrap();
        let result = db.load_session_for_recovery(sid).unwrap();
        assert!(result.is_none(), "ENDED session must not trigger recovery");
    }

    #[test]
    fn test_load_session_for_recovery_returns_data_for_live_session() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.write_state_transition(sid, &SessionState::Live).unwrap();
        db.write_transcript_chunk(&sample_chunk(sid, 100, "a question"))
            .unwrap();
        db.write_transcript_chunk(&sample_chunk(sid, 200, "another"))
            .unwrap();
        db.write_response(&sample_response(sid)).unwrap();

        let recovery = db.load_session_for_recovery(sid).unwrap();
        let data = recovery.expect("LIVE session must return RecoveryData");

        assert_eq!(data.session_id, sid);
        assert_eq!(data.state, SessionState::Live);
        assert_eq!(data.transcript_chunks.len(), 2);
        assert_eq!(data.responses.len(), 1);
    }

    #[test]
    fn test_load_session_for_recovery_covers_crashed_state() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.write_state_transition(sid, &SessionState::Crashed)
            .unwrap();
        let result = db.load_session_for_recovery(sid).unwrap();
        assert!(result.is_some(), "CRASHED session must trigger recovery");
    }

    #[test]
    fn test_load_session_for_recovery_covers_ending_state() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.write_state_transition(sid, &SessionState::Ending)
            .unwrap();
        let result = db.load_session_for_recovery(sid).unwrap();
        assert!(result.is_some(), "ENDING session must trigger recovery");
    }

    // ── Clear session ────────────────────────────────────────────────────────

    #[test]
    fn test_clear_session_removes_all_data() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.write_state_transition(sid, &SessionState::Live).unwrap();
        db.write_transcript_chunk(&sample_chunk(sid, 100, "text"))
            .unwrap();
        db.write_response(&sample_response(sid)).unwrap();

        db.clear_session(sid).unwrap();

        let result = db.load_session_for_recovery(sid).unwrap();
        assert!(
            result.is_none(),
            "cleared session must return None for recovery"
        );

        let conn = db.db.lock().unwrap();
        let tc: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transcript_chunks WHERE session_id = ?1",
                params![sid.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tc, 0);
        let rc: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM responses WHERE session_id = ?1",
                params![sid.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rc, 0);
    }

    // ── find_incomplete_session ──────────────────────────────────────────────

    #[test]
    fn test_find_incomplete_session_returns_live_session_id() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.write_state_transition(sid, &SessionState::Live).unwrap();
        let found = db.find_incomplete_session().unwrap();
        assert_eq!(found, Some(sid));
    }

    #[test]
    fn test_find_incomplete_session_returns_none_when_all_clean() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.write_state_transition(sid, &SessionState::Ended)
            .unwrap();
        let found = db.find_incomplete_session().unwrap();
        assert_eq!(found, None);
    }

    // ── WAL mode ─────────────────────────────────────────────────────────────

    #[test]
    fn test_wal_mode_is_set_on_file_db() {
        use std::path::PathBuf;

        let dir = std::env::temp_dir();
        let db_path: PathBuf = dir.join(format!("flint_wal_test_{}.sqlite", Uuid::new_v4()));

        {
            let db = SessionPersistence::new(db_path.to_str().unwrap()).unwrap();
            let conn = db.db.lock().unwrap();
            let mode: String = conn
                .query_row("PRAGMA journal_mode", [], |r| r.get(0))
                .unwrap();
            assert_eq!(mode, "wal", "WAL mode must be active on file-backed DB");
        }
        let _ = std::fs::remove_file(db_path);
    }
}
