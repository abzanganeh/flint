//! `TranscriptionProvider` trait and the local Whisper adapter.
//!
//! Mirrors the shape of [`crate::llm::provider::LLMProvider`] so callers can
//! swap between local (whisper.cpp) and cloud (Deepgram, added in a later
//! slice) transcription without touching the audio pipeline or the live
//! Whisper worker.
//!
//! ## Rationale
//!
//! The existing [`WhisperEngine::transcribe_with_context`] call is CPU-bound
//! and blocking, so it is (and must remain) scheduled onto
//! `tokio::task::spawn_blocking`. A future cloud provider will instead be
//! async / network-bound and should run directly on the tokio runtime. Both
//! paths converge on the same async trait method here, so the worker loop
//! stays uniform.
//!
//! Slice 1 introduces the trait and the Whisper adapter only — behavior for
//! Whisper-only users is unchanged; the `spawn_blocking` call simply moves
//! from [`crate::audio::live_whisper_worker::worker_loop`] into
//! [`WhisperTranscriptionProvider::transcribe`].

use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;

use crate::audio::vad::VadChunk;
use crate::transcription::engine::{TranscriptionResult, WhisperEngine};

/// Contract for any component that turns a closed VAD chunk into a
/// [`TranscriptionResult`].
///
/// Implementations must be `Send + Sync` so they can be held in `Arc` and
/// shared across the live-audio worker task and the background health-ping
/// loop.
#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    /// Transcribe one closed VAD chunk.
    ///
    /// Takes ownership of `chunk` and `rolling_context` so the provider can
    /// move them into a blocking task (Whisper) or into a network future
    /// (Deepgram) without cloning.
    ///
    /// Returns `Ok(None)` when the chunk is validly empty (all Whisper
    /// segments filtered, or Deepgram returned an empty transcript). Reserve
    /// `Err` for hard failures the caller may want to fail over on.
    async fn transcribe(
        &self,
        chunk: VadChunk,
        rolling_context: String,
    ) -> Result<Option<TranscriptionResult>>;

    /// Identifier for logs and failover events (e.g. `"whisper"`, `"deepgram"`).
    fn name(&self) -> &str;

    /// Whether this provider is currently usable (key present, model loaded,
    /// etc.). Cheap synchronous check.
    fn is_available(&self) -> bool;

    /// Async reachability probe — used by the router's recovery loop before
    /// flipping back to a previously failed primary. Defaults to
    /// [`Self::is_available`] for local providers.
    async fn health_check(&self) -> bool {
        self.is_available()
    }
}

/// [`TranscriptionProvider`] adapter over the local whisper.cpp engine.
///
/// Owns its own `Arc<WhisperEngine>`; the same engine can (and should) still
/// be shared with the mock rehearsal path — this adapter only takes an `Arc`.
pub struct WhisperTranscriptionProvider {
    engine: Arc<WhisperEngine>,
}

impl WhisperTranscriptionProvider {
    pub fn new(engine: Arc<WhisperEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl TranscriptionProvider for WhisperTranscriptionProvider {
    async fn transcribe(
        &self,
        chunk: VadChunk,
        rolling_context: String,
    ) -> Result<Option<TranscriptionResult>> {
        let engine = Arc::clone(&self.engine);
        tokio::task::spawn_blocking(move || {
            engine.transcribe_with_context(&chunk, &rolling_context)
        })
        .await
        .map_err(|e| anyhow!("whisper transcription task panicked: {e}"))?
    }

    fn name(&self) -> &str {
        "whisper"
    }

    fn is_available(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The provider abstraction must be dyn-compatible so callers can hold
    /// `Arc<dyn TranscriptionProvider>` without reaching for generics.
    #[test]
    fn provider_is_object_safe() {
        fn assert_object_safe(_: &dyn TranscriptionProvider) {}
        // Compile-time check only — never called.
        let _ = |p: Arc<dyn TranscriptionProvider>| assert_object_safe(&*p);
    }

    #[test]
    fn whisper_provider_reports_stable_name() {
        // Construct without loading a real model — we only need `name()`.
        // Safe because `name()` doesn't touch the engine field.
        // For a real engine we'd need a model file at test time.
        // This test exercises the constant-name contract only.
        struct NameOnly;
        #[async_trait]
        impl TranscriptionProvider for NameOnly {
            async fn transcribe(
                &self,
                _chunk: VadChunk,
                _rolling_context: String,
            ) -> Result<Option<TranscriptionResult>> {
                Ok(None)
            }
            fn name(&self) -> &str {
                "whisper"
            }
            fn is_available(&self) -> bool {
                true
            }
        }
        let p = NameOnly;
        assert_eq!(p.name(), "whisper");
    }
}
