//! Resume in-progress session setup after app restart.
//!
//! Crash recovery handles LIVE/CRASHED. Draft recovery handles CONFIGURING
//! through READY so users don't re-enter Session Design after every restart.

use anyhow::Result;
use tauri::AppHandle;
use tracing::{info, warn};

use crate::events::{emit_session_state_change, SessionStateChangePayload};
use crate::session::state::SessionState;
use crate::state::AppState;

const DRAFT_STATES: &[SessionState] = &[
    SessionState::Configuring,
    SessionState::Ingesting,
    SessionState::DigestReview,
    SessionState::PreWarming,
    SessionState::Rehearsing,
    SessionState::Ready,
];

/// Re-anchor the in-memory state machine to the most recent draft session in SQLite.
pub async fn restore_draft_session(app: &AppHandle, state: &AppState) -> Result<bool> {
    {
        let machine = state.state_machine.lock().await;
        if *machine.current() != SessionState::Idle || machine.session_id().is_some() {
            return Ok(false);
        }
    }

    let Some(session_id) = state.persistence.find_draft_session()? else {
        return Ok(false);
    };

    let meta = state.persistence.get_session_metadata(session_id)?;

    let mut restored_state = meta.state;
    if restored_state == SessionState::Ingesting {
        restored_state = SessionState::Configuring;
        if let Err(e) = state
            .persistence
            .write_state_transition(session_id, &restored_state)
        {
            warn!(session_id = %session_id, error = %e, "failed to persist INGESTING rollback");
        }
    } else if restored_state == SessionState::PreWarming {
        // Pre-warm is not resumable — return to digest review so the user can re-confirm.
        restored_state = SessionState::DigestReview;
        if let Err(e) = state
            .persistence
            .write_state_transition(session_id, &restored_state)
        {
            warn!(session_id = %session_id, error = %e, "failed to persist PRE_WARMING rollback");
        }
    }

    if !DRAFT_STATES.contains(&restored_state) {
        return Ok(false);
    }

    if let Some(digest) = state.persistence.load_session_digest(session_id)? {
        *state.session_digest.write().await = Some(digest);
    }

    {
        let mut machine = state.state_machine.lock().await;
        machine.restore_state_for_recovery(restored_state, session_id);
    }

    emit_session_state_change(
        app,
        SessionStateChangePayload {
            state: restored_state.as_str().to_string(),
        },
    );

    info!(
        session_id = %session_id,
        state = %restored_state,
        name = %meta.name,
        "draft session restored from SQLite"
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::digest::Digest;
    use crate::session::persistence::SessionPersistence;
    use crate::session::state::SessionState;

    fn test_persistence() -> Arc<SessionPersistence> {
        Arc::new(SessionPersistence::new(":memory:").expect("in-memory db"))
    }

    #[test]
    fn find_draft_session_returns_rehearsing_row() {
        let db = test_persistence();
        let sid = uuid::Uuid::new_v4();
        db.create_session_row(sid, "Draft", "interview", "swe")
            .unwrap();
        db.write_state_transition(sid, &SessionState::Rehearsing)
            .unwrap();

        let found = db.find_draft_session().unwrap();
        assert_eq!(found, Some(sid));
    }

    #[test]
    fn find_draft_session_ignores_ended_sessions() {
        let db = test_persistence();
        let sid = uuid::Uuid::new_v4();
        db.create_session_row(sid, "Done", "interview", "swe").unwrap();
        db.write_state_transition(sid, &SessionState::Ended).unwrap();

        assert!(db.find_draft_session().unwrap().is_none());
    }

    #[test]
    fn digest_round_trip() {
        let db = test_persistence();
        let sid = uuid::Uuid::new_v4();
        db.create_session_row(sid, "Draft", "interview", "swe")
            .unwrap();
        let digest = Digest {
            role: "Engineer".into(),
            company: "Acme".into(),
            domain: "swe".into(),
            key_skills: vec!["Rust".into()],
            seniority: "senior".into(),
            likely_questions: vec!["Tell me about yourself".into()],
            topics_to_avoid: vec![],
        };
        db.store_session_digest(sid, &digest).unwrap();
        let loaded = db.load_session_digest(sid).unwrap().expect("digest stored");
        assert_eq!(loaded.role, "Engineer");
    }
}
