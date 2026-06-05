//! Crash recovery orchestration.
//!
//! On app startup, [`check_for_recovery`] scans the local SQLite database for
//! any session left in `LIVE`, `ENDING`, or `CRASHED` state. If one is found,
//! the session state machine transitions to `RECOVERING` and the caller is
//! given a [`RecoveryOffer`] to surface to the user.
//!
//! The user can choose to resume or discard. Either path transitions back to a
//! stable state (`READY` for resume, `IDLE` for discard).

use std::sync::Arc;

use anyhow::Result;
use serde::Serialize;
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

use crate::interfaces::vector::VectorInterface;
use crate::session::persistence::{RecoveryData, SessionPersistence, SessionRecoverySummary};
use crate::session::state::{SessionState, SessionStateMachine};

// ──────────────────────────────────────────────────────────────────────────────
// Public types
// ──────────────────────────────────────────────────────────────────────────────

/// Summary of an incomplete session offered to the user on startup.
#[derive(Debug, Clone, Serialize)]
pub struct RecoveryOffer {
    pub session_id: String,
    pub interrupted_state: String,
    pub transcript_chunk_count: usize,
    pub response_count: usize,
    /// Display name from the session config (may be empty for legacy rows).
    pub name: String,
    /// `interview`, `mock`, etc.
    pub session_type: String,
    /// Domain string captured during configuration.
    pub domain: String,
    /// Unix epoch seconds when the session row was first created.
    pub created_at: i64,
    /// Wall-clock offset of the last transcript chunk, in ms, or `None` if
    /// the session never reached LIVE.
    pub last_chunk_timestamp_ms: Option<i64>,
    /// Number of additional crashed sessions waiting in the DB. The UI can
    /// surface a "review older crashed sessions" affordance when this is
    /// non-zero.
    pub additional_crashed_count: usize,
}

