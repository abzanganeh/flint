//! Transcription router — two-tier failover between an optional cloud
//! primary (Deepgram) and the always-present local Whisper.
//!
//! Structurally mirrors [`crate::llm::failover::FailoverManager`], but
//! simplified to exactly two tiers and specialised to the STT trait:
//!
//! 1. `primary = None` (Whisper only): every call goes straight to Whisper.
//!    No atomics, no background task, zero overhead — the default path.
//! 2. `primary = Some(Deepgram)`: try primary; on hard failure flip a
//!    single `AtomicBool`, emit
//!    [`crate::events::TranscriptionFailoverTriggeredPayload`], and route
//!    this and all subsequent chunks to Whisper. A background probe every
//!    30 s attempts to restore the primary and emits
//!    [`crate::events::TranscriptionPrimaryRestoredPayload`] on recovery.
//!
//! [`TranscriptionRouter`] itself implements [`TranscriptionProvider`], so
//! it drops straight into [`crate::audio::live_whisper_worker::LiveWhisperWorker::start`]
//! without any pipeline changes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tauri::{AppHandle, Runtime};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::audio::vad::VadChunk;
use crate::events::{
    emit_transcription_failover_triggered, emit_transcription_primary_restored,
    TranscriptionFailoverTriggeredPayload, TranscriptionPrimaryRestoredPayload,
};
use crate::transcription::engine::TranscriptionResult;
use crate::transcription::provider::TranscriptionProvider;

/// How often the background probe polls the failed primary. Matches the
/// LLM failover cadence so LLM and STT recovery indicators tick together.
pub const PRIMARY_PING_INTERVAL: Duration = Duration::from_secs(30);

pub struct TranscriptionRouter<R: Runtime> {
    primary: Option<Arc<dyn TranscriptionProvider>>,
    local: Arc<dyn TranscriptionProvider>,
    active_is_local: AtomicBool,
    app: AppHandle<R>,
    _ping_task: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl<R: Runtime> TranscriptionRouter<R> {
    /// Router with only local Whisper — Deepgram not configured or the user
    /// opted out. All calls go straight to Whisper; no background task.
    pub fn whisper_only(local: Arc<dyn TranscriptionProvider>, app: AppHandle<R>) -> Self {
        Self {
            primary: None,
            local,
            active_is_local: AtomicBool::new(true),
            app,
            _ping_task: std::sync::Mutex::new(None),
        }
    }

    /// Router with a cloud primary and local fallback.
    ///
    /// After construction call [`Self::start_ping_loop`] once so the router
    /// can flip back to `primary` after a transient outage.
    pub fn with_primary(
        primary: Arc<dyn TranscriptionProvider>,
        local: Arc<dyn TranscriptionProvider>,
        app: AppHandle<R>,
    ) -> Self {
        Self {
            primary: Some(primary),
            local,
            active_is_local: AtomicBool::new(false),
            app,
            _ping_task: std::sync::Mutex::new(None),
        }
    }

    /// True when the router is currently routing to local (either because
    /// no primary was configured, or because primary failed and we haven't
    /// yet detected recovery).
    pub fn is_using_local(&self) -> bool {
        self.active_is_local.load(Ordering::Acquire)
    }

    /// Name of the provider currently serving requests — used for
    /// observability, not decision-making.
    pub fn active_provider_name(&self) -> &str {
        if self.is_using_local() {
            self.local.name()
        } else {
            self.primary
                .as_ref()
                .map(|p| p.name())
                .unwrap_or_else(|| self.local.name())
        }
    }

    /// Spawn the background probe loop. No-op when no primary is configured.
    pub fn start_ping_loop(self: &Arc<Self>) {
        if self.primary.is_none() {
            return;
        }
        let this = Arc::clone(self);
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(PRIMARY_PING_INTERVAL).await;
                this.probe_primary_once().await;
            }
        });
        if let Ok(mut slot) = self._ping_task.lock() {
            *slot = Some(handle);
        }
    }

    async fn probe_primary_once(&self) {
        if !self.active_is_local.load(Ordering::Acquire) {
            // Already on primary — nothing to restore.
            return;
        }
        let Some(primary) = self.primary.as_ref() else {
            return;
        };
        if primary.health_check().await {
            self.active_is_local.store(false, Ordering::Release);
            info!(provider = %primary.name(), "transcription primary restored");
            emit_transcription_primary_restored(
                &self.app,
                TranscriptionPrimaryRestoredPayload {
                    provider: primary.name().to_string(),
                },
            );
        }
    }

    fn trigger_failover(&self, primary_name: &str) {
        // Flip once — concurrent chunks racing here just emit the event
        // multiple times worst case, but the atomic guarantees only the
        // first winner emits.
        let was_primary = self
            .active_is_local
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if was_primary {
            warn!(
                from = %primary_name,
                to = %self.local.name(),
                "transcription primary failed — routing subsequent chunks to local"
            );
            emit_transcription_failover_triggered(
                &self.app,
                TranscriptionFailoverTriggeredPayload {
                    from: primary_name.to_string(),
                    to: self.local.name().to_string(),
                },
            );
        }
    }
}

