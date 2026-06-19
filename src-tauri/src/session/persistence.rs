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

use crate::digest::Digest;
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

/// Latest practice outcome for a question within a session.
#[derive(Debug, Clone)]
pub struct QuestionAttempt {
    pub question: String,
    pub question_key: String,
    pub last_source: String,
    pub confidence_score: f32,
    pub coach_score: u8,
    pub satisfied: bool,
    /// User-tailored script used in Live (and as rehearsal anchor when set).
    pub preferred_answer: String,
}

/// One turn in a mock interview session.
#[derive(Debug, Clone)]
pub struct MockTurn {
    pub id: Uuid,
    pub session_id: Uuid,
    pub turn_n: u32,
    pub question: String,
    /// Speech-to-text transcript of the user's spoken answer.
    pub user_text: String,
    /// Absolute local path to the per-turn WAV file. Empty until recording ends.
    pub audio_path: String,
    /// Serialised `CoachFeedback` JSON. Empty `{}` until coach completes.
    pub coach_json: String,
    /// Full suggested answer text (single merged panel). Empty until LLM responds.
    pub suggested: String,
    /// Coach score 0–100. 0 until coach completes.
    pub score: u8,
}

/// Session-level interview focus (rehearsal / mock filtering — never live).
#[derive(Debug, Clone, Default)]
pub struct SessionFocus {
    pub focus_name: String,
    pub focus_tags: Vec<String>,
    pub recruiter_brief: String,
    pub focus_notes: String,
    /// Unix epoch seconds when user confirmed focus, or None if never set.
    pub focus_confirmed_at: Option<i64>,
    /// After live session ends, prompt user to refresh focus before rehearsal.
    pub needs_focus_refresh: bool,
}

/// Lightweight metadata returned alongside the recovery offer so the user
/// can decide whether to resume or discard.
#[derive(Debug, Clone)]
pub struct SessionRecoverySummary {
    pub session_id: Uuid,
    /// Unix epoch seconds.
    pub created_at: i64,
    pub name: String,
    pub session_type: String,
    pub domain: String,
    /// Wall-clock offset of the most recent transcript chunk in ms, or
    /// `None` if the session crashed before any audio was captured.
    pub last_chunk_timestamp_ms: Option<i64>,
}

/// All structured context fields entered on the Session Design screen.
///
/// Each field maps 1-to-1 to a SQLite column so draft restore can repopulate
/// the form without parsing the assembled RAG blob.
#[derive(Debug, Clone, Default)]
pub struct SessionContextFields {
    pub job_description: String,
    pub profile: String,
    pub company_overview: String,
    pub leadership_principles: String,
    pub role_expectations: String,
    pub technical_prep: String,
    pub strategy_notes: String,
}

/// Metadata for an in-progress session setup draft (pre-live).
#[derive(Debug, Clone)]
pub struct DraftSessionMetadata {
    pub session_id: Uuid,
    pub state: SessionState,
    pub name: String,
    pub session_type: String,
    pub domain: String,
    /// Assembled RAG blob (kept for backward compat / clone path).
    pub context_text: String,
    /// Structured fields; all empty for sessions created before v6 migration.
    pub context_fields: SessionContextFields,
}

// ──────────────────────────────────────────────────────────────────────────────
// Schema
// ──────────────────────────────────────────────────────────────────────────────

/// Schema version stored in `PRAGMA user_version`. Increment when adding
/// columns or tables; the migration runner applies deltas sequentially.
const SCHEMA_VERSION: u32 = 13;

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

    if current < 4 {
        // Store the raw context text so sessions can be cloned with the same
        // context pre-filled. Existing sessions default to empty string.
        conn.execute_batch(
            "
            ALTER TABLE sessions ADD COLUMN context_text TEXT NOT NULL DEFAULT '';

            PRAGMA user_version = 4;
            ",
        )
        .context("schema migration v4")?;
        info!("sqlite schema migrated to version 4");
    }

    if current < 5 {
        conn.execute_batch(
            "
            ALTER TABLE sessions ADD COLUMN digest_json TEXT NOT NULL DEFAULT '';

            PRAGMA user_version = 5;
            ",
        )
        .context("schema migration v5")?;
        info!("sqlite schema migrated to version 5");
    }

    if current < 6 {
        // Phase 5.5.1 — structured Session Design fields.
        // Each field is stored as its own column so draft restore can repopulate
        // the form precisely. The assembled RAG blob continues to live in
        // `context_text` for the embedding pipeline and backward compat.
        conn.execute_batch(
            "
            ALTER TABLE sessions ADD COLUMN job_description       TEXT NOT NULL DEFAULT '';
            ALTER TABLE sessions ADD COLUMN profile               TEXT NOT NULL DEFAULT '';
            ALTER TABLE sessions ADD COLUMN company_overview      TEXT NOT NULL DEFAULT '';
            ALTER TABLE sessions ADD COLUMN leadership_principles TEXT NOT NULL DEFAULT '';
            ALTER TABLE sessions ADD COLUMN role_expectations     TEXT NOT NULL DEFAULT '';
            ALTER TABLE sessions ADD COLUMN technical_prep        TEXT NOT NULL DEFAULT '';
            ALTER TABLE sessions ADD COLUMN strategy_notes        TEXT NOT NULL DEFAULT '';

            PRAGMA user_version = 6;
            ",
        )
        .context("schema migration v6")?;
        info!("sqlite schema migrated to version 6");
    }

    if current < 7 {
        // Phase 5.5.3 — question bank persisted per session.
        // Stored as a JSON array of strings so the Rehearsal UI can add/remove
        // questions without a column-per-question schema.
        conn.execute_batch(
            "
            ALTER TABLE sessions ADD COLUMN question_bank_json TEXT NOT NULL DEFAULT '[]';

            PRAGMA user_version = 7;
            ",
        )
        .context("schema migration v7")?;
        info!("sqlite schema migrated to version 7");
    }

    if current < 8 {
        // Mock Interview — one row per turn storing the AI question, user's
        // transcript, path to the per-turn WAV, coach feedback JSON, and the
        // suggested answer text.  Audio files live outside the DB so they can
        // be pruned independently; the path is absolute and local.
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS mock_turns (
                id           TEXT    PRIMARY KEY,
                session_id   TEXT    NOT NULL,
                turn_n       INTEGER NOT NULL,
                question     TEXT    NOT NULL,
                user_text    TEXT    NOT NULL DEFAULT '',
                audio_path   TEXT    NOT NULL DEFAULT '',
                coach_json   TEXT    NOT NULL DEFAULT '{}',
                suggested    TEXT    NOT NULL DEFAULT '',
                score        INTEGER NOT NULL DEFAULT 0,
                created_at   INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE INDEX IF NOT EXISTS idx_mt_session ON mock_turns(session_id, turn_n);

            PRAGMA user_version = 8;
            ",
        )
        .context("schema migration v8")?;
        info!("sqlite schema migrated to version 8");
    }

    if current < 9 {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS question_attempts (
                session_id        TEXT    NOT NULL,
                question_key      TEXT    NOT NULL,
                question          TEXT    NOT NULL,
                last_source       TEXT    NOT NULL DEFAULT 'rehearsal',
                confidence_score  REAL    NOT NULL DEFAULT 0,
                coach_score       INTEGER NOT NULL DEFAULT 0,
                satisfied         INTEGER NOT NULL DEFAULT 0,
                updated_at        INTEGER NOT NULL DEFAULT (strftime('%s','now')),
                PRIMARY KEY (session_id, question_key)
            );
            CREATE INDEX IF NOT EXISTS idx_qa_session ON question_attempts(session_id);

            PRAGMA user_version = 9;
            ",
        )
        .context("schema migration v9")?;
        info!("sqlite schema migrated to version 9");
    }

    if current < 10 {
        dedupe_mock_turns(conn)?;
        conn.execute_batch(
            "
            CREATE UNIQUE INDEX IF NOT EXISTS uq_mock_turns_session_turn
                ON mock_turns(session_id, turn_n);

            PRAGMA user_version = 10;
            ",
        )
        .context("schema migration v10")?;
        info!("sqlite schema migrated to version 10");
    }

    if current < 11 {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS app_preferences (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            PRAGMA user_version = 11;
            ",
        )
        .context("schema migration v11")?;
        info!("sqlite schema migrated to version 11");
    }

    if current < 12 {
        conn.execute_batch(
            "
            ALTER TABLE question_attempts
                ADD COLUMN preferred_answer TEXT NOT NULL DEFAULT '';

            PRAGMA user_version = 12;
            ",
        )
        .context("schema migration v12")?;
        info!("sqlite schema migrated to version 12");
    }

    if current < 13 {
        conn.execute_batch(
            "
            ALTER TABLE sessions ADD COLUMN focus_name TEXT NOT NULL DEFAULT '';
            ALTER TABLE sessions ADD COLUMN focus_tags_json TEXT NOT NULL DEFAULT '[]';
            ALTER TABLE sessions ADD COLUMN recruiter_brief TEXT NOT NULL DEFAULT '';
            ALTER TABLE sessions ADD COLUMN focus_notes TEXT NOT NULL DEFAULT '';
            ALTER TABLE sessions ADD COLUMN focus_confirmed_at INTEGER;
            ALTER TABLE sessions ADD COLUMN needs_focus_refresh INTEGER NOT NULL DEFAULT 0;

            PRAGMA user_version = 13;
            ",
        )
        .context("schema migration v13")?;
        info!("sqlite schema migrated to version 13");
    }

    Ok(())
}

