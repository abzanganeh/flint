//! Session state machine — the single source of truth for the Flint session
//! lifecycle.
//!
//! Reference: design doc §25 (Session State Machine), `.cursor/rules`
//! §4.2 (state machine lives in Rust).
//!
//! All session state transitions go through [`SessionStateMachine`]. The state
//! machine validates every transition against the allow-list in §25, rejects
//! invalid transitions as hard errors, persists each successful transition to
//! SQLite via the [`StatePersister`] abstraction, and emits a structured
//! `tracing` event so every transition is auditable.
//!
//! React never drives transitions directly — Tauri commands ask the state
//! machine, the state machine validates, and then the orchestrator emits a
//! `session_state_change` event with the canonical state name.

// Public surface consumed by Task 2.7 (persistence), Task 2.9 (Tauri commands),
// and the broader orchestrator. Silence dead-code until those tasks land.
#![allow(dead_code)]

use std::fmt;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use tracing::{info, warn};
use uuid::Uuid;

/// Canonical lifecycle states for a Flint session.
///
/// The string form (`as_str` / `Display`) is the wire format used by the
/// `session_state_change` Tauri event and by the SQLite `sessions.status`
/// column. It MUST match the names in design doc §25 verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionState {
    Idle,
    Configuring,
    Ingesting,
    DigestReview,
    PreWarming,
    Rehearsing,
    Ready,
    Live,
    Paused,
    Ending,
    Ended,
    Crashed,
    Recovering,
}

impl SessionState {
    /// Canonical SCREAMING_SNAKE_CASE name (matches design doc §25 and the
    /// `session_state_change` event payload).
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionState::Idle => "IDLE",
            SessionState::Configuring => "CONFIGURING",
            SessionState::Ingesting => "INGESTING",
            SessionState::DigestReview => "DIGEST_REVIEW",
            SessionState::PreWarming => "PRE_WARMING",
            SessionState::Rehearsing => "REHEARSING",
            SessionState::Ready => "READY",
            SessionState::Live => "LIVE",
            SessionState::Paused => "PAUSED",
            SessionState::Ending => "ENDING",
            SessionState::Ended => "ENDED",
            SessionState::Crashed => "CRASHED",
            SessionState::Recovering => "RECOVERING",
        }
    }
}

impl fmt::Display for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Persistence contract the state machine depends on.
///
/// The production implementation lives in `session::persistence` (Task 2.7)
/// and writes to the local SQLite database with `PRAGMA journal_mode = WAL`.
/// Implementations MUST be synchronous from the state machine's perspective:
/// a crash immediately after `transition()` returns `Ok(())` must leave the
/// persisted state consistent with the in-memory state.
pub trait StatePersister: Send + Sync {
    fn write_state_transition(&self, session_id: Uuid, state: SessionState) -> Result<()>;
}

/// No-op persister for callers that have no session row yet (e.g. the very
/// first IDLE state on app start, before the orchestrator has created a
/// session). Also useful in unit tests that do not exercise the persistence
/// path.
#[derive(Debug, Default)]
pub struct NoopStatePersister;

impl StatePersister for NoopStatePersister {
    fn write_state_transition(&self, _session_id: Uuid, _state: SessionState) -> Result<()> {
        Ok(())
    }
}

/// Authoritative session lifecycle. The only object permitted to mutate the
/// current session state.
///
/// Invariants:
///
/// * `current` only changes via [`transition`](Self::transition).
/// * Every successful `transition` is persisted via `persister` before
///   `current` is updated in memory — on-disk and in-memory state never
///   diverge.
/// * Invalid transitions (anything not in the §25 allow-list) are HARD
///   errors. They are logged at `WARN` and the in-memory state is unchanged.
/// * `session_id` is assigned at most once. The orchestrator calls
///   [`set_session_id`](Self::set_session_id) when a new session is created
///   (typically immediately before the `IDLE → CONFIGURING` transition).
pub struct SessionStateMachine {
    current: SessionState,
    session_id: Option<Uuid>,
    persister: Arc<dyn StatePersister>,
}

impl SessionStateMachine {
    /// Build a state machine with a no-op persister. Used during app startup
    /// before any session exists, and in unit tests that do not exercise the
    /// persistence path. Always starts at [`SessionState::Idle`].
    pub fn new() -> Self {
        Self::with_persister(Arc::new(NoopStatePersister))
    }

