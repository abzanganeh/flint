use std::sync::Arc;

use tauri::{AppHandle, State};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::digest::extract_digest;
use crate::dto::{
    DigestDto, HardwareProfileDto, HealthCheckResultDto, SessionConfigDto, SessionSnapshotDto,
    UserDto,
};
use crate::events::{emit_session_state_change, SessionStateChangePayload};
use crate::health::{checks, hardware};
use crate::interfaces::auth::AuthToken;
use crate::interfaces::vector::Chunk;
use crate::keychain;
use crate::orchestrator::prewarm::{run_prewarm, PreWarmCache};
use crate::rag::chunker::chunk_text;
use crate::session::state::SessionState;
use crate::state::AppState;

const GENERIC_AUTH_ERROR: &str = "Authentication failed. Please try again.";
const KEYCHAIN_SAVE_ERROR: &str = "Could not save credentials. Please try again.";
const NOT_LOGGED_IN: &str = "You are not logged in. Please sign in again.";

const NO_ACTIVE_SESSION: &str = "No active session. Please create a session first.";
const SESSION_ID_MISMATCH: &str =
    "Session ID does not match the active session. Refresh and try again.";

// ──────────────────────────────────────────────────────────────────────────────
// Error helpers
// ──────────────────────────────────────────────────────────────────────────────

fn map_user_error(err: anyhow::Error) -> String {
    let msg = err.to_string();
    if is_user_facing_auth_message(&msg) {
        msg
    } else {
        GENERIC_AUTH_ERROR.to_string()
    }
}

fn is_user_facing_auth_message(msg: &str) -> bool {
    matches!(
        msg,
        "Invalid credentials"
            | "Too many attempts, try again later"
            | "Flint could not reach the auth service. Check your connection."
            | "Authentication failed. Please try again."
            | "Could not read credentials. Please log in again."
            | "Could not save credentials. Please try again."
            | NOT_LOGGED_IN
    ) || msg.starts_with("Supabase URL is not configured")
        || msg.starts_with("Supabase anon key is not configured")
}

fn session_error(err: anyhow::Error) -> String {
    error!(error = %err, "session command error");
    err.to_string()
}

async fn persist_auth_token(state: &AppState, token: AuthToken) -> Result<(), String> {
    keychain::store_auth_token(&token).map_err(|_| KEYCHAIN_SAVE_ERROR.to_string())?;
    state.set_auth_token(Some(token)).await;
    Ok(())
}

async fn active_auth_token(state: &AppState) -> Result<AuthToken, String> {
    if let Some(token) = state.auth_token().await {
        return Ok(token);
    }
    let token = keychain::get_auth_token().map_err(|_| NOT_LOGGED_IN.to_string())?;
    state.set_auth_token(Some(token.clone())).await;
    Ok(token)
}

/// Validate that `session_id` string parses and matches the active session.
async fn validate_session_id(
    state: &AppState,
    session_id: &str,
) -> Result<Uuid, String> {
    let id = session_id
        .parse::<Uuid>()
        .map_err(|_| "Invalid session ID format.".to_string())?;

    let machine = state.state_machine.lock().await;
    match machine.session_id() {
        None => Err(NO_ACTIVE_SESSION.to_string()),
        Some(active) if active != id => Err(SESSION_ID_MISMATCH.to_string()),
        Some(active) => Ok(active),
    }
}

