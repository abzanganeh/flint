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
use tracing::info;

use crate::session::persistence::{RecoveryData, SessionPersistence};
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
}

impl RecoveryOffer {
    fn from_data(data: &RecoveryData) -> Self {
        Self {
            session_id: data.session_id.to_string(),
            interrupted_state: data.state.as_str().to_string(),
            transcript_chunk_count: data.transcript_chunks.len(),
            response_count: data.responses.len(),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Startup check
// ──────────────────────────────────────────────────────────────────────────────

/// Check the local database for an unclean session.
///
/// If found, transition the state machine to `RECOVERING` and return a
/// `RecoveryOffer` for the frontend to display. Returns `Ok(None)` if the
/// previous session ended cleanly.
pub async fn check_for_recovery(
    persistence: &SessionPersistence,
    state_machine: &Mutex<SessionStateMachine>,
) -> Result<Option<RecoveryOffer>> {
    let incomplete_id = match persistence.find_incomplete_session()? {
        Some(id) => id,
        None => return Ok(None),
    };

    let data = match persistence.load_session_for_recovery(incomplete_id)? {
        Some(d) => d,
        None => return Ok(None),
    };

    // Regardless of whether the persisted state is LIVE, ENDING, or CRASHED,
    // we treat it as CRASHED on startup — the machine terminated abnormally.
    // `restore_state_for_recovery` bypasses the transition guard to anchor
    // the in-memory machine (which started at IDLE) to CRASHED, so the normal
    // `CRASHED → RECOVERING` transition can then proceed.
    {
        let mut machine = state_machine.lock().await;
        machine.restore_state_for_recovery(SessionState::Crashed, data.session_id);
        machine
            .transition(SessionState::Recovering)
            .map_err(|e| anyhow::anyhow!("recovery transition failed: {e}"))?;
    }

    let offer = RecoveryOffer::from_data(&data);
    info!(
        session_id = %offer.session_id,
        state     = %offer.interrupted_state,
        chunks    = offer.transcript_chunk_count,
        responses = offer.response_count,
        "crash recovery offer built"
    );

    Ok(Some(offer))
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

/// Discard a crashed session: delete local data and return to `IDLE`.
pub async fn discard_session(
    persistence: Arc<SessionPersistence>,
    state_machine: &Mutex<SessionStateMachine>,
) -> Result<()> {
    let sid = {
        let machine = state_machine.lock().await;
        machine.session_id()
    };

    if let Some(id) = sid {
        if let Err(e) = persistence.clear_session(id) {
            tracing::warn!(session_id = %id, error = %e, "failed to clear crashed session data");
        }
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