    /// Build a state machine backed by `persister`. Always starts at
    /// [`SessionState::Idle`].
    pub fn with_persister(persister: Arc<dyn StatePersister>) -> Self {
        Self {
            current: SessionState::Idle,
            session_id: None,
            persister,
        }
    }

    /// Bind a freshly-generated `session_id` to this state machine. Typically
    /// called by the orchestrator at the moment a new session is born, just
    /// before the `IDLE → CONFIGURING` transition.
    ///
    /// Session IDs are immutable: re-assigning to a different UUID is an
    /// error. Re-assigning the same UUID is a no-op.
    pub fn set_session_id(&mut self, id: Uuid) -> Result<()> {
        match self.session_id {
            Some(existing) if existing == id => Ok(()),
            Some(existing) => Err(anyhow!(
                "Cannot reassign session_id (existing {existing}, requested {id})"
            )),
            None => {
                self.session_id = Some(id);
                Ok(())
            }
        }
    }

    pub fn current(&self) -> &SessionState {
        &self.current
    }

    pub fn session_id(&self) -> Option<Uuid> {
        self.session_id
    }

    /// Restore state directly without going through the transition guard.
    ///
    /// Only called from `session::recovery::check_for_recovery` at app startup
    /// when re-anchoring the in-memory machine to a persisted crashed session.
    /// All normal state changes must go through `transition`.
    pub fn restore_state_for_recovery(&mut self, state: SessionState, session_id: Uuid) {
        self.current = state;
        self.session_id = Some(session_id);
        tracing::info!(
            state = %self.current,
            session_id = %session_id,
            "state machine restored for crash recovery"
        );
    }

    /// Force the machine back to `IDLE` and drop any bound `session_id`.
    ///
    /// Reserved for `gdpr::delete_account` (Phase 7.5): after the user
    /// confirms account deletion every backing store has been wiped, so the
    /// only sane in-memory state is `IDLE` with no session bound. This skips
    /// the transition allow-list deliberately — the allow-list assumes the
    /// underlying session data still exists, which is no longer true here.
    pub fn reset_to_idle(&mut self) {
        let from = self.current;
        let prior_id = self.session_id;
        self.current = SessionState::Idle;
        self.session_id = None;
        tracing::info!(
            from = %from,
            prior_session_id = ?prior_id,
            "state machine forced to IDLE (account deletion)"
        );
    }

    /// Drive the machine to `to`.
    ///
    /// Order of operations:
    ///
    /// 1. Validate the transition against the §25 allow-list. On failure,
    ///    log `state_transition_rejected` at WARN and return
    ///    `Err("Invalid transition: FROM → TO")`. In-memory state is
    ///    unchanged.
    /// 2. If a `session_id` is bound, ask `persister` to write the new state
    ///    to SQLite. On failure, return the error with context; in-memory
    ///    state is unchanged so the next call retries cleanly.
    /// 3. Commit the in-memory state change and log `state_transition` at
    ///    INFO with `from`/`to`/`session_id`.
    pub fn transition(&mut self, to: SessionState) -> Result<()> {
        let from = self.current;
        // Compute once so the tracing macros stay simple and stay coverable —
        // an inline `.map(|id| id.to_string())` lives inside the macro
        // expansion and is reported as uncovered by both ptrace and llvm
        // engines even when the macro fires.
        let session_id_str = session_id_for_log(self.session_id);

        if !is_valid_transition(from, to) {
            warn!(
                event = "state_transition_rejected",
                session_id = %session_id_str,
                from = %from,
                to = %to,
                "invalid session state transition rejected",
            );
            return Err(anyhow!("Invalid transition: {from} → {to}"));
        }

        if let Some(id) = self.session_id {
            self.persister
                .write_state_transition(id, to)
                .with_context(|| format!("failed to persist transition {from} → {to}"))?;
        }

        self.current = to;

        info!(
            event = "state_transition",
            session_id = %session_id_str,
            from = %from,
            to = %to,
            "session state transitioned",
        );

        Ok(())
    }
}

