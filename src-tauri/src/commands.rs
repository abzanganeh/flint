use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use tauri::{AppHandle, Emitter, Manager, State};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::audio::capture::AudioCapture;
use crate::audio::pipeline::{run_audio_pipeline, DetectedQuestion};
use crate::digest::extract_digest;
use crate::dto::{
    AppendResearchResultDto, DigestDto, HardwareProfileDto, HealthCheckResultDto, SessionConfigDto,
    SessionContextFieldsDto, SessionSnapshotDto, SmartResumeImportDto, UserDto, WebSourceDto,
};
use crate::events::{
    emit_session_state_change, emit_token_usage_update, SessionStateChangePayload,
    TokenUsageUpdatePayload,
};
use crate::health::{checks, hardware};
use crate::interfaces::auth::AuthToken;
use crate::interfaces::vector::Chunk;
use crate::keychain;
use crate::llm::failover::FailoverManager;
use crate::llm::groq::GroqProvider;
use crate::llm::ollama::OllamaProvider;
use crate::llm::provider::{CompletionConfig, LLMProvider};
use crate::llm::rate_limiter::RateLimiter;
use crate::orchestrator::prewarm::{run_prewarm, PreWarmCache};
use crate::orchestrator::{dispatch_turn, run_orchestrator, OrchestratorConfig};
use crate::rag::chunker::chunk_text;
use crate::research::{self, tavily, ResearchSource};

/// Approximate base chars added to a research prompt (template + labels).
const RESEARCH_PROMPT_OVERHEAD_CHARS: usize = 800;
use crate::session::draft;
use crate::session::memory::ConversationMemory;
use crate::session::recovery;
use crate::session::state::SessionState;
use crate::smart_resume;
use crate::state::{AppState, LiveTaskHandles};
use crate::transcription::detector::QuestionDetector;
use crate::transcription::engine::WhisperEngine;

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
async fn validate_session_id(state: &AppState, session_id: &str) -> Result<Uuid, String> {
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
    // Wrap immediately so the password buffer is zeroed on drop
    // (flint-security.mdc §"Hard Constraints"). The original `password`
    // moves into `SecretString` and is dropped at the end of this scope.
    let password = SecretString::new(password);
    state
        .auth
        .signup(&email, password.expose_secret())
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
    let password = SecretString::new(password);
    let token = state
        .auth
        .login(&email, password.expose_secret())
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
    Ok(results
        .into_iter()
        .map(HealthCheckResultDto::from)
        .collect())
}

// ──────────────────────────────────────────────────────────────────────────────
// Session design commands (Phase 2)
// ──────────────────────────────────────────────────────────────────────────────

/// LLM for digest extraction: Groq when a key is in the keychain, else stub.
fn llm_for_digest(state: &AppState) -> Arc<dyn LLMProvider> {
    match keychain::get_api_key("groq") {
        Ok(api_key) => match GroqProvider::new(api_key) {
            Ok(provider) => Arc::new(provider),
            Err(e) => {
                warn!(error = %e, "Groq provider init failed for digest — using stub");
                Arc::clone(&state.llm)
            }
        },
        Err(_) => Arc::clone(&state.llm),
    }
}

/// Discard an in-progress session setup and return to IDLE (Start Over).
#[tauri::command]
pub async fn abandon_session_draft(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let allowed = {
        let machine = state.state_machine.lock().await;
        matches!(
            *machine.current(),
            SessionState::Configuring
                | SessionState::Ingesting
                | SessionState::DigestReview
                | SessionState::PreWarming
                | SessionState::Rehearsing
                | SessionState::Ready
        )
    };
    if !allowed {
        return Err(format!(
            "Cannot abandon session draft from state {}",
            state.state_machine.lock().await.current()
        ));
    }

    let sid = {
        let machine = state.state_machine.lock().await;
        machine.session_id()
    };

    *state.session_digest.write().await = None;
    *state.prewarm_cache.lock().await = PreWarmCache::new();
    *state.rehearsal_turn.lock().await = 0;
    if let Some(session_id) = sid {
        if let Err(e) = state.persistence.clear_session(session_id) {
            warn!(session_id = %session_id, error = %e, "failed to clear abandoned draft session");
        }
    }
    {
        let mut machine = state.state_machine.lock().await;
        machine.reset_to_idle();
    }
    emit_state(&app, SessionState::Idle);
    Ok(())
}

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

        machine.set_session_id(session_id).map_err(session_error)?;

        machine
            .transition(SessionState::Configuring)
            .map_err(session_error)?;
    }

    // Clear stale digest / cache from any previous session.
    *state.session_digest.write().await = None;
    *state.prewarm_cache.lock().await = PreWarmCache::new();
    *state.rehearsal_turn.lock().await = 0;
    *state.session_memory.lock().await = None;

    // Persist session row with metadata for the session list.
    state
        .persistence
        .create_session_row(
            session_id,
            &config.name,
            &config.session_type,
            &config.domain,
        )
        .map_err(session_error)?;

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

/// Roll back a failed ingest so Session Design can retry without creating a new session.
async fn rollback_ingest_to_configuring(state: &AppState, app: &AppHandle) {
    let mut machine = state.state_machine.lock().await;
    if *machine.current() == SessionState::Ingesting {
        match machine.transition(SessionState::Configuring) {
            Ok(()) => emit_state(app, SessionState::Configuring),
            Err(e) => warn!(error = %e, "failed to rollback INGESTING to CONFIGURING"),
        }
    }
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
        rollback_ingest_to_configuring(&state, &app).await;
        return Err(
            "Context text is empty — please paste your job description or notes.".to_string(),
        );
    }

    // Persist raw text for session cloning (best-effort — non-fatal).
    if let Err(e) = state.persistence.store_context_text(sid, &text) {
        warn!(session_id = %sid, error = %e, "failed to store context text");
    }

    let ingest_result: Result<(), String> = async {
        // ── 2. Embed ─────────────────────────────────────────────────────────
        let embedder = state
            .wait_for_embedder(Duration::from_secs(120))
            .await
            .map_err(|e| format!("Embedding failed: {e}"))?;
        let raw_chunks_owned = raw_chunks.clone();
        let embeddings = tokio::task::spawn_blocking(move || {
            let refs: Vec<&str> = raw_chunks_owned.iter().map(|s| s.as_str()).collect();
            embedder.embed_batch(&refs)
        })
        .await
        .map_err(|e| format!("Embedder task panicked: {e}"))?
        .map_err(|e| format!("Embedding failed: {e}"))?;

        // ── 3. Build Chunk structs ───────────────────────────────────────────
        let refs: Vec<&str> = raw_chunks.iter().map(|s| s.as_str()).collect();
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

        // ── 4. Ingest into vector store ──────────────────────────────────────
        state
            .vector_store
            .ingest(sid, chunks)
            .await
            .map_err(|e| format!("Vector store ingestion failed: {e}"))?;

        // ── 5. Extract digest via LLM ────────────────────────────────────────
        let llm = llm_for_digest(&state);
        let digest = extract_digest(&text, llm.as_ref()).await.map_err(|e| {
            warn!(error = %e, "digest extraction failed");
            format!("Digest extraction failed — try rephrasing your context. ({e})")
        })?;

        *state.session_digest.write().await = Some(digest.clone());
        if let Err(e) = state.persistence.store_session_digest(sid, &digest) {
            warn!(session_id = %sid, error = %e, "failed to persist session digest");
        }

        // ── 6. Transition → DIGEST_REVIEW ────────────────────────────────────
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
    .await;

    if ingest_result.is_err() {
        rollback_ingest_to_configuring(&state, &app).await;
    }
    ingest_result
}

// ──────────────────────────────────────────────────────────────────────────────
// Phase 5.5.1 — structured Session Design context fields
// ──────────────────────────────────────────────────────────────────────────────