#[async_trait]
impl<R: Runtime> TranscriptionProvider for TranscriptionRouter<R> {
    async fn transcribe(
        &self,
        chunk: VadChunk,
        rolling_context: String,
    ) -> Result<Option<TranscriptionResult>> {
        // No primary configured => straight to local, zero overhead.
        let Some(primary) = self.primary.as_ref() else {
            return self.local.transcribe(chunk, rolling_context).await;
        };

        // Already fell back this session — stay on local until the ping
        // loop restores primary.
        if self.active_is_local.load(Ordering::Acquire) {
            return self.local.transcribe(chunk, rolling_context).await;
        }

        // Clone samples up front so a failed primary attempt still has an
        // owned VadChunk available for the local fallback. VadChunk's
        // `samples: Vec<f32>` is the only non-Copy field of substance.
        let fallback_chunk = VadChunk {
            samples: chunk.samples.clone(),
            source: chunk.source,
            duration_ms: chunk.duration_ms,
        };
        let fallback_context = rolling_context.clone();

        match primary.transcribe(chunk, rolling_context).await {
            Ok(result) => Ok(result),
            Err(e) => {
                let primary_name = primary.name().to_string();
                warn!(
                    provider = %primary_name,
                    error = %e,
                    "primary transcription failed — falling back to local for this chunk"
                );
                self.trigger_failover(&primary_name);
                self.local
                    .transcribe(fallback_chunk, fallback_context)
                    .await
            }
        }
    }

    fn name(&self) -> &str {
        self.active_provider_name()
    }

    fn is_available(&self) -> bool {
        self.local.is_available()
            || self
                .primary
                .as_ref()
                .map(|p| p.is_available())
                .unwrap_or(false)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::capture::AudioSource;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};

    fn mock_app_handle() -> AppHandle<MockRuntime> {
        mock_builder()
            .build(mock_context(noop_assets()))
            .expect("mock app")
            .handle()
            .clone()
    }

    fn silent_chunk() -> VadChunk {
        VadChunk {
            samples: vec![0.0; 16_000],
            source: AudioSource::System,
            duration_ms: 1_000,
        }
    }

    struct FixedProvider {
        name_: &'static str,
        available: bool,
        calls: Arc<AtomicUsize>,
        text: String,
    }