/// Render `Option<Uuid>` for structured-logging fields.
///
/// Returns the empty string when no session is bound. Extracted from
/// [`SessionStateMachine::transition`] so the formatting logic lives outside
/// of `tracing` macros — both coverage engines record this branch correctly,
/// and the helper is trivially unit-testable.
fn session_id_for_log(id: Option<Uuid>) -> String {
    match id {
        Some(uuid) => uuid.to_string(),
        None => String::new(),
    }
}

impl Default for SessionStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

/// Truth table for valid transitions (design doc §25, "Valid Transitions").
///
/// Any `(from, to)` pair not listed here is rejected as a hard error.
/// Idempotent self-transitions are NOT allowed (e.g. `INGESTING → INGESTING`
/// is explicitly listed as an invalid transition in §25).
///
/// Keep this list in lock-step with §25; the test module asserts every row.
const fn is_valid_transition(from: SessionState, to: SessionState) -> bool {
    use SessionState::*;
    matches!(
        (from, to),
        (Idle, Configuring)
            | (Configuring, Ingesting)
            | (Ingesting, DigestReview)
            | (DigestReview, PreWarming)
            | (PreWarming, Rehearsing)
            | (PreWarming, Ready)
            | (Rehearsing, Ready)
            | (Ready, Live)
            | (Live, Paused)
            | (Paused, Live)
            | (Live, Ending)
            | (Ending, Ended)
            | (Ended, Idle)
            | (Ended, Configuring)
            | (Live, Crashed)
            | (Ended, Crashed)
            | (Crashed, Recovering)
            | (Recovering, Ready)
            | (Recovering, Ended)
    )
}

#[cfg(test)]
mod tests {
    //! Full coverage test battery for the session state machine.
    //!
    //! Sections:
    //!   A. Every valid transition from §25 (19 tests)
    //!   B. Every named invalid transition from §25 (6 tests)
    //!   C. Additional invalid transitions (3 tests)
    //!   D. SQLite persistence via RusqlitePersister (2 tests)
    //!   E. Recovery path tests (2 tests)
    //!   F. ENDED → IDLE / ENDED → CONFIGURING divergence (2 tests)
    //!   Smoke tests (9 tests, retained from initial implementation)

    use super::*;
    use std::sync::Mutex;

    // ─── Test helpers ────────────────────────────────────────────────────────

    /// Drive a fresh state machine through `transitions` in order.
    /// Panics if any transition is unexpectedly rejected.
    fn drive(transitions: &[SessionState]) -> SessionStateMachine {
        let mut sm = SessionStateMachine::new();
        for &s in transitions {
            sm.transition(s)
                .unwrap_or_else(|e| panic!("drive failed at {s}: {e}"));
        }
        sm
    }

    /// Test persister that records every call so tests can assert the
    /// state machine invoked persistence.
    #[derive(Default)]
    struct RecordingPersister {
        calls: Mutex<Vec<(Uuid, SessionState)>>,
    }

    impl RecordingPersister {
        fn calls(&self) -> Vec<(Uuid, SessionState)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl StatePersister for RecordingPersister {
        fn write_state_transition(&self, session_id: Uuid, state: SessionState) -> Result<()> {
            self.calls.lock().unwrap().push((session_id, state));
            Ok(())
        }
    }

    /// Persister that always fails — used to verify the state machine does
    /// NOT mutate in-memory state when persistence fails.
    struct FailingPersister;
    impl StatePersister for FailingPersister {
        fn write_state_transition(&self, _session_id: Uuid, _state: SessionState) -> Result<()> {
            Err(anyhow!("disk full"))
        }
    }

    // ─── SQLite test persister (Section D) ───────────────────────────────────

    struct RusqlitePersister {
        conn: Mutex<rusqlite::Connection>,
    }

    impl RusqlitePersister {
        fn new_in_memory() -> anyhow::Result<Self> {
            let conn = rusqlite::Connection::open_in_memory()?;
            conn.execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE IF NOT EXISTS session_states (
                     session_id TEXT NOT NULL,
                     state TEXT NOT NULL,
                     written_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
                 );",
            )?;
            Ok(Self {
                conn: Mutex::new(conn),
            })
        }