/// Merge duplicate `mock_turns` rows (same session + turn_n) before adding the
/// unique index. Keeps the richest field values from each duplicate.
fn dedupe_mock_turns(conn: &rusqlite::Connection) -> Result<()> {
    let mut stmt = conn
        .prepare(
            "SELECT id, session_id, turn_n, question, user_text,
                    audio_path, coach_json, suggested, score
             FROM mock_turns ORDER BY turn_n ASC",
        )
        .context("prepare mock_turns dedupe")?;
    let rows: Vec<MockTurn> = stmt
        .query_map([], |r| {
            Ok(MockTurn {
                id: Uuid::parse_str(&r.get::<_, String>(0)?).unwrap_or_default(),
                session_id: Uuid::parse_str(&r.get::<_, String>(1)?).unwrap_or_default(),
                turn_n: r.get(2)?,
                question: r.get(3)?,
                user_text: r.get(4)?,
                audio_path: r.get(5)?,
                coach_json: r.get(6)?,
                suggested: r.get(7)?,
                score: r.get(8)?,
            })
        })
        .context("query mock_turns for dedupe")?
        .collect::<Result<Vec<_>, _>>()
        .context("collect mock_turns for dedupe")?;

    if rows.is_empty() {
        return Ok(());
    }

    let merged = merge_mock_turn_rows(rows);
    conn.execute("DELETE FROM mock_turns", [])
        .context("clear mock_turns for dedupe")?;
    for turn in &merged {
        conn.execute(
            "INSERT INTO mock_turns
             (id, session_id, turn_n, question, user_text, audio_path, coach_json, suggested, score)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                turn.id.to_string(),
                turn.session_id.to_string(),
                turn.turn_n,
                turn.question,
                turn.user_text,
                turn.audio_path,
                turn.coach_json,
                turn.suggested,
                turn.score,
            ],
        )
        .context("reinsert deduped mock turn")?;
    }
    Ok(())
}

fn merge_mock_turn_fields(acc: &mut MockTurn, other: &MockTurn) {
    if other.audio_path.len() > acc.audio_path.len() {
        acc.audio_path.clone_from(&other.audio_path);
        acc.id = other.id;
    }
    if other.coach_json.len() > acc.coach_json.len() {
        acc.coach_json.clone_from(&other.coach_json);
    }
    if other.score > acc.score {
        acc.score = other.score;
    }
    if other.user_text.len() > acc.user_text.len() {
        acc.user_text.clone_from(&other.user_text);
    }
    if other.suggested.len() > acc.suggested.len() {
        acc.suggested.clone_from(&other.suggested);
    }
    if acc.question.is_empty() && !other.question.is_empty() {
        acc.question.clone_from(&other.question);
    }
}

