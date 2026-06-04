//! Integration test for Phase 6 crash recovery (task 6.6).
//!
//! Scenario: a session is left in LIVE state (simulating a crash), the app
//! restarts, `check_for_recovery` finds it, and the state machine transitions
//! IDLE → RECOVERING → READY on resume.

use std::sync::Arc;

use flint_lib::session::persistence::{
    Response, ResponseType, SessionPersistence, TranscriptChunk,
};
use flint_lib::session::recovery;
use flint_lib::session::state::{SessionState, SessionStateMachine};
use tokio::sync::Mutex;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn in_memory_persistence() -> Arc<SessionPersistence> {
    Arc::new(SessionPersistence::new(":memory:").expect("open :memory: db"))
}

fn fresh_machine() -> Mutex<SessionStateMachine> {
    Mutex::new(SessionStateMachine::new())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// Simulate a LIVE session left in SQLite (crash) and verify that
/// `check_for_recovery` detects it and transitions the machine to RECOVERING.
#[tokio::test]
async fn check_for_recovery_detects_live_session() {
    let persistence = in_memory_persistence();
    let sid = Uuid::new_v4();

    // Persist a session in LIVE state (crash simulation).
    persistence
        .write_state_transition(sid, &SessionState::Live)
        .expect("write LIVE state");

    // Write a transcript chunk and a response to confirm they are loadable.
    persistence
        .write_transcript_chunk(&TranscriptChunk {
            id: Uuid::new_v4(),
            session_id: sid,
            speaker: "System".to_string(),
            text: "Tell me about a challenge you faced.".to_string(),
            timestamp_ms: 1000,
        })
        .expect("write chunk");

    persistence
        .write_response(&Response {
            id: Uuid::new_v4(),
            session_id: sid,
            response_type: ResponseType::Directional,
            content: "That was a challenging situation…".to_string(),
            confidence: 0.85,
        })
        .expect("write response");

    // Simulate app restart: build a fresh state machine starting at IDLE.
    let machine = fresh_machine();

    let offer = recovery::check_for_recovery(&persistence, &machine)
        .await
        .expect("check_for_recovery failed");

    let offer = offer.expect("expected a RecoveryOffer for LIVE session");
    assert_eq!(offer.interrupted_state, "LIVE");
    assert_eq!(offer.transcript_chunk_count, 1);
    assert_eq!(offer.response_count, 1);

    // State machine must now be in RECOVERING.
    let current = *machine.lock().await.current();
    assert_eq!(current, SessionState::Recovering);
}

/// After recovery is detected, resume_session transitions RECOVERING → READY.
#[tokio::test]
async fn resume_session_transitions_to_ready() {
    let machine = fresh_machine();

    // Simulate what check_for_recovery does: restore to CRASHED then → RECOVERING.
    {
        let mut m = machine.lock().await;
        m.restore_state_for_recovery(SessionState::Crashed, Uuid::new_v4());
        m.transition(SessionState::Recovering)
            .expect("CRASHED → RECOVERING");
    }

    recovery::resume_session(&machine)
        .await
        .expect("resume_session failed");

    let current = *machine.lock().await.current();
    assert_eq!(current, SessionState::Ready);
}

/// Discard clears the SQLite data and returns to IDLE.
#[tokio::test]
async fn discard_session_clears_data_and_returns_idle() {
    let persistence = in_memory_persistence();
    let sid = Uuid::new_v4();

    persistence
        .write_state_transition(sid, &SessionState::Crashed)
        .expect("write CRASHED state");
    persistence
        .write_transcript_chunk(&TranscriptChunk {
            id: Uuid::new_v4(),
            session_id: sid,
            speaker: "System".to_string(),
            text: "Describe your experience.".to_string(),
            timestamp_ms: 500,
        })
        .expect("write chunk");

    // Simulate restart + detection: restore to CRASHED then → RECOVERING.
    let machine = fresh_machine();
    {
        let mut m = machine.lock().await;
        m.restore_state_for_recovery(SessionState::Crashed, sid);
        m.transition(SessionState::Recovering)
            .expect("CRASHED → RECOVERING");
    }

    recovery::discard_session(Arc::clone(&persistence), &machine)
        .await
        .expect("discard_session failed");

    // State machine must be back at IDLE.
    let current = *machine.lock().await.current();
    assert_eq!(current, SessionState::Idle);

    // SQLite data for that session must be gone.
    let offer = persistence
        .load_session_for_recovery(sid)
        .expect("load after discard");
    assert!(offer.is_none(), "session data should be cleared after discard");
}

/// No incomplete session in SQLite → check_for_recovery returns None.
#[tokio::test]
async fn no_recovery_needed_returns_none() {
    let persistence = in_memory_persistence();
    let machine = fresh_machine();

    let offer = recovery::check_for_recovery(&persistence, &machine)
        .await
        .expect("check_for_recovery failed");

    assert!(offer.is_none());
    // State machine remains at IDLE.
    let current = *machine.lock().await.current();
    assert_eq!(current, SessionState::Idle);
}