        fn last_state(&self, session_id: Uuid) -> Option<String> {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT state FROM session_states \
                 WHERE session_id = ?1 \
                 ORDER BY written_at DESC, rowid DESC LIMIT 1",
                rusqlite::params![session_id.to_string()],
                |row| row.get(0),
            )
            .ok()
        }
    }

    impl StatePersister for RusqlitePersister {
        fn write_state_transition(&self, session_id: Uuid, state: SessionState) -> Result<()> {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO session_states (session_id, state) VALUES (?1, ?2)",
                rusqlite::params![session_id.to_string(), state.as_str()],
            )?;
            Ok(())
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Section A — every valid transition (§25)
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_valid_idle_to_configuring() {
        let mut sm = SessionStateMachine::new();
        assert!(sm.transition(SessionState::Configuring).is_ok());
        assert_eq!(*sm.current(), SessionState::Configuring);
    }

    #[test]
    fn test_valid_configuring_to_ingesting() {
        let mut sm = drive(&[SessionState::Configuring]);
        assert!(sm.transition(SessionState::Ingesting).is_ok());
        assert_eq!(*sm.current(), SessionState::Ingesting);
    }

    #[test]
    fn test_valid_ingesting_to_digest_review() {
        let mut sm = drive(&[SessionState::Configuring, SessionState::Ingesting]);
        assert!(sm.transition(SessionState::DigestReview).is_ok());
        assert_eq!(*sm.current(), SessionState::DigestReview);
    }

    #[test]
    fn test_valid_digest_review_to_pre_warming() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
        ]);
        assert!(sm.transition(SessionState::PreWarming).is_ok());
        assert_eq!(*sm.current(), SessionState::PreWarming);
    }

    #[test]
    fn test_valid_pre_warming_to_rehearsing() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
        ]);
        assert!(sm.transition(SessionState::Rehearsing).is_ok());
        assert_eq!(*sm.current(), SessionState::Rehearsing);
    }

    #[test]
    fn test_valid_pre_warming_to_ready() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
        ]);
        assert!(sm.transition(SessionState::Ready).is_ok());
        assert_eq!(*sm.current(), SessionState::Ready);
    }

    #[test]
    fn test_valid_rehearsing_to_ready() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Rehearsing,
        ]);
        assert!(sm.transition(SessionState::Ready).is_ok());
        assert_eq!(*sm.current(), SessionState::Ready);
    }

    #[test]
    fn test_valid_ready_to_live() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
        ]);
        assert!(sm.transition(SessionState::Live).is_ok());
        assert_eq!(*sm.current(), SessionState::Live);
    }

    #[test]
    fn test_valid_live_to_paused() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
        ]);
        assert!(sm.transition(SessionState::Paused).is_ok());
        assert_eq!(*sm.current(), SessionState::Paused);
    }

    #[test]
    fn test_valid_paused_to_live() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
            SessionState::Paused,
        ]);
        assert!(sm.transition(SessionState::Live).is_ok());
        assert_eq!(*sm.current(), SessionState::Live);
    }

    #[test]
    fn test_valid_live_to_ending() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
        ]);
        assert!(sm.transition(SessionState::Ending).is_ok());
        assert_eq!(*sm.current(), SessionState::Ending);
    }

    #[test]
    fn test_valid_ending_to_ended() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
            SessionState::Ending,
        ]);
        assert!(sm.transition(SessionState::Ended).is_ok());
        assert_eq!(*sm.current(), SessionState::Ended);
    }

    #[test]
    fn test_valid_ended_to_idle() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
            SessionState::Ending,
            SessionState::Ended,
        ]);
        assert!(sm.transition(SessionState::Idle).is_ok());
        assert_eq!(*sm.current(), SessionState::Idle);
    }

    #[test]
    fn test_valid_ended_to_configuring() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
            SessionState::Ending,
            SessionState::Ended,
        ]);
        assert!(sm.transition(SessionState::Configuring).is_ok());
        assert_eq!(*sm.current(), SessionState::Configuring);
    }

    #[test]
    fn test_valid_live_to_crashed() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
        ]);
        assert!(sm.transition(SessionState::Crashed).is_ok());
        assert_eq!(*sm.current(), SessionState::Crashed);
    }

    #[test]
    fn test_valid_ended_to_crashed() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
            SessionState::Ending,
            SessionState::Ended,
        ]);
        assert!(sm.transition(SessionState::Crashed).is_ok());
        assert_eq!(*sm.current(), SessionState::Crashed);
    }

    #[test]
    fn test_valid_crashed_to_recovering() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
            SessionState::Crashed,
        ]);
        assert!(sm.transition(SessionState::Recovering).is_ok());
        assert_eq!(*sm.current(), SessionState::Recovering);
    }

    #[test]
    fn test_valid_recovering_to_ready() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
            SessionState::Crashed,
            SessionState::Recovering,
        ]);
        assert!(sm.transition(SessionState::Ready).is_ok());
        assert_eq!(*sm.current(), SessionState::Ready);
    }

    #[test]
    fn test_valid_recovering_to_ended() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
            SessionState::Crashed,
            SessionState::Recovering,
        ]);
        assert!(sm.transition(SessionState::Ended).is_ok());
        assert_eq!(*sm.current(), SessionState::Ended);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Section B — every named invalid transition from §25
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_invalid_idle_to_live_is_rejected() {
        let mut sm = SessionStateMachine::new();
        let result = sm.transition(SessionState::Live);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid transition"));
        assert_eq!(*sm.current(), SessionState::Idle);
    }

    #[test]
    fn test_invalid_crashed_to_live_is_rejected() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
            SessionState::Crashed,
        ]);
        let result = sm.transition(SessionState::Live);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid transition"));
        assert_eq!(*sm.current(), SessionState::Crashed);
    }

    #[test]
    fn test_invalid_pre_warming_to_live_is_rejected() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
        ]);
        let result = sm.transition(SessionState::Live);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid transition"));
        assert_eq!(*sm.current(), SessionState::PreWarming);
    }

    #[test]
    fn test_invalid_ingesting_to_ingesting_self_transition_is_rejected() {
        let mut sm = drive(&[SessionState::Configuring, SessionState::Ingesting]);
        let result = sm.transition(SessionState::Ingesting);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid transition"));
        assert_eq!(*sm.current(), SessionState::Ingesting);
    }

    #[test]
    fn test_invalid_live_to_ingesting_is_rejected() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
        ]);
        let result = sm.transition(SessionState::Ingesting);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid transition"));
        assert_eq!(*sm.current(), SessionState::Live);
    }

    #[test]
    fn test_invalid_rehearsing_to_live_is_rejected() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Rehearsing,
        ]);
        let result = sm.transition(SessionState::Live);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid transition"));
        assert_eq!(*sm.current(), SessionState::Rehearsing);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Section C — additional invalid transitions (not named in §25)
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_extra_invalid_ended_to_live_is_rejected() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
            SessionState::Ending,
            SessionState::Ended,
        ]);
        let result = sm.transition(SessionState::Live);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid transition"));
        assert_eq!(*sm.current(), SessionState::Ended);
    }

    #[test]
    fn test_extra_invalid_paused_to_ended_is_rejected() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
            SessionState::Paused,
        ]);
        let result = sm.transition(SessionState::Ended);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid transition"));
        assert_eq!(*sm.current(), SessionState::Paused);
    }

    #[test]
    fn test_extra_invalid_configuring_to_live_is_rejected() {
        let mut sm = drive(&[SessionState::Configuring]);
        let result = sm.transition(SessionState::Live);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid transition"));
        assert_eq!(*sm.current(), SessionState::Configuring);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Section D — SQLite persistence via RusqlitePersister
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_state_persisted_to_sqlite_on_transition() {
        let persister = Arc::new(RusqlitePersister::new_in_memory().unwrap());
        let mut sm = SessionStateMachine::with_persister(persister.clone());
        let session_id = Uuid::new_v4();
        sm.set_session_id(session_id).unwrap();

        sm.transition(SessionState::Configuring).unwrap();
        assert_eq!(
            persister.last_state(session_id).as_deref(),
            Some("CONFIGURING")
        );

        sm.transition(SessionState::Ingesting).unwrap();
        assert_eq!(
            persister.last_state(session_id).as_deref(),
            Some("INGESTING")
        );

        sm.transition(SessionState::DigestReview).unwrap();
        assert_eq!(
            persister.last_state(session_id).as_deref(),
            Some("DIGEST_REVIEW")
        );
    }

    #[test]
    fn test_sqlite_not_written_on_invalid_transition() {
        let persister = Arc::new(RusqlitePersister::new_in_memory().unwrap());
        let mut sm = SessionStateMachine::with_persister(persister.clone());
        let session_id = Uuid::new_v4();
        sm.set_session_id(session_id).unwrap();

        // Attempt an invalid transition from the initial IDLE state.
        let result = sm.transition(SessionState::Live);
        assert!(result.is_err());
        assert_eq!(persister.last_state(session_id), None);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Section E — Recovery path tests
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_recovery_crashed_to_recovering_to_ready() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
            SessionState::Crashed,
        ]);
        assert!(sm.transition(SessionState::Recovering).is_ok());
        assert_eq!(*sm.current(), SessionState::Recovering);
        assert!(sm.transition(SessionState::Ready).is_ok());
        assert_eq!(*sm.current(), SessionState::Ready);
    }

    #[test]
    fn test_recovery_recovering_to_ended_escape_hatch() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
            SessionState::Crashed,
            SessionState::Recovering,
        ]);
        assert!(sm.transition(SessionState::Ended).is_ok());
        assert_eq!(*sm.current(), SessionState::Ended);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Section F — ENDED → IDLE / ENDED → CONFIGURING divergence
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_ended_divergence_to_idle_resets_to_home_screen() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
            SessionState::Ending,
            SessionState::Ended,
        ]);
        // User closes the session — should go back to IDLE (home screen).
        assert!(sm.transition(SessionState::Idle).is_ok());
        assert_eq!(*sm.current(), SessionState::Idle);
    }

    #[test]
    fn test_ended_divergence_to_configuring_clones_session() {
        let mut sm = drive(&[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
            SessionState::Live,
            SessionState::Ending,
            SessionState::Ended,
        ]);
        // User clones session as new — should jump directly to CONFIGURING.
        assert!(sm.transition(SessionState::Configuring).is_ok());
        assert_eq!(*sm.current(), SessionState::Configuring);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Smoke tests (retained from initial implementation)
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn starts_at_idle() {
        let sm = SessionStateMachine::new();
        assert_eq!(*sm.current(), SessionState::Idle);
        assert!(sm.session_id().is_none());
    }

    #[test]
    fn happy_path_idle_to_configuring() {
        let mut sm = SessionStateMachine::new();
        sm.transition(SessionState::Configuring).unwrap();
        assert_eq!(*sm.current(), SessionState::Configuring);
    }

    #[test]
    fn rejects_idle_to_live() {
        let mut sm = SessionStateMachine::new();
        let err = sm.transition(SessionState::Live).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Invalid transition"), "got: {msg}");
        assert!(msg.contains("IDLE"), "got: {msg}");
        assert!(msg.contains("LIVE"), "got: {msg}");
        assert_eq!(*sm.current(), SessionState::Idle, "state must not change");
    }

    #[test]
    fn persister_called_for_each_successful_transition_when_session_bound() {
        let persister = Arc::new(RecordingPersister::default());
        let mut sm = SessionStateMachine::with_persister(persister.clone());
        let session_id = Uuid::new_v4();
        sm.set_session_id(session_id).unwrap();

        sm.transition(SessionState::Configuring).unwrap();
        sm.transition(SessionState::Ingesting).unwrap();
        sm.transition(SessionState::DigestReview).unwrap();

        assert_eq!(
            persister.calls(),
            vec![
                (session_id, SessionState::Configuring),
                (session_id, SessionState::Ingesting),
                (session_id, SessionState::DigestReview),
            ]
        );
    }

    #[test]
    fn persister_not_called_when_no_session_id_bound() {
        let persister = Arc::new(RecordingPersister::default());
        let mut sm = SessionStateMachine::with_persister(persister.clone());
        sm.transition(SessionState::Configuring).unwrap();
        assert!(persister.calls().is_empty());
    }

    #[test]
    fn invalid_transition_does_not_call_persister() {
        let persister = Arc::new(RecordingPersister::default());
        let mut sm = SessionStateMachine::with_persister(persister.clone());
        sm.set_session_id(Uuid::new_v4()).unwrap();
        let _ = sm.transition(SessionState::Live);
        assert!(persister.calls().is_empty());
    }

    #[test]
    fn persistence_failure_does_not_mutate_in_memory_state() {
        let mut sm = SessionStateMachine::with_persister(Arc::new(FailingPersister));
        sm.set_session_id(Uuid::new_v4()).unwrap();
        let err = sm.transition(SessionState::Configuring).unwrap_err();
        assert!(err.to_string().contains("failed to persist"));
        assert_eq!(*sm.current(), SessionState::Idle);
    }

    #[test]
    fn session_id_is_immutable_once_set() {
        let mut sm = SessionStateMachine::new();
        let first = Uuid::new_v4();
        sm.set_session_id(first).unwrap();
        sm.set_session_id(first).unwrap();
        let err = sm.set_session_id(Uuid::new_v4()).unwrap_err();
        assert!(err.to_string().contains("Cannot reassign session_id"));
    }

    #[test]
    fn state_display_matches_canonical_names() {
        // Exhaustive: every variant must Display to its design-doc §25 name.
        assert_eq!(SessionState::Idle.to_string(), "IDLE");
        assert_eq!(SessionState::Configuring.to_string(), "CONFIGURING");
        assert_eq!(SessionState::Ingesting.to_string(), "INGESTING");
        assert_eq!(SessionState::DigestReview.to_string(), "DIGEST_REVIEW");
        assert_eq!(SessionState::PreWarming.to_string(), "PRE_WARMING");
        assert_eq!(SessionState::Rehearsing.to_string(), "REHEARSING");
        assert_eq!(SessionState::Ready.to_string(), "READY");
        assert_eq!(SessionState::Live.to_string(), "LIVE");
        assert_eq!(SessionState::Paused.to_string(), "PAUSED");
        assert_eq!(SessionState::Ending.to_string(), "ENDING");
        assert_eq!(SessionState::Ended.to_string(), "ENDED");
        assert_eq!(SessionState::Crashed.to_string(), "CRASHED");
        assert_eq!(SessionState::Recovering.to_string(), "RECOVERING");
    }

    /// `NoopStatePersister` is the default impl used when no SQLite is wired
    /// (e.g. unit tests, app startup before a session exists). Ensure its
    /// `Ok(())` path is exercised directly.
    #[test]
    fn noop_persister_returns_ok() {
        let p = NoopStatePersister;
        let result = p.write_state_transition(Uuid::new_v4(), SessionState::Configuring);
        assert!(result.is_ok());
    }

    /// `Default` is the entry point used by `AppState` on startup; covers
    /// the trait impl that the constructor tests bypass.
    #[test]
    fn default_starts_at_idle_without_session() {
        let sm = SessionStateMachine::default();
        assert_eq!(*sm.current(), SessionState::Idle);
        assert!(sm.session_id().is_none());
    }

    /// Exercises the `Some(_)` branch of `session_id.map(...)` inside both
    /// the rejection (`warn!`) and the success (`info!`) tracing macros, so
    /// the structured-logging arms are not dead under coverage.
    #[test]
    fn tracing_emits_session_id_when_bound_for_success_and_rejection() {
        let persister = Arc::new(RecordingPersister::default());
        let mut sm = SessionStateMachine::with_persister(persister.clone());
        let session_id = Uuid::new_v4();
        sm.set_session_id(session_id).unwrap();

        // Successful transition exercises the info! macro with Some(id).
        sm.transition(SessionState::Configuring).unwrap();
        assert_eq!(*sm.current(), SessionState::Configuring);

        // Invalid transition from CONFIGURING exercises the warn! macro
        // with Some(id) and leaves state unchanged.
        let err = sm.transition(SessionState::Live).unwrap_err();
        assert!(err.to_string().contains("Invalid transition"));
        assert_eq!(*sm.current(), SessionState::Configuring);

        // Only the successful transition should reach the persister.
        let calls = persister.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], (session_id, SessionState::Configuring));
    }

    /// `restore_state_for_recovery` is the only API that bypasses the
    /// transition guard — guarantee both fields are restored exactly.
    #[test]
    fn restore_state_for_recovery_sets_state_and_session_id() {
        let mut sm = SessionStateMachine::new();
        let session_id = Uuid::new_v4();
        sm.restore_state_for_recovery(SessionState::Crashed, session_id);
        assert_eq!(*sm.current(), SessionState::Crashed);
        assert_eq!(sm.session_id(), Some(session_id));
    }

    #[test]
    fn session_id_for_log_renders_some_and_none() {
        assert_eq!(session_id_for_log(None), "");
        let id = Uuid::new_v4();
        assert_eq!(session_id_for_log(Some(id)), id.to_string());
    }
}
