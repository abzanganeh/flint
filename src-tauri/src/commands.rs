use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as AnyhowContext;
use secrecy::{ExposeSecret, SecretString};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::audio::capture::AudioCapture;
use crate::audio::pipeline::{run_audio_pipeline, DetectedQuestion};
use crate::digest::extract_digest;
use crate::dto::{
    AppendResearchResultDto, DigestDto, HardwareProfileDto, HealthCheckResultDto, SessionConfigDto,
    SessionContextFieldsDto, SessionSnapshotDto, SmartResumeImportDto, UserDto, WebSourceDto,
};
use crate::events::{
    emit_mock_coach_feedback, emit_mock_suggested_token, emit_session_state_change,
    emit_token_usage_update, MockCoachFeedbackPayload, MockSuggestedTokenPayload,
    SessionStateChangePayload, TokenUsageUpdatePayload,
};
use crate::health::{checks, hardware};
use crate::interfaces::auth::AuthToken;
use crate::interfaces::vector::Chunk;
use crate::keychain;
use crate::knowledge::packs_for_role;
use crate::llm::failover::FailoverManager;
use crate::llm::groq::GroqProvider;
use crate::llm::ollama::OllamaProvider;
use crate::llm::openrouter;
use crate::llm::provider::{CompletionConfig, LLMProvider};
use crate::llm::rate_limiter::RateLimiter;
use crate::mock::coach::{coach_failure_payload, run_coach};
use crate::mock::conductor::{Conductor, ConductorCommand, MockMode, MockPace};
use crate::mock::mic_capture::MicCapture;
use crate::mock::rag::query_mock_rag;
use crate::mock::tts;
use crate::orchestrator::prewarm::{run_prewarm, PreWarmCache};
use crate::orchestrator::{dispatch_turn, run_orchestrator, OrchestratorConfig};
use crate::rag::chunker::chunk_text;
use crate::research::{self, tavily, ResearchSource};

/// Approximate base chars added to a research prompt (template + labels).
const RESEARCH_PROMPT_OVERHEAD_CHARS: usize = 800;

/// Number of Tavily results to fetch per enrichment query.
const ENRICHMENT_RESULTS_PER_QUERY: usize = 4;
use crate::session::draft;
use crate::session::memory::ConversationMemory;
use crate::session::recovery;
use crate::session::state::SessionState;
use crate::smart_resume;
use crate::state::{AppState, LiveTaskHandles, MockTaskHandles};
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

/// Open the system browser to start Google OAuth (PKCE + `flint://auth/callback`).
#[tauri::command]
pub async fn start_google_oauth(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let (verifier, challenge) = crate::supabase::oauth::generate_pkce_pair();
    keychain::store_oauth_code_verifier(&verifier).map_err(|_| KEYCHAIN_SAVE_ERROR.to_string())?;
    let url = state.supabase_auth.google_authorize_url(&challenge);
    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|_| "Could not open your browser for Google sign-in.".to_string())?;
    Ok(())
}

/// Abort a pending Google OAuth flow started by [`start_google_oauth`].
///
/// Google has no server-side "cancel" API for an in-flight browser consent page.
/// We invalidate the local PKCE verifier so a late `flint://` callback cannot
/// complete, and emit `auth_oauth_error` so the UI leaves the waiting state.
#[tauri::command]
pub async fn cancel_google_oauth(app: AppHandle) -> Result<(), String> {
    use crate::events::{emit_auth_oauth_error, AuthOAuthErrorPayload};

    keychain::clear_oauth_code_verifier();
    emit_auth_oauth_error(
        &app,
        AuthOAuthErrorPayload {
            message: "Google sign-in was cancelled. Try again or use email below.".to_string(),
        },
    );
    Ok(())
}