impl RecoveryOffer {
    fn from_parts(
        data: &RecoveryData,
        summary: Option<SessionRecoverySummary>,
        additional_crashed_count: usize,
    ) -> Self {
        let (name, session_type, domain, created_at, last_chunk_timestamp_ms) = match summary {
            Some(s) => (
                s.name,
                s.session_type,
                s.domain,
                s.created_at,
                s.last_chunk_timestamp_ms,
            ),
            None => (String::new(), String::new(), String::new(), 0, None),
        };
        Self {
            session_id: data.session_id.to_string(),
            interrupted_state: data.state.as_str().to_string(),
            transcript_chunk_count: data.transcript_chunks.len(),
            response_count: data.responses.len(),
            name,
            session_type,
            domain,
            created_at,
            last_chunk_timestamp_ms,
            additional_crashed_count,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Startup check
// ──────────────────────────────────────────────────────────────────────────────

/// Check the local database for an unclean session.
///
/// Startup contract:
///
/// 1. Every session in `LIVE`, `ENDING`, or `RECOVERING` is force-flipped to
///    `CRASHED` and audited (handles power-loss between live writes, or a
///    second crash during a recovery flow).
/// 2. The most recent `CRASHED` session — if any — is loaded and returned
///    as a [`RecoveryOffer`]. The state machine transitions
///    `IDLE → CRASHED → RECOVERING` so the orchestrator stays in lockstep.
/// 3. If older crashed sessions remain they are surfaced via
///    `additional_crashed_count` so the UI can offer batch discard / triage.
pub async fn check_for_recovery(
    persistence: &SessionPersistence,
    state_machine: &Mutex<SessionStateMachine>,
) -> Result<Option<RecoveryOffer>> {
    // Defensive: only honour a startup check when the in-memory machine is
    // pristine. Re-invoking this against a machine already mid-flow (e.g.
    // RECOVERING / LIVE) would silently re-anchor state and lose context.
    {
        let machine = state_machine.lock().await;
        let current = *machine.current();
        if current != SessionState::Idle {
            return Err(anyhow::anyhow!(
                "check_for_recovery refused — state machine is {} (must be IDLE)",
                current
            ));
        }
    }

    if let Err(e) = persistence.mark_stale_sessions_as_crashed() {
        warn!(error = %e, "failed to mark stale sessions as CRASHED — proceeding");
    }

    let incomplete_id = match persistence.find_incomplete_session()? {
        Some(id) => id,
        None => return Ok(None),
    };

    let data = match persistence.load_session_for_recovery(incomplete_id)? {
        Some(d) => d,
        None => return Ok(None),
    };

    let summary = persistence
        .session_summary(data.session_id)
        .unwrap_or_else(|e| {
            warn!(error = %e, "failed to load session summary for recovery offer");
            None
        });

    // Count other CRASHED rows so the UI can show a "N more interrupted
    // sessions to triage" hint. Subtract 1 for the current offer.
    let additional_crashed_count = persistence
        .list_incomplete_sessions()
        .map(|ids| ids.len().saturating_sub(1))
        .unwrap_or(0);

    {
        let mut machine = state_machine.lock().await;
        machine.restore_state_for_recovery(SessionState::Crashed, data.session_id);
        machine
            .transition(SessionState::Recovering)
            .map_err(|e| anyhow::anyhow!("recovery transition failed: {e}"))?;
    }

    let offer = RecoveryOffer::from_parts(&data, summary, additional_crashed_count);
    info!(
        session_id = %offer.session_id,
        state     = %offer.interrupted_state,
        chunks    = offer.transcript_chunk_count,
        responses = offer.response_count,
        additional = offer.additional_crashed_count,
        "crash recovery offer built"
    );

    Ok(Some(offer))
}

/// Discard every crashed session in the database. Returns the IDs that were
/// cleared, useful for the UI's "discard all interrupted sessions" affordance.
pub async fn discard_all_crashed(
    persistence: Arc<SessionPersistence>,
    vector_store: Arc<dyn VectorInterface>,
    state_machine: &Mutex<SessionStateMachine>,
) -> Result<Vec<Uuid>> {
    let active_id = {
        let machine = state_machine.lock().await;
        machine.session_id()
    };

    let to_clear = persistence.list_incomplete_sessions()?;
    for id in &to_clear {
        clear_session_artifacts(&persistence, vector_store.as_ref(), *id).await;
    }

    if let Some(id) = active_id {
        if to_clear.contains(&id) {
            let mut machine = state_machine.lock().await;
            if *machine.current() == SessionState::Recovering {
                machine.transition(SessionState::Ended).map_err(|e| {
                    anyhow::anyhow!("batch discard: RECOVERING → ENDED failed: {e}")
                })?;
                machine
                    .transition(SessionState::Idle)
                    .map_err(|e| anyhow::anyhow!("batch discard: ENDED → IDLE failed: {e}"))?;
            }
        }
    }

    info!(count = to_clear.len(), "discard_all_crashed completed");
    Ok(to_clear)
}

// ──────────────────────────────────────────────────────────────────────────────
// Resume / discard
// ──────────────────────────────────────────────────────────────────────────────

/// Resume a crashed session: transition `RECOVERING → READY`.
pub async fn resume_session(state_machine: &Mutex<SessionStateMachine>) -> Result<()> {
    let mut machine = state_machine.lock().await;
    machine
        .transition(SessionState::Ready)
        .map_err(|e| anyhow::anyhow!("resume transition failed: {e}"))?;
    info!("crashed session resumed → READY");
    Ok(())
}

/// Discard a crashed session: delete local data (sessions + transcript +
/// responses + RAG vectors) and return to `IDLE`.
///
/// Phase 7.5: `vector_store` is now cleaned alongside the relational data
/// so a discard leaves no orphan embeddings behind.
pub async fn discard_session(
    persistence: Arc<SessionPersistence>,
    vector_store: Arc<dyn VectorInterface>,
    state_machine: &Mutex<SessionStateMachine>,
) -> Result<()> {
    let sid = {
        let machine = state_machine.lock().await;
        machine.session_id()
    };

    if let Some(id) = sid {
        clear_session_artifacts(&persistence, vector_store.as_ref(), id).await;
    }

    // RECOVERING → ENDED → IDLE — mirrors the clean session end path.
    let mut machine = state_machine.lock().await;
    machine
        .transition(SessionState::Ended)
        .map_err(|e| anyhow::anyhow!("discard: RECOVERING → ENDED failed: {e}"))?;
    machine
        .transition(SessionState::Idle)
        .map_err(|e| anyhow::anyhow!("discard: ENDED → IDLE failed: {e}"))?;

    info!("crashed session discarded → IDLE");
    Ok(())
}

/// Drop persistence + vector data for `session_id`, logging but not
/// propagating failures (we always want the recovery state machine to make
/// forward progress even if cleanup partially fails).
async fn clear_session_artifacts(
    persistence: &SessionPersistence,
    vector_store: &dyn VectorInterface,
    session_id: Uuid,
) {
    if let Err(e) = persistence.clear_session(session_id) {
        warn!(session_id = %session_id, error = %e, "failed to clear session SQLite data");
    }
    if let Err(e) = vector_store.delete_session(session_id).await {
        warn!(session_id = %session_id, error = %e, "failed to clear session vector data");
    }
}