/// Assemble a single labelled RAG blob from structured context fields.
///
/// Only non-empty fields contribute a section so the embedding model can
/// distinguish signal types without noise from blank placeholders.
fn assemble_rag_blob(fields: &SessionContextFieldsDto) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(7);
    let mut push = |label: &str, text: &str| {
        let t = text.trim();
        if !t.is_empty() {
            parts.push(format!("[{label}]\n{t}"));
        }
    };
    push("JOB DESCRIPTION", &fields.job_description);
    push("YOUR PROFILE", &fields.profile);
    push("COMPANY OVERVIEW", &fields.company_overview);
    push("LEADERSHIP PRINCIPLES", &fields.leadership_principles);
    push("ROLE EXPECTATIONS", &fields.role_expectations);
    push("TECHNICAL PREPARATION", &fields.technical_prep);
    push("STRATEGY NOTES", &fields.strategy_notes);
    parts.join("\n\n")
}

/// Chunk, embed, and store structured Session Design context; extract digest.
///
/// Replaces the freeform `ingest_context` for v1.5 sessions. Each field is
/// stored in its own SQLite column (for exact draft restore), and all non-empty
/// fields are assembled into a labelled RAG blob before embedding.
///
/// Required fields (`job_description`, `profile`) must be non-empty or an
/// error is returned before any state transition.
///
/// Valid from: `CONFIGURING`.  
/// Emits: `INGESTING` immediately, then `DIGEST_REVIEW` when done.
#[tauri::command]
pub async fn ingest_structured_context(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    fields: SessionContextFieldsDto,
) -> Result<(), String> {
    let sid = validate_session_id(&state, &session_id).await?;

    if fields.job_description.trim().is_empty() {
        return Err("Job description is required — please paste the job posting.".to_string());
    }
    if fields.profile.trim().is_empty() {
        return Err("Your profile is required — please paste your resume or bio.".to_string());
    }

    {
        let machine = state.state_machine.lock().await;
        if *machine.current() != SessionState::Configuring {
            return Err(format!(
                "ingest_structured_context is only valid from CONFIGURING (current: {})",
                machine.current()
            ));
        }
    }

    // Transition → INGESTING before any work begins.
    {
        let mut machine = state.state_machine.lock().await;
        machine
            .transition(SessionState::Ingesting)
            .map_err(session_error)?;
    }
    emit_state(&app, SessionState::Ingesting);

    let blob = assemble_rag_blob(&fields);
    let raw_chunks = chunk_text(&blob, 200, 50);
    if raw_chunks.is_empty() {
        rollback_ingest_to_configuring(&state, &app).await;
        return Err(
            "Assembled context is empty — check that required fields are filled.".to_string(),
        );
    }

    // Persist the assembled blob (context_text) and each individual field so
    // draft restore can repopulate the form precisely.
    if let Err(e) = state.persistence.store_context_text(sid, &blob) {
        warn!(session_id = %sid, error = %e, "failed to store context blob");
    }
    let domain_fields = crate::session::persistence::SessionContextFields::from(fields.clone());
    if let Err(e) = state.persistence.store_context_fields(sid, &domain_fields) {
        warn!(session_id = %sid, error = %e, "failed to store context fields");
    }

    let ingest_result: Result<(), String> = async {
        // ── 1. Embed ──────────────────────────────────────────────────────────
        let embedder = state
            .wait_for_embedder(Duration::from_secs(120))
            .await
            .map_err(|e| format!("Embedding failed: {e}"))?;
        let raw_chunks_owned = raw_chunks.clone();
        let embeddings = tokio::task::spawn_blocking(move || {
            let refs: Vec<&str> = raw_chunks_owned.iter().map(|s| s.as_str()).collect();
            embedder.embed_batch(&refs)
        })
        .await
        .map_err(|e| format!("Embedder task panicked: {e}"))?
        .map_err(|e| format!("Embedding failed: {e}"))?;

        // ── 2. Ingest into vector store ───────────────────────────────────────
        let refs: Vec<&str> = raw_chunks.iter().map(|s| s.as_str()).collect();
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
        state
            .vector_store
            .ingest(sid, chunks)
            .await
            .map_err(|e| format!("Vector store ingestion failed: {e}"))?;

        // ── 3. Extract digest via LLM ─────────────────────────────────────────
        let llm = llm_for_digest(&state);
        let digest = extract_digest(&blob, llm.as_ref()).await.map_err(|e| {
            warn!(error = %e, "digest extraction failed");
            format!("Digest extraction failed — try rephrasing your context. ({e})")
        })?;

        *state.session_digest.write().await = Some(digest.clone());
        if let Err(e) = state.persistence.store_session_digest(sid, &digest) {
            warn!(session_id = %sid, error = %e, "failed to persist session digest");
        }

        // ── 4. Transition → DIGEST_REVIEW ─────────────────────────────────────
        {
            let mut machine = state.state_machine.lock().await;
            machine
                .transition(SessionState::DigestReview)
                .map_err(session_error)?;
        }
        emit_state(&app, SessionState::DigestReview);

        info!(session_id = %sid, chunks = raw_chunks.len(), "structured context ingested");
        Ok(())
    }
    .await;

    if ingest_result.is_err() {
        rollback_ingest_to_configuring(&state, &app).await;
    }
    ingest_result
}

/// Load persisted structured context fields for the given session.
///
/// Used by draft restore to re-populate the Session Design form exactly as
/// the user left it. All fields default to empty string for sessions created
/// before the v6 migration — callers check `job_description.is_empty()` to
/// detect legacy sessions.
#[tauri::command]
pub async fn get_session_context_fields(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<SessionContextFieldsDto, String> {
    let sid = Uuid::parse_str(&session_id).map_err(|e| format!("Invalid session ID: {e}"))?;
    state
        .persistence
        .load_context_fields(sid)
        .map(SessionContextFieldsDto::from)
        .map_err(|e| e.to_string())
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
    if let Err(e) = state.persistence.store_session_digest(sid, &digest_rust) {
        warn!(session_id = %sid, error = %e, "failed to persist confirmed digest");
    }

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
    let embedder = state.require_embedder()?;
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

/// Return the raw context text persisted for a session (for cloning / re-open).
///
/// Does not require the session to be the active in-memory session.
#[tauri::command]
pub async fn get_session_context(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let sid = Uuid::parse_str(&session_id).map_err(|e| format!("Invalid session ID: {e}"))?;
    state
        .persistence
        .get_session_context(sid)
        .map_err(|e| e.to_string())
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

    let sid = Uuid::parse_str(&session_id).map_err(|_| "Invalid session_id format".to_string())?;

    // Fast path: digest is still warm in memory.
    {
        let guard = state.session_digest.read().await;
        if let Some(digest) = guard.as_ref() {
            return Ok(DigestDto::from(digest.clone()));
        }
    }

    // Cold path: app restarted since digest was generated — load from SQLite
    // and repopulate the in-memory cache so subsequent calls are free.
    match state.persistence.load_session_digest(sid) {
        Ok(Some(digest)) => {
            *state.session_digest.write().await = Some(digest.clone());
            Ok(DigestDto::from(digest))
        }
        Ok(None) => Err("Digest not yet available. Complete context ingestion first.".to_string()),
        Err(e) => {
            tracing::warn!(error = %e, "get_digest: SQLite fallback failed");
            Err("Digest not yet available. Complete context ingestion first.".to_string())
        }
    }
}

/// Redeem a single-use Smart Resume handoff token and return session pre-fill data.
#[tauri::command]
pub async fn import_from_smart_resume(token: String) -> Result<SmartResumeImportDto, String> {
    info!(
        event = "smart_resume_import.started",
        "redeeming Smart Resume handoff token"
    );
    match smart_resume::redeem_handoff_token(&token).await {
        Ok(dto) => {
            info!(
                event = "smart_resume_import.success",
                session_type = %dto.session_type,
                domain = %dto.domain,
                export_version = dto.export_version,
                "Smart Resume handoff token redeemed"
            );
            Ok(dto)
        }
        Err(ref e) => {
            warn!(event = "smart_resume_import.error", error = %e, "Smart Resume handoff redeem failed");
            Err(e.clone())
        }
    }
}

/// Return and clear the import token stored at cold start.
///
/// Called once by React during bootstrap. Returns `None` after the first call
/// or when no token was present (cold start without a deep link).
#[tauri::command]
pub async fn get_pending_import_token(
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    Ok(state.pending_import_token.lock().await.take())
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

    let digest = {
        let mem = state.session_digest.read().await.clone();
        if mem.is_some() {
            mem.map(DigestDto::from)
        } else if let Some(sid) = session_id {
            state
                .persistence
                .load_session_digest(sid)
                .map_err(|e| e.to_string())?
                .map(DigestDto::from)
        } else {
            None
        }
    };

    let resume_meta = session_id.and_then(|sid| state.persistence.get_session_metadata(sid).ok());

    Ok(SessionSnapshotDto {
        session_id,
        state: current_state,
        digest,
        name: resume_meta.as_ref().map(|m| m.name.clone()),
        session_type: resume_meta.as_ref().map(|m| m.session_type.clone()),
        domain: resume_meta.as_ref().map(|m| m.domain.clone()),
        context_text: resume_meta
            .as_ref()
            .filter(|m| !m.context_text.is_empty())
            .map(|m| m.context_text.clone()),
        context_fields: resume_meta
            .as_ref()
            .map(|m| SessionContextFieldsDto::from(m.context_fields.clone())),
    })
}

/// Re-anchor the state machine to the most recent pre-live draft in SQLite.
///
/// Called once at startup (after crash recovery check). Returns `true` when a
/// draft was restored.
#[tauri::command]
pub async fn restore_draft_session(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    draft::restore_draft_session(&app, &state)
        .await
        .map_err(|e| e.to_string())
}

// ──────────────────────────────────────────────────────────────────────────────
// Live session commands (Phase 3)
// ──────────────────────────────────────────────────────────────────────────────

/// Resolve the `prompts/` base directory.
///
/// Mirrors `digest.rs::prompts_base_dir()` — single source of truth for the
/// prompt layout. The `FLINT_PROMPTS_DIR` env var overrides the default so
/// integration tests and bundled releases can point to the right location.
fn prompts_base_dir() -> PathBuf {
    std::env::var("FLINT_PROMPTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("prompts")
        })
}

/// Resolve the ggml model path for the given hardware profile.
///
/// Whisper model files are expected at `~/.cache/whisper/ggml-<name>.bin`
/// (standard whisper.cpp convention). The health check (`checks.rs`) verifies
/// the file exists before `start_session` is ever called.
fn whisper_model_path(profile: &hardware::HardwareProfile) -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let filename = format!("ggml-{}.bin", profile.recommended_whisper_model);
    PathBuf::from(home)
        .join(".cache")
        .join("whisper")
        .join(filename)
        .to_string_lossy()
        .into_owned()
}

