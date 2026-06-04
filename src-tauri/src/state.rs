use std::sync::Arc;

use anyhow::{Context, Result};
use tauri::{App, Manager};
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::audio::pipeline::DetectedQuestion;

use crate::auth_session::restore_auth_from_keychain;
use crate::digest::Digest;
use crate::interfaces::auth::{AuthInterface, AuthToken};
use crate::interfaces::session::{SessionInterface, StubSession};
use crate::interfaces::vector::VectorInterface;
use crate::llm::provider::{LLMProvider, StubLLMProvider};
use crate::orchestrator::prewarm::PreWarmCache;
use crate::rag::embedder::Embedder;
use crate::rag::store::SqliteVecStore;
use crate::session::memory::ConversationMemory;
use crate::session::persistence::SessionPersistence;
use crate::session::state::SessionStateMachine;
use crate::supabase::SupabaseAuth;

// ── Live session handles (Phase 3) ───────────────────────────────────────────

/// Handles for the running audio capture thread and background tasks.
///
/// `AudioCapture` contains `cpal::Stream` which is `!Send`, so the capture
/// lives on a dedicated OS thread. Communication with that thread happens via
/// `stop_tx`: sending `()` causes the thread to call `AudioCapture::stop()`
/// (which zeroes both ring buffers) and then exit, closing the audio channels
/// and allowing the pipeline task to drain and terminate naturally.
pub struct LiveTaskHandles {
    /// Signal the audio capture thread to stop. Dropping this also triggers
    /// the thread to stop, so dropping the whole struct is safe.
    pub stop_tx: oneshot::Sender<()>,
    /// Becomes ready when `AudioCapture::stop()` has returned and both ring
    /// buffers have been zeroed. `stop_session` awaits this before emitting
    /// ENDED so the security invariant ("cleared on session end") is met.
    pub zeroed_rx: oneshot::Receiver<()>,
    /// Background pipeline task. Aborted on stop.
    pub pipeline: JoinHandle<anyhow::Result<()>>,
    /// Orchestrator task — receives `DetectedQuestion`s and fires the three
    /// parallel response threads. Replaces the Phase 3 drain task.
    pub orchestrator: JoinHandle<()>,
    /// Sender side of the question channel — used by `trigger_response` to
    /// inject a manual question into the orchestrator.
    pub question_tx: mpsc::Sender<DetectedQuestion>,
}

/// Shared application state for Tauri commands.
pub struct AppState {
    // ── Auth (Phase 1) ───────────────────────────────────────────────────────
    pub auth: Arc<dyn AuthInterface>,
    #[allow(dead_code)] // Supabase session sync — Phase 3+
    pub session: Arc<dyn SessionInterface>,
    pub plugins: std::collections::HashMap<String, serde_json::Value>,
    auth_token: RwLock<Option<AuthToken>>,

    // ── Session (Phase 2) ────────────────────────────────────────────────────
    /// The single authoritative session lifecycle state machine.
    pub state_machine: Arc<Mutex<SessionStateMachine>>,
    /// Digest extracted from context text; set after INGESTING, read in
    /// DIGEST_REVIEW and PRE_WARMING.
    pub session_digest: Arc<RwLock<Option<Digest>>>,
    /// Pre-warm responses keyed by question embedding. Checked before every
    /// live-session inference call (cache hit threshold: cosine ≥ 0.85).
    pub prewarm_cache: Arc<Mutex<PreWarmCache>>,
    /// Write-through SQLite persistence for state transitions, transcript
    /// chunks, and responses. WAL mode enforced at open time.
    #[allow(dead_code)] // used by commands after Phase 2.9 integration
    pub persistence: Arc<SessionPersistence>,
    /// bge-small-en-v1.5 embedder — initialised once at startup.
    pub embedder: Arc<Embedder>,
    /// sqlite-vec vector store. Isolated per session (separate virtual table
    /// per session UUID).
    pub vector_store: Arc<dyn VectorInterface>,
    /// Active LLM provider. Defaults to `StubLLMProvider` until a real
    /// provider is configured in Phase 3.
    pub llm: Arc<dyn LLMProvider>,

    // ── Live session (Phase 3 / Phase 4) ────────────────────────────────────
    /// Audio capture thread stop signal + background task handles. `Some`
    /// only while the session is LIVE. Cleared on `stop_session`.
    pub live_tasks: Mutex<Option<LiveTaskHandles>>,
    /// Per-session conversation memory. Populated at `start_session`, cleared
    /// at `stop_session`. Shared between the orchestrator and any commands that
    /// need to inspect history (e.g. `get_session_snapshot`).
    pub session_memory: Arc<Mutex<Option<ConversationMemory>>>,
}

impl AppState {
    pub fn new(app: &App) -> Result<Self> {
        let plugins = app.config().plugins.0.clone();

        let auth = Arc::new(
            SupabaseAuth::from_tauri_plugins(&plugins).context("Failed to initialise auth")?,
        );

        // ── App data directory ───────────────────────────────────────────────
        let data_dir = app
            .path()
            .app_data_dir()
            .context("Cannot determine app data directory")?;
        std::fs::create_dir_all(&data_dir)
            .context("Cannot create app data directory")?;

        let persistence_path = data_dir.join("flint.db");
        let vec_db_path = data_dir.join("flint_vec.db");

        // ── Session persistence ──────────────────────────────────────────────
        let persistence = Arc::new(
            SessionPersistence::new(
                persistence_path
                    .to_str()
                    .context("Non-UTF-8 persistence path")?,
            )
            .context("Failed to open session persistence DB")?,
        );

        // ── Vector store ─────────────────────────────────────────────────────
        let vector_store: Arc<dyn VectorInterface> = Arc::new(
            SqliteVecStore::new(
                vec_db_path
                    .to_str()
                    .context("Non-UTF-8 vector store path")?,
            )
            .context("Failed to open vector store DB")?,
        );

        // ── Embedder ─────────────────────────────────────────────────────────
        // Embedder::new() downloads/loads the bge-small-en-v1.5 ONNX model.
        // The model is cached locally after the first run; subsequent startups
        // take < 1 s.
        let embedder = Arc::new(
            Embedder::new().context("Failed to initialise embedder — check network on first run")?,
        );

        // ── State machine wired to persistence ───────────────────────────────
        let persister = Arc::clone(&persistence) as Arc<dyn crate::session::state::StatePersister>;
        let state_machine = Arc::new(Mutex::new(
            SessionStateMachine::with_persister(persister),
        ));

        Ok(Self {
            auth,
            session: Arc::new(StubSession),
            plugins,
            auth_token: RwLock::new(None),
            state_machine,
            session_digest: Arc::new(RwLock::new(None)),
            prewarm_cache: Arc::new(Mutex::new(PreWarmCache::new())),
            persistence,
            embedder,
            vector_store,
            llm: Arc::new(StubLLMProvider),
            live_tasks: Mutex::new(None),
            session_memory: Arc::new(Mutex::new(None)),
        })
    }

    // ── Auth helpers ─────────────────────────────────────────────────────────

    pub async fn set_auth_token(&self, token: Option<AuthToken>) {
        *self.auth_token.write().await = token;
    }

    pub async fn auth_token(&self) -> Option<AuthToken> {
        self.auth_token.read().await.clone()
    }

    /// Load keychain tokens into memory, refreshing if the access token expired.
    pub async fn restore_auth_from_keychain(&self) -> bool {
        let restored = restore_auth_from_keychain(self.auth.as_ref()).await;
        let logged_in = restored.is_some();
        self.set_auth_token(restored).await;
        logged_in
    }
}
