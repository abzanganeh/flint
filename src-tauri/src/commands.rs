use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Manager, State};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::audio::capture::AudioCapture;
use crate::audio::pipeline::{run_audio_pipeline, DetectedQuestion};
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
use crate::llm::failover::FailoverManager;
use crate::llm::groq::GroqProvider;
use crate::llm::ollama::OllamaProvider;
use crate::llm::provider::LLMProvider;
use crate::llm::rate_limiter::RateLimiter;
use crate::orchestrator::prewarm::{run_prewarm, PreWarmCache};
use crate::orchestrator::{dispatch_turn, run_orchestrator, OrchestratorConfig};
use crate::rag::chunker::chunk_text;
use crate::session::memory::ConversationMemory;
use crate::session::state::SessionState;
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
    Ok(results
        .into_iter()
        .map(HealthCheckResultDto::from)
        .collect())
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
        return Err(
            "Context text is empty — please paste your job description or notes.".to_string(),
        );
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
                (stub, stub.context_window())
            }
        },
        Err(_) => {
            warn!("No Groq API key in keychain — using stub LLM provider");
            let stub = Arc::clone(&state.llm);
            (stub, stub.context_window())
        }
    };

    let local_provider: Arc<dyn LLMProvider> =
        Arc::new(OllamaProvider::new().map_err(|e| format!("Failed to build Ollama provider: {e}"))?);
    let rate_limiter = Arc::new(RateLimiter::new(
        primary_provider.name(),
        primary_provider.rate_limit().requests_per_minute,
        primary_provider.rate_limit().tokens_per_minute,
    ));
    let mut failover =
        FailoverManager::new(primary_provider, Arc::clone(&local_provider), rate_limiter);
    failover.start_ping_loop(app.clone());
    Ok((
        Arc::new(failover),
        local_provider,
        context_window,
    ))
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
            *guard = Some(Arc::new(tokio::sync::Mutex::new(
                ConversationMemory::new(context_window),
            )));
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
        Arc::clone(&state.embedder),
        Arc::clone(&state.vector_store),
        Arc::clone(&state.prewarm_cache),
        memory,
        load_compression_prompt(),
        turn_cancel,
        local_provider,
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
        return Err(
            "Complete rehearsal before starting a live session.".to_string(),
        );
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
        embedder: Arc::clone(&state.embedder),
        vector_store: Arc::clone(&state.vector_store),
        prewarm_cache: Arc::clone(&state.prewarm_cache),
        memory,
        compression_prompt,
        local_llm: local_provider,
        turn_cancel_slot: Arc::clone(&turn_cancel_slot),
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

    // ENDING → ENDED
    {
        let mut machine = state.state_machine.lock().await;
        machine
            .transition(SessionState::Ended)
            .map_err(session_error)?;
    }
    emit_state(&app, SessionState::Ended);

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
        info!(session_id = %sid, question = %question_text, "manual trigger_response");
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
            emit_overlay_visibility(
                &app,
                OverlayVisibilityPayload { hidden: true },
            );
            Ok(true)
        } else {
            window
                .show()
                .map_err(|e| format!("Failed to show overlay: {e}"))?;
            let _ = window.set_focus();
            emit_overlay_visibility(
                &app,
                OverlayVisibilityPayload { hidden: false },
            );
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