/// Abort all live tasks and signal the capture thread to stop.
///
/// Called on `start_session` failure after tasks have been spawned, to prevent
/// a hidden audio pipeline from running while the state machine is not LIVE.
async fn abort_live_tasks(state: &AppState) {
    if let Some(handles) = state.live_tasks.lock().await.take() {
        let _ = handles.stop_tx.send(());
        handles.pipeline.abort();
        handles.orchestrator.abort();
    }
}

/// Build the failover manager and local LLM provider used by live and rehearsal paths.
async fn build_failover_stack(
    app: &AppHandle,
    state: &AppState,
) -> Result<(Arc<FailoverManager>, Arc<dyn LLMProvider>, usize), String> {
    let (primary_provider, context_window) = match keychain::get_api_key("groq") {
        Ok(api_key) => match GroqProvider::new(api_key) {
            Ok(p) => {
                let cw = p.context_window();
                (Arc::new(p) as Arc<dyn LLMProvider>, cw)
            }
            Err(e) => {
                warn!(error = %e, "Failed to build Groq provider — using stub");
                let stub = Arc::clone(&state.llm);
                let cw = stub.context_window();
                (stub, cw)
            }
        },
        Err(_) => {
            warn!("No Groq API key in keychain — using stub LLM provider");
            let stub = Arc::clone(&state.llm);
            let cw = stub.context_window();
            (stub, cw)
        }
    };

    let local_provider: Arc<dyn LLMProvider> = Arc::new(
        OllamaProvider::new().map_err(|e| format!("Failed to build Ollama provider: {e}"))?,
    );
    let rate_limiter = Arc::new(RateLimiter::new(
        primary_provider.name(),
        primary_provider.rate_limit().requests_per_minute,
        primary_provider.rate_limit().tokens_per_minute,
    ));
    let mut failover =
        FailoverManager::new(primary_provider, Arc::clone(&local_provider), rate_limiter);
    failover.start_ping_loop(app.clone());
    Ok((Arc::new(failover), local_provider, context_window))
}

fn load_compression_prompt() -> String {
    std::fs::read_to_string(prompts_base_dir().join("compression").join("default.txt"))
        .unwrap_or_else(|_| {
            "Summarise the conversation below in 3-5 sentences.\n\n{old_turns}\n\n[Summary]"
                .to_string()
        })
}

/// Whether the user completed the mandatory rehearsal on this device.
#[tauri::command]
pub fn get_rehearsal_completed() -> bool {
    keychain::is_rehearsal_completed()
}

/// Fire a single orchestrator turn during rehearsal (no audio pipeline).
#[tauri::command]
pub async fn run_rehearsal_turn(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    question: String,
    rephrase: Option<bool>,
) -> Result<(), String> {
    let sid = validate_session_id(&state, &session_id).await?;

    {
        let machine = state.state_machine.lock().await;
        if *machine.current() != SessionState::Rehearsing {
            return Err(format!(
                "run_rehearsal_turn is only valid from REHEARSING (current: {})",
                machine.current()
            ));
        }
    }

    let digest = state.session_digest.read().await.clone().ok_or_else(|| {
        "No session digest — complete digest review before rehearsal.".to_string()
    })?;

    let (failover, local_provider, context_window) = build_failover_stack(&app, &state).await?;

    let memory = {
        let mut guard = state.session_memory.lock().await;
        if guard.is_none() {
            *guard = Some(Arc::new(tokio::sync::Mutex::new(ConversationMemory::new(
                context_window,
            ))));
        }
        Arc::clone(guard.as_ref().unwrap())
    };

    let turn_number = {
        let mut turn = state.rehearsal_turn.lock().await;
        *turn += 1;
        *turn
    };

    let question_text = if rephrase.unwrap_or(false) {
        format!("Rephrase your previous answer to: {question}")
    } else {
        question
    };

    let turn_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));

    dispatch_turn(
        sid,
        question_text,
        turn_number,
        Arc::new(digest),
        prompts_base_dir(),
        failover,
        state.require_embedder()?,
        Arc::clone(&state.vector_store),
        Arc::clone(&state.prewarm_cache),
        memory,
        load_compression_prompt(),
        turn_cancel,
        local_provider,
        Arc::clone(&state.persistence),
        Arc::clone(&state.cost_tracker),
        app,
    )
    .await
    .map_err(|e| format!("Rehearsal turn failed: {e}"))?;

    Ok(())
}

/// Complete mandatory rehearsal and transition to READY.
#[tauri::command]
pub async fn complete_rehearsal(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    validate_session_id(&state, &session_id).await?;

    {
        let machine = state.state_machine.lock().await;
        if *machine.current() != SessionState::Rehearsing {
            return Err(format!(
                "complete_rehearsal is only valid from REHEARSING (current: {})",
                machine.current()
            ));
        }
    }

    keychain::set_rehearsal_completed().map_err(|_| KEYCHAIN_SAVE_ERROR.to_string())?;

    {
        let mut machine = state.state_machine.lock().await;
        machine
            .transition(SessionState::Ready)
            .map_err(session_error)?;
    }
    emit_state(&app, SessionState::Ready);

    info!(session_id = %session_id, "rehearsal completed");
    Ok(())
}