    #[async_trait]
    impl TranscriptionProvider for FixedProvider {
        async fn transcribe(
            &self,
            chunk: VadChunk,
            _rolling_context: String,
        ) -> Result<Option<TranscriptionResult>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Some(TranscriptionResult {
                text: self.text.clone(),
                source: chunk.source,
                word_timestamps: Vec::new(),
                avg_logprob: None,
            }))
        }
        fn name(&self) -> &str {
            self.name_
        }
        fn is_available(&self) -> bool {
            self.available
        }
    }

    struct AlwaysFails {
        name_: &'static str,
        calls: Arc<AtomicUsize>,
        healthy: AtomicBool,
    }

    #[async_trait]
    impl TranscriptionProvider for AlwaysFails {
        async fn transcribe(
            &self,
            _chunk: VadChunk,
            _rolling_context: String,
        ) -> Result<Option<TranscriptionResult>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(anyhow::anyhow!("primary boom"))
        }
        fn name(&self) -> &str {
            self.name_
        }
        fn is_available(&self) -> bool {
            true
        }
        async fn health_check(&self) -> bool {
            self.healthy.load(Ordering::Acquire)
        }
    }

    #[tokio::test]
    async fn whisper_only_bypasses_primary_logic_entirely() {
        let calls = Arc::new(AtomicUsize::new(0));
        let local: Arc<dyn TranscriptionProvider> = Arc::new(FixedProvider {
            name_: "whisper",
            available: true,
            calls: Arc::clone(&calls),
            text: "local".to_string(),
        });
        let router = TranscriptionRouter::whisper_only(local, mock_app_handle());

        let out = router
            .transcribe(silent_chunk(), String::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out.text, "local");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(router.is_using_local());
        assert_eq!(router.active_provider_name(), "whisper");
    }

    #[tokio::test]
    async fn primary_success_stays_on_primary() {
        let p_calls = Arc::new(AtomicUsize::new(0));
        let l_calls = Arc::new(AtomicUsize::new(0));
        let primary: Arc<dyn TranscriptionProvider> = Arc::new(FixedProvider {
            name_: "deepgram",
            available: true,
            calls: Arc::clone(&p_calls),
            text: "cloud".to_string(),
        });
        let local: Arc<dyn TranscriptionProvider> = Arc::new(FixedProvider {
            name_: "whisper",
            available: true,
            calls: Arc::clone(&l_calls),
            text: "local".to_string(),
        });
        let router = TranscriptionRouter::with_primary(primary, local, mock_app_handle());

        let out = router
            .transcribe(silent_chunk(), String::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out.text, "cloud");
        assert_eq!(p_calls.load(Ordering::SeqCst), 1);
        assert_eq!(l_calls.load(Ordering::SeqCst), 0);
        assert!(!router.is_using_local());
    }

    #[tokio::test]
    async fn primary_hard_failure_falls_back_and_serves_from_local() {
        let p_calls = Arc::new(AtomicUsize::new(0));
        let l_calls = Arc::new(AtomicUsize::new(0));
        let primary: Arc<dyn TranscriptionProvider> = Arc::new(AlwaysFails {
            name_: "deepgram",
            calls: Arc::clone(&p_calls),
            healthy: AtomicBool::new(false),
        });
        let local: Arc<dyn TranscriptionProvider> = Arc::new(FixedProvider {
            name_: "whisper",
            available: true,
            calls: Arc::clone(&l_calls),
            text: "local".to_string(),
        });
        let router = TranscriptionRouter::with_primary(primary, local, mock_app_handle());

        let out = router
            .transcribe(silent_chunk(), String::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out.text, "local");
        assert!(router.is_using_local(), "flip must persist for the session");
        assert_eq!(p_calls.load(Ordering::SeqCst), 1);
        assert_eq!(l_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn subsequent_chunks_short_circuit_to_local_after_failover() {
        let p_calls = Arc::new(AtomicUsize::new(0));
        let l_calls = Arc::new(AtomicUsize::new(0));
        let primary: Arc<dyn TranscriptionProvider> = Arc::new(AlwaysFails {
            name_: "deepgram",
            calls: Arc::clone(&p_calls),
            healthy: AtomicBool::new(false),
        });
        let local: Arc<dyn TranscriptionProvider> = Arc::new(FixedProvider {
            name_: "whisper",
            available: true,
            calls: Arc::clone(&l_calls),
            text: "local".to_string(),
        });
        let router = TranscriptionRouter::with_primary(primary, local, mock_app_handle());

        for _ in 0..5 {
            let _ = router.transcribe(silent_chunk(), String::new()).await;
        }
        // Primary is called exactly once — the initial call that fails and
        // flips the atomic. Subsequent chunks skip it entirely.
        assert_eq!(p_calls.load(Ordering::SeqCst), 1);
        assert_eq!(l_calls.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn probe_primary_once_restores_when_health_check_passes() {
        let p_calls = Arc::new(AtomicUsize::new(0));
        let l_calls = Arc::new(AtomicUsize::new(0));
        let primary = Arc::new(AlwaysFails {
            name_: "deepgram",
            calls: Arc::clone(&p_calls),
            healthy: AtomicBool::new(false),
        });
        let primary_probe = Arc::clone(&primary);
        let primary_dyn: Arc<dyn TranscriptionProvider> = primary;
        let local: Arc<dyn TranscriptionProvider> = Arc::new(FixedProvider {
            name_: "whisper",
            available: true,
            calls: Arc::clone(&l_calls),
            text: "local".to_string(),
        });
        let router = TranscriptionRouter::with_primary(primary_dyn, local, mock_app_handle());

        // Force a failover.
        let _ = router.transcribe(silent_chunk(), String::new()).await;
        assert!(router.is_using_local());

        // Primary "comes back" — probe should flip us back to primary.
        primary_probe.healthy.store(true, Ordering::Release);
        router.probe_primary_once().await;
        assert!(!router.is_using_local(), "probe must restore active tier");
    }

    #[tokio::test]
    async fn probe_primary_once_noop_when_not_in_fallback() {
        let p_calls = Arc::new(AtomicUsize::new(0));
        let primary_ok: Arc<dyn TranscriptionProvider> = Arc::new(FixedProvider {
            name_: "deepgram",
            available: true,
            calls: Arc::clone(&p_calls),
            text: "cloud".to_string(),
        });
        let local: Arc<dyn TranscriptionProvider> = Arc::new(FixedProvider {
            name_: "whisper",
            available: true,
            calls: Arc::new(AtomicUsize::new(0)),
            text: "local".to_string(),
        });
        let router = TranscriptionRouter::with_primary(primary_ok, local, mock_app_handle());
        assert!(!router.is_using_local());
        router.probe_primary_once().await;
        // Still on primary, no state change, no primary calls made by the probe.
        assert!(!router.is_using_local());
    }

    #[tokio::test]
    async fn active_provider_name_reflects_current_tier() {
        let primary: Arc<dyn TranscriptionProvider> = Arc::new(AlwaysFails {
            name_: "deepgram",
            calls: Arc::new(AtomicUsize::new(0)),
            healthy: AtomicBool::new(false),
        });
        let local: Arc<dyn TranscriptionProvider> = Arc::new(FixedProvider {
            name_: "whisper",
            available: true,
            calls: Arc::new(AtomicUsize::new(0)),
            text: "local".to_string(),
        });
        let router = TranscriptionRouter::with_primary(primary, local, mock_app_handle());
        assert_eq!(router.active_provider_name(), "deepgram");
        let _ = router.transcribe(silent_chunk(), String::new()).await;
        assert_eq!(router.active_provider_name(), "whisper");
    }
}