/// Handle `flint://auth/callback` from the browser redirect (deep link / argv).
pub async fn process_oauth_callback_url<R: Runtime>(app: &AppHandle<R>, url: &str) -> bool {
    use crate::events::{emit_auth_oauth_complete, emit_auth_oauth_error, AuthOAuthErrorPayload};
    use crate::supabase::oauth::{parse_auth_callback, tokens_to_auth_token, AuthCallback};

    let Some(callback) = parse_auth_callback(url) else {
        return false;
    };

    let Some(state) = app.try_state::<AppState>() else {
        return true;
    };

    match callback {
        AuthCallback::Error { message } => {
            emit_auth_oauth_error(app, AuthOAuthErrorPayload { message });
        }
        AuthCallback::Tokens(tokens) => {
            let token = tokens_to_auth_token(&tokens);
            if persist_auth_token(&state, token).await.is_ok() {
                emit_auth_oauth_complete(app);
            } else {
                emit_auth_oauth_error(
                    app,
                    AuthOAuthErrorPayload {
                        message: KEYCHAIN_SAVE_ERROR.to_string(),
                    },
                );
            }
        }
        AuthCallback::Code(code) => {
            let verifier = match keychain::take_oauth_code_verifier() {
                Ok(Some(v)) => v,
                _ => {
                    emit_auth_oauth_error(
                        app,
                        AuthOAuthErrorPayload {
                            message: "OAuth session expired. Please try again.".to_string(),
                        },
                    );
                    return true;
                }
            };
            match state
                .supabase_auth
                .exchange_pkce_code(&code.code, &verifier)
                .await
            {
                Ok(token) => {
                    if persist_auth_token(&state, token).await.is_ok() {
                        emit_auth_oauth_complete(app);
                    } else {
                        emit_auth_oauth_error(
                            app,
                            AuthOAuthErrorPayload {
                                message: KEYCHAIN_SAVE_ERROR.to_string(),
                            },
                        );
                    }
                }
                Err(e) => {
                    emit_auth_oauth_error(
                        app,
                        AuthOAuthErrorPayload {
                            message: map_user_error(e),
                        },
                    );
                }
            }
        }
    }
    true
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
/// Attempt digest extraction through a provider chain: Groq → OpenRouter → Ollama.
///
/// Every provider failure is logged and appended to an accumulated error string so
/// the final message tells the user exactly what was tried and why each step failed.
/// The real HTTP/network error is included via `{:#}` (full anyhow chain).
async fn extract_digest_resilient(
    context_text: &str,
    _state: &AppState,
) -> Result<crate::digest::Digest, String> {
    let mut failures: Vec<String> = Vec::new();

    // ── 1. Groq ──────────────────────────────────────────────────────────────
    if let Ok(api_key) = keychain::get_api_key("groq") {
        match GroqProvider::new(api_key) {
            Ok(provider) => match extract_digest(context_text, &provider).await {
                Ok(digest) => return Ok(digest),
                Err(e) => {
                    let detail = format!("{e:#}");
                    warn!(error = %detail, "digest extraction via Groq failed");
                    failures.push(format!("Groq: {detail}"));
                }
            },
            Err(e) => failures.push(format!("Groq provider init: {e}")),
        }
    }

    // ── 2. OpenRouter ─────────────────────────────────────────────────────────
    if let Some(provider) = openrouter::resolve_openrouter() {
        match extract_digest(context_text, provider.as_ref()).await {
            Ok(digest) => return Ok(digest),
            Err(e) => {
                let detail = format!("{e:#}");
                warn!(error = %detail, "digest extraction via OpenRouter failed");
                failures.push(format!("OpenRouter: {detail}"));
            }
        }
    }

    // ── 3. Ollama ─────────────────────────────────────────────────────────────
    if let Ok(ollama) = OllamaProvider::new() {
        if ollama.health_check().await {
            match extract_digest(context_text, &ollama).await {
                Ok(digest) => return Ok(digest),
                Err(e) => {
                    let detail = format!("{e:#}");
                    warn!(error = %detail, "digest extraction via Ollama failed");
                    failures.push(format!("Ollama: {detail}"));
                }
            }
        }
    }

    // ── Build actionable guidance ─────────────────────────────────────────────
    let detail = failures.join("; ");
    let guidance = if detail.contains("401")
        || detail.contains("Unauthorized")
        || detail.contains("invalid_api_key")
    {
        "Your Groq API key appears to be invalid — re-enter it in Settings → API Keys."
    } else if detail.contains("429") || detail.contains("rate_limit") {
        "Groq rate limit reached. Wait a moment, or add an OpenRouter key in Settings as a cloud fallback."
    } else if failures.is_empty() {
        "No LLM provider is configured. Add a Groq API key in Settings → API Keys."
    } else {
        "LLM call failed. Check your API key in Settings → API Keys, or start Ollama on localhost:11434."
    };

    error!(providers_tried = ?failures, "all digest extraction providers exhausted");
    Err(if failures.is_empty() {
        guidance.to_string()
    } else {
        format!("{guidance}\n\nDetail: {detail}")
    })
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
        let digest = extract_digest_resilient(&text, &state).await?;

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
        let digest = extract_digest_resilient(&blob, &state).await?;

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

/// Ingest global question-bank entries into the session context vector store.
fn spawn_global_bank_enrichment(
    session_id: Uuid,
    questions: Vec<smart_resume::InterviewQuestionDto>,
    embedder: Arc<crate::rag::embedder::Embedder>,
    vector_store: Arc<dyn crate::interfaces::vector::VectorInterface>,
) {
    if questions.is_empty() {
        return;
    }

    tokio::spawn(async move {
        let chunks_text = smart_resume::bank_entries_for_context_embed(&questions);
        if chunks_text.is_empty() {
            return;
        }

        let embeddings = {
            let chunks_clone = chunks_text.clone();
            let emb = Arc::clone(&embedder);
            tokio::task::spawn_blocking(move || {
                let refs: Vec<&str> = chunks_clone.iter().map(String::as_str).collect();
                emb.embed_batch(&refs)
            })
            .await
        };

        let embeddings = match embeddings {
            Ok(Ok(e)) => e,
            Ok(Err(e)) => {
                warn!(session_id = %session_id, error = %e, "global bank embedding failed");
                return;
            }
            Err(e) => {
                warn!(session_id = %session_id, error = %e, "global bank embedder task panicked");
                return;
            }
        };

        let chunks: Vec<Chunk> = chunks_text
            .into_iter()
            .zip(embeddings)
            .filter(|(_, emb)| !emb.is_empty())
            .map(|(text, embedding)| Chunk {
                id: Uuid::new_v4(),
                text,
                embedding,
                session_id,
            })
            .collect();

        let count = chunks.len();
        if count == 0 {
            return;
        }

        match vector_store.ingest_context(session_id, chunks).await {
            Ok(()) => {
                info!(
                    session_id = %session_id,
                    chunks = count,
                    "global question bank ingested into session context store"
                )
            }
            Err(e) => {
                warn!(session_id = %session_id, error = %e, "global bank ingest failed")
            }
        }
    });
}

/// Fire-and-forget background task that runs targeted Tavily searches for the
/// company + role, then ingests the results into the session RAG store.
///
/// Silently skips when no Tavily API key is present.  Results are available
/// in both rehearsal prep-chat and mock interview RAG by the time the user
/// navigates to either screen.
fn spawn_company_enrichment(
    session_id: Uuid,
    company: String,
    role: String,
    domain: String,
    embedder: Arc<crate::rag::embedder::Embedder>,
    vector_store: Arc<dyn crate::interfaces::vector::VectorInterface>,
) {
    tokio::spawn(async move {
        let web = match tavily::resolve_tavily() {
            Some(t) => t,
            None => {
                tracing::debug!(session_id = %session_id, "no Tavily key; skipping company enrichment");
                return;
            }
        };

        let queries = [
            format!("{company} software engineering interview questions {role}"),
            format!("{company} engineering culture values {domain}"),
        ];

        let mut combined = String::new();
        for query in &queries {
            match web.search(query, ENRICHMENT_RESULTS_PER_QUERY).await {
                Ok(results) => {
                    for r in &results {
                        combined.push_str(&format!("## {}\n{}\n\n", r.title, r.snippet));
                    }
                    tracing::debug!(session_id = %session_id, query = %query, results = results.len(), "company enrichment search done");
                }
                Err(e) => {
                    warn!(session_id = %session_id, query = %query, error = %e, "company enrichment search failed")
                }
            }
        }

        if combined.trim().is_empty() {
            return;
        }

        let raw_chunks = chunk_text(&combined, 200, 50);
        if raw_chunks.is_empty() {
            return;
        }

        let embeddings = {
            let chunks_clone = raw_chunks.clone();
            let emb = Arc::clone(&embedder);
            tokio::task::spawn_blocking(move || {
                let refs: Vec<&str> = chunks_clone.iter().map(String::as_str).collect();
                emb.embed_batch(&refs)
            })
            .await
        };

        let embeddings = match embeddings {
            Ok(Ok(e)) => e,
            Ok(Err(e)) => {
                warn!(session_id = %session_id, error = %e, "company enrichment embedding failed");
                return;
            }
            Err(e) => {
                warn!(session_id = %session_id, error = %e, "company enrichment embedder task panicked");
                return;
            }
        };

        let chunks: Vec<Chunk> = raw_chunks
            .into_iter()
            .zip(embeddings)
            .filter(|(_, emb)| !emb.is_empty())
            .map(|(text, embedding)| Chunk {
                id: Uuid::new_v4(),
                text,
                embedding,
                session_id,
            })
            .collect();

        let count = chunks.len();
        if count == 0 {
            return;
        }

        match vector_store.ingest(session_id, chunks).await {
            Ok(()) => {
                info!(session_id = %session_id, chunks = count, company = %company, "company enrichment ingested into session RAG")
            }
            Err(e) => {
                warn!(session_id = %session_id, error = %e, "company enrichment ingest failed")
            }
        }
    });
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

    // Sync the question bank to the newly confirmed digest. This is essential
    // when the user returns to Session Design and re-extracts — without this
    // the bank retains stale questions from the previous extraction run.
    //
    // We also scan the raw context text for question-lines (ending with `?`)
    // and merge them in. This is the safety net for large structured question
    // banks that exceed the LLM digest token budget: the model can only fit
    // ~25–30 questions in its JSON output at 4096 tokens, but a user may paste
    // 100+ staged questions. The regex path costs zero LLM tokens and runs in
    // O(lines).
    let context_blob_opt = state.persistence.get_session_context(sid).ok();
    let mut bank_questions_for_embed = Vec::new();
    {
        let mut bank: Vec<String> = digest_rust.likely_questions.clone();

        if let Some(ref context_blob) = context_blob_opt {
            let extracted = extract_questions_from_text(context_blob);
            let bank_lower: Vec<String> = bank.iter().map(|q| q.to_lowercase()).collect();
            for q in extracted {
                if !bank_lower
                    .iter()
                    .any(|existing| *existing == q.to_lowercase())
                {
                    bank.push(q);
                }
            }
        }

        match smart_resume::fetch_interview_questions(
            &digest_rust.domain,
            &digest_rust.company,
            &digest_rust.role,
            smart_resume::DEFAULT_BANK_FETCH_LIMIT,
        )
        .await
        {
            Ok(mut remote) => {
                if remote.is_empty() {
                    remote = crate::global_bank::fetch_global_bank_questions(
                        &digest_rust.domain,
                        &digest_rust.role,
                        smart_resume::DEFAULT_BANK_FETCH_LIMIT,
                    )
                    .await;
                }
                bank_questions_for_embed = remote;
                let added = bank.len();
                smart_resume::merge_question_bank(&mut bank, &bank_questions_for_embed);
                debug!(
                    session_id = %sid,
                    merged = bank.len().saturating_sub(added),
                    total = bank.len(),
                    "global question bank merged into session question bank"
                );
            }
            Err(e) => {
                warn!(
                    session_id = %sid,
                    error = %e,
                    "global question bank fetch skipped (non-fatal)"
                );
            }
        }

        if let Err(e) = state.persistence.store_question_bank(sid, &bank) {
            warn!(session_id = %sid, error = %e, "failed to reset question bank on confirm_digest");
        } else {
            debug!(session_id = %sid, count = bank.len(), "question bank synced on confirm_digest");
        }
    }

    // Embed user-provided Q&A pairs into the context store (Phase 9).
    //
    // If the user pasted structured "Q: X\nA: Y" content, we extract and
    // re-embed each pair as a unified chunk. This gives semantic retrieval a
    // structured target rather than relying on the chunker to happen to keep Q
    // and A adjacent. Pairs are embedded into the context store (trusted
    // ground truth), not the Q&A store (AI-generated answers only).
    //
    // Fire-and-forget: failure here is non-fatal.
    if let (Some(context_blob), Ok(embedder)) = (context_blob_opt, state.require_embedder()) {
        let pairs = extract_qa_pairs_from_text(&context_blob);
        if !pairs.is_empty() {
            let store = Arc::clone(&state.vector_store);
            tokio::spawn(async move {
                let result: anyhow::Result<()> = async {
                    let pair_refs: Vec<&str> = pairs.iter().map(|s| s.as_str()).collect();
                    let embeddings = tokio::task::spawn_blocking({
                        let pair_refs: Vec<String> = pairs.clone();
                        move || {
                            let refs: Vec<&str> = pair_refs.iter().map(|s| s.as_str()).collect();
                            embedder
                                .embed_batch(&refs)
                                .context("batch embed Q&A pairs failed")
                        }
                    })
                    .await
                    .context("embed_batch task panicked")?
                    .context("embed_batch failed")?;
                    let _ = pair_refs; // consumed above
                    let chunks: Vec<crate::interfaces::vector::Chunk> = pairs
                        .into_iter()
                        .zip(embeddings)
                        .map(|(text, embedding)| crate::interfaces::vector::Chunk {
                            id: uuid::Uuid::new_v4(),
                            text,
                            embedding,
                            session_id: sid,
                        })
                        .collect();
                    store
                        .ingest_context(sid, chunks)
                        .await
                        .context("ingest user Q&A pairs into context store failed")?;
                    Ok(())
                }
                .await;
                if let Err(e) = result {
                    tracing::warn!(
                        session_id = %sid,
                        error = %e,
                        "user Q&A pair embedding skipped (non-fatal)"
                    );
                }
            });
        }
    }

    // Kick off a background Tavily sweep for company/role context so results
    // are in session RAG before the user reaches rehearsal or mock interview.
    if let Ok(embedder) = state.require_embedder() {
        if !bank_questions_for_embed.is_empty() {
            spawn_global_bank_enrichment(
                sid,
                bank_questions_for_embed,
                Arc::clone(&embedder),
                Arc::clone(&state.vector_store),
            );
        }
        spawn_company_enrichment(
            sid,
            digest_rust.company.clone(),
            digest_rust.role.clone(),
            digest_rust.domain.clone(),
            embedder,
            Arc::clone(&state.vector_store),
        );
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

    if crate::digest::digest_is_prewarm_eligible(&digest_rust) {
        if let Err(e) = run_prewarm(&digest_rust, llm, embedder, cache).await {
            // Pre-warm failures are non-fatal — the session can still proceed
            // without cached responses.
            warn!(session_id = %sid, error = %e, "pre-warm failed; continuing without cache");
        }
    } else {
        cache.lock().await.clear();
        warn!(
            session_id = %sid,
            "digest has placeholder role/company or empty skills — pre-warm skipped"
        );
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

/// Re-bind the in-memory state machine to a persisted session (Past Sessions → Reopen).
///
/// Restores the same `session_id` so structured context columns, digest, vectors,
/// and question bank remain attached. Does not delete any SQLite rows.
#[tauri::command]
pub async fn reopen_session(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
) -> Result<SessionSnapshotDto, String> {
    let sid = Uuid::parse_str(&session_id).map_err(|e| format!("Invalid session ID: {e}"))?;

    {
        let machine = state.state_machine.lock().await;
        if *machine.current() == SessionState::Live {
            return Err("End the live session before reopening a past session.".to_string());
        }
    }

    let meta = state
        .persistence
        .get_session_metadata(sid)
        .map_err(|e| format!("Session not found: {e}"))?;

    let has_digest = state
        .persistence
        .load_session_digest(sid)
        .map_err(|e| e.to_string())?
        .is_some();

    let target = reopen_ui_state(meta.state, has_digest);

    *state.session_digest.write().await = None;
    *state.prewarm_cache.lock().await = PreWarmCache::new();
    *state.rehearsal_turn.lock().await = 0;

    if let Some(digest) = state
        .persistence
        .load_session_digest(sid)
        .map_err(|e| e.to_string())?
    {
        *state.session_digest.write().await = Some(digest);
    }

    {
        let mut machine = state.state_machine.lock().await;
        machine.restore_state_for_recovery(target, sid);
    }

    state
        .persistence
        .write_state_transition(sid, &target)
        .map_err(|e| format!("Failed to persist reopened session state: {e}"))?;

    emit_state(&app, target);

    info!(session_id = %sid, target = %target, "past session reopened");
    get_session_snapshot(state).await
}

/// Pick the UI/state-machine target when reopening a row from Past Sessions.
fn reopen_ui_state(stored: SessionState, has_digest: bool) -> SessionState {
    use SessionState::*;
    match stored {
        Ended | Ready => {
            if has_digest {
                Rehearsing
            } else {
                Configuring
            }
        }
        DigestReview => DigestReview,
        PreWarming => DigestReview,
        Rehearsing | MockInterview => Rehearsing,
        Configuring | Ingesting => Configuring,
        Live | Ending | Crashed | Recovering | Paused | Idle => {
            if has_digest {
                Rehearsing
            } else {
                Configuring
            }
        }
    }
}

/// Load the session digest from memory, falling back to SQLite after restart.
async fn resolve_session_digest(
    state: &AppState,
    sid: Uuid,
) -> Result<crate::digest::Digest, String> {
    {
        let guard = state.session_digest.read().await;
        if let Some(digest) = guard.as_ref() {
            return Ok(digest.clone());
        }
    }

    match state.persistence.load_session_digest(sid) {
        Ok(Some(digest)) => {
            *state.session_digest.write().await = Some(digest.clone());
            Ok(digest)
        }
        Ok(None) => Err("No session digest — complete digest review before rehearsal.".to_string()),
        Err(e) => {
            warn!(error = %e, "resolve_session_digest: SQLite fallback failed");
            Err("No session digest — complete digest review before rehearsal.".to_string())
        }
    }
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

    let digest = resolve_session_digest(state.inner(), sid).await?;
    Ok(DigestDto::from(digest))
}

/// Re-run digest extraction for the current session without re-embedding.
///
/// Valid from `DIGEST_REVIEW` only. Uses the persisted context blob and the
/// currently configured Groq key. Context fields and Smart Resume handoff data
/// are untouched.
#[tauri::command]
pub async fn reextract_digest(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<DigestDto, String> {
    let sid = validate_session_id(&state, &session_id).await?;

    {
        let machine = state.state_machine.lock().await;
        if *machine.current() != SessionState::DigestReview {
            return Err(format!(
                "reextract_digest is only valid from DIGEST_REVIEW (current: {})",
                machine.current()
            ));
        }
    }

    let blob = state
        .persistence
        .get_session_context(sid)
        .map_err(|e| e.to_string())?;
    if blob.trim().is_empty() {
        return Err(
            "Session context is missing — use Edit context to return to Session Design."
                .to_string(),
        );
    }

    let digest = extract_digest_resilient(&blob, &state).await?;

    *state.session_digest.write().await = Some(digest.clone());
    if let Err(e) = state.persistence.store_session_digest(sid, &digest) {
        warn!(session_id = %sid, error = %e, "failed to persist re-extracted digest");
    }

    Ok(DigestDto::from(digest))
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

/// Reopen a completed (ENDED) session at Rehearsal with its digest and question bank intact.
#[tauri::command]
pub async fn reopen_past_session(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
) -> Result<SessionSnapshotDto, String> {
    let sid = Uuid::parse_str(&session_id).map_err(|e| format!("Invalid session ID: {e}"))?;

    let meta = state
        .persistence
        .get_session_metadata(sid)
        .map_err(|e| e.to_string())?;

    if meta.state != SessionState::Ended {
        return Err(format!(
            "Only ended sessions can be reopened (current: {})",
            meta.state
        ));
    }

    {
        let machine = state.state_machine.lock().await;
        let current = *machine.current();
        if matches!(
            current,
            SessionState::Live | SessionState::Ending | SessionState::MockInterview
        ) {
            return Err(format!(
                "Stop the active {} session before reopening a past session.",
                current
            ));
        }
    }

    if let Some(digest) = state
        .persistence
        .load_session_digest(sid)
        .map_err(|e| e.to_string())?
    {
        *state.session_digest.write().await = Some(digest);
    } else {
        *state.session_digest.write().await = None;
    }
    *state.prewarm_cache.lock().await = PreWarmCache::new();
    *state.rehearsal_turn.lock().await = 0;
    *state.session_memory.lock().await = None;

    state
        .persistence
        .write_state_transition(sid, &SessionState::Rehearsing)
        .map_err(|e| e.to_string())?;

    {
        let mut machine = state.state_machine.lock().await;
        machine.restore_state_for_recovery(SessionState::Rehearsing, sid);
    }

    emit_state(&app, SessionState::Rehearsing);

    get_session_snapshot(state).await
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
/// Resolution order:
///   1. `FLINT_WHISPER_MODEL` — either an absolute path to a ggml file, or
///      a Whisper model name (`tiny.en`, `base.en`, `small.en`, `medium.en`).
///   2. The hardware-recommended model from the profile.
///   3. If the recommended file does not exist, fall back to the largest
///      installed model in `~/.cache/whisper/` so a missing download cannot
///      block the session.
///
/// Whisper model files are expected at `~/.cache/whisper/ggml-<name>.bin`
/// (standard whisper.cpp convention).
fn whisper_model_path(profile: &hardware::HardwareProfile) -> String {
    if let Ok(raw) = std::env::var("FLINT_WHISPER_MODEL") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            let as_path = PathBuf::from(trimmed);
            if as_path.is_absolute() && as_path.exists() {
                info!(path = %as_path.display(), "FLINT_WHISPER_MODEL: using explicit path");
                return as_path.to_string_lossy().into_owned();
            }
            if let Some(model) = hardware::WhisperModel::from_name(trimmed) {
                let path = whisper_cache_path(model.as_str());
                if PathBuf::from(&path).exists() {
                    info!(model = %model, "FLINT_WHISPER_MODEL: using named model");
                    return path;
                }
                warn!(
                    requested = %trimmed,
                    "FLINT_WHISPER_MODEL points to a missing file; falling back"
                );
            } else {
                warn!(
                    value = %trimmed,
                    "FLINT_WHISPER_MODEL is not a known model name or existing path; ignoring"
                );
            }
        }
    }

    let recommended = profile.recommended_whisper_model.as_str();
    let recommended_path = whisper_cache_path(recommended);
    if PathBuf::from(&recommended_path).exists() {
        return recommended_path;
    }

    if let Some(fallback) = best_installed_whisper_model() {
        warn!(
            recommended = %recommended,
            fallback = %fallback,
            "recommended Whisper model not installed; falling back to best installed model"
        );
        return whisper_cache_path(&fallback);
    }

    recommended_path
}

fn whisper_cache_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".cache").join("whisper")
}

fn whisper_cache_path(model_name: &str) -> String {
    whisper_cache_dir()
        .join(format!("ggml-{model_name}.bin"))
        .to_string_lossy()
        .into_owned()
}

/// Return the name of the largest installed Whisper model, if any.
fn best_installed_whisper_model() -> Option<String> {
    use hardware::WhisperModel::{BaseEn, MediumEn, SmallEn, TinyEn};
    let cache = whisper_cache_dir();
    for model in [MediumEn, SmallEn, BaseEn, TinyEn] {
        let path = cache.join(format!("ggml-{}.bin", model.as_str()));
        if path.exists() {
            return Some(model.as_str().to_string());
        }
    }
    None
}

/// Log the failing `start_session` step at ERROR and return a UI-facing message.
///
/// `start_session` runs many short-lived fallible steps before the session
/// reaches LIVE. When any of them returns `Err`, the Tauri command path
/// propagates the error to the React toast but nothing reaches `tracing`,
/// which makes silent startup failures (missing API key, model load, etc.)
/// invisible from the terminal. Routing every `map_err` through this helper
/// guarantees the failing step is named in the logs.
fn start_session_step_err(step: &'static str, err: impl std::fmt::Display) -> String {
    let msg = err.to_string();
    error!(step = %step, error = %msg, "start_session step failed");
    format!("{step}: {msg}")
}

/// Stop a capture thread and abort pipeline/orchestrator tasks.
async fn abort_local_live_handles(handles: LiveTaskHandles) {
    let _ = handles.stop_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handles.zeroed_rx).await;
    handles.pipeline.abort();
    handles.orchestrator.abort();
}

/// Build the failover manager and local LLM provider used by live and rehearsal paths.
async fn build_failover_stack(
    app: &AppHandle,
    _state: &AppState,
) -> Result<(Arc<FailoverManager>, Arc<dyn LLMProvider>, usize), String> {
    let (primary_provider, context_window) = match keychain::get_api_key("groq") {
        Ok(api_key) => match GroqProvider::new(api_key) {
            Ok(p) => {
                let cw = p.context_window();
                (Arc::new(p) as Arc<dyn LLMProvider>, cw)
            }
            Err(e) => {
                return Err(format!(
                    "Groq API key is stored but the provider failed to initialize: {e}. \
                     Re-enter your key in Settings → API Keys."
                ));
            }
        },
        Err(_) => {
            return Err(
                "No Groq API key found. Add your key in Settings → API Keys (Groq) \
                 before running rehearsal turns."
                    .to_string(),
            );
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
    let cloud_fallback = openrouter::resolve_openrouter();
    if cloud_fallback.is_some() {
        info!("failover stack: OpenRouter cloud fallback configured");
    } else {
        info!("failover stack: no OpenRouter key in Settings — Groq 429 will use Ollama only");
    }
    let mut failover = FailoverManager::new(
        primary_provider,
        cloud_fallback,
        Arc::clone(&local_provider),
        rate_limiter,
    );
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

    let digest = resolve_session_digest(state.inner(), sid).await?;

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
    {
        let mut slot = state.rehearsal_turn_cancel.lock().await;
        if let Some(prev) = slot.take() {
            prev.store(true, std::sync::atomic::Ordering::Release);
        }
        *slot = Some(Arc::clone(&turn_cancel));
    }

    const REHEARSAL_TURN_TIMEOUT: Duration = Duration::from_secs(120);
    let cancel_on_timeout = Arc::clone(&turn_cancel);
    let turn_result = tokio::time::timeout(
        REHEARSAL_TURN_TIMEOUT,
        dispatch_turn(
            sid,
            question_text,
            turn_number,
            Arc::new(digest),
            prompts_base_dir(),
            failover,
            state.wait_for_embedder(Duration::from_secs(45)).await?,
            Arc::clone(&state.vector_store),
            Arc::clone(&state.prewarm_cache),
            memory,
            load_compression_prompt(),
            Arc::clone(&turn_cancel),
            local_provider,
            Arc::clone(&state.persistence),
            Arc::clone(&state.cost_tracker),
            app,
        ),
    )
    .await;

    match turn_result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("Rehearsal turn failed: {e}")),
        Err(_) => {
            cancel_on_timeout.store(true, Ordering::Release);
            Err(
                "Rehearsal turn timed out after 120 seconds. Common causes: Groq free-tier \
                 rate limit (429 — wait ~10 minutes and retry), a missing Groq API key in \
                 Settings → API Keys, or Ollama fallback running without a pulled model."
                    .to_string(),
            )
        }
    }
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
        match *machine.current() {
            SessionState::Ready => {
                // Idempotent: rehearsal was already completed (e.g. start_session failed
                // after REHEARSING → READY but before LIVE).
                return Ok(());
            }
            SessionState::Rehearsing => {}
            other => {
                return Err(format!(
                    "complete_rehearsal is only valid from REHEARSING (current: {other})"
                ));
            }
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
        SessionState::Rehearsing
        | SessionState::Ready
        | SessionState::Ended
        | SessionState::DigestReview => {
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

/// Extract question-like lines from raw context text without an LLM call.
///
/// A line is treated as a question when it ends with `?` after stripping
/// whitespace, bullet markers (`-`, `*`, `•`), and numbering (`1.`, `a)`).
/// This runs in O(lines) and produces zero LLM tokens — it is the safety net
/// for large explicitly-pasted question banks that exceed the digest token budget.
fn extract_questions_from_text(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|raw| {
            // Strip leading list markers before checking content.
            let stripped = raw
                .trim()
                .trim_start_matches(|c: char| {
                    c == '-' || c == '*' || c == '•' || c == '⭐' || c == '#'
                })
                .trim();

            // Strip leading numbering: "1.", "1)", "a.", "a)" etc.
            let content = {
                let mut chars = stripped.chars().peekable();
                let mut prefix_len = 0;
                // Consume up to 3 alphanumeric chars followed by a `.` or `)`.
                let mut count = 0;
                while let Some(&c) = chars.peek() {
                    if count >= 3 {
                        break;
                    }
                    if c.is_alphanumeric() {
                        chars.next();
                        count += 1;
                        prefix_len += c.len_utf8();
                    } else if (c == '.' || c == ')') && count > 0 {
                        chars.next();
                        prefix_len += c.len_utf8();
                        break;
                    } else {
                        break;
                    }
                }
                let remainder = stripped[prefix_len..].trim();
                if remainder.is_empty() {
                    stripped
                } else {
                    remainder
                }
            };

            if content.ends_with('?') && content.len() > 5 {
                Some(content.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Extract "Q: ...\nA: ..." pairs from raw context text for embedding.
///
/// Scans for lines starting with "Q:" (case-insensitive, optionally "Question:")
/// followed by one or more "A:" lines. Blank lines between pairs are tolerated.
/// Returns each pair formatted as "Q: {question}\nA: {answer}" for embedding
/// into the context vector store.
///
/// Only called from `confirm_digest` to extract trusted user-curated Q&A content
/// that should be retrievable at inference time.
fn extract_qa_pairs_from_text(text: &str) -> Vec<String> {
    let mut pairs: Vec<String> = Vec::new();
    let mut current_q: Option<String> = None;
    let mut current_a_lines: Vec<String> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();

        let is_q_line = lower.starts_with("q:") || lower.starts_with("question:");
        let is_a_line = lower.starts_with("a:") || lower.starts_with("answer:");

        if is_q_line {
            // Flush previous pair if complete.
            if let Some(q) = current_q.take() {
                let a = current_a_lines.join(" ").trim().to_string();
                if !a.is_empty() {
                    pairs.push(format!("Q: {q}\nA: {a}"));
                }
                current_a_lines.clear();
            }
            let q_text = trimmed
                .trim_start_matches(|c: char| c.is_alphabetic() || c == ':')
                .trim()
                .to_string();
            if !q_text.is_empty() {
                current_q = Some(q_text);
            }
        } else if is_a_line {
            let a_text = trimmed
                .trim_start_matches(|c: char| c.is_alphabetic() || c == ':')
                .trim()
                .to_string();
            current_a_lines.push(a_text);
        } else if !trimmed.is_empty() && current_q.is_some() && !current_a_lines.is_empty() {
            // Continuation of the current answer.
            current_a_lines.push(trimmed.to_string());
        }
    }

    // Flush last pair.
    if let Some(q) = current_q {
        let a = current_a_lines.join(" ").trim().to_string();
        if !a.is_empty() {
            pairs.push(format!("Q: {q}\nA: {a}"));
        }
    }

    pairs
}

/// Return the question bank for a session.
///
/// Returns digest `likely_questions` merged with any user-added questions,
/// with duplicates removed. Order: digest Qs first (stable), then user-added.
#[tauri::command]
pub async fn get_question_bank(
    state: State<'_, AppState>,
    session_id: String,
    shuffle: Option<bool>,
) -> Result<Vec<crate::dto::QuestionBankEntryDto>, String> {
    use crate::session::question_attempts::normalize_question_key;
    use std::collections::HashMap;

    let sid = validate_session_id(&state, &session_id).await?;
    let shuffle = shuffle.unwrap_or(false);

    let mut bank = state
        .persistence
        .load_question_bank(sid)
        .map_err(|e| e.to_string())?;

    if bank.is_empty() {
        bank = state
            .persistence
            .load_session_digest(sid)
            .ok()
            .flatten()
            .map(|d| d.likely_questions)
            .unwrap_or_default();

        if !bank.is_empty() {
            state
                .persistence
                .store_question_bank(sid, &bank)
                .map_err(|e| e.to_string())?;
        }
    }

    let attempts = state
        .persistence
        .load_question_attempts(sid)
        .map_err(|e| e.to_string())?;

    let entries: Vec<crate::dto::QuestionBankEntryDto> = bank
        .iter()
        .map(|q| {
            let key = normalize_question_key(q);
            if let Some(a) = attempts.get(&key) {
                crate::dto::QuestionBankEntryDto {
                    question: q.clone(),
                    satisfied: a.satisfied,
                    confidence_score: a.confidence_score,
                    coach_score: a.coach_score,
                    last_source: Some(a.last_source.clone()),
                }
            } else {
                crate::dto::QuestionBankEntryDto {
                    question: q.clone(),
                    satisfied: false,
                    confidence_score: 0.0,
                    coach_score: 0,
                    last_source: None,
                }
            }
        })
        .collect();

    let (mut pending, completed): (Vec<_>, Vec<_>) =
        entries.into_iter().partition(|e| !e.satisfied);

    if shuffle && pending.len() > 1 {
        let mut order: Vec<String> = pending.iter().map(|e| e.question.clone()).collect();
        crate::session::shuffle::shuffle_strings(
            &mut order,
            crate::session::shuffle::session_shuffle_seed(sid),
        );
        let rank: HashMap<String, usize> =
            order.into_iter().enumerate().map(|(i, q)| (q, i)).collect();
        pending.sort_by_key(|e| rank.get(&e.question).copied().unwrap_or(0));
    }

    pending.extend(completed);
    Ok(pending)
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

    let (failover, _, _) = build_failover_stack(&app, &state).await?;
    let web = tavily::resolve_tavily();

    let outcome = research::run_prep_research_turn(&message, chunks, failover, web, app.clone())
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

    // StrictMode double-mount can fire two concurrent starts; serialize them.
    let _live_start_guard = state.live_start_lock.lock().await;

    let current = {
        let machine = state.state_machine.lock().await;
        *machine.current()
    };
    if current == SessionState::Live && state.live_tasks.lock().await.is_some() {
        return Ok(());
    }
    if current != SessionState::Ready {
        return Err(format!("start_session requires READY (current: {current})"));
    }

    if state.live_tasks.lock().await.is_some() {
        return Err("A live session is already running.".to_string());
    }

    // ── 1. Hardware profile and Whisper model ─────────────────────────────
    let profile = hardware::assess_hardware();
    let model_path = whisper_model_path(&profile);

    let whisper = Arc::new(WhisperEngine::new(&model_path, profile.tier).map_err(|e| {
        start_session_step_err(
            "whisper init",
            format!("model={model_path} tier={} error={e}", profile.tier),
        )
    })?);

    // ── 2. Question detector ──────────────────────────────────────────────
    let detector = Arc::new(
        QuestionDetector::new(
            profile.tier,
            Some(Arc::clone(&state.llm)),
            &prompts_base_dir(),
        )
        .map_err(|e| start_session_step_err("question detector init", e))?,
    );

    // ── 3. Audio channels ─────────────────────────────────────────────────
    //
    // Capacity sized for ~1 s of audio frames per channel: 480-sample frames
    // at 48 kHz = 100 frames/s. 1024 absorbs startup transients (capture
    // emits frames before the pipeline task has been polled) without
    // dropping. At 4 bytes/sample × 480 × 1024 ≈ 1.9 MB per channel — cheap.
    let (system_tx, system_rx) = tokio::sync::mpsc::channel(1024);
    let (mic_tx, mic_rx) = tokio::sync::mpsc::channel(1024);
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
        .map_err(|_| start_session_step_err("audio capture", "startup timed out after 5s"))?
        .map_err(|_| {
            start_session_step_err(
                "audio capture",
                "capture thread exited before sending ready signal",
            )
        })?
        .map_err(|e| start_session_step_err("audio capture", e))?;

    // ── 5. Build failover manager and conversation memory ─────────────────

    let (failover, local_provider, context_window) = build_failover_stack(&app, &state)
        .await
        .map_err(|e| start_session_step_err("failover stack", e))?;

    let memory = Arc::new(tokio::sync::Mutex::new(ConversationMemory::new(
        context_window,
    )));
    *state.session_memory.lock().await = Some(Arc::clone(&memory));

    let turn_cancel_slot: Arc<tokio::sync::Mutex<Option<crate::state::TurnCancelFlag>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    let compression_prompt = load_compression_prompt();

    // ── 6. Spawn background tasks ─────────────────────────────────────────

    let digest = resolve_session_digest(state.inner(), sid)
        .await
        .map_err(|e| start_session_step_err("digest resolve", e))?;

    let orch_config = OrchestratorConfig {
        session_id: sid,
        digest: Arc::new(digest),
        prompts_dir: prompts_base_dir(),
        failover: Arc::clone(&failover),
        embedder: state
            .require_embedder()
            .map_err(|e| start_session_step_err("embedder ready", e))?,
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

    let handles = LiveTaskHandles {
        stop_tx,
        zeroed_rx,
        pipeline,
        orchestrator,
        question_tx,
        turn_cancel: turn_cancel_slot,
    };

    // ── 7. State transition READY → LIVE ──────────────────────────────────
    //
    // Install `live_tasks` only after a successful transition so a concurrent
    // duplicate `start_session` cannot overwrite the winner's handles. If
    // another caller already reached LIVE, drop our duplicate tasks quietly.

    {
        let mut machine = state.state_machine.lock().await;
        if *machine.current() == SessionState::Live {
            drop(machine);
            abort_local_live_handles(handles).await;
            return Ok(());
        }
        if let Err(e) = machine.transition(SessionState::Live) {
            let err_msg = e.to_string();
            drop(machine);
            abort_local_live_handles(handles).await;
            if err_msg.contains("LIVE → LIVE") {
                return Ok(());
            }
            return Err(session_error(e));
        }
    }

    *state.live_tasks.lock().await = Some(handles);
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

/// Cancel any running inference — valid from LIVE or REHEARSING.
///
/// Sets the active turn's cancellation flag so in-flight token streams stop.
#[tauri::command]
pub async fn cancel_inference(state: State<'_, AppState>) -> Result<(), String> {
    let current = {
        let machine = state.state_machine.lock().await;
        *machine.current()
    };

    match current {
        SessionState::Live => {
            let guard = state.live_tasks.lock().await;
            if let Some(handles) = guard.as_ref() {
                let slot = handles.turn_cancel.lock().await;
                if let Some(flag) = slot.as_ref() {
                    flag.store(true, std::sync::atomic::Ordering::Release);
                    info!("cancel_inference: live turn cancelled");
                }
            }
        }
        SessionState::Rehearsing => {
            let slot = state.rehearsal_turn_cancel.lock().await;
            if let Some(flag) = slot.as_ref() {
                flag.store(true, std::sync::atomic::Ordering::Release);
                info!("cancel_inference: rehearsal turn cancelled");
            }
        }
        other => {
            return Err(format!(
                "cancel_inference is only valid from LIVE or REHEARSING (current: {other})"
            ));
        }
    }

    Ok(())
}

/// Toggle overlay visibility (panic hotkey path).
///
/// Hides panel content via `overlay_visibility` only — does not close the window,
/// so the restore chord still works on Wayland.
#[tauri::command]
pub async fn panic_hide_overlay(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    use crate::events::{emit_overlay_visibility, OverlayVisibilityPayload};

    let mut hidden = state
        .overlay_panic_hidden
        .lock()
        .map_err(|e| format!("overlay_panic_hidden lock poisoned: {e}"))?;
    *hidden = !*hidden;
    emit_overlay_visibility(&app, OverlayVisibilityPayload { hidden: *hidden });
    Ok(*hidden)
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
    const MIN_TOKEN_CAP: u64 = 500;
    if let Some(t) = max_total_tokens {
        if t > 0 && t < MIN_TOKEN_CAP {
            return Err(format!(
                "Token limit must be at least {MIN_TOKEN_CAP} — each question uses \
                 ~300+ estimated tokens (3 LLM calls). This is a ceiling, not a target."
            ));
        }
    }

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
        Box::new(keychain::clear_account_secrets),
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

/// Copy text to the OS clipboard (native path — reliable in the Tauri WebView).
#[tauri::command]
pub fn copy_text_to_clipboard(text: String) -> Result<(), String> {
    if text.is_empty() {
        return Err("Nothing to copy".to_string());
    }
    arboard::Clipboard::new()
        .map_err(|e| format!("Clipboard unavailable: {e}"))?
        .set_text(text)
        .map_err(|e| format!("Failed to copy: {e}"))
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

// ──────────────────────────────────────────────────────────────────────────────
// Phase 8 — Mock Interview commands
// ──────────────────────────────────────────────────────────────────────────────

fn mock_audio_dir(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .map(|d| d.join("mock_audio"))
        .unwrap_or_else(|_| PathBuf::from("mock_audio"))
}

fn validate_mock_audio_path(app: &AppHandle, path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err("mock audio path must be absolute".to_string());
    }
    let audio_dir = mock_audio_dir(app);
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    let dir_canonical = audio_dir
        .canonicalize()
        .unwrap_or_else(|_| audio_dir.clone());
    if !canonical.starts_with(&dir_canonical) {
        return Err("mock audio path is outside the session audio directory".to_string());
    }
    Ok(canonical)
}

async fn signal_mock_turn_complete(state: &AppState, user_text: String, audio_path: String) {
    if let Some(handles) = state.mock_tasks.lock().await.as_ref() {
        let _ = handles
            .conductor
            .cmd_tx
            .send(ConductorCommand::TurnComplete {
                user_text,
                audio_path,
            })
            .await;
    }
}

/// Transition to MOCK_INTERVIEW and start the conductor + mic capture loop.
///
/// Valid from: `REHEARSING` or `READY`.
///
/// `READY` is allowed because draft recovery and the Rehearsal screen both
/// surface mock entry while the persisted state is already READY (user
/// completed rehearsal but has not gone live yet).
///
/// Steps:
/// 1. REHEARSING → MOCK_INTERVIEW.
/// 2. Build (or reuse) FailoverManager.
/// 3. Retrieve session digest (must be confirmed before calling this).
/// 4. Retrieve RAG chunks for the question bank.
/// 5. Start `MicCapture` and `Conductor`.
/// 6. Store `MockTaskHandles` in AppState.
#[tauri::command]
pub async fn start_mock(
    app: AppHandle,
    state: State<'_, AppState>,
    guided: Option<bool>,
    mode: Option<String>,
    shuffle: Option<bool>,
) -> Result<(), String> {
    let guided = guided.unwrap_or(false);
    let shuffle = shuffle.unwrap_or(false);
    let mock_mode = MockMode::parse(mode.as_deref().unwrap_or("practice"));
    let pace = if guided {
        MockPace::Guided
    } else {
        MockPace::Continuous
    };
    // Guard: must be in REHEARSING. If we are already in MOCK_INTERVIEW with an
    // active session, treat this as idempotent so React StrictMode double-mounts
    // and re-navigation to the screen do not blow up the conductor.
    {
        let machine = state.state_machine.lock().await;
        match machine.current() {
            SessionState::Rehearsing | SessionState::Ready => {}
            SessionState::MockInterview if state.mock_tasks.lock().await.is_some() => {
                return Ok(());
            }
            current => {
                return Err(format!(
                    "start_mock is only valid from REHEARSING or READY (current: {current})"
                ));
            }
        }
    }

    let session_id = {
        let machine = state.state_machine.lock().await;
        machine.session_id().ok_or("no active session")?
    };

    // Each mock run starts fresh — drop turns and WAV files from prior runs on
    // this session so End & review only reflects the current interview.
    state
        .persistence
        .delete_mock_turns(session_id)
        .map_err(|e| e.to_string())?;

    let digest = state
        .session_digest
        .read()
        .await
        .clone()
        .ok_or("digest not set — confirm session design first")?;
    let digest = Arc::new(digest);
    if digest.likely_questions.is_empty() {
        return Err(
            "No practice questions in session digest — complete Digest Review first.".to_string(),
        );
    }

    // Build FailoverManager.
    let (failover, _, _) = build_failover_stack(&app, &state).await?;

    let embedder = state.require_embedder()?;
    let suggested_buffer = Arc::new(std::sync::RwLock::new(String::new()));

    // Create audio directory.
    let audio_dir = mock_audio_dir(&app);
    std::fs::create_dir_all(&audio_dir).map_err(|e| e.to_string())?;

    // Initialise WhisperEngine.
    let profile = hardware::assess_hardware();
    let model_path = whisper_model_path(&profile);
    let whisper =
        Arc::new(WhisperEngine::new(&model_path, profile.tier).map_err(|e| e.to_string())?);

    let mic_capture = MicCapture::start(
        app.clone(),
        session_id,
        audio_dir.clone(),
        Arc::clone(&whisper),
    )
    .await
    .map_err(|e| e.to_string())?;

    let role_packs = packs_for_role(&digest.domain, &digest.role);

    let active_turn_n = Arc::new(AtomicU32::new(0));
    let mic_recording = Arc::new(AtomicBool::new(false));

    let conductor = Conductor::start(
        app.clone(),
        session_id,
        Arc::clone(&digest),
        Arc::clone(&failover),
        Arc::clone(&state.persistence),
        prompts_base_dir(),
        embedder,
        Arc::clone(&state.vector_store),
        Arc::clone(&state.global_kb),
        role_packs.clone(),
        Arc::clone(&suggested_buffer),
        pace,
        mock_mode,
        shuffle,
        Arc::clone(&active_turn_n),
    );

    // REHEARSING → MOCK_INTERVIEW.
    {
        let mut machine = state.state_machine.lock().await;
        machine
            .transition(SessionState::MockInterview)
            .map_err(session_error)?;
    }
    emit_state(&app, SessionState::MockInterview);

    *state.mock_tasks.lock().await = Some(MockTaskHandles {
        conductor,
        mic_capture,
        active_turn_n,
        mic_recording,
        guided,
        mode: mock_mode,
        suggested_text: suggested_buffer,
        role_packs,
    });

    info!(
        session_id = %session_id,
        guided,
        shuffle,
        mode = mock_mode.as_str(),
        "mock interview started"
    );
    Ok(())
}

/// Ask the conductor to speak the next question (guided mode only).
#[tauri::command]
pub async fn ask_mock_question(state: State<'_, AppState>) -> Result<(), String> {
    let guard = state.mock_tasks.lock().await;
    let handles = guard.as_ref().ok_or("no active mock session")?;
    if !handles.guided {
        return Err("ask_mock_question is only valid in guided (step-by-step) mode".to_string());
    }
    handles
        .conductor
        .cmd_tx
        .send(ConductorCommand::AskQuestion)
        .await
        .map_err(|e| e.to_string())
}

/// Start recording the user's microphone for the current mock turn.
///
/// Call after the frontend has confirmed the AI question has been spoken.
#[tauri::command]
pub async fn start_mock_turn(state: State<'_, AppState>) -> Result<(), String> {
    let guard = state.mock_tasks.lock().await;
    let handles = guard.as_ref().ok_or("no active mock session")?;
    let turn_n = handles.active_turn_n.load(Ordering::SeqCst);
    if turn_n == 0 {
        return Err("No mock question is active yet.".to_string());
    }
    handles
        .mic_capture
        .start_turn(turn_n)
        .await
        .map_err(|e| e.to_string())?;
    handles.mic_recording.store(true, Ordering::SeqCst);
    Ok(())
}

/// Stop recording, run coach LLM, persist results, and signal the conductor.
///
/// Coach runs before the conductor advances so `mock_coach_feedback` is emitted
/// before the next `mock_question_started` resets the UI.
#[tauri::command]
pub async fn end_mock_turn(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // Pull the turn number and queue the `EndTurn` command while holding the
    // mock_tasks guard, then drop it before the (potentially long) await on
    // the reply so concurrent commands like `stop_mock` are not blocked.
    let (turn_n, reply_rx, mic_recording) = {
        let guard = state.mock_tasks.lock().await;
        let handles = guard.as_ref().ok_or("no active mock session")?;
        let turn_n = handles.active_turn_n.load(Ordering::SeqCst);
        if turn_n == 0 {
            return Err("No mock question is active.".to_string());
        }
        let reply_rx = handles
            .mic_capture
            .send_end_turn()
            .await
            .map_err(|e| e.to_string())?;
        (turn_n, reply_rx, Arc::clone(&handles.mic_recording))
    };

    // Give the user up to 120 s to finish answering; in practice they stop
    // manually well before that.
    let (transcript, audio_path) =
        crate::mock::mic_capture::await_end_turn_reply(reply_rx, Duration::from_secs(120))
            .await
            .map_err(|e| e.to_string())?;
    mic_recording.store(false, Ordering::SeqCst);

    let session_id = state
        .state_machine
        .lock()
        .await
        .session_id()
        .ok_or("no active session")?;

    if transcript.trim().is_empty() {
        if audio_path.is_empty() {
            state
                .persistence
                .mark_mock_turn_skipped(session_id, turn_n)
                .map_err(|e| e.to_string())?;
        } else {
            state
                .persistence
                .update_mock_turn_user_answer_by_turn_n(session_id, turn_n, "", &audio_path, "")
                .map_err(|e| e.to_string())?;
        }
        let (coach_json, score) = coach_failure_payload(
            "No speech detected — speak for a few seconds before finishing your answer.",
        );
        emit_mock_coach_feedback(
            &app,
            MockCoachFeedbackPayload {
                turn_n,
                coach_json: coach_json.clone(),
                score,
            },
        );
        if let Err(e) = state.persistence.update_mock_turn_coach_by_turn_n(
            session_id,
            turn_n,
            &coach_json,
            score,
        ) {
            warn!(error = %e, turn_n, "failed to persist empty-transcript coach feedback");
        }
        signal_mock_turn_complete(&state, transcript, audio_path).await;
        return Ok(());
    }

    let (mock_mode, suggested_answer, role_packs) = {
        let guard = state.mock_tasks.lock().await;
        let handles = guard.as_ref().ok_or("no active mock session")?;
        let suggested = handles
            .suggested_text
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        (handles.mode, suggested, handles.role_packs.clone())
    };

    let session_id = state
        .state_machine
        .lock()
        .await
        .session_id()
        .ok_or("no active session")?;
    let (failover, _, _) = build_failover_stack(&app, &state).await?;
    let persistence = Arc::clone(&state.persistence);
    let question = persistence
        .load_mock_turns(session_id)
        .unwrap_or_default()
        .into_iter()
        .find(|t| t.turn_n == turn_n)
        .map(|t| t.question)
        .unwrap_or_default();

    if let Err(e) = persistence.update_mock_turn_user_answer_by_turn_n(
        session_id,
        turn_n,
        &transcript,
        &audio_path,
        &suggested_answer,
    ) {
        warn!(error = %e, turn_n, "failed to persist mock turn user answer");
    }

    let rag_chunks = if let Ok(embedder) = state.require_embedder() {
        query_mock_rag(
            session_id,
            &question,
            &embedder,
            state.vector_store.as_ref(),
            Some((state.global_kb.as_ref(), &role_packs)),
            8,
        )
        .await
    } else {
        vec![]
    };
    let prompts = prompts_base_dir();

    let (coach_json, score) = match run_coach(
        app.clone(),
        session_id,
        turn_n,
        question.clone(),
        transcript.clone(),
        suggested_answer.clone(),
        rag_chunks,
        mock_mode,
        failover,
        &prompts,
    )
    .await
    {
        Ok(result) => result,
        Err(e) => {
            warn!(error = %e, turn_n, "coach LLM failed");
            let (json, score) = coach_failure_payload(
                "Coach analysis failed — check your Groq key or rate limits.",
            );
            emit_mock_coach_feedback(
                &app,
                MockCoachFeedbackPayload {
                    turn_n,
                    coach_json: json.clone(),
                    score,
                },
            );
            (json, score)
        }
    };

    if mock_mode == MockMode::Practice && !suggested_answer.is_empty() {
        emit_mock_suggested_token(
            &app,
            MockSuggestedTokenPayload {
                token: suggested_answer,
            },
        );
    }

    if let Err(e) =
        persistence.update_mock_turn_coach_by_turn_n(session_id, turn_n, &coach_json, score)
    {
        warn!(error = %e, "failed to persist mock turn coach feedback");
    }

    if !question.is_empty() {
        let satisfied = crate::session::question_attempts::mock_attempt_satisfied(score, false);
        if let Err(e) = persistence
            .upsert_question_attempt(session_id, &question, "mock", 0.0, score, satisfied)
        {
            warn!(error = %e, "failed to record mock question attempt");
        }
    }

    // Signal the conductor to advance to the next question.
    signal_mock_turn_complete(&state, transcript, audio_path).await;

    Ok(())
}

/// Return a data URL for a mock turn WAV so the summary screen can play it
/// without relying on the asset protocol scope.
#[tauri::command]
pub fn read_mock_audio_data_url(app: AppHandle, path: String) -> Result<String, String> {
    use base64::Engine;

    let canonical = validate_mock_audio_path(&app, &path)?;
    let bytes = std::fs::read(&canonical).map_err(|e| e.to_string())?;
    if bytes.is_empty() {
        return Err("recording file is empty".to_string());
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:audio/wav;base64,{encoded}"))
}

/// Skip the current mock turn without recording an answer.
#[tauri::command]
pub async fn skip_mock_turn(state: State<'_, AppState>) -> Result<(), String> {
    let session_id = state
        .state_machine
        .lock()
        .await
        .session_id()
        .ok_or("no active session")?;

    let (turn_n, persistence, conductor_tx, mic_recording) = {
        let guard = state.mock_tasks.lock().await;
        let handles = guard.as_ref().ok_or("no active mock session")?;
        let turn_n = handles.active_turn_n.load(Ordering::SeqCst);
        if turn_n == 0 {
            return Err("No mock question is active.".to_string());
        }
        (
            turn_n,
            Arc::clone(&state.persistence),
            handles.conductor.cmd_tx.clone(),
            Arc::clone(&handles.mic_recording),
        )
    };

    tts::stop_active().await;

    if mic_recording.swap(false, Ordering::SeqCst) {
        if let Some(handles) = state.mock_tasks.lock().await.as_ref() {
            if let Ok(reply_rx) = handles.mic_capture.send_end_turn().await {
                let _ = crate::mock::mic_capture::await_end_turn_reply(
                    reply_rx,
                    Duration::from_secs(3),
                )
                .await;
            }
        }
    }

    if let Some(question) = persistence
        .load_mock_turns(session_id)
        .unwrap_or_default()
        .into_iter()
        .find(|t| t.turn_n == turn_n)
        .map(|t| t.question)
    {
        let _ = persistence.upsert_question_attempt(session_id, &question, "mock", 0.0, 0, false);
    }

    persistence
        .mark_mock_turn_skipped(session_id, turn_n)
        .map_err(|e| e.to_string())?;

    let _ = conductor_tx
        .send(ConductorCommand::TurnComplete {
            user_text: String::new(),
            audio_path: String::new(),
        })
        .await;

    Ok(())
}

/// Abort the mock interview and transition to REHEARSING.
/// Stop the mock interview session.
///
/// `finish: true` — end early and emit `mock_ended` for the summary screen.
/// `finish: false` (default) — cancel without summary; user can restart from the picker.
///
/// Valid from: `MOCK_INTERVIEW`.
#[tauri::command]
pub async fn stop_mock(
    app: AppHandle,
    state: State<'_, AppState>,
    finish: Option<bool>,
) -> Result<(), String> {
    let finish = finish.unwrap_or(false);
    // Signal conductor to stop.
    if let Some(handles) = state.mock_tasks.lock().await.as_ref() {
        let cmd = if finish {
            ConductorCommand::FinishEarly
        } else {
            ConductorCommand::Abort
        };
        let _ = handles.conductor.cmd_tx.send(cmd).await;
    }

    // Shutdown mic capture.
    if let Some(handles) = state.mock_tasks.lock().await.take() {
        handles.mic_capture.shutdown().await;
    }

    {
        let mut machine = state.state_machine.lock().await;
        if *machine.current() == SessionState::MockInterview {
            machine
                .transition(SessionState::Rehearsing)
                .map_err(session_error)?;
        }
    }
    emit_state(&app, SessionState::Rehearsing);

    Ok(())
}

/// Return all completed mock turns for the session (used by Summary screen).
#[tauri::command]
pub async fn get_mock_turns(state: State<'_, AppState>) -> Result<Vec<MockTurnDto>, String> {
    let session_id = state
        .state_machine
        .lock()
        .await
        .session_id()
        .ok_or("no active session")?;
    let turns = state
        .persistence
        .load_mock_turns(session_id)
        .map_err(|e| e.to_string())?;
    Ok(turns
        .into_iter()
        .map(|t| MockTurnDto {
            id: t.id.to_string(),
            turn_n: t.turn_n,
            question: t.question,
            user_text: t.user_text,
            audio_path: t.audio_path,
            coach_json: t.coach_json,
            suggested: t.suggested,
            score: t.score,
        })
        .collect())
}

#[derive(serde::Serialize)]
pub struct MockTurnDto {
    pub id: String,
    pub turn_n: u32,
    pub question: String,
    pub user_text: String,
    pub audio_path: String,
    pub coach_json: String,
    pub suggested: String,
    pub score: u8,
}
