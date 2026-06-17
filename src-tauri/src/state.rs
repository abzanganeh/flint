use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tauri::{App, Manager};
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};

use crate::deep_link;
use tokio::task::JoinHandle;

use crate::knowledge::{knowledge_base_dir, GlobalKnowledgeBase, PackId};
use crate::mock::conductor::{Conductor, MockMode};
use crate::mock::mic_capture::MicCapture;

use crate::audio::pipeline::DetectedQuestion;

use crate::auth_session::restore_auth_from_keychain;
use crate::cost::CostTracker;
use crate::digest::Digest;
use crate::flags::{cache_path_in, FeatureFlagClient};
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
/// Per-turn cancellation flag. Set by `cancel_inference`; checked by response
/// threads between token emissions.
pub type TurnCancelFlag = Arc<AtomicBool>;

/// Handles for a running mock interview session.
pub struct MockTaskHandles {
    /// Conductor loop handle — receives `ConductorCommand` msgs from commands.
    pub conductor: Conductor,
    /// Mic capture task — manages cpal stream and VAD+Whisper loop.
    pub mic_capture: MicCapture,
    /// Conductor's active question turn (1-based). Updated when each question starts.
    pub active_turn_n: Arc<AtomicU32>,
    /// When true, questions are gated on `ask_mock_question`.
    pub guided: bool,
    /// Practice hides suggested answer until after the user responds.
    pub mode: MockMode,
    /// Latest suggested-answer text for the active turn (shared with conductor).
    pub suggested_text: Arc<std::sync::RwLock<String>>,
    /// Knowledge packs relevant for this session's role/domain.
    pub role_packs: Vec<PackId>,
}

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
    /// Active turn's cancellation flag. Replaced on each orchestrator dispatch.
    pub turn_cancel: Arc<Mutex<Option<TurnCancelFlag>>>,
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
    pub persistence: Arc<SessionPersistence>,
    /// bge-small-en-v1.5 embedder — loaded in the background so the UI can appear
    /// before the ONNX model finishes downloading / initialising.
    embedder: Arc<StdRwLock<Option<Arc<Embedder>>>>,
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
    /// Serializes `start_session` — React StrictMode can invoke it twice in dev.
    pub live_start_lock: Mutex<()>,
    /// Shared orchestrator conversation memory. Same `Arc` as passed to
    /// `OrchestratorConfig` — not a duplicate instance.
    pub session_memory: Arc<Mutex<Option<Arc<Mutex<ConversationMemory>>>>>,
    /// Turn counter for rehearsal-mode orchestrator dispatches.
    pub rehearsal_turn: Mutex<usize>,
    /// Active rehearsal turn cancellation flag. Replaced on each
    /// `run_rehearsal_turn`; the previous flag is set before replacement.
    pub rehearsal_turn_cancel: Mutex<Option<TurnCancelFlag>>,

    // ── Mock interview (Phase 8) ─────────────────────────────────────────────
    /// Mock session handles. `Some` only while a mock interview is active.
    pub mock_tasks: Mutex<Option<MockTaskHandles>>,

    /// Static interview knowledge base — domain packs embedded once at first
    /// launch into a dedicated `flint_knowledge.db`.  Queried alongside
    /// session RAG during mock interviews.
    pub global_kb: Arc<GlobalKnowledgeBase>,

    /// Phase 7.4 — process-wide cumulative token / cost accounting. Read by
    /// the orchestrator pre-dispatch to enforce the configured cap; mutated
    /// post-turn to advance the totals and fire warning / suspension events.
    pub cost_tracker: Arc<CostTracker>,

    /// Phase 7.6 — feature flag evaluator. Loads the cached bundle on
    /// startup (kill switch) and refreshes from Supabase via
    /// `commands::refresh_feature_flags`. Reads are lock-free-ish via
    /// `RwLock` so UI panels can call `is_feature_enabled` on every render.
    pub feature_flags: Arc<FeatureFlagClient>,

    /// Stable ONNX embedding model cache (`<app_data>/fastembed_cache`).
    embedder_cache_dir: PathBuf,

    /// Pending Smart Resume import token captured at cold start before the
    /// React WebView mounted. React polls this once via `get_pending_import_token`
    /// and clears it. The warm path (second click, Flint already running) still
    /// uses the `smart_resume_import_token` event emitted by `single_instance`.
    pub pending_import_token: Mutex<Option<String>>,

    /// Panic-hide toggles `overlay_visibility` in React only — the main window
    /// stays alive so Wayland can still receive the restore chord.
    pub overlay_panic_hidden: std::sync::Mutex<bool>,
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
        std::fs::create_dir_all(&data_dir).context("Cannot create app data directory")?;

        let persistence_path = data_dir.join("flint.db");
        let vec_db_path = data_dir.join("flint_vec.db");
        let flags_cache_path = cache_path_in(&data_dir);
        let embedder_cache_dir = prepare_embedder_cache_dir(&data_dir);

        // Capture a cold-start deep-link token before the React WebView mounts.
        // React polls `get_pending_import_token` during bootstrap to pick this up.
        let pending_import_token = Mutex::new(deep_link::capture_cold_start_token());

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

        // ── Embedder slot (shared between AppState and GlobalKnowledgeBase) ───
        let embedder_slot = Arc::new(StdRwLock::new(None::<Arc<Embedder>>));

        // ── Global knowledge base (separate DB, pack-UUID keyed) ─────────────
        let kb_db_path = data_dir.join("flint_knowledge.db");
        let kb_store: Arc<dyn VectorInterface> = Arc::new(
            SqliteVecStore::new(
                kb_db_path
                    .to_str()
                    .context("Non-UTF-8 knowledge store path")?,
            )
            .context("Failed to open knowledge store DB")?,
        );
        let global_kb = Arc::new(GlobalKnowledgeBase::new(
            kb_store,
            Arc::clone(&embedder_slot),
            knowledge_base_dir(),
        ));

        // ── State machine wired to persistence ───────────────────────────────
        let persister = Arc::clone(&persistence) as Arc<dyn crate::session::state::StatePersister>;
        let state_machine = Arc::new(Mutex::new(SessionStateMachine::with_persister(persister)));

        Ok(Self {
            auth,
            session: Arc::new(StubSession),
            plugins,
            auth_token: RwLock::new(None),
            state_machine,
            session_digest: Arc::new(RwLock::new(None)),
            prewarm_cache: Arc::new(Mutex::new(PreWarmCache::new())),
            persistence,
            embedder: embedder_slot,
            vector_store,
            llm: Arc::new(StubLLMProvider),
            live_tasks: Mutex::new(None),
            live_start_lock: Mutex::new(()),
            session_memory: Arc::new(Mutex::new(None)),
            rehearsal_turn: Mutex::new(0),
            rehearsal_turn_cancel: Mutex::new(None),
            mock_tasks: Mutex::new(None),
            global_kb,
            cost_tracker: Arc::new(CostTracker::new()),
            feature_flags: Arc::new(FeatureFlagClient::load(flags_cache_path)),
            embedder_cache_dir,
            pending_import_token,
            overlay_panic_hidden: std::sync::Mutex::new(false),
        })
    }

    /// Shared embedder handle for commands and the orchestrator.
    pub fn require_embedder(&self) -> Result<Arc<Embedder>, String> {
        self.embedder
            .read()
            .map_err(|_| "Embedding model lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| {
                "Embedding model is still loading — wait a few seconds and try again.".to_string()
            })
    }

    /// Poll until the background embedder init finishes or `timeout` elapses.
    pub async fn wait_for_embedder(&self, timeout: Duration) -> Result<Arc<Embedder>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(guard) = self.embedder.read() {
                if let Some(model) = guard.clone() {
                    return Ok(model);
                }
            }
            if Instant::now() >= deadline {
                return Err(
                    "Embedding model is still loading — wait a few seconds and try again."
                        .to_string(),
                );
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Load the embedder on a background thread (first run may download the model).
    pub fn spawn_embedder_init(&self) {
        let slot = Arc::clone(&self.embedder);
        let cache_dir = self.embedder_cache_dir.clone();
        std::thread::Builder::new()
            .name("embedder-init".into())
            .spawn(move || {
                tracing::info!(
                    cache_dir = %cache_dir.display(),
                    "embedder init started"
                );
                match Embedder::new_in_cache(&cache_dir) {
                    Ok(model) => {
                        *slot.write().expect("embedder lock poisoned") = Some(Arc::new(model));
                        tracing::info!("embedder init complete");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "embedder init failed");
                    }
                }
            })
            .expect("spawn embedder-init thread");
    }

    /// Trigger background loading of all knowledge packs.
    ///
    /// Must be called after `spawn_embedder_init` — the loader waits internally
    /// for the embedder slot to become populated before embedding begins.
    ///
    /// Uses `tauri::async_runtime::spawn` (callable from any thread) rather than
    /// `tokio::spawn` (requires an active Tokio context) because this is invoked
    /// from the synchronous Tauri setup callback.
    pub fn spawn_knowledge_init(&self) {
        let kb = Arc::clone(&self.global_kb);
        tauri::async_runtime::spawn(async move {
            kb.spawn_background_load();
        });
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

/// Ensure `<app_data>/fastembed_cache` exists and migrate a dev-build cache
/// from `src-tauri/.fastembed_cache` when present.
fn prepare_embedder_cache_dir(data_dir: &Path) -> PathBuf {
    let cache_dir = data_dir.join("fastembed_cache");
    let _ = std::fs::create_dir_all(&cache_dir);

    let target_model = cache_dir.join("models--Xenova--bge-small-en-v1.5");
    if !target_model.exists() {
        let legacy_model = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".fastembed_cache/models--Xenova--bge-small-en-v1.5");
        if legacy_model.exists() {
            tracing::info!(
                from = %legacy_model.display(),
                to = %target_model.display(),
                "migrating embedder model cache into app data directory"
            );
            if std::fs::rename(&legacy_model, &target_model).is_err() {
                let _ = copy_dir_recursive(&legacy_model, &target_model);
            }
        }
    }

    cache_dir
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