/// Leave live/rehearsal and return to Session Design to edit pasted context.
///
/// Valid from: `REHEARSING`, `READY`, `CONFIGURING`, or `ENDED` (after stop).
/// Call `stop_session` first when the machine is in `LIVE`.
#[tauri::command]
pub async fn return_to_session_design(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
) -> Result<SessionSnapshotDto, String> {
    let sid = validate_session_id(&state, &session_id).await?;

    let current = {
        let machine = state.state_machine.lock().await;
        *machine.current()
    };

    match current {
        SessionState::Rehearsing | SessionState::Ready | SessionState::Ended => {
            let mut machine = state.state_machine.lock().await;
            machine
                .transition(SessionState::Configuring)
                .map_err(session_error)?;
            emit_state(&app, SessionState::Configuring);
        }
        SessionState::Configuring => {}
        SessionState::Live => {
            return Err(
                "Session is still live — end the session first, then return to setup.".to_string(),
            );
        }
        other => {
            return Err(format!(
                "Cannot return to session design from {} (current session {})",
                other, session_id
            ));
        }
    }

    if let Ok(Some(digest)) = state.persistence.load_session_digest(sid) {
        *state.session_digest.write().await = Some(digest);
    }

    get_session_snapshot(state).await
}

// ──────────────────────────────────────────────────────────────────────────────
// Phase 5.5.3 — Question bank
// ──────────────────────────────────────────────────────────────────────────────