fn emit_state(app: &AppHandle, state: SessionState) {
    emit_session_state_change(
        app,
        SessionStateChangePayload {
            state: state.as_str().to_string(),
        },
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Auth commands (Phase 1)
// ──────────────────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_legal_consent_accepted() -> bool {
    keychain::is_legal_consent_accepted()
}

#[tauri::command]
pub fn set_legal_consent_accepted() -> Result<(), String> {
    keychain::set_legal_consent_accepted().map_err(|_| KEYCHAIN_SAVE_ERROR.to_string())
}

#[tauri::command]
pub async fn signup(
    state: State<'_, AppState>,
    email: String,
    password: String,
) -> Result<(), String> {
    state
        .auth
        .signup(&email, &password)
        .await
        .map(|_| ())
        .map_err(map_user_error)
}

#[tauri::command]
pub fn set_session_state(app: AppHandle, state: String) -> Result<(), String> {
    emit_session_state_change(&app, SessionStateChangePayload { state });
    Ok(())
}

#[tauri::command]
pub async fn login(
    state: State<'_, AppState>,
    email: String,
    password: String,
) -> Result<(), String> {
    let token = state
        .auth
        .login(&email, &password)
        .await
        .map_err(map_user_error)?;

    persist_auth_token(&state, token).await
}

#[tauri::command]
pub async fn logout(state: State<'_, AppState>) -> Result<(), String> {
    if let Ok(token) = active_auth_token(&state).await {
        let _ = state.auth.logout(&token).await;
    }
    let _ = keychain::clear_auth_token();
    state.set_auth_token(None).await;
    Ok(())
}

#[tauri::command]
pub async fn get_current_user(state: State<'_, AppState>) -> Result<UserDto, String> {
    let token = active_auth_token(&state).await?;
    let user = state
        .auth
        .get_current_user(&token)
        .await
        .map_err(map_user_error)?;
    Ok(UserDto::from(user))
}

#[tauri::command]
pub fn get_hardware_profile() -> HardwareProfileDto {
    HardwareProfileDto::from(hardware::assess_hardware())
}

#[tauri::command]
pub async fn run_health_check(
    state: State<'_, AppState>,
) -> Result<Vec<HealthCheckResultDto>, String> {
    let results = checks::run_health_check(&state.plugins).await;
    Ok(results.into_iter().map(HealthCheckResultDto::from).collect())
}

// ──────────────────────────────────────────────────────────────────────────────
// Session design commands (Phase 2)
// ──────────────────────────────────────────────────────────────────────────────

/// Create a new session and transition the state machine to CONFIGURING.
///
/// Returns the new `session_id` (UUID string) that the frontend must pass to
/// all subsequent session commands.
#[tauri::command]
pub async fn create_session(
    app: AppHandle,
    state: State<'_, AppState>,
    config: SessionConfigDto,
) -> Result<String, String> {
    let session_id = Uuid::new_v4();

    {
        let mut machine = state.state_machine.lock().await;

        // Guard: only allow create from Idle (or reset machine if needed).
        if *machine.current() != SessionState::Idle {
            return Err(format!(
                "Cannot create a session — current state is {}. \
                 End the active session first.",
                machine.current()
            ));
        }

        machine
            .set_session_id(session_id)
            .map_err(session_error)?;

        machine
            .transition(SessionState::Configuring)
            .map_err(session_error)?;
    }

    // Clear stale digest / cache from any previous session.
    *state.session_digest.write().await = None;
    *state.prewarm_cache.lock().await = PreWarmCache::new();

    info!(
        session_id = %session_id,
        name = %config.name,
        session_type = %config.session_type,
        domain = %config.domain,
        "session created",
    );
    emit_state(&app, SessionState::Configuring);

    Ok(session_id.to_string())
}

/// Chunk, embed, and store context text; then extract the digest.
///
/// Valid from: `CONFIGURING`.  
/// Emits: `INGESTING` immediately, then `DIGEST_REVIEW` when done.
#[tauri::command]
pub async fn ingest_context(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    text: String,
) -> Result<(), String> {
    let sid = validate_session_id(&state, &session_id).await?;

    // Validate we are in CONFIGURING before touching anything else.
    {
        let machine = state.state_machine.lock().await;
        if *machine.current() != SessionState::Configuring {
            return Err(format!(
                "ingest_context is only valid from CONFIGURING (current: {})",
                machine.current()
            ));
        }
    }

    // Transition → INGESTING (must succeed before we start work).
    {
        let mut machine = state.state_machine.lock().await;
        machine
            .transition(SessionState::Ingesting)
            .map_err(session_error)?;
    }
    emit_state(&app, SessionState::Ingesting);

    // ── 1. Chunk ─────────────────────────────────────────────────────────────
    let raw_chunks = chunk_text(&text, 200, 50);
    if raw_chunks.is_empty() {
        // Transition back to Configuring so the user can try again.
        let mut machine = state.state_machine.lock().await;
        let _ = machine.transition(SessionState::Configuring);
        emit_state(&app, SessionState::Configuring);
        return Err("Context text is empty — please paste your job description or notes.".to_string());
    }

    // ── 2. Embed ─────────────────────────────────────────────────────────────
    let refs: Vec<&str> = raw_chunks.iter().map(|s| s.as_str()).collect();
    let embeddings = tokio::task::spawn_blocking({
        let embedder = Arc::clone(&state.embedder);
        let refs_owned: Vec<String> = raw_chunks.clone();
        move || {
            let refs: Vec<&str> = refs_owned.iter().map(|s| s.as_str()).collect();
            embedder.embed_batch(&refs)
        }
    })
    .await
    .map_err(|e| format!("Embedder task panicked: {e}"))?
    .map_err(|e| format!("Embedding failed: {e}"))?;

    // ── 3. Build Chunk structs ────────────────────────────────────────────────
    let chunks: Vec<Chunk> = refs
        .iter()
        .zip(embeddings)
        .map(|(text, embedding)| Chunk {
            id: Uuid::new_v4(),
            text: text.to_string(),
            embedding,
            session_id: sid,
        })
        .collect();

    // ── 4. Ingest into vector store ───────────────────────────────────────────
    state
        .vector_store
        .ingest(sid, chunks)
        .await
        .map_err(|e| format!("Vector store ingestion failed: {e}"))?;

    // ── 5. Extract digest via LLM ─────────────────────────────────────────────
    let digest = extract_digest(&text, state.llm.as_ref())
        .await
        .map_err(|e| {
            warn!(error = %e, "digest extraction failed");
            format!("Digest extraction failed — try rephrasing your context. ({e})")
        })?;

    *state.session_digest.write().await = Some(digest);

    // ── 6. Transition → DIGEST_REVIEW ─────────────────────────────────────────
    {
        let mut machine = state.state_machine.lock().await;
        machine
            .transition(SessionState::DigestReview)
            .map_err(session_error)?;
    }
    emit_state(&app, SessionState::DigestReview);

    info!(session_id = %sid, chunks = raw_chunks.len(), "context ingested");
    Ok(())
}

/// Accept the (possibly edited) digest and trigger pre-warming.
///
/// Valid from: `DIGEST_REVIEW`.  
/// Emits: `PRE_WARMING` immediately, then `REHEARSING` when complete.
#[tauri::command]
pub async fn confirm_digest(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    digest: DigestDto,
) -> Result<(), String> {
    let sid = validate_session_id(&state, &session_id).await?;

    {
        let machine = state.state_machine.lock().await;
        if *machine.current() != SessionState::DigestReview {
            return Err(format!(
                "confirm_digest is only valid from DIGEST_REVIEW (current: {})",
                machine.current()
            ));
        }
    }

    // Store the user-edited digest.
    let digest_rust = crate::digest::Digest::from(digest);
    *state.session_digest.write().await = Some(digest_rust.clone());

    // Transition → PRE_WARMING.
    {
        let mut machine = state.state_machine.lock().await;
        machine
            .transition(SessionState::PreWarming)
            .map_err(session_error)?;
    }
    emit_state(&app, SessionState::PreWarming);

    // Run pre-warming. All 10 LLM calls are spawned concurrently inside
    // `run_prewarm` via `tokio::spawn`; the blocking `embed_batch` is
    // dispatched via `spawn_blocking` internally — no extra wrapping needed.
    let llm = Arc::clone(&state.llm);
    let embedder = Arc::clone(&state.embedder);
    let cache = Arc::clone(&state.prewarm_cache);

    if let Err(e) = run_prewarm(&digest_rust, llm, embedder, cache).await {
        // Pre-warm failures are non-fatal — the session can still proceed
        // without cached responses.
        warn!(session_id = %sid, error = %e, "pre-warm failed; continuing without cache");
    }

    // Transition → REHEARSING.
    {
        let mut machine = state.state_machine.lock().await;
        machine
            .transition(SessionState::Rehearsing)
            .map_err(session_error)?;
    }
    emit_state(&app, SessionState::Rehearsing);

    info!(session_id = %sid, "digest confirmed, pre-warm complete");
    Ok(())
}

/// Return the current digest for the active session.
///
/// Valid after INGESTING completes (i.e. from DIGEST_REVIEW onward).
#[tauri::command]
pub async fn get_digest(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<DigestDto, String> {
    validate_session_id(&state, &session_id).await?;

    let guard = state.session_digest.read().await;
    match guard.as_ref() {
        Some(digest) => Ok(DigestDto::from(digest.clone())),
        None => Err("Digest not yet available. Complete context ingestion first.".to_string()),
    }
}

/// Return the full current state snapshot for React resync.
///
/// Called after window focus is regained, app resume, or any missed event.
#[tauri::command]
pub async fn get_session_snapshot(
    state: State<'_, AppState>,
) -> Result<SessionSnapshotDto, String> {
    let machine = state.state_machine.lock().await;
    let session_id = machine.session_id();
    let current_state = machine.current().as_str().to_string();
    drop(machine);

    let digest = state
        .session_digest
        .read()
        .await
        .as_ref()
        .map(|d| DigestDto::from(d.clone()));

    Ok(SessionSnapshotDto {
        session_id,
        state: current_state,
        digest,
    })
}

// ──────────────────────────────────────────────────────────────────────────────
// Live session stubs (Phase 3)
// ──────────────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn start_session(_session_id: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn stop_session() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn trigger_response() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn cancel_inference() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn panic_hide_overlay() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn switch_provider(_name: String) -> Result<(), String> {
    Ok(())
}
