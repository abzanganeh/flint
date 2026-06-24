//! Integration test for Phase 6 crash recovery (task 6.6).
//!
//! Scenario: a session is left in LIVE state (simulating a crash), the app
//! restarts, `check_for_recovery` finds it, and the state machine transitions
//! IDLE → RECOVERING → READY on resume.

use std::sync::Arc;

use flint_lib::interfaces::vector::VectorInterface;
use flint_lib::rag::store::SqliteVecStore;
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

fn in_memory_vector_store() -> Arc<dyn VectorInterface> {
    Arc::new(SqliteVecStore::new(":memory:").expect("open :memory: vec store"))
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
            label_source: "channel".to_string(),
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
    // Phase 7.5: startup flips LIVE/ENDING/RECOVERING to CRASHED before
    // loading, so the offer always reports as CRASHED.
    assert_eq!(offer.interrupted_state, "CRASHED");
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
            label_source: "channel".to_string(),
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

    let vector_store = in_memory_vector_store();
    recovery::discard_session(
        Arc::clone(&persistence),
        Arc::clone(&vector_store),
        &machine,
    )
    .await
    .expect("discard_session failed");

    // State machine must be back at IDLE.
    let current = *machine.lock().await.current();
    assert_eq!(current, SessionState::Idle);

    // SQLite data for that session must be gone.
    let offer = persistence
        .load_session_for_recovery(sid)
        .expect("load after discard");
    assert!(
        offer.is_none(),
        "session data should be cleared after discard"
    );
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
    let current = *machine.lock().await.current();
    assert_eq!(current, SessionState::Idle);
}

// ── Phase 7.5 hardening ──────────────────────────────────────────────────────

/// A session stuck in `RECOVERING` (i.e. crashed during a recovery flow) must
/// be re-offered on the next startup. Pre-7.5 the filter only looked at
/// LIVE/ENDING/CRASHED, so RECOVERING leaked forever.
#[tokio::test]
async fn recovering_state_is_picked_up_on_restart() {
    let persistence = in_memory_persistence();
    let sid = Uuid::new_v4();

    persistence
        .write_state_transition(sid, &SessionState::Recovering)
        .expect("write RECOVERING state");

    let machine = fresh_machine();
    let offer = recovery::check_for_recovery(&persistence, &machine)
        .await
        .expect("check_for_recovery")
        .expect("RECOVERING session must trigger recovery");
    assert_eq!(offer.session_id, sid.to_string());
    // mark_stale_sessions_as_crashed flips RECOVERING -> CRASHED first, so
    // the loaded state reports as CRASHED.
    assert_eq!(offer.interrupted_state, "CRASHED");
}

/// Multiple incomplete sessions: every one is flipped to CRASHED at startup,
/// the most recent is offered, and the offer reports the leftover count.
#[tokio::test]
async fn multiple_incomplete_sessions_are_marked_crashed_and_counted() {
    let persistence = in_memory_persistence();
    let older = Uuid::new_v4();
    let middle = Uuid::new_v4();
    let newest = Uuid::new_v4();

    // Insertion order = rowid order; the `(updated_at DESC, rowid DESC)`
    // tiebreaker means the newest insert wins even when they all land in the
    // same SQLite second.
    persistence
        .write_state_transition(older, &SessionState::Live)
        .unwrap();
    persistence
        .write_state_transition(middle, &SessionState::Ending)
        .unwrap();
    persistence
        .write_state_transition(newest, &SessionState::Recovering)
        .unwrap();

    let machine = fresh_machine();
    let offer = recovery::check_for_recovery(&persistence, &machine)
        .await
        .expect("check_for_recovery")
        .expect("expected an offer");

    assert_eq!(
        offer.session_id,
        newest.to_string(),
        "most recent updated row wins"
    );
    assert_eq!(
        offer.additional_crashed_count, 2,
        "the other two incomplete sessions should be surfaced as triage backlog"
    );

    // Every incomplete row should now be CRASHED in the audit log.
    for id in [older, middle, newest] {
        let data = persistence
            .load_session_for_recovery(id)
            .unwrap()
            .expect("still recoverable");
        assert_eq!(data.state, SessionState::Crashed);
    }
}

/// `discard_all_crashed` clears every incomplete row including the active
/// recovery and drives the state machine back to IDLE.
#[tokio::test]
async fn discard_all_crashed_clears_every_row_and_returns_idle() {
    let persistence = in_memory_persistence();
    let vector_store = in_memory_vector_store();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    persistence
        .write_state_transition(a, &SessionState::Live)
        .unwrap();
    persistence
        .write_state_transition(b, &SessionState::Crashed)
        .unwrap();

    let machine = fresh_machine();
    let offer = recovery::check_for_recovery(&persistence, &machine)
        .await
        .unwrap()
        .expect("offer");
    assert!(offer.additional_crashed_count >= 1);

    let cleared = recovery::discard_all_crashed(
        Arc::clone(&persistence),
        Arc::clone(&vector_store),
        &machine,
    )
    .await
    .expect("discard_all_crashed");

    assert_eq!(cleared.len(), 2);
    assert!(persistence.list_incomplete_sessions().unwrap().is_empty());
    let current = *machine.lock().await.current();
    assert_eq!(current, SessionState::Idle);
}

/// A second `check_for_recovery` call (e.g. user closed the prompt and the
/// app re-checked) must not re-promote the same session past RECOVERING.
#[tokio::test]
async fn double_check_for_recovery_is_safe() {
    let persistence = in_memory_persistence();
    let sid = Uuid::new_v4();
    persistence
        .write_state_transition(sid, &SessionState::Live)
        .unwrap();

    let machine = fresh_machine();
    let _ = recovery::check_for_recovery(&persistence, &machine)
        .await
        .unwrap();

    // The state machine is now RECOVERING. Calling check_for_recovery again
    // attempts to re-anchor at CRASHED and step → RECOVERING, which the
    // transition guard rejects (RECOVERING → RECOVERING is invalid). The
    // function should return Err rather than silently corrupting state.
    let result = recovery::check_for_recovery(&persistence, &machine).await;
    assert!(
        result.is_err(),
        "second check should refuse to clobber an active RECOVERING machine"
    );
}