/// Return the question bank for a session.
///
/// Returns digest `likely_questions` merged with any user-added questions,
/// with duplicates removed. Order: digest Qs first (stable), then user-added.
#[tauri::command]
pub async fn get_question_bank(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<String>, String> {
    let sid = validate_session_id(&state, &session_id).await?;

    let persisted = state
        .persistence
        .load_question_bank(sid)
        .map_err(|e| e.to_string())?;

    if !persisted.is_empty() {
        return Ok(persisted);
    }

    // First call: seed from digest likely_questions.
    let seed: Vec<String> = state
        .session_digest
        .read()
        .await
        .as_ref()
        .map(|d| d.likely_questions.clone())
        .unwrap_or_default();

    if !seed.is_empty() {
        state
            .persistence
            .store_question_bank(sid, &seed)
            .map_err(|e| e.to_string())?;
    }

    Ok(seed)
}

/// Add a question to the session question bank (dedup by lowercase trim).
#[tauri::command]
pub async fn add_to_question_bank(
    state: State<'_, AppState>,
    session_id: String,
    question: String,
) -> Result<Vec<String>, String> {
    let sid = validate_session_id(&state, &session_id).await?;
    let trimmed = question.trim().to_string();
    if trimmed.is_empty() {
        return Err("Question must not be empty.".to_string());
    }

    let mut bank = state
        .persistence
        .load_question_bank(sid)
        .map_err(|e| e.to_string())?;

    let lower = trimmed.to_lowercase();
    if !bank.iter().any(|q| q.to_lowercase() == lower) {
        bank.push(trimmed);
        state
            .persistence
            .store_question_bank(sid, &bank)
            .map_err(|e| e.to_string())?;
    }

    Ok(bank)
}

/// Remove a question from the session question bank by exact match.
#[tauri::command]
pub async fn remove_from_question_bank(
    state: State<'_, AppState>,
    session_id: String,
    question: String,
) -> Result<Vec<String>, String> {
    let sid = validate_session_id(&state, &session_id).await?;

    let mut bank = state
        .persistence
        .load_question_bank(sid)
        .map_err(|e| e.to_string())?;

    bank.retain(|q| q != &question);
    state
        .persistence
        .store_question_bank(sid, &bank)
        .map_err(|e| e.to_string())?;

    Ok(bank)
}

// ──────────────────────────────────────────────────────────────────────────────
// Phase 5.5.6 / 5.6 — Research chat (RAG + prep web search)
// ──────────────────────────────────────────────────────────────────────────────

fn emit_research_citation(
    app: &AppHandle,
    source: ResearchSource,
    rag_chunks: &[String],
    web_sources: &[crate::interfaces::web_search::WebSearchResult],
) {
    let web_payload: Vec<serde_json::Value> = web_sources
        .iter()
        .map(|s| {
            serde_json::json!({
                "title": s.title,
                "url": s.url,
                "snippet": s.snippet,
            })
        })
        .collect();
    let can_add = source == ResearchSource::Web || source == ResearchSource::RagAndWeb;
    let _ = app.emit(
        "research_citation",
        serde_json::json!({
            "chunks": rag_chunks,
            "webSources": web_payload,
            "source": source.as_str(),
            "canAddToContext": can_add,
        }),
    );
}

fn record_research_usage(
    app: &AppHandle,
    state: &AppState,
    prompt_len: usize,
    response_len: usize,
) {
    let input_tokens = (prompt_len as u64 + 500) / 4;
    let output_tokens = (response_len as u64 + 100) / 4;
    let cost_estimate = (input_tokens + output_tokens) as f64 * 0.0000002;
    emit_token_usage_update(
        app,
        TokenUsageUpdatePayload {
            input: input_tokens,
            output: output_tokens,
            total: input_tokens + output_tokens,
            cost_estimate,
            usage_category: "research_chat".to_string(),
        },
    );
    let _ =
        state
            .cost_tracker
            .record_turn_with_transition(input_tokens, output_tokens, cost_estimate);
}

/// Run a single research chat turn: RAG when sufficient, otherwise web search (Tavily).
///
/// Valid from: `REHEARSING` only.
#[tauri::command]
pub async fn run_research_chat(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    message: String,
) -> Result<(), String> {
    let sid = validate_session_id(&state, &session_id).await?;

    {
        let machine = state.state_machine.lock().await;
        if *machine.current() != SessionState::Rehearsing {
            return Err(format!(
                "run_research_chat is only valid from REHEARSING (current: {})",
                machine.current()
            ));
        }
    }

    if state.cost_tracker.is_suspended() {
        return Err(
            "Inference is suspended because the cost cap was reached. Lift the cap or reset the tracker to continue."
                .to_string(),
        );
    }

    let message = message.trim().to_string();
    if message.is_empty() {
        return Err("Research message must not be empty.".to_string());
    }

    let embedder = state
        .wait_for_embedder(std::time::Duration::from_secs(30))
        .await
        .map_err(|e| format!("Embedding unavailable: {e}"))?;

    let embedding = tokio::task::spawn_blocking({
        let msg = message.clone();
        move || embedder.embed_batch(&[msg.as_str()])
    })
    .await
    .map_err(|e| format!("Embedder task panicked: {e}"))?
    .map_err(|e| format!("Embedding failed: {e}"))?
    .into_iter()
    .next()
    .ok_or_else(|| "Embedder returned no vectors.".to_string())?;

    let chunks = state
        .vector_store
        .query(sid, &embedding, 8)
        .await
        .map_err(|e| format!("RAG retrieval failed: {e}"))?;

    let llm = llm_for_digest(&state);
    let web = tavily::resolve_tavily();

    let outcome = research::run_prep_research_turn(&message, chunks, llm, web)
        .await
        .map_err(|e| format!("Research turn failed: {e}"))?;

    let _ = app.emit(
        "research_token",
        serde_json::json!({ "token": outcome.response }),
    );

    emit_research_citation(
        &app,
        outcome.source,
        &outcome.rag_citations,
        &outcome.web_sources,
    );

    record_research_usage(
        &app,
        &state,
        message.len() + RESEARCH_PROMPT_OVERHEAD_CHARS,
        outcome.response.len(),
    );

    Ok(())
}

/// Append a prep research answer (and optional web sources) into session RAG context.
///
/// Adds a labelled block to Technical Prep, re-chunks, embeds, and ingests.
/// Valid from: `REHEARSING` only.
#[tauri::command]
pub async fn append_research_to_context(
    state: State<'_, AppState>,
    session_id: String,
    question: String,
    answer: String,
    web_sources: Vec<WebSourceDto>,
) -> Result<AppendResearchResultDto, String> {
    let sid = validate_session_id(&state, &session_id).await?;

    {
        let machine = state.state_machine.lock().await;
        if *machine.current() != SessionState::Rehearsing {
            return Err(format!(
                "append_research_to_context is only valid from REHEARSING (current: {})",
                machine.current()
            ));
        }
    }

    let question = question.trim().to_string();
    let answer = answer.trim().to_string();
    if question.is_empty() || answer.is_empty() {
        return Err("Question and answer must not be empty.".to_string());
    }

    let web_hits: Vec<crate::interfaces::web_search::WebSearchResult> = web_sources
        .into_iter()
        .map(|s| crate::interfaces::web_search::WebSearchResult {
            title: s.title,
            url: s.url,
            snippet: s.snippet,
        })
        .collect();

    let block = research::format_research_append_block(&question, &answer, &web_hits);

    let mut fields = state
        .persistence
        .load_context_fields(sid)
        .map_err(|e| e.to_string())?;

    if !fields.technical_prep.trim().is_empty() {
        fields.technical_prep.push_str("\n\n");
    }
    fields.technical_prep.push_str(&block);
    state
        .persistence
        .store_context_fields(sid, &fields)
        .map_err(|e| e.to_string())?;

    let existing_text = state
        .persistence
        .get_session_context(sid)
        .map_err(|e| e.to_string())?;
    let merged = if existing_text.trim().is_empty() {
        format!("[TECHNICAL PREPARATION]\n{block}")
    } else {
        format!("{existing_text}\n\n[TECHNICAL PREPARATION — WEB RESEARCH]\n{block}")
    };
    state
        .persistence
        .store_context_text(sid, &merged)
        .map_err(|e| e.to_string())?;

    let raw_chunks = chunk_text(&block, 200, 50);
    if raw_chunks.is_empty() {
        return Ok(AppendResearchResultDto { chunks_added: 0 });
    }

    let embedder = state
        .wait_for_embedder(std::time::Duration::from_secs(30))
        .await
        .map_err(|e| format!("Embedding unavailable: {e}"))?;

    let raw_owned = raw_chunks.clone();
    let embeddings = tokio::task::spawn_blocking(move || {
        let refs: Vec<&str> = raw_owned.iter().map(|s| s.as_str()).collect();
        embedder.embed_batch(&refs)
    })
    .await
    .map_err(|e| format!("Embedder task panicked: {e}"))?
    .map_err(|e| format!("Embedding failed: {e}"))?;

    let ingest_chunks: Vec<Chunk> = raw_chunks
        .into_iter()
        .zip(embeddings)
        .map(|(text, embedding)| Chunk {
            id: Uuid::new_v4(),
            text,
            embedding,
            session_id: sid,
        })
        .collect();
    let count = ingest_chunks.len();

    state
        .vector_store
        .ingest(sid, ingest_chunks)
        .await
        .map_err(|e| format!("Failed to ingest research into RAG: {e}"))?;

    info!(session_id = %sid, chunks_added = count, "prep research appended to context");
    Ok(AppendResearchResultDto {
        chunks_added: count,
    })
}

/// Start a live session: initialise the audio pipeline and transition to LIVE.
///
/// Valid from: `READY` only (rehearsal must be completed first).
#[tauri::command]
pub async fn start_session(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    let sid = validate_session_id(&state, &session_id).await?;

    if !keychain::is_rehearsal_completed() {
        return Err("Complete rehearsal before starting a live session.".to_string());
    }

    checks::run_stealth_self_test()?;

    {
        let machine = state.state_machine.lock().await;
        if *machine.current() != SessionState::Ready {
            return Err(format!(
                "start_session requires READY (current: {})",
                machine.current()
            ));
        }
    }

    // Refuse to start if a live session is already running.
    if state.live_tasks.lock().await.is_some() {
        return Err("A live session is already running.".to_string());
    }

    // ── 1. Hardware profile and Whisper model ─────────────────────────────
    let profile = hardware::assess_hardware();
    let model_path = whisper_model_path(&profile);

    let whisper = Arc::new(
        WhisperEngine::new(&model_path, profile.tier)
            .map_err(|e| format!("Failed to load Whisper model ({model_path}): {e}"))?,
    );

    // ── 2. Question detector ──────────────────────────────────────────────
    let detector = Arc::new(
        QuestionDetector::new(
            profile.tier,
            Some(Arc::clone(&state.llm)),
            &prompts_base_dir(),
        )
        .map_err(|e| format!("Failed to init question detector: {e}"))?,
    );

    // ── 3. Audio channels ─────────────────────────────────────────────────
    let (system_tx, system_rx) = tokio::sync::mpsc::channel(256);
    let (mic_tx, mic_rx) = tokio::sync::mpsc::channel(256);
    let (question_tx, question_rx) = tokio::sync::mpsc::channel::<DetectedQuestion>(64);
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let (zeroed_tx, zeroed_rx) = tokio::sync::oneshot::channel::<()>();

    // ── 4. Audio capture on a dedicated OS thread (cpal::Stream is !Send) ─
    //
    // The thread owns the AudioCapture. When stop_tx fires (or is dropped),
    // blocking_recv() returns, capture.stop() zeroes the ring buffers, then
    // zeroed_tx fires so stop_session can confirm zeroing before emitting ENDED.
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<anyhow::Result<()>>();

    std::thread::spawn(move || {
        match AudioCapture::start(system_tx, mic_tx) {
            Ok(capture) => {
                let _ = ready_tx.send(Ok(()));
                // Block here until stop_session fires or AppState is dropped.
                let _ = stop_rx.blocking_recv();
                if let Err(e) = capture.stop() {
                    tracing::warn!(error = %e, "audio capture stop error");
                }
                // Signal that ring buffers are zeroed. stop_session awaits
                // this before emitting ENDED (security invariant).
                let _ = zeroed_tx.send(());
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        }
    });

    // Wait up to 5 seconds for the capture thread to confirm startup.
    tokio::time::timeout(Duration::from_secs(5), ready_rx)
        .await
        .map_err(|_| "Audio capture startup timed out.".to_string())?
        .map_err(|_| "Audio capture thread exited unexpectedly.".to_string())?
        .map_err(|e| format!("Failed to start audio capture: {e}"))?;

    // ── 5. Build failover manager and conversation memory ─────────────────

    let (failover, local_provider, context_window) = build_failover_stack(&app, &state).await?;

    let memory = Arc::new(tokio::sync::Mutex::new(ConversationMemory::new(
        context_window,
    )));
    *state.session_memory.lock().await = Some(Arc::clone(&memory));

    let turn_cancel_slot: Arc<tokio::sync::Mutex<Option<crate::state::TurnCancelFlag>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    let compression_prompt = load_compression_prompt();

    // ── 6. Spawn background tasks ─────────────────────────────────────────

    let digest = state.session_digest.read().await.clone().ok_or_else(|| {
        "No session digest — run ingest_context before starting a live session.".to_string()
    })?;

    let orch_config = OrchestratorConfig {
        session_id: sid,
        digest: Arc::new(digest),
        prompts_dir: prompts_base_dir(),
        failover: Arc::clone(&failover),
        embedder: state.require_embedder()?,
        vector_store: Arc::clone(&state.vector_store),
        prewarm_cache: Arc::clone(&state.prewarm_cache),
        memory,
        compression_prompt,
        local_llm: local_provider,
        turn_cancel_slot: Arc::clone(&turn_cancel_slot),
        persistence: Arc::clone(&state.persistence),
        cost_tracker: Arc::clone(&state.cost_tracker),
    };

    let orch_app = app.clone();
    let orchestrator = tokio::spawn(async move {
        run_orchestrator(question_rx, orch_config, orch_app).await;
    });

    let pipeline = tokio::spawn(run_audio_pipeline(
        app.clone(),
        sid,
        whisper,
        detector,
        question_tx.clone(),
        system_rx,
        mic_rx,
        Arc::clone(&state.persistence),
    ));

    *state.live_tasks.lock().await = Some(LiveTaskHandles {
        stop_tx,
        zeroed_rx,
        pipeline,
        orchestrator,
        question_tx,
        turn_cancel: turn_cancel_slot,
    });

    // ── 7. State transition READY → LIVE ──────────────────────────────────

    {
        let mut machine = state.state_machine.lock().await;
        if let Err(e) = machine.transition(SessionState::Live) {
            drop(machine);
            abort_live_tasks(&state).await;
            return Err(session_error(e));
        }
    }
    emit_state(&app, SessionState::Live);

    info!(
        session_id = %sid,
        tier = profile.tier,
        model = %profile.recommended_whisper_model,
        "live session started",
    );
    Ok(())
}

/// Stop the live session and zero all audio ring buffers.
///
/// Valid from: `LIVE`.
///
/// Sequence:
/// 1. `LIVE → ENDING` — initiate shutdown; emit.
/// 2. Signal the audio capture thread to stop → `AudioCapture::stop()`
///    zeroes ring buffers. Abort pipeline and drain tasks.
/// 3. `ENDING → ENDED` — cleanup confirmed; emit.
///
/// The caller is responsible for the final `ENDED → IDLE` (or
/// `ENDED → CONFIGURING`) transition, which is a frontend UX decision.
#[tauri::command]
pub async fn stop_session(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    {
        let machine = state.state_machine.lock().await;
        if *machine.current() != SessionState::Live {
            return Err(format!(
                "stop_session is only valid from LIVE (current: {})",
                machine.current()
            ));
        }
    }

    // LIVE → ENDING
    {
        let mut machine = state.state_machine.lock().await;
        machine
            .transition(SessionState::Ending)
            .map_err(session_error)?;
    }
    emit_state(&app, SessionState::Ending);

    // Signal the audio capture thread to stop, then wait for it to confirm
    // that AudioCapture::stop() has completed (ring buffers zeroed).
    // Only then do we emit ENDED so the "cleared on session end" invariant holds.
    if let Some(handles) = state.live_tasks.lock().await.take() {
        let _ = handles.stop_tx.send(());

        // 2-second timeout — capture.stop() is drops + fill(0.0), ~0ms in
        // practice. Timeout guards against a hung capture thread.
        let _ = tokio::time::timeout(Duration::from_secs(2), handles.zeroed_rx).await;

        handles.pipeline.abort();
        handles.orchestrator.abort();
    }

    // Clear per-session memory.
    *state.session_memory.lock().await = None;

    // Phase 7.4 — zero the cost tracker so the next session starts fresh.
    state.cost_tracker.reset();

    // ENDING → ENDED
    let session_id = {
        let mut machine = state.state_machine.lock().await;
        let sid = machine.session_id();
        machine
            .transition(SessionState::Ended)
            .map_err(session_error)?;
        sid
    };
    emit_state(&app, SessionState::Ended);

    // Post-session Supabase sync — fire-and-forget, non-fatal on failure.
    if let Some(sid) = session_id {
        let token_opt = state.auth_token().await;
        let digest_opt = state.session_digest.read().await.clone();
        if let Some(token) = token_opt {
            let plugins = state.plugins.clone();
            let persistence = Arc::clone(&state.persistence);
            tokio::spawn(async move {
                let metadata = crate::supabase::SessionMetadata {
                    name: digest_opt
                        .as_ref()
                        .map(|d| d.role.clone())
                        .unwrap_or_else(|| "Interview Session".to_string()),
                    session_type: "interview".to_string(),
                    domain: digest_opt
                        .as_ref()
                        .map(|d| d.domain.clone())
                        .unwrap_or_else(|| "general".to_string()),
                };
                match crate::supabase::resolve_supabase_config(&plugins) {
                    Some(cfg) => {
                        if let Ok(sync) =
                            crate::supabase::SupabaseSessionSync::new(cfg.url, cfg.anon_key)
                        {
                            if let Err(e) = sync
                                .sync_session(sid, &token, &persistence, &metadata)
                                .await
                            {
                                warn!(session_id = %sid, error = %e, "Supabase session sync failed");
                            }
                        }
                    }
                    None => {
                        warn!("Supabase not configured — skipping session sync");
                    }
                }
            });
        } else {
            warn!("No auth token — skipping session sync");
        }
    }

    info!("session stopped");
    Ok(())
}

/// Manual response trigger — valid only from LIVE.
///
/// Sends a synthetic `DetectedQuestion` directly to the orchestrator so the
/// user can manually fire a response without waiting for VAD/Whisper.
#[tauri::command]
pub async fn trigger_response(
    state: State<'_, AppState>,
    question: String,
    session_id: String,
    rephrase: Option<bool>,
) -> Result<(), String> {
    let sid = validate_session_id(&state, &session_id).await?;

    {
        let machine = state.state_machine.lock().await;
        if *machine.current() != SessionState::Live {
            return Err(format!(
                "trigger_response is only valid from LIVE (current: {})",
                machine.current()
            ));
        }
    }

    if state.cost_tracker.is_suspended() {
        return Err(
            "Inference is suspended because the cost cap was reached. Lift the cap or reset the tracker to continue."
                .to_string(),
        );
    }

    let question_text = if rephrase.unwrap_or(false) {
        format!("Rephrase your previous answer to: {question}")
    } else {
        question
    };

    let guard = state.live_tasks.lock().await;
    if let Some(handles) = guard.as_ref() {
        let detected = DetectedQuestion {
            text: question_text.clone(),
            session_id: sid,
            detected_at: std::time::Instant::now(),
        };
        handles
            .question_tx
            .try_send(detected)
            .map_err(|e| format!("Failed to send question to orchestrator: {e}"))?;
        info!(
            session_id = %sid,
            question_len = question_text.len(),
            "manual trigger_response",
        );
        #[cfg(debug_assertions)]
        tracing::debug!(
            session_id = %sid,
            question = %question_text,
            "manual trigger_response (debug-only content)",
        );
    }

    Ok(())
}

/// Cancel any running inference — valid only from LIVE.
///
/// Sets the active turn's cancellation flag so in-flight token streams stop.
#[tauri::command]
pub async fn cancel_inference(state: State<'_, AppState>) -> Result<(), String> {
    {
        let machine = state.state_machine.lock().await;
        if *machine.current() != SessionState::Live {
            return Err(format!(
                "cancel_inference is only valid from LIVE (current: {})",
                machine.current()
            ));
        }
    }

    let guard = state.live_tasks.lock().await;
    if let Some(handles) = guard.as_ref() {
        let slot = handles.turn_cancel.lock().await;
        if let Some(flag) = slot.as_ref() {
            flag.store(true, std::sync::atomic::Ordering::Release);
            info!("cancel_inference: active turn cancelled");
        }
    }

    Ok(())
}

/// Toggle overlay visibility (panic hotkey path).
#[tauri::command]
pub async fn panic_hide_overlay(app: AppHandle) -> Result<bool, String> {
    use crate::events::{emit_overlay_visibility, OverlayVisibilityPayload};

    if let Some(window) = app.get_webview_window("main") {
        let visible = window.is_visible().unwrap_or(true);
        if visible {
            window
                .hide()
                .map_err(|e| format!("Failed to hide overlay: {e}"))?;
            emit_overlay_visibility(&app, OverlayVisibilityPayload { hidden: true });
            Ok(true)
        } else {
            window
                .show()
                .map_err(|e| format!("Failed to show overlay: {e}"))?;
            let _ = window.set_focus();
            emit_overlay_visibility(&app, OverlayVisibilityPayload { hidden: false });
            Ok(false)
        }
    } else {
        Ok(false)
    }
}

/// Switch the active LLM provider.
///
/// Mid-session model switching is explicitly out of v1 scope.
/// This command is registered so the frontend IPC contract compiles;
/// it will always return an error.
#[tauri::command]
pub async fn switch_provider(_name: String) -> Result<(), String> {
    Err("Provider switching is not available in v1. Configure your provider before starting a session.".to_string())
}

// ──────────────────────────────────────────────────────────────────────────────
// 6.2 — Crash recovery
// ──────────────────────────────────────────────────────────────────────────────

/// On app startup: scan SQLite for an incomplete session.
///
/// Returns `Some(RecoveryOffer)` if one is found and the state machine has
/// moved to `RECOVERING`, or `None` if the previous session ended cleanly.
#[tauri::command]
pub async fn check_crash_recovery(
    state: State<'_, AppState>,
) -> Result<Option<recovery::RecoveryOffer>, String> {
    recovery::check_for_recovery(&state.persistence, &state.state_machine)
        .await
        .map_err(|e| e.to_string())
}

/// Resume a crashed session: `RECOVERING → READY`.
#[tauri::command]
pub async fn resume_crashed_session(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    recovery::resume_session(&state.state_machine)
        .await
        .map_err(|e| e.to_string())?;

    let current = *state.state_machine.lock().await.current();
    emit_state(&app, current);
    Ok(())
}

/// Discard a crashed session: delete local data (SQLite + RAG vectors) and
/// return to `IDLE`.
#[tauri::command]
pub async fn discard_crashed_session(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    recovery::discard_session(
        Arc::clone(&state.persistence),
        Arc::clone(&state.vector_store),
        &state.state_machine,
    )
    .await
    .map_err(|e| e.to_string())?;

    emit_state(&app, SessionState::Idle);
    Ok(())
}

/// Discard every crashed session in the database. Returns the list of
/// cleared session IDs so the UI can confirm what was purged.
#[tauri::command]
pub async fn discard_all_crashed_sessions(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let ids = recovery::discard_all_crashed(
        Arc::clone(&state.persistence),
        Arc::clone(&state.vector_store),
        &state.state_machine,
    )
    .await
    .map_err(|e| e.to_string())?;

    let current = *state.state_machine.lock().await.current();
    emit_state(&app, current);
    Ok(ids.into_iter().map(|id| id.to_string()).collect())
}

// ──────────────────────────────────────────────────────────────────────────────
// 6.4 — Post-session summary
// ──────────────────────────────────────────────────────────────────────────────

/// Generate a structured post-session summary using the `session_essence` prompt.
///
/// Loads the full transcript from SQLite, passes it through the LLM prompt
/// defined in `/prompts/session_essence/`, and returns a JSON summary blob.
/// Only callable after the session has reached `ENDED`.
#[tauri::command]
pub async fn generate_session_summary(state: State<'_, AppState>) -> Result<String, String> {
    let sid = {
        let machine = state.state_machine.lock().await;
        machine.session_id().ok_or("No session to summarise.")?
    };

    // Load the session_essence prompt — must come from file, never inlined.
    let prompt_template = std::fs::read_to_string(
        prompts_base_dir()
            .join("session_essence")
            .join("default.txt"),
    )
    .map_err(|e| format!("Failed to load session_essence prompt: {e}"))?;

    // Fetch transcript rows from SQLite and build a flat string.
    let transcript_text = {
        let data = state
            .persistence
            .load_session_data(sid)
            .map_err(|e| e.to_string())?;
        match data {
            Some(d) => d
                .transcript_chunks
                .iter()
                .map(|c| format!("[{}] {}", c.speaker, c.text))
                .collect::<Vec<_>>()
                .join("\n"),
            None => {
                // Session was already cleared — return an empty summary.
                return Ok(serde_json::json!({
                    "date": "",
                    "domain": "",
                    "role": "",
                    "company": "",
                    "questions_count": 0,
                    "topics_covered": [],
                    "confidence_distribution": {"high": 0, "medium": 0, "low": 0},
                    "key_moments": [],
                    "follow_up_actions": [],
                    "one_line_summary": "No transcript data available."
                })
                .to_string());
            }
        }
    };

    let prompt = prompt_template.replace("{full_transcript}", &transcript_text);

    // Reject if a live session is running.
    {
        let guard = state.live_tasks.lock().await;
        if guard.is_some() {
            return Err("Cannot generate summary while a session is live.".to_string());
        }
    }

    // Build a one-shot provider for the summary call.
    let provider: Arc<dyn LLMProvider> = match keychain::get_api_key("groq") {
        Ok(api_key) => match GroqProvider::new(api_key) {
            Ok(p) => Arc::new(p),
            Err(_) => Arc::clone(&state.llm),
        },
        Err(_) => Arc::clone(&state.llm),
    };

    let summary = provider
        .complete(
            prompt,
            CompletionConfig {
                max_tokens: Some(600),
                temperature: 0.0,
                stream: false,
            },
        )
        .await
        .map_err(|e| format!("LLM summary call failed: {e}"))?;

    info!(session_id = %sid, "post-session summary generated");
    Ok(summary)
}

// ──────────────────────────────────────────────────────────────────────────────
// 6.5 — Session list management
// ──────────────────────────────────────────────────────────────────────────────

/// List all sessions stored in the local SQLite database.
#[tauri::command]
pub async fn list_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<crate::dto::SessionSummaryDto>, String> {
    state.persistence.list_sessions().map_err(|e| e.to_string())
}

/// Mark a session as promoted (exempted from 30-day expiry).
#[tauri::command]
pub async fn promote_session(session_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let sid = Uuid::parse_str(&session_id).map_err(|e| format!("Invalid session ID: {e}"))?;
    state
        .persistence
        .promote_session(sid)
        .map_err(|e| e.to_string())
}

/// Remove the promoted flag from a session so it resumes normal 30-day expiry.
#[tauri::command]
pub async fn demote_session(session_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let sid = Uuid::parse_str(&session_id).map_err(|e| format!("Invalid session ID: {e}"))?;
    state
        .persistence
        .demote_session(sid)
        .map_err(|e| e.to_string())
}

// ──────────────────────────────────────────────────────────────────────────────
// 7.4 — Cost cap enforcement
// ──────────────────────────────────────────────────────────────────────────────

/// DTO mirroring [`crate::cost::CostStatus`] for the frontend.
#[derive(serde::Serialize)]
pub struct CostStatusDto {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cost_estimate_usd: f64,
    pub max_total_tokens: Option<u64>,
    pub max_cost_estimate_usd: Option<f64>,
    pub suspended: bool,
    pub status: String,
    pub fraction_used: Option<f64>,
}

impl From<crate::cost::CostStatus> for CostStatusDto {
    fn from(s: crate::cost::CostStatus) -> Self {
        let status = match s.status {
            crate::cost::CostCapStatus::Ok => "ok",
            crate::cost::CostCapStatus::Warning80 => "warning_80",
            crate::cost::CostCapStatus::Reached => "reached",
        };
        Self {
            input_tokens: s.usage.input_tokens,
            output_tokens: s.usage.output_tokens,
            total_tokens: s.usage.total_tokens,
            cost_estimate_usd: s.usage.cost_estimate_usd,
            max_total_tokens: s.cap.max_total_tokens,
            max_cost_estimate_usd: s.cap.max_cost_estimate_usd,
            suspended: s.suspended,
            status: status.to_string(),
            fraction_used: s.fraction_used,
        }
    }
}

/// Snapshot the current cumulative usage, configured cap, and suspension flag.
#[tauri::command]
pub async fn get_cost_status(state: State<'_, AppState>) -> Result<CostStatusDto, String> {
    Ok(state.cost_tracker.snapshot().into())
}

/// Configure the per-session token / cost cap. `None` on either field
/// disables that dimension; both `None` removes the cap entirely.
#[tauri::command]
pub async fn set_cost_cap(
    state: State<'_, AppState>,
    max_total_tokens: Option<u64>,
    max_cost_estimate_usd: Option<f64>,
) -> Result<CostStatusDto, String> {
    let cap = crate::cost::CostCap {
        max_total_tokens,
        max_cost_estimate_usd,
    };
    let snap = state.cost_tracker.set_cap(cap);
    info!(
        max_total_tokens = ?max_total_tokens,
        max_cost_estimate_usd = ?max_cost_estimate_usd,
        suspended = snap.suspended,
        "cost cap updated",
    );
    Ok(snap.into())
}

/// Clear the suspended flag while preserving cumulative counters. The cap
/// itself is unchanged — the next turn will re-suspend unless the user also
/// widens the cap via `set_cost_cap`.
#[tauri::command]
pub async fn lift_cost_suspension(state: State<'_, AppState>) -> Result<CostStatusDto, String> {
    let snap = state.cost_tracker.lift_suspension();
    info!("cost-cap suspension lifted by user");
    Ok(snap.into())
}

/// Zero all cumulative counters. Useful when the user wants a fresh budget
/// without restarting the session.
#[tauri::command]
pub async fn reset_cost_tracker(state: State<'_, AppState>) -> Result<CostStatusDto, String> {
    let snap = state.cost_tracker.reset();
    info!("cost tracker reset by user");
    Ok(snap.into())
}

/// Delete a session and all its data from local SQLite and Supabase.
#[tauri::command]
pub async fn delete_session(session_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let sid = Uuid::parse_str(&session_id).map_err(|e| format!("Invalid session ID: {e}"))?;

    // Delete locally first (always succeeds even if Supabase is unreachable).
    state
        .persistence
        .clear_session(sid)
        .map_err(|e| e.to_string())?;

    // Best-effort Supabase deletion — non-fatal on failure.
    let token_opt = state.auth_token().await;
    if let Some(token) = token_opt {
        let plugins = state.plugins.clone();
        tokio::spawn(async move {
            if let Some(cfg) = crate::supabase::resolve_supabase_config(&plugins) {
                if let Ok(sync) = crate::supabase::SupabaseSessionSync::new(cfg.url, cfg.anon_key) {
                    if let Err(e) = sync.delete_session(sid, &token).await {
                        warn!(session_id = %sid, error = %e, "Supabase session delete failed");
                    }
                }
            }
        });
    }

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// 7.5 — GDPR right-to-deletion + right-to-export
// ──────────────────────────────────────────────────────────────────────────────

/// Delete the authenticated user's account end-to-end.
///
/// Flow (each step is independently best-effort — see [`crate::gdpr`]):
/// 1. Supabase `DELETE /auth/v1/user` — removes the auth row.
/// 2. Local vector store — drops every per-session `vec_chunks_{hex}` table.
/// 3. Local SQLite — truncates all user-data tables in one transaction.
/// 4. OS keychain — purges tokens, consent flags, and known API keys.
///
/// The state machine is reset to IDLE and the in-memory auth token is
/// cleared, regardless of whether the Supabase call succeeded.
///
/// Returns the per-step [`crate::gdpr::DeleteAccountReport`] so the UI can
/// surface partial failures (e.g. "we deleted everything locally, but the
/// server is unreachable — your account row will be removed when Supabase
/// is back online").
#[tauri::command]
pub async fn delete_account(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<crate::gdpr::DeleteAccountReport, String> {
    // Refuse to delete while a live session is running — would orphan the
    // audio thread and leave SQLite mid-write.
    {
        let guard = state.live_tasks.lock().await;
        if guard.is_some() {
            return Err(
                "Cannot delete account while a session is live. Stop the session first."
                    .to_string(),
            );
        }
    }

    let token = active_auth_token(&state).await?;

    let report = crate::gdpr::delete_account(
        Arc::clone(&state.auth),
        token,
        Arc::clone(&state.persistence),
        Arc::clone(&state.vector_store),
        Box::new(keychain::clear_all_user_secrets),
    )
    .await;

    // Reset the in-memory auth + session state so the next request looks
    // like a fresh launch.
    state.set_auth_token(None).await;
    {
        let mut machine = state.state_machine.lock().await;
        machine.reset_to_idle();
    }
    state.cost_tracker.reset();
    *state.session_digest.write().await = None;
    *state.session_memory.lock().await = None;
    state.prewarm_cache.lock().await.clear();

    emit_state(&app, SessionState::Idle);

    info!(
        all_succeeded = report.all_succeeded(),
        "account deletion finished"
    );
    Ok(report)
}

/// Produce a JSON dump of all locally-stored sessions, transcripts,
/// responses, and state transitions for the GDPR right-to-export.
///
/// Returns the JSON as a string so the frontend can hand it to the user
/// (download, copy to clipboard, etc.). The backend deliberately does NOT
/// write to disk — file I/O belongs in the platform-native dialog layer.
#[tauri::command]
pub async fn export_user_data(state: State<'_, AppState>) -> Result<String, String> {
    let export = crate::gdpr::export_user_data(&state.persistence).map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&export).map_err(|e| format!("Could not serialise export: {e}"))
}

// ──────────────────────────────────────────────────────────────────────────────
// 7.7 — Provider API key management (Settings → Providers)
// ──────────────────────────────────────────────────────────────────────────────

/// Reject any provider name that isn't in the canonical allowlist. Keeps a
/// compromised frontend from spraying arbitrary keychain entries.
fn validate_provider(provider: &str) -> Result<(), String> {
    if keychain::KNOWN_API_PROVIDERS.contains(&provider) {
        Ok(())
    } else {
        Err(format!(
            "Unknown provider '{provider}' — expected one of {:?}",
            keychain::KNOWN_API_PROVIDERS
        ))
    }
}

/// Persist an LLM provider API key in the OS keychain. The plaintext key
/// is wrapped in `SecretString` immediately on entry so the IPC buffer is
/// zeroed when this function returns.
#[tauri::command]
pub async fn save_provider_key(provider: String, key: String) -> Result<(), String> {
    validate_provider(&provider)?;
    let secret = SecretString::new(key);
    keychain::store_api_key(&provider, secret).map_err(|e| e.to_string())
}

/// Report whether an API key for `provider` is currently stored. Never
/// returns the key itself — the frontend only needs presence to render
/// the Settings UI.
#[tauri::command]
pub async fn is_provider_key_present(provider: String) -> Result<bool, String> {
    validate_provider(&provider)?;
    Ok(keychain::get_api_key(&provider).is_ok())
}

/// Remove a provider's API key from the OS keychain. No-op if no key is
/// stored.
#[tauri::command]
pub async fn clear_provider_key(provider: String) -> Result<(), String> {
    validate_provider(&provider)?;
    keychain::delete_api_key(&provider).map_err(|e| e.to_string())
}

// ──────────────────────────────────────────────────────────────────────────────
// 7.6 — Feature flag commands
// ──────────────────────────────────────────────────────────────────────────────

/// Resolve the evaluation context for the current request. Falls back to
/// an anonymous user (nil UUID + Free plan) when no auth token is loaded
/// so the UI can still gate features pre-login.
async fn evaluation_context(state: &AppState) -> crate::flags::EvaluationContext {
    if let Some(token) = state.auth_token().await {
        if let Ok(user) = state.auth.get_current_user(&token).await {
            return crate::flags::EvaluationContext {
                user_id: user.id,
                plan: user.plan,
            };
        }
    }
    crate::flags::EvaluationContext {
        user_id: Uuid::nil(),
        plan: crate::interfaces::auth::Plan::Free,
    }
}

/// Evaluate a single feature flag for the currently authenticated user.
///
/// Reads from the in-memory cache that was either pulled from Supabase or
/// loaded from disk on startup — no network call here, even on first hit.
#[tauri::command]
pub async fn is_feature_enabled(flag: String, state: State<'_, AppState>) -> Result<bool, String> {
    let ctx = evaluation_context(&state).await;
    Ok(state.feature_flags.is_enabled(&flag, &ctx))
}

/// Trigger a refresh from the Supabase Edge Function. On failure the
/// existing cache (or compiled defaults) stays authoritative — the error
/// is returned to the caller for logging but is not fatal.
#[tauri::command]
pub async fn refresh_feature_flags(state: State<'_, AppState>) -> Result<(), String> {
    let source = match crate::flags::supabase_source_from_plugins(&state.plugins) {
        Some(s) => s,
        None => return Err("Supabase plugin config is missing or malformed".to_string()),
    };
    state
        .feature_flags
        .refresh_from(&source)
        .await
        .map_err(|e| e.to_string())
}

/// Dev/debug helper: dump the currently active flag set and its provenance
/// (remote / cache / defaults). Useful when a tester reports "I don't see
/// the new panel" — `source = Defaults` tells you the device never reached
/// Supabase.
#[tauri::command]
pub async fn get_feature_flags_snapshot(
    state: State<'_, AppState>,
) -> Result<crate::flags::ClientSnapshot, String> {
    Ok(state.feature_flags.snapshot())
}