fn merge_mock_turn_rows(rows: Vec<MockTurn>) -> Vec<MockTurn> {
    use std::collections::BTreeMap;
    let mut by_key: BTreeMap<(Uuid, u32), MockTurn> = BTreeMap::new();
    for row in rows {
        let key = (row.session_id, row.turn_n);
        by_key
            .entry(key)
            .and_modify(|m| merge_mock_turn_fields(m, &row))
            .or_insert(row);
    }
    by_key.into_values().collect()
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
    ///
    /// Phase 7.5 hardening:
    /// * `PRAGMA synchronous = FULL` on file-backed DBs — fsync on every
    ///   commit, so a kill-9 or power loss between two `write_response`
    ///   calls cannot lose more than the in-flight statement.
    /// * `PRAGMA integrity_check` runs immediately after migrations. A
    ///   corrupted file produces a hard error with the full sqlite report
    ///   so the user can be guided to reset local data.
    /// * `PRAGMA user_version` is rejected when greater than
    ///   [`SCHEMA_VERSION`] — that means the file came from a newer Flint
    ///   build and downgrading would silently drop columns.
    pub fn new(db_path: &str) -> Result<Self> {
        let conn =
            rusqlite::Connection::open(db_path).context("failed to open session database")?;

        // WAL mode: mandatory (never DELETE mode).
        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .context("set journal_mode WAL")?;
        let is_in_memory = db_path == ":memory:";
        if mode != "wal" && !is_in_memory {
            warn!(mode = %mode, "WAL mode not active on file-backed DB");
        }

        // Durability fence for crash recovery. `FULL` syncs the WAL on every
        // commit; `NORMAL` (the default) can lose the last commit on power
        // loss. In-memory DBs ignore the pragma but tolerate it cleanly.
        if !is_in_memory {
            conn.execute_batch("PRAGMA synchronous = FULL;")
                .context("set synchronous = FULL")?;
        }

        // Foreign-key enforcement.
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .context("enable foreign keys")?;

        verify_user_version_compatible(&conn)?;
        run_migrations(&conn)?;
        verify_integrity(&conn).context("integrity check")?;

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

    const PREF_PRIMARY_PROVIDER: &'static str = "preferred_primary_provider";

    /// Read the user's preferred primary LLM provider (`groq`, `openai`, etc.).
    pub fn get_preferred_primary_provider(&self) -> Result<Option<String>> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let mut stmt = conn
            .prepare("SELECT value FROM app_preferences WHERE key = ?1")
            .context("prepare preferred_primary_provider read")?;
        let mut rows = stmt
            .query(params![Self::PREF_PRIMARY_PROVIDER])
            .context("query preferred_primary_provider")?;
        if let Some(row) = rows
            .next()
            .context("fetch preferred_primary_provider row")?
        {
            return Ok(Some(
                row.get(0)
                    .context("read preferred_primary_provider value")?,
            ));
        }
        Ok(None)
    }

    /// Persist the user's preferred primary LLM provider for new sessions.
    pub fn set_preferred_primary_provider(&self, provider: &str) -> Result<()> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        conn.execute(
            "INSERT INTO app_preferences (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![Self::PREF_PRIMARY_PROVIDER, provider],
        )
        .context("write preferred_primary_provider")?;
        Ok(())
    }

    /// Persist a state transition. Creates the session row if it doesn't exist
    /// (UPSERT), updates `sessions.state`, and appends to the audit log.
    ///
    /// Phase 7.5: both writes happen inside a single transaction so a crash
    /// between them cannot leave `sessions.state` out of sync with the audit
    /// log. With WAL + `synchronous = FULL` this gives all-or-nothing
    /// durability per transition — the contract `SessionStateMachine`
    /// depends on.
    pub fn write_state_transition(&self, session_id: Uuid, state: &SessionState) -> Result<()> {
        let mut conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        let state_str = state.as_str();

        let tx = conn.transaction().context("begin state transition tx")?;

        tx.execute(
            "INSERT INTO sessions (id, state) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET
                 state = excluded.state,
                 updated_at = strftime('%s','now')",
            params![sid, state_str],
        )
        .context("upsert sessions row")?;

        tx.execute(
            "INSERT INTO session_state_transitions (session_id, state) VALUES (?1, ?2)",
            params![sid, state_str],
        )
        .context("insert session_state_transitions row")?;

        tx.commit().context("commit state transition tx")?;

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
    /// `None` if no session is in a recoverable state.
    ///
    /// Recoverable states are `LIVE`, `ENDING`, `CRASHED`, and `RECOVERING`.
    /// `RECOVERING` is included so that crashing during a recovery flow
    /// re-offers the same session on the next launch instead of stranding
    /// it forever.
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
                   AND state IN ('LIVE', 'ENDING', 'CRASHED', 'RECOVERING')
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

    /// Find the most recent incomplete session (LIVE/ENDING/CRASHED/
    /// RECOVERING).
    ///
    /// Used on app startup to detect crash-interrupted sessions proactively.
    /// Older incomplete sessions are still in the database; use
    /// [`Self::list_incomplete_sessions`] to inspect or batch-clean them.
    ///
    /// `rowid` ties `updated_at` so that two sessions written in the same
    /// second still resolve deterministically — the last insert wins. This
    /// matters after `mark_stale_sessions_as_crashed` because it rewrites
    /// every stale row's `updated_at` in a single transaction.
    pub fn find_incomplete_session(&self) -> Result<Option<Uuid>> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let row: Option<String> = conn
            .query_row(
                "SELECT id FROM sessions
                 WHERE state IN ('LIVE', 'ENDING', 'CRASHED', 'RECOVERING')
                 ORDER BY updated_at DESC, rowid DESC
                 LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .context("find incomplete session")?;

        row.map(|s| Uuid::parse_str(&s).context("parse session uuid from DB"))
            .transpose()
    }

    /// Return every incomplete session, most recent first. Used by startup
    /// hardening to mark stale sessions as CRASHED before offering recovery.
    pub fn list_incomplete_sessions(&self) -> Result<Vec<Uuid>> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id FROM sessions
                 WHERE state IN ('LIVE', 'ENDING', 'CRASHED', 'RECOVERING')
                 ORDER BY updated_at DESC, rowid DESC",
            )
            .context("prepare list_incomplete_sessions")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .context("query list_incomplete_sessions")?
            .map(|row| {
                let id = row.context("read incomplete session row")?;
                Uuid::parse_str(&id).context("parse session uuid")
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Force any session in `LIVE`, `ENDING`, or `RECOVERING` to `CRASHED`.
    ///
    /// Returns the IDs that were flipped. Called once at startup so the
    /// recovery surface only ever has to deal with the `CRASHED` state. Each
    /// flip is recorded in `session_state_transitions` so the audit log
    /// reflects the post-crash truth.
    pub fn mark_stale_sessions_as_crashed(&self) -> Result<Vec<Uuid>> {
        let mut conn = self.db.lock().expect("session persistence mutex poisoned");
        let tx = conn
            .transaction()
            .context("begin mark_stale_sessions_as_crashed tx")?;

        let mut stale: Vec<String> = Vec::new();
        {
            let mut stmt = tx
                .prepare(
                    "SELECT id FROM sessions
                     WHERE state IN ('LIVE', 'ENDING', 'RECOVERING')",
                )
                .context("prepare select stale sessions")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .context("query stale sessions")?;
            for row in rows {
                stale.push(row.context("read stale session row")?);
            }
        }

        for sid in &stale {
            tx.execute(
                "UPDATE sessions SET state = 'CRASHED', updated_at = strftime('%s','now')
                 WHERE id = ?1",
                params![sid],
            )
            .context("flip stale session to CRASHED")?;
            tx.execute(
                "INSERT INTO session_state_transitions (session_id, state)
                 VALUES (?1, 'CRASHED')",
                params![sid],
            )
            .context("audit crashed transition")?;
        }

        tx.commit()
            .context("commit mark_stale_sessions_as_crashed")?;

        let parsed: Result<Vec<Uuid>> = stale
            .iter()
            .map(|s| Uuid::parse_str(s).context("parse stale session uuid"))
            .collect();
        let ids = parsed?;
        if !ids.is_empty() {
            info!(
                count = ids.len(),
                "stale sessions flipped to CRASHED at startup"
            );
        }
        Ok(ids)
    }

    /// Lightweight metadata for the recovery offer — used to give the user
    /// enough context to decide whether to resume or discard.
    pub fn session_summary(&self, session_id: Uuid) -> Result<Option<SessionRecoverySummary>> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();

        let summary: Option<(i64, String, String, String)> = conn
            .query_row(
                "SELECT created_at, name, session_type, domain FROM sessions WHERE id = ?1",
                params![sid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .context("query session summary")?;
        let (created_at, name, session_type, domain) = match summary {
            Some(s) => s,
            None => return Ok(None),
        };

        let last_chunk_ms: Option<i64> = conn
            .query_row(
                "SELECT MAX(timestamp_ms) FROM transcript_chunks WHERE session_id = ?1",
                params![sid],
                |r| r.get(0),
            )
            .optional()
            .context("query last chunk timestamp")?
            .flatten();

        Ok(Some(SessionRecoverySummary {
            session_id,
            created_at,
            name,
            session_type,
            domain,
            last_chunk_timestamp_ms: last_chunk_ms,
        }))
    }

    /// Count sessions that count toward the concurrent open-session cap.
    ///
    /// Open = any state other than `IDLE` or `ENDED`. See `session::limits`.
    pub fn count_open_sessions(&self) -> Result<usize> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions
                 WHERE state NOT IN ('IDLE', 'ENDED')",
                [],
                |r| r.get(0),
            )
            .context("count open sessions")?;
        Ok(count as usize)
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

    /// Remove the promoted flag from a session so it resumes normal 30-day expiry.
    pub fn demote_session(&self, session_id: Uuid) -> Result<()> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        conn.execute(
            "UPDATE sessions SET promoted = 0 WHERE id = ?1",
            params![sid],
        )
        .context("demote session")?;
        debug!(session_id = %session_id, "session demoted");
        Ok(())
    }

    /// Persist the raw context text supplied during ingest so it can be
    /// retrieved later when the user wants to clone the session.
    pub fn store_context_text(&self, session_id: Uuid, text: &str) -> Result<()> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        conn.execute(
            "UPDATE sessions SET context_text = ?1 WHERE id = ?2",
            params![text, sid],
        )
        .context("store context text")?;
        debug!(session_id = %session_id, "context text stored");
        Ok(())
    }

    /// Persist each structured Session Design field in its own SQLite column.
    ///
    /// Called by `ingest_structured_context` after the RAG blob is assembled so
    /// draft restore can repopulate the form exactly as the user left it.
    /// Also bumps `updated_at` so `find_draft_session` ordering stays accurate.
    pub fn store_context_fields(
        &self,
        session_id: Uuid,
        fields: &SessionContextFields,
    ) -> Result<()> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        let rows = conn
            .execute(
                "UPDATE sessions SET
                     job_description       = ?1,
                     profile               = ?2,
                     company_overview      = ?3,
                     leadership_principles = ?4,
                     role_expectations     = ?5,
                     technical_prep        = ?6,
                     strategy_notes        = ?7,
                     updated_at            = strftime('%s','now')
                 WHERE id = ?8",
                rusqlite::params![
                    fields.job_description,
                    fields.profile,
                    fields.company_overview,
                    fields.leadership_principles,
                    fields.role_expectations,
                    fields.technical_prep,
                    fields.strategy_notes,
                    sid,
                ],
            )
            .context("store context fields")?;
        if rows == 0 {
            anyhow::bail!("store_context_fields: session {session_id} not found in database");
        }
        debug!(session_id = %session_id, "structured context fields stored");
        Ok(())
    }

    /// Load structured Session Design fields from SQLite.
    ///
    /// All fields default to empty string for sessions created before the v6
    /// migration — callers should check `job_description.is_empty()` to detect
    /// legacy sessions and fall back to `context_text` if needed.
    pub fn load_context_fields(&self, session_id: Uuid) -> Result<SessionContextFields> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        let (jd, profile, co, lp, re, tp, sn): (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT job_description, profile, company_overview,
                        leadership_principles, role_expectations,
                        technical_prep, strategy_notes
                 FROM sessions WHERE id = ?1",
                rusqlite::params![sid],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .context("load context fields")?;
        Ok(SessionContextFields {
            job_description: jd,
            profile,
            company_overview: co,
            leadership_principles: lp,
            role_expectations: re,
            technical_prep: tp,
            strategy_notes: sn,
        })
    }

    // ── Question bank (Phase 5.5.3) ──────────────────────────────────────────

    /// Persist the question bank JSON for a session.
    ///
    /// `questions` is the full ordered list; the caller owns de-dup / ordering.
    pub fn store_question_bank(&self, session_id: Uuid, questions: &[String]) -> Result<()> {
        use crate::session::question_bank::BankQuestionEntry;
        let existing = self
            .load_question_bank_entries(session_id)
            .unwrap_or_default();
        let tag_map: std::collections::HashMap<String, Vec<String>> = existing
            .into_iter()
            .map(|e| (e.question.trim().to_lowercase(), e.tags))
            .collect();
        let entries: Vec<BankQuestionEntry> = questions
            .iter()
            .map(|q| {
                let key = q.trim().to_lowercase();
                let tags = tag_map
                    .get(&key)
                    .cloned()
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| BankQuestionEntry::question_only(q).tags);
                BankQuestionEntry::new(q.clone(), tags)
            })
            .collect();
        self.store_question_bank_entries(session_id, &entries)
    }

    /// Store tagged question bank entries.
    pub fn store_question_bank_entries(
        &self,
        session_id: Uuid,
        entries: &[crate::session::question_bank::BankQuestionEntry],
    ) -> Result<()> {
        use crate::session::question_bank::bank_to_json;
        let json = bank_to_json(entries);
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        conn.execute(
            "UPDATE sessions SET question_bank_json = ?1 WHERE id = ?2",
            rusqlite::params![json, sid],
        )
        .context("store question bank entries")?;
        debug!(session_id = %session_id, count = entries.len(), "question bank stored");
        Ok(())
    }

    /// Load tagged question bank entries.
    pub fn load_question_bank_entries(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<crate::session::question_bank::BankQuestionEntry>> {
        use crate::session::question_bank::parse_bank_json;
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        let json: String = conn
            .query_row(
                "SELECT question_bank_json FROM sessions WHERE id = ?1",
                rusqlite::params![sid],
                |r| r.get(0),
            )
            .context("load question bank entries")?;
        Ok(parse_bank_json(&json))
    }

    /// Load the persisted question bank for a session. Returns an empty vec
    /// when no questions have been saved or the column defaulted to `'[]'`.
    pub fn load_question_bank(&self, session_id: Uuid) -> Result<Vec<String>> {
        use crate::session::question_bank::bank_questions;
        Ok(bank_questions(
            &self.load_question_bank_entries(session_id)?,
        ))
    }

    /// Load question strings for mock/rehearsal, optionally filtered by session focus tags.
    pub fn load_practice_questions(
        &self,
        session_id: Uuid,
        filter_by_focus: bool,
    ) -> Result<Vec<String>> {
        use crate::session::question_bank::{bank_questions, filter_by_focus_tags};
        let entries = self.load_question_bank_entries(session_id)?;
        if !filter_by_focus {
            return Ok(bank_questions(&entries));
        }
        let focus = self.load_session_focus(session_id)?;
        let filtered = filter_by_focus_tags(&entries, &focus.focus_tags);
        Ok(bank_questions(&filtered))
    }

    pub fn load_session_focus(&self, session_id: Uuid) -> Result<SessionFocus> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        conn.query_row(
            "SELECT focus_name, focus_tags_json, recruiter_brief, focus_notes,
                    focus_confirmed_at, needs_focus_refresh
             FROM sessions WHERE id = ?1",
            rusqlite::params![sid],
            |r| {
                let tags_json: String = r.get(1)?;
                let focus_tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
                Ok(SessionFocus {
                    focus_name: r.get(0)?,
                    focus_tags,
                    recruiter_brief: r.get(2)?,
                    focus_notes: r.get(3)?,
                    focus_confirmed_at: r.get(4)?,
                    needs_focus_refresh: r.get::<_, i64>(5)? != 0,
                })
            },
        )
        .context("load session focus")
    }

    pub fn save_session_focus(&self, session_id: Uuid, focus: &SessionFocus) -> Result<()> {
        let tags_json = serde_json::to_string(&focus.focus_tags).context("serialize focus tags")?;
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        conn.execute(
            "UPDATE sessions SET focus_name = ?1, focus_tags_json = ?2,
             recruiter_brief = ?3, focus_notes = ?4, focus_confirmed_at = ?5,
             needs_focus_refresh = ?6 WHERE id = ?7",
            rusqlite::params![
                focus.focus_name,
                tags_json,
                focus.recruiter_brief,
                focus.focus_notes,
                focus.focus_confirmed_at,
                i64::from(focus.needs_focus_refresh),
                sid,
            ],
        )
        .context("save session focus")?;
        Ok(())
    }

    pub fn mark_needs_focus_refresh(&self, session_id: Uuid) -> Result<()> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        conn.execute(
            "UPDATE sessions SET needs_focus_refresh = 1 WHERE id = ?1",
            rusqlite::params![session_id.to_string()],
        )
        .context("mark needs focus refresh")?;
        Ok(())
    }

    pub fn list_question_bank_tags(&self, session_id: Uuid) -> Result<Vec<String>> {
        use crate::session::question_bank::collect_bank_tags;
        let entries = self.load_question_bank_entries(session_id)?;
        Ok(collect_bank_tags(&entries))
    }

    /// Upsert the latest practice outcome for a question in this session.
    pub fn upsert_question_attempt(
        &self,
        session_id: Uuid,
        question: &str,
        last_source: &str,
        confidence_score: f32,
        coach_score: u8,
        satisfied: bool,
    ) -> Result<()> {
        use crate::session::question_attempts::normalize_question_key;

        let key = normalize_question_key(question);
        if key.is_empty() {
            return Ok(());
        }
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        conn.execute(
            "INSERT INTO question_attempts
                (session_id, question_key, question, last_source,
                 confidence_score, coach_score, satisfied, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, strftime('%s','now'))
             ON CONFLICT(session_id, question_key) DO UPDATE SET
                question = excluded.question,
                last_source = excluded.last_source,
                confidence_score = excluded.confidence_score,
                coach_score = excluded.coach_score,
                satisfied = excluded.satisfied,
                updated_at = excluded.updated_at",
            params![
                session_id.to_string(),
                key,
                question.trim(),
                last_source,
                confidence_score as f64,
                coach_score,
                i32::from(satisfied),
            ],
        )
        .context("upsert question attempt")?;
        Ok(())
    }

    /// Whether the latest attempt for this question was satisfactory.
    pub fn is_question_satisfied(&self, session_id: Uuid, question: &str) -> bool {
        use crate::session::question_attempts::normalize_question_key;

        let key = normalize_question_key(question);
        if key.is_empty() {
            return false;
        }
        let conn = match self.db.lock() {
            Ok(c) => c,
            Err(_) => return false,
        };
        let sid = session_id.to_string();
        conn.query_row(
            "SELECT satisfied FROM question_attempts
             WHERE session_id = ?1 AND question_key = ?2",
            params![sid, key],
            |r| r.get::<_, i32>(0),
        )
        .map(|v| v != 0)
        .unwrap_or(false)
    }

    /// All recorded question attempts for a session keyed by `question_key`.
    pub fn load_question_attempts(
        &self,
        session_id: Uuid,
    ) -> Result<std::collections::HashMap<String, QuestionAttempt>> {
        use std::collections::HashMap;

        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        let mut stmt = conn
            .prepare(
                "SELECT question_key, question, last_source, confidence_score,
                        coach_score, satisfied, preferred_answer
                 FROM question_attempts WHERE session_id = ?1",
            )
            .context("prepare load question attempts")?;
        let rows = stmt
            .query_map(params![sid], |r| {
                Ok(QuestionAttempt {
                    question_key: r.get(0)?,
                    question: r.get(1)?,
                    last_source: r.get(2)?,
                    confidence_score: r.get::<_, f64>(3)? as f32,
                    coach_score: r.get(4)?,
                    satisfied: r.get::<_, i32>(5)? != 0,
                    preferred_answer: r.get(6)?,
                })
            })
            .context("query question attempts")?;
        let mut map = HashMap::new();
        for row in rows {
            let attempt = row.context("read question attempt row")?;
            map.insert(attempt.question_key.clone(), attempt);
        }
        Ok(map)
    }

    /// User-tailored script for a question (empty when none saved).
    pub fn get_preferred_answer(&self, session_id: Uuid, question: &str) -> Result<String> {
        use crate::session::question_attempts::normalize_question_key;

        let key = normalize_question_key(question);
        if key.is_empty() {
            return Ok(String::new());
        }
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        conn.query_row(
            "SELECT preferred_answer FROM question_attempts
             WHERE session_id = ?1 AND question_key = ?2",
            params![sid, key],
            |r| r.get::<_, String>(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => rusqlite::Error::QueryReturnedNoRows,
            other => other,
        })
        .or_else(|e| {
            if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                Ok(String::new())
            } else {
                Err(anyhow::Error::from(e).context("read preferred answer"))
            }
        })
    }

    /// Persist a user-tailored answer for Live and embed into the Q&A vector store.
    pub fn save_preferred_answer(
        &self,
        session_id: Uuid,
        question: &str,
        answer: &str,
    ) -> Result<()> {
        use crate::session::question_attempts::normalize_question_key;

        let key = normalize_question_key(question);
        if key.is_empty() {
            return Ok(());
        }
        let trimmed_q = question.trim();
        let trimmed_a = answer.trim();
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        conn.execute(
            "INSERT INTO question_attempts
                (session_id, question_key, question, preferred_answer, updated_at)
             VALUES (?1, ?2, ?3, ?4, strftime('%s','now'))
             ON CONFLICT(session_id, question_key) DO UPDATE SET
                question = excluded.question,
                preferred_answer = excluded.preferred_answer,
                updated_at = excluded.updated_at",
            params![session_id.to_string(), key, trimmed_q, trimmed_a],
        )
        .context("save preferred answer")?;
        Ok(())
    }

    // ── Mock Interview ────────────────────────────────────────────────────

    /// Start a mock turn: one stable row per `(session_id, turn_n)`.
    ///
    /// Replaces any prior row for the same turn number so coach and audio_path
    /// never land on separate SQLite rows.
    pub fn begin_mock_turn(&self, session_id: Uuid, turn_n: u32, question: &str) -> Result<Uuid> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        conn.execute(
            "DELETE FROM mock_turns WHERE session_id = ?1 AND turn_n = ?2",
            params![sid, turn_n],
        )
        .context("clear prior mock turn row")?;
        let id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO mock_turns
             (id, session_id, turn_n, question, user_text, audio_path, coach_json, suggested, score)
             VALUES (?1, ?2, ?3, ?4, '', '', '', '', 0)",
            params![id.to_string(), sid, turn_n, question],
        )
        .context("insert mock turn row")?;
        Ok(id)
    }

    /// Insert a mock turn row (tests and migration helpers only).
    pub fn write_mock_turn(&self, turn: &MockTurn) -> Result<()> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        conn.execute(
            "INSERT INTO mock_turns
             (id, session_id, turn_n, question, user_text, audio_path, coach_json, suggested, score)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                turn.id.to_string(),
                turn.session_id.to_string(),
                turn.turn_n,
                turn.question,
                turn.user_text,
                turn.audio_path,
                turn.coach_json,
                turn.suggested,
                turn.score,
            ],
        )
        .context("write mock turn")?;
        Ok(())
    }

    /// Resolve the row id for a session turn number.
    pub fn mock_turn_id(&self, session_id: Uuid, turn_n: u32) -> Result<Option<Uuid>> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        let id_str: Option<String> = conn
            .query_row(
                "SELECT id FROM mock_turns WHERE session_id = ?1 AND turn_n = ?2",
                params![sid, turn_n],
                |r| r.get(0),
            )
            .optional()
            .context("lookup mock turn id")?;
        Ok(id_str.and_then(|s| Uuid::parse_str(&s).ok()))
    }

    /// Update the user-facing turn fields (transcript, audio path, suggested
    /// answer) without touching coach output.
    ///
    /// Coach feedback is written separately by [`update_mock_turn_coach`](Self::update_mock_turn_coach)
    /// so the two writers cannot race on the same column set. The conductor
    /// owns this write because it is the only task that has the final
    /// suggested-answer text from its parallel LLM stream.
    pub fn update_mock_turn_user_answer(
        &self,
        turn_id: Uuid,
        user_text: &str,
        audio_path: &str,
        suggested: &str,
    ) -> Result<()> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        conn.execute(
            "UPDATE mock_turns
             SET user_text = ?1, audio_path = ?2, suggested = ?3
             WHERE id = ?4",
            params![user_text, audio_path, suggested, turn_id.to_string()],
        )
        .context("update mock turn user answer")?;
        Ok(())
    }

    /// Update only the coach feedback columns for a turn.
    ///
    /// Pairs with [`update_mock_turn_user_answer`](Self::update_mock_turn_user_answer);
    /// see that method for the rationale behind splitting these writes.
    pub fn update_mock_turn_coach(&self, turn_id: Uuid, coach_json: &str, score: u8) -> Result<()> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        conn.execute(
            "UPDATE mock_turns
             SET coach_json = ?1, score = ?2
             WHERE id = ?3",
            params![coach_json, score, turn_id.to_string()],
        )
        .context("update mock turn coach")?;
        Ok(())
    }

    /// Update coach columns by session turn number (stable even if id drifted).
    pub fn update_mock_turn_coach_by_turn_n(
        &self,
        session_id: Uuid,
        turn_n: u32,
        coach_json: &str,
        score: u8,
    ) -> Result<()> {
        if let Some(id) = self.mock_turn_id(session_id, turn_n)? {
            self.update_mock_turn_coach(id, coach_json, score)?;
        }
        Ok(())
    }

    /// Update user answer columns by session turn number.
    pub fn update_mock_turn_user_answer_by_turn_n(
        &self,
        session_id: Uuid,
        turn_n: u32,
        user_text: &str,
        audio_path: &str,
        suggested: &str,
    ) -> Result<()> {
        if let Some(id) = self.mock_turn_id(session_id, turn_n)? {
            self.update_mock_turn_user_answer(id, user_text, audio_path, suggested)?;
        }
        Ok(())
    }

    /// Clear answer, coach, and score for a skipped turn.
    pub fn mark_mock_turn_skipped(&self, session_id: Uuid, turn_n: u32) -> Result<()> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        conn.execute(
            "UPDATE mock_turns
             SET user_text = '', audio_path = '', coach_json = '', suggested = '', score = 0
             WHERE session_id = ?1 AND turn_n = ?2",
            params![session_id.to_string(), turn_n],
        )
        .context("mark mock turn skipped")?;
        Ok(())
    }

    /// Load all turns for a session in order, for replay and summary.
    pub fn load_mock_turns(&self, session_id: Uuid) -> Result<Vec<MockTurn>> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, turn_n, question, user_text,
                        audio_path, coach_json, suggested, score
                 FROM mock_turns WHERE session_id = ?1 ORDER BY turn_n ASC",
            )
            .context("prepare load mock turns")?;
        let rows = stmt
            .query_map(params![sid], |r| {
                Ok(MockTurn {
                    id: Uuid::parse_str(&r.get::<_, String>(0)?).unwrap_or_default(),
                    session_id: Uuid::parse_str(&r.get::<_, String>(1)?).unwrap_or_default(),
                    turn_n: r.get(2)?,
                    question: r.get(3)?,
                    user_text: r.get(4)?,
                    audio_path: r.get(5)?,
                    coach_json: r.get(6)?,
                    suggested: r.get(7)?,
                    score: r.get(8)?,
                })
            })
            .context("query mock turns")?;
        let rows = rows
            .collect::<Result<Vec<_>, _>>()
            .context("collect mock turns")?;
        Ok(merge_mock_turn_rows(rows))
    }

    /// Delete all mock audio files for a session and remove the rows.
    pub fn delete_mock_turns(&self, session_id: Uuid) -> Result<()> {
        // Remove audio files first so we don't orphan them if the DELETE fails.
        if let Ok(turns) = self.load_mock_turns(session_id) {
            for turn in &turns {
                if !turn.audio_path.is_empty() {
                    let _ = std::fs::remove_file(&turn.audio_path);
                }
            }
        }
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        conn.execute(
            "DELETE FROM mock_turns WHERE session_id = ?1",
            params![session_id.to_string()],
        )
        .context("delete mock turns")?;
        Ok(())
    }

    /// Return the raw context text for a session (empty string if not stored).
    pub fn get_session_context(&self, session_id: Uuid) -> Result<String> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        let text: String = conn
            .query_row(
                "SELECT context_text FROM sessions WHERE id = ?1",
                params![sid],
                |r| r.get(0),
            )
            .context("get session context")?;
        Ok(text)
    }

    /// Most recent pre-live draft session, if any.
    pub fn find_draft_session(&self) -> Result<Option<Uuid>> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let row: Option<String> = conn
            .query_row(
                "SELECT id FROM sessions
                 WHERE state IN (
                     'CONFIGURING', 'INGESTING', 'DIGEST_REVIEW',
                     'PRE_WARMING', 'REHEARSING', 'MOCK_INTERVIEW', 'READY'
                 )
                 ORDER BY updated_at DESC, rowid DESC
                 LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .context("find draft session")?;

        row.map(|s| Uuid::parse_str(&s).context("parse draft session uuid"))
            .transpose()
    }

    /// Load session row metadata for draft resume / snapshot enrichment.
    pub fn get_session_metadata(&self, session_id: Uuid) -> Result<DraftSessionMetadata> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        let row: (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT state, name, session_type, domain, context_text,
                        job_description, profile, company_overview,
                        leadership_principles, role_expectations,
                        technical_prep, strategy_notes
                 FROM sessions WHERE id = ?1",
                params![sid],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                        r.get(11)?,
                    ))
                },
            )
            .context("get session metadata")?;
        let (state_str, name, session_type, domain, context_text, jd, profile, co, lp, re, tp, sn) =
            row;
        Ok(DraftSessionMetadata {
            session_id,
            state: parse_session_state(&state_str)?,
            name,
            session_type,
            domain,
            context_text,
            context_fields: SessionContextFields {
                job_description: jd,
                profile,
                company_overview: co,
                leadership_principles: lp,
                role_expectations: re,
                technical_prep: tp,
                strategy_notes: sn,
            },
        })
    }

    /// Persist extracted or user-edited digest JSON on the session row.
    pub fn store_session_digest(&self, session_id: Uuid, digest: &Digest) -> Result<()> {
        let json = serde_json::to_string(digest).context("serialize session digest")?;
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        conn.execute(
            "UPDATE sessions SET digest_json = ?1 WHERE id = ?2",
            params![json, sid],
        )
        .context("store session digest")?;
        debug!(session_id = %session_id, "session digest stored");
        Ok(())
    }

    /// Load persisted digest JSON, if any.
    pub fn load_session_digest(&self, session_id: Uuid) -> Result<Option<Digest>> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let sid = session_id.to_string();
        let json: String = conn
            .query_row(
                "SELECT digest_json FROM sessions WHERE id = ?1",
                params![sid],
                |r| r.get(0),
            )
            .context("load session digest column")?;
        if json.is_empty() {
            return Ok(None);
        }
        let digest: Digest = serde_json::from_str(&json).context("deserialize session digest")?;
        Ok(Some(digest))
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

    // ──────────────────────────────────────────────────────────────────────
    // GDPR helpers — Phase 7.5
    // ──────────────────────────────────────────────────────────────────────

    /// Return every session id currently persisted, regardless of state.
    /// Used during account deletion to clear each session's vector store.
    pub fn list_all_session_ids(&self) -> Result<Vec<Uuid>> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");
        let mut stmt = conn
            .prepare("SELECT id FROM sessions")
            .context("prepare list_all_session_ids")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .context("query session ids")?
            .map(|row| {
                let id = row.context("read session id row")?;
                Uuid::parse_str(&id).context("parse session uuid")
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(ids)
    }

    /// Atomically truncate every user-data table in the database.
    ///
    /// Wrapped in a single transaction so the SQLite file is never observed
    /// in a partially-cleared state. Schema (PRAGMA `user_version`, table
    /// definitions, WAL mode) is preserved — only rows are removed.
    pub fn clear_all_user_data(&self) -> Result<()> {
        let mut conn = self.db.lock().expect("session persistence mutex poisoned");
        let tx = conn.transaction().context("begin clear_all transaction")?;

        for table in [
            "transcript_chunks",
            "responses",
            "session_state_transitions",
            "sessions",
        ] {
            tx.execute(&format!("DELETE FROM {table}"), [])
                .with_context(|| format!("delete from {table}"))?;
        }

        tx.commit().context("commit clear_all transaction")?;
        info!("all local session data cleared");
        Ok(())
    }

    /// Dump every persisted session (with transcripts, responses, and state
    /// transitions) into an in-memory structure suitable for JSON export.
    pub fn export_all_data(&self) -> Result<Vec<SessionExport>> {
        let conn = self.db.lock().expect("session persistence mutex poisoned");

        let mut stmt = conn
            .prepare(
                "SELECT id, state, created_at, expires_at, promoted, name,
                        session_type, domain, COALESCE(context_text, ''),
                        COALESCE(job_description, ''), COALESCE(profile, ''),
                        COALESCE(company_overview, ''), COALESCE(leadership_principles, ''),
                        COALESCE(role_expectations, ''), COALESCE(technical_prep, ''),
                        COALESCE(strategy_notes, '')
                 FROM sessions
                 ORDER BY created_at ASC",
            )
            .context("prepare export sessions")?;

        let session_rows = stmt
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
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, String>(10)?,
                    r.get::<_, String>(11)?,
                    r.get::<_, String>(12)?,
                    r.get::<_, String>(13)?,
                    r.get::<_, String>(14)?,
                    r.get::<_, String>(15)?,
                ))
            })
            .context("query sessions for export")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("read sessions rows for export")?;

        let mut exports = Vec::with_capacity(session_rows.len());
        for row in session_rows {
            let (
                id,
                state,
                created_at,
                expires_at,
                promoted,
                name,
                session_type,
                domain,
                ctx,
                jd,
                profile,
                co,
                lp,
                re,
                tp,
                sn,
            ) = row;
            let sid = Uuid::parse_str(&id).context("parse session uuid for export")?;

            let transcripts = Self::select_transcripts_for_export(&conn, &id)?;
            let responses = Self::select_responses_for_export(&conn, &id)?;
            let transitions = Self::select_transitions_for_export(&conn, &id)?;

            exports.push(SessionExport {
                id: sid,
                state,
                created_at,
                expires_at,
                promoted: promoted != 0,
                name,
                session_type,
                domain,
                context_text: ctx,
                job_description: jd,
                profile,
                company_overview: co,
                leadership_principles: lp,
                role_expectations: re,
                technical_prep: tp,
                strategy_notes: sn,
                transcript_chunks: transcripts,
                responses,
                state_transitions: transitions,
            });
        }

        Ok(exports)
    }

    fn select_transcripts_for_export(
        conn: &rusqlite::Connection,
        session_id: &str,
    ) -> Result<Vec<TranscriptChunkExport>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, speaker, text, timestamp_ms, created_at
                 FROM transcript_chunks
                 WHERE session_id = ?1
                 ORDER BY created_at ASC",
            )
            .context("prepare export transcripts")?;
        let rows = stmt
            .query_map(params![session_id], |r| {
                Ok(TranscriptChunkExport {
                    id: r.get::<_, String>(0)?,
                    speaker: r.get::<_, String>(1)?,
                    text: r.get::<_, String>(2)?,
                    timestamp_ms: r.get::<_, i64>(3)?,
                    created_at: r.get::<_, i64>(4)?,
                })
            })
            .context("query transcript chunks for export")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("read transcript rows for export")?;
        Ok(rows)
    }

    fn select_responses_for_export(
        conn: &rusqlite::Connection,
        session_id: &str,
    ) -> Result<Vec<ResponseExport>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, response_type, content, confidence, created_at
                 FROM responses
                 WHERE session_id = ?1
                 ORDER BY created_at ASC",
            )
            .context("prepare export responses")?;
        let rows = stmt
            .query_map(params![session_id], |r| {
                Ok(ResponseExport {
                    id: r.get::<_, String>(0)?,
                    response_type: r.get::<_, String>(1)?,
                    content: r.get::<_, String>(2)?,
                    confidence: r.get::<_, f64>(3)?,
                    created_at: r.get::<_, i64>(4)?,
                })
            })
            .context("query responses for export")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("read response rows for export")?;
        Ok(rows)
    }

    fn select_transitions_for_export(
        conn: &rusqlite::Connection,
        session_id: &str,
    ) -> Result<Vec<StateTransitionExport>> {
        let mut stmt = conn
            .prepare(
                "SELECT state, transitioned_at
                 FROM session_state_transitions
                 WHERE session_id = ?1
                 ORDER BY transitioned_at ASC, id ASC",
            )
            .context("prepare export transitions")?;
        let rows = stmt
            .query_map(params![session_id], |r| {
                Ok(StateTransitionExport {
                    state: r.get::<_, String>(0)?,
                    transitioned_at: r.get::<_, i64>(1)?,
                })
            })
            .context("query transitions for export")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("read transition rows for export")?;
        Ok(rows)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Export DTOs — only used by Phase 7.5 GDPR right-to-export
// ──────────────────────────────────────────────────────────────────────────

/// Full snapshot of a single session for inclusion in a user-data export.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionExport {
    pub id: Uuid,
    pub state: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub promoted: bool,
    pub name: String,
    pub session_type: String,
    pub domain: String,
    /// Assembled RAG blob (legacy / backward compat).
    pub context_text: String,
    /// Structured Session Design fields (v6+; empty strings for legacy rows).
    pub job_description: String,
    pub profile: String,
    pub company_overview: String,
    pub leadership_principles: String,
    pub role_expectations: String,
    pub technical_prep: String,
    pub strategy_notes: String,
    pub transcript_chunks: Vec<TranscriptChunkExport>,
    pub responses: Vec<ResponseExport>,
    pub state_transitions: Vec<StateTransitionExport>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TranscriptChunkExport {
    pub id: String,
    pub speaker: String,
    pub text: String,
    pub timestamp_ms: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ResponseExport {
    pub id: String,
    pub response_type: String,
    pub content: String,
    pub confidence: f64,
    pub created_at: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StateTransitionExport {
    pub state: String,
    pub transitioned_at: i64,
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

/// Refuse to open a database whose `user_version` is ahead of the version
/// this build knows how to handle. Migrating an older Flint binary against
/// a DB written by a newer one would silently drop columns added in the
/// newer schema — fail loud instead.
fn verify_user_version_compatible(conn: &rusqlite::Connection) -> Result<()> {
    let current: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .context("read user_version for compatibility check")?;
    if current > SCHEMA_VERSION {
        bail!(
            "session database is from a newer Flint build (user_version={current}, \
             this build supports up to {SCHEMA_VERSION}). Refusing to open to avoid \
             silent data loss."
        );
    }
    Ok(())
}

/// Run `PRAGMA integrity_check` and surface non-`ok` results as a hard
/// error. sqlite returns the literal string `"ok"` on success and a
/// human-readable list of problems otherwise.
fn verify_integrity(conn: &rusqlite::Connection) -> Result<()> {
    let mut stmt = conn
        .prepare("PRAGMA integrity_check")
        .context("prepare integrity_check")?;
    let rows: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .context("execute integrity_check")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collect integrity_check rows")?;

    if rows.len() == 1 && rows[0] == "ok" {
        return Ok(());
    }
    bail!(
        "session database integrity check failed: {}",
        rows.join("; ")
    );
}

fn parse_session_state(s: &str) -> Result<SessionState> {
    match s {
        "IDLE" => Ok(SessionState::Idle),
        "CONFIGURING" => Ok(SessionState::Configuring),
        "INGESTING" => Ok(SessionState::Ingesting),
        "DIGEST_REVIEW" => Ok(SessionState::DigestReview),
        "PRE_WARMING" => Ok(SessionState::PreWarming),
        "REHEARSING" => Ok(SessionState::Rehearsing),
        "MOCK_INTERVIEW" => Ok(SessionState::MockInterview),
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

    #[test]
    fn count_open_sessions_excludes_idle_and_ended() {
        let db = new_db();
        let open = Uuid::new_v4();
        let ended = Uuid::new_v4();
        let idle = Uuid::new_v4();

        db.create_session_row(open, "Open", "interview", "swe")
            .unwrap();
        db.write_state_transition(open, &SessionState::Rehearsing)
            .unwrap();

        db.create_session_row(ended, "Done", "interview", "swe")
            .unwrap();
        db.write_state_transition(ended, &SessionState::Ended)
            .unwrap();

        db.create_session_row(idle, "", "interview", "").unwrap();
        db.write_state_transition(idle, &SessionState::Idle)
            .unwrap();

        assert_eq!(db.count_open_sessions().unwrap(), 1);
    }

    // ── Phase 7.5 hardening ──────────────────────────────────────────────────

    /// Helper: write the given `user_version` into a fresh DB file, then try
    /// to reopen it through `SessionPersistence::new`.
    fn try_open_with_user_version(version: u32) -> Result<()> {
        use std::path::PathBuf;
        let dir = std::env::temp_dir();
        let db_path: PathBuf = dir.join(format!("flint_uv_test_{}.sqlite", Uuid::new_v4()));
        let path_str = db_path.to_str().unwrap().to_string();
        {
            let raw = rusqlite::Connection::open(&path_str).unwrap();
            raw.execute_batch(&format!("PRAGMA user_version = {version};"))
                .unwrap();
        }
        let result = SessionPersistence::new(&path_str).map(|_| ());
        let _ = std::fs::remove_file(&db_path);
        result
    }

    #[test]
    fn future_schema_version_is_rejected() {
        let err = try_open_with_user_version(SCHEMA_VERSION + 10).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("newer Flint build"), "unexpected error: {msg}");
    }

    #[test]
    fn current_or_older_schema_version_opens_cleanly() {
        // v0: empty DB — all migrations run from scratch.
        try_open_with_user_version(0).expect("legacy DB must migrate forward");
        // Current version: no migration work needed.
        try_open_with_user_version(SCHEMA_VERSION).expect("current schema must open");
    }

    #[test]
    fn integrity_check_passes_on_fresh_db() {
        let db = new_db();
        let conn = db.db.lock().unwrap();
        assert!(verify_integrity(&conn).is_ok());
    }

    #[test]
    fn mark_stale_sessions_flips_live_ending_recovering_to_crashed() {
        let db = new_db();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let untouched = Uuid::new_v4();
        db.write_state_transition(a, &SessionState::Live).unwrap();
        db.write_state_transition(b, &SessionState::Ending).unwrap();
        db.write_state_transition(c, &SessionState::Recovering)
            .unwrap();
        db.write_state_transition(untouched, &SessionState::Ready)
            .unwrap();

        let flipped = db.mark_stale_sessions_as_crashed().unwrap();
        assert_eq!(flipped.len(), 3);
        for id in [a, b, c] {
            let data = db.load_session_for_recovery(id).unwrap().unwrap();
            assert_eq!(data.state, SessionState::Crashed);
        }
        // Sessions in clean states are not touched.
        let conn = db.db.lock().unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM sessions WHERE id = ?1",
                params![untouched.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "READY");
    }

    #[test]
    fn mark_stale_sessions_appends_audit_row_per_flip() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.write_state_transition(sid, &SessionState::Live).unwrap();
        db.mark_stale_sessions_as_crashed().unwrap();

        let conn = db.db.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_state_transitions
                 WHERE session_id = ?1 AND state = 'CRASHED'",
                params![sid.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn list_incomplete_sessions_returns_all_in_recent_order() {
        let db = new_db();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        db.write_state_transition(a, &SessionState::Live).unwrap();
        db.write_state_transition(b, &SessionState::Crashed)
            .unwrap();
        db.write_state_transition(c, &SessionState::Recovering)
            .unwrap();

        // c was inserted last → highest rowid → sorts first under the
        // (updated_at DESC, rowid DESC) ordering when all three writes
        // land in the same SQLite second.
        let list = db.list_incomplete_sessions().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], c, "most recent insert must lead the list");
    }

    #[test]
    fn recovering_state_is_now_loadable_for_recovery() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.write_state_transition(sid, &SessionState::Recovering)
            .unwrap();
        let result = db.load_session_for_recovery(sid).unwrap();
        assert!(
            result.is_some(),
            "RECOVERING must be in the recoverable set"
        );
    }

    #[test]
    fn write_state_transition_is_atomic_per_row() {
        // After a successful transition, sessions.state and the audit log
        // must agree. We can't easily inject a mid-transaction crash, but we
        // can assert the post-condition invariant after a normal write.
        let db = new_db();
        let sid = Uuid::new_v4();
        db.write_state_transition(sid, &SessionState::Configuring)
            .unwrap();
        db.write_state_transition(sid, &SessionState::Ingesting)
            .unwrap();
        let conn = db.db.lock().unwrap();
        let (sessions_state, last_audit): (String, String) = conn
            .query_row(
                "SELECT s.state, t.state FROM sessions s
                 JOIN (
                     SELECT session_id, state
                     FROM session_state_transitions
                     WHERE session_id = ?1
                     ORDER BY id DESC LIMIT 1
                 ) t ON t.session_id = s.id
                 WHERE s.id = ?1",
                params![sid.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(sessions_state, last_audit);
        assert_eq!(sessions_state, "INGESTING");
    }

    #[test]
    fn session_summary_returns_metadata_and_last_chunk() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.create_session_row(sid, "My interview", "interview", "software")
            .unwrap();
        db.write_state_transition(sid, &SessionState::Live).unwrap();
        db.write_transcript_chunk(&sample_chunk(sid, 500, "first"))
            .unwrap();
        db.write_transcript_chunk(&sample_chunk(sid, 12_000, "later"))
            .unwrap();

        let summary = db.session_summary(sid).unwrap().expect("summary");
        assert_eq!(summary.name, "My interview");
        assert_eq!(summary.session_type, "interview");
        assert_eq!(summary.domain, "software");
        assert_eq!(summary.last_chunk_timestamp_ms, Some(12_000));
        assert!(summary.created_at > 0);
    }

    #[test]
    fn session_summary_returns_none_for_unknown_id() {
        let db = new_db();
        let summary = db.session_summary(Uuid::new_v4()).unwrap();
        assert!(summary.is_none());
    }

    // ── GDPR helpers (Phase 7.5) ────────────────────────────────────────────

    #[test]
    fn list_all_session_ids_returns_every_session() {
        let db = new_db();
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();
        let s3 = Uuid::new_v4();
        db.create_session_row(s1, "A", "interview", "swe").unwrap();
        db.create_session_row(s2, "B", "interview", "swe").unwrap();
        db.create_session_row(s3, "C", "interview", "swe").unwrap();
        let ids = db.list_all_session_ids().unwrap();
        assert_eq!(ids.len(), 3);
        for id in [s1, s2, s3] {
            assert!(ids.contains(&id), "missing {id}");
        }
    }

    #[test]
    fn clear_all_user_data_truncates_every_table() {
        let db = new_db();
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();
        db.create_session_row(s1, "A", "interview", "swe").unwrap();
        db.create_session_row(s2, "B", "interview", "swe").unwrap();
        db.write_state_transition(s1, &SessionState::Live).unwrap();
        db.write_transcript_chunk(&sample_chunk(s1, 100, "hi"))
            .unwrap();
        db.write_response(&sample_response(s1)).unwrap();

        db.clear_all_user_data().unwrap();

        assert!(db.list_all_session_ids().unwrap().is_empty());
        let conn = db.db.lock().unwrap();
        for table in [
            "transcript_chunks",
            "responses",
            "session_state_transitions",
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table} not truncated");
        }
        // Schema is preserved.
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert!(version > 0, "schema version dropped after wipe");
    }

    #[test]
    fn store_and_load_context_fields_round_trip() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.create_session_row(sid, "Test", "interview", "swe")
            .unwrap();
        let fields = SessionContextFields {
            job_description: "We are hiring a Rust engineer.".to_string(),
            profile: "5 years of systems programming.".to_string(),
            company_overview: "Fast-paced startup.".to_string(),
            leadership_principles: "Bias for action.".to_string(),
            role_expectations: "Own the backend.".to_string(),
            technical_prep: "Review distributed systems.".to_string(),
            strategy_notes: "Emphasise ownership.".to_string(),
        };
        db.store_context_fields(sid, &fields).unwrap();
        let loaded = db.load_context_fields(sid).unwrap();
        assert_eq!(loaded.job_description, fields.job_description);
        assert_eq!(loaded.profile, fields.profile);
        assert_eq!(loaded.company_overview, fields.company_overview);
        assert_eq!(loaded.leadership_principles, fields.leadership_principles);
        assert_eq!(loaded.role_expectations, fields.role_expectations);
        assert_eq!(loaded.technical_prep, fields.technical_prep);
        assert_eq!(loaded.strategy_notes, fields.strategy_notes);
    }

    #[test]
    fn load_context_fields_defaults_to_empty_for_legacy_row() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.create_session_row(sid, "Legacy", "interview", "swe")
            .unwrap();
        let fields = db.load_context_fields(sid).unwrap();
        assert!(fields.job_description.is_empty());
        assert!(fields.profile.is_empty());
    }

    #[test]
    fn get_session_metadata_includes_context_fields() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.create_session_row(sid, "Meta", "interview", "swe")
            .unwrap();
        let cf = SessionContextFields {
            job_description: "Senior Rust engineer".to_string(),
            ..Default::default()
        };
        db.store_context_fields(sid, &cf).unwrap();
        let meta = db.get_session_metadata(sid).unwrap();
        assert_eq!(meta.context_fields.job_description, "Senior Rust engineer");
        assert!(meta.context_fields.profile.is_empty());
    }

    // ── Phase 5.5.3 — question bank ──────────────────────────────────────

    #[test]
    fn store_and_load_question_bank_round_trip() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.create_session_row(sid, "Bank", "interview", "swe")
            .unwrap();
        let qs = vec![
            "Tell me about yourself".to_string(),
            "Why this company?".to_string(),
            "Describe a hard bug".to_string(),
        ];
        db.store_question_bank(sid, &qs).unwrap();
        let loaded = db.load_question_bank(sid).unwrap();
        assert_eq!(loaded, qs);
    }

    #[test]
    fn load_question_bank_defaults_to_empty_for_new_session() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.create_session_row(sid, "New", "interview", "swe")
            .unwrap();
        let loaded = db.load_question_bank(sid).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn store_question_bank_overwrites_previous_value() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.create_session_row(sid, "Overwrite", "interview", "swe")
            .unwrap();
        db.store_question_bank(sid, &["A".to_string(), "B".to_string()])
            .unwrap();
        db.store_question_bank(sid, &["C".to_string()]).unwrap();
        let loaded = db.load_question_bank(sid).unwrap();
        assert_eq!(loaded, vec!["C".to_string()]);
    }

    #[test]
    fn store_question_bank_preserves_order() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.create_session_row(sid, "Order", "interview", "swe")
            .unwrap();
        let qs = vec!["z".to_string(), "a".to_string(), "m".to_string()];
        db.store_question_bank(sid, &qs).unwrap();
        let loaded = db.load_question_bank(sid).unwrap();
        assert_eq!(
            loaded, qs,
            "insertion order must be preserved across reload"
        );
    }

    #[test]
    fn session_focus_save_and_load_round_trip() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.create_session_row(sid, "Focus", "interview", "swe")
            .unwrap();
        let focus = SessionFocus {
            focus_name: "HR screen".into(),
            focus_tags: vec!["behavioral".into(), "motivation".into()],
            recruiter_brief: "Competency round".into(),
            focus_notes: "Emphasise IAM".into(),
            focus_confirmed_at: Some(1_700_000_000),
            needs_focus_refresh: false,
        };
        db.save_session_focus(sid, &focus).unwrap();
        let loaded = db.load_session_focus(sid).unwrap();
        assert_eq!(loaded.focus_name, "HR screen");
        assert_eq!(loaded.focus_tags, focus.focus_tags);
        assert_eq!(loaded.recruiter_brief, "Competency round");
        assert!(!loaded.needs_focus_refresh);
    }

    #[test]
    fn load_practice_questions_filters_by_focus_tags() {
        use crate::session::question_bank::BankQuestionEntry;
        let db = new_db();
        let sid = Uuid::new_v4();
        db.create_session_row(sid, "Filter", "interview", "swe")
            .unwrap();
        db.store_question_bank_entries(
            sid,
            &[
                BankQuestionEntry::new("Behavioral Q".to_string(), vec!["behavioral".to_string()]),
                BankQuestionEntry::new("Technical Q".to_string(), vec!["technical".to_string()]),
            ],
        )
        .unwrap();
        db.save_session_focus(
            sid,
            &SessionFocus {
                focus_tags: vec!["behavioral".into()],
                focus_confirmed_at: Some(1),
                ..SessionFocus::default()
            },
        )
        .unwrap();
        let filtered = db.load_practice_questions(sid, true).unwrap();
        assert_eq!(filtered, vec!["Behavioral Q".to_string()]);
    }

    #[test]
    fn load_question_bank_recovers_from_corrupted_json() {
        // The migration default is `'[]'` but a future bug or manual edit could
        // leave a malformed value. We never want to crash the session on read.
        let db = new_db();
        let sid = Uuid::new_v4();
        db.create_session_row(sid, "Corrupted", "interview", "swe")
            .unwrap();
        {
            let conn = db.db.lock().unwrap();
            conn.execute(
                "UPDATE sessions SET question_bank_json = ?1 WHERE id = ?2",
                params!["{not json", sid.to_string()],
            )
            .unwrap();
        }
        let loaded = db.load_question_bank(sid).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn clear_all_user_data_is_idempotent() {
        let db = new_db();
        db.clear_all_user_data().unwrap();
        db.clear_all_user_data().unwrap();
        assert!(db.list_all_session_ids().unwrap().is_empty());
    }

    #[test]
    fn export_all_data_round_trips_transcripts_and_responses() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.create_session_row(sid, "Export", "interview", "swe")
            .unwrap();
        db.write_state_transition(sid, &SessionState::Configuring)
            .unwrap();
        db.write_transcript_chunk(&sample_chunk(sid, 250, "hello world"))
            .unwrap();
        db.write_response(&sample_response(sid)).unwrap();

        let export = db.export_all_data().unwrap();
        assert_eq!(export.len(), 1);
        let session = &export[0];
        assert_eq!(session.id, sid);
        assert_eq!(session.name, "Export");
        assert_eq!(session.transcript_chunks.len(), 1);
        assert_eq!(session.transcript_chunks[0].text, "hello world");
        assert_eq!(session.responses.len(), 1);
        // Both the create (IDLE) and explicit CONFIGURING write should be present.
        assert!(!session.state_transitions.is_empty());
    }

    // ── Mock interview (Phase 8) ─────────────────────────────────────────────

    fn sample_mock_turn(session_id: Uuid, turn_n: u32) -> MockTurn {
        MockTurn {
            id: Uuid::new_v4(),
            session_id,
            turn_n,
            question: format!("Question {turn_n}"),
            user_text: String::new(),
            audio_path: String::new(),
            coach_json: String::new(),
            suggested: String::new(),
            score: 0,
        }
    }

    #[test]
    fn write_and_load_mock_turn_round_trip() {
        let db = new_db();
        let sid = Uuid::new_v4();
        let turn = sample_mock_turn(sid, 1);
        db.write_mock_turn(&turn).unwrap();

        let loaded = db.load_mock_turns(sid).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, turn.id);
        assert_eq!(loaded[0].turn_n, 1);
        assert_eq!(loaded[0].question, "Question 1");
    }

    #[test]
    fn update_mock_turn_user_answer_writes_only_user_columns() {
        let db = new_db();
        let sid = Uuid::new_v4();
        let turn = sample_mock_turn(sid, 1);
        db.write_mock_turn(&turn).unwrap();

        // Coach lands first (rare but possible — keeps the test honest).
        db.update_mock_turn_coach(turn.id, "{\"score\":81}", 81)
            .unwrap();
        // Conductor then writes user-side fields.
        db.update_mock_turn_user_answer(
            turn.id,
            "I led the migration to microservices.",
            "/tmp/answer.wav",
            "Suggested: lead with outcome, then how.",
        )
        .unwrap();

        let row = db
            .load_mock_turns(sid)
            .unwrap()
            .into_iter()
            .find(|t| t.id == turn.id)
            .unwrap();
        // User-side write must NOT clobber coach fields.
        assert_eq!(row.coach_json, "{\"score\":81}");
        assert_eq!(row.score, 81);
        assert_eq!(row.user_text, "I led the migration to microservices.");
        assert_eq!(row.audio_path, "/tmp/answer.wav");
        assert_eq!(row.suggested, "Suggested: lead with outcome, then how.");
    }

    #[test]
    fn update_mock_turn_coach_writes_only_coach_columns() {
        let db = new_db();
        let sid = Uuid::new_v4();
        let turn = sample_mock_turn(sid, 1);
        db.write_mock_turn(&turn).unwrap();

        // Conductor lands first with the user answer and suggested text.
        db.update_mock_turn_user_answer(
            turn.id,
            "transcript",
            "/tmp/a.wav",
            "Suggested polished answer",
        )
        .unwrap();
        // Coach updates only its own columns — must NOT wipe suggested/etc.
        db.update_mock_turn_coach(turn.id, "{\"score\":72}", 72)
            .unwrap();

        let row = db
            .load_mock_turns(sid)
            .unwrap()
            .into_iter()
            .find(|t| t.id == turn.id)
            .unwrap();
        assert_eq!(row.suggested, "Suggested polished answer");
        assert_eq!(row.user_text, "transcript");
        assert_eq!(row.audio_path, "/tmp/a.wav");
        assert_eq!(row.coach_json, "{\"score\":72}");
        assert_eq!(row.score, 72);
    }

    #[test]
    fn load_mock_turns_orders_by_turn_n_ascending() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.write_mock_turn(&sample_mock_turn(sid, 3)).unwrap();
        db.write_mock_turn(&sample_mock_turn(sid, 1)).unwrap();
        db.write_mock_turn(&sample_mock_turn(sid, 2)).unwrap();

        let loaded = db.load_mock_turns(sid).unwrap();
        let turns: Vec<u32> = loaded.iter().map(|t| t.turn_n).collect();
        assert_eq!(turns, vec![1, 2, 3]);
    }

    #[test]
    fn delete_mock_turns_removes_rows() {
        let db = new_db();
        let sid = Uuid::new_v4();
        db.write_mock_turn(&sample_mock_turn(sid, 1)).unwrap();
        db.write_mock_turn(&sample_mock_turn(sid, 2)).unwrap();

        db.delete_mock_turns(sid).unwrap();
        assert!(db.load_mock_turns(sid).unwrap().is_empty());
    }

    #[test]
    fn begin_mock_turn_keeps_single_row_per_turn_n() {
        let db = new_db();
        let sid = Uuid::new_v4();
        let id1 = db.begin_mock_turn(sid, 1, "Question 1").unwrap();
        db.update_mock_turn_user_answer(id1, "answer", "/tmp/a.wav", "suggested")
            .unwrap();
        let id2 = db.begin_mock_turn(sid, 2, "Question 2").unwrap();
        assert_ne!(id1, id2);
        db.update_mock_turn_coach_by_turn_n(sid, 1, "{\"score\":70}", 70)
            .unwrap();

        let loaded = db.load_mock_turns(sid).unwrap();
        assert_eq!(loaded.len(), 2);
        let turn1 = loaded.iter().find(|t| t.turn_n == 1).unwrap();
        assert_eq!(turn1.score, 70);
        assert_eq!(turn1.audio_path, "/tmp/a.wav");
    }

    #[test]
    fn merge_mock_turn_rows_combines_split_fields() {
        let sid = Uuid::new_v4();
        let coach_row = MockTurn {
            id: Uuid::new_v4(),
            session_id: sid,
            turn_n: 1,
            question: "Q1".into(),
            user_text: String::new(),
            audio_path: String::new(),
            coach_json: "{\"score\":80}".into(),
            suggested: String::new(),
            score: 80,
        };
        let audio_row = MockTurn {
            id: Uuid::new_v4(),
            session_id: sid,
            turn_n: 1,
            question: "Q1".into(),
            user_text: "my answer".into(),
            audio_path: "/tmp/recording.wav".into(),
            coach_json: String::new(),
            suggested: String::new(),
            score: 0,
        };
        let merged = merge_mock_turn_rows(vec![coach_row, audio_row]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].score, 80);
        assert_eq!(merged[0].audio_path, "/tmp/recording.wav");
        assert_eq!(merged[0].user_text, "my answer");
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
