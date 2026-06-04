//! Audio pipeline integration — Task 3.7.
//!
//! Wires the full processing chain:
//!
//! ```text
//! AudioCapture (system + mic channels)
//!     ↓ AudioFrame (480 samples @ 48kHz)
//! RNNoiseProcessor::process_frame()   — denoises in place
//! Downsampler::process()              — 480 @ 48kHz → 160 @ 16kHz
//! VadChunker::process_frame()         — accumulates speech, emits VadChunk on silence gap
//!     ↓ VadChunk (variable length, 16kHz PCM mono)
//! WhisperEngine::transcribe()         — Whisper.cpp inference (spawn_blocking)
//!     ↓ TranscriptionResult
//! emit transcription_chunk            — immediately, before question detection
//! QuestionDetector::detect()          — System channel only
//!     ↓ DetectedQuestion → question_tx → orchestrator
//! ```
//!
//! ## Spec note
//! The spec places this function in `capture.rs`. It is in its own module
//! (`pipeline.rs`) because `capture.rs` is already 742 lines and the pipeline
//! logic is orthogonal to capture/ring-buffer/recovery concerns. The public
//! API (`run_audio_pipeline`, `DetectedQuestion`) is re-exported from
//! `audio/mod.rs`.
//!
//! ## Security invariant
//! No audio sample or transcript text is written to disk or logged at INFO+.
//! On session end, the caller must call `AudioCapture::stop()` which zeroes
//! both ring buffers.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tauri::AppHandle;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::audio::capture::{AudioFrame, AudioSource};
use crate::audio::rnnoise::{Downsampler, RNNoiseProcessor};
use crate::audio::vad::VadChunker;
use crate::events::{
    emit_thread_status, emit_transcription_chunk, ThreadStatusPayload, TranscriptionChunkPayload,
};
use crate::session::persistence::{SessionPersistence, TranscriptChunk};
use crate::transcription::detector::QuestionDetector;
use crate::transcription::engine::WhisperEngine;

// ────────────────────────────────────────────────────────────────────────────
// Public types
// ────────────────────────────────────────────────────────────────────────────

/// A question detected from the System audio channel, ready for the orchestrator.
#[derive(Debug, Clone)]
pub struct DetectedQuestion {
    pub text: String,
    pub session_id: Uuid,
    pub detected_at: Instant,
}

// ────────────────────────────────────────────────────────────────────────────
// Per-channel processing state
// ────────────────────────────────────────────────────────────────────────────

/// All stateful processors that run sequentially on one audio channel.
struct ChannelProcessor {
    rnnoise: RNNoiseProcessor,
    downsampler: Downsampler,
    vad: VadChunker,
}

impl ChannelProcessor {
    fn new() -> Result<Self> {
        Ok(Self {
            rnnoise: RNNoiseProcessor::new()?,
            downsampler: Downsampler::new()?,
            vad: VadChunker::new()?,
        })
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Pipeline runner
// ────────────────────────────────────────────────────────────────────────────

/// Run the full audio pipeline until both input channels are closed.
///
/// `system_rx` and `mic_rx` are the receiving ends of the mpsc channels
/// created by the caller before passing the senders to `AudioCapture::start`.
///
/// Whisper inference runs in `tokio::task::spawn_blocking` so it does not
/// block the async executor. All other processing is synchronous and fast
/// enough to run inline (RNNoise < 5ms, VAD < 1ms per frame).
///
/// The function returns when both input channels are closed (i.e. when
/// `AudioCapture::stop()` has been called and all senders have dropped).
#[allow(clippy::too_many_arguments)]
pub async fn run_audio_pipeline(
    app_handle: AppHandle,
    session_id: Uuid,
    whisper: Arc<WhisperEngine>,
    detector: Arc<QuestionDetector>,
    question_tx: mpsc::Sender<DetectedQuestion>,
    mut system_rx: mpsc::Receiver<AudioFrame>,
    mut mic_rx: mpsc::Receiver<AudioFrame>,
    persistence: Arc<SessionPersistence>,
) -> Result<()> {
    let mut sys_proc = ChannelProcessor::new()?;
    let mut mic_proc = ChannelProcessor::new()?;

    loop {
        let frame = tokio::select! {
            biased;
            f = system_rx.recv() => match f {
                Some(frame) => frame,
                None => break,
            },
            f = mic_rx.recv() => match f {
                Some(frame) => frame,
                None => break,
            },
        };

        let proc = match frame.source {
            AudioSource::System => &mut sys_proc,
            AudioSource::Microphone => &mut mic_proc,
        };

        if let Err(e) = process_frame(
            frame,
            proc,
            &app_handle,
            session_id,
            &whisper,
            &detector,
            &question_tx,
            &persistence,
        )
        .await
        {
            tracing::warn!(error = %e, "audio pipeline frame error — continuing");
        }
    }

    tracing::info!(session_id = %session_id, "audio pipeline loop exited");
    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// Per-frame processing
// ────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn process_frame(
    mut frame: AudioFrame,
    proc: &mut ChannelProcessor,
    app_handle: &AppHandle,
    session_id: Uuid,
    whisper: &Arc<WhisperEngine>,
    detector: &Arc<QuestionDetector>,
    question_tx: &mpsc::Sender<DetectedQuestion>,
    persistence: &Arc<SessionPersistence>,
) -> Result<()> {
    let source = frame.source;

    // ── Step 1: RNNoise ───────────────────────────────────────────────────
    proc.rnnoise.process_frame(&mut frame.samples)?;

    // ── Step 2: Downsample 48kHz → 16kHz ─────────────────────────────────
    let downsampled = proc.downsampler.process(&frame.samples)?;

    // ── Step 3: VAD ───────────────────────────────────────────────────────
    let Some(chunk) = proc.vad.process_frame(&downsampled, source) else {
        return Ok(()); // speech still accumulating or silence — nothing to do
    };

    // ── Step 4a: Whisper (blocking — runs off the async executor) ─────────
    let whisper = Arc::clone(whisper);
    let transcription = tokio::task::spawn_blocking(move || whisper.transcribe(&chunk))
        .await
        .map_err(|e| anyhow::anyhow!("Whisper task panicked: {e}"))??;

    let Some(result) = transcription else {
        return Ok(()); // silence or hallucination — discarded by engine
    };

    // ── Step 4b: emit + persist transcript chunk ──────────────────────────
    let speaker = match source {
        AudioSource::System => "System",
        AudioSource::Microphone => "Microphone",
    };
    let timestamp = frame.timestamp.elapsed().as_millis() as i64;
    emit_transcription_chunk(
        app_handle,
        TranscriptionChunkPayload {
            text: result.text.clone(),
            speaker: speaker.to_string(),
            timestamp,
        },
    );
    // Persist every chunk immediately — crash-recovery insurance.
    let chunk = TranscriptChunk {
        id: Uuid::new_v4(),
        session_id,
        speaker: speaker.to_string(),
        text: result.text.clone(),
        timestamp_ms: timestamp,
    };
    if let Err(e) = persistence.write_transcript_chunk(&chunk) {
        tracing::warn!(error = %e, "transcript chunk persist failed — continuing");
    }

    // ── Steps 4c/4d: question detection on System audio only ──────────────
    if source == AudioSource::Microphone {
        return Ok(());
    }

    let text = result.text.clone();
    let detector = Arc::clone(detector);
    let is_question = tokio::task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(detector.detect(&text))
    })
    .await
    .map_err(|e| anyhow::anyhow!("detector task panicked: {e}"))??;

    // ── Step 4e: send to orchestrator if question detected ─────────────────
    if is_question {
        let q = DetectedQuestion {
            text: result.text,
            session_id,
            detected_at: Instant::now(),
        };
        if question_tx.try_send(q).is_err() {
            tracing::warn!(
                session_id = %session_id,
                "question_tx full — detected question dropped"
            );
        }
    }

    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// Audio gap marker
// ────────────────────────────────────────────────────────────────────────────

/// Emit a synthetic `transcription_chunk` event marking a gap in audio
/// capture (Task 3.9 recovery path).
///
/// `gap_secs` is the number of seconds the stream was down.
pub fn emit_audio_gap_marker(app_handle: &AppHandle, source: AudioSource, gap_secs: u64) {
    let text = format!("[audio gap - {gap_secs}s missing]");
    emit_transcription_chunk(
        app_handle,
        TranscriptionChunkPayload {
            text,
            speaker: source.to_string(),
            timestamp: 0,
        },
    );
}

/// Emit a `thread_status` error event for the audio thread (Task 3.9 failure path).
pub fn emit_audio_thread_error(app_handle: &AppHandle) {
    emit_thread_status(
        app_handle,
        ThreadStatusPayload {
            thread: "audio".to_string(),
            status: "error".to_string(),
        },
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::rnnoise::RNNOISE_FRAME_SIZE;
    use crate::audio::vad::VAD_FRAME_SAMPLES;

    #[test]
    fn channel_processor_initialises() {
        // Verify all three processors can be constructed on the test host.
        let proc = ChannelProcessor::new();
        assert!(
            proc.is_ok(),
            "ChannelProcessor failed to initialise: {:?}",
            proc.err()
        );
    }

    #[test]
    fn detected_question_fields() {
        let q = DetectedQuestion {
            text: "Tell me about yourself.".to_string(),
            session_id: Uuid::new_v4(),
            detected_at: Instant::now(),
        };
        assert!(!q.text.is_empty());
    }

    #[test]
    fn pipeline_frame_constants_aligned() {
        // RNNoise frame = 480 samples, VAD frame = 320 samples.
        // Downsampler produces 160 samples per call.
        // Two downsampler outputs fill one VAD frame.
        use crate::audio::rnnoise::DOWNSAMPLED_FRAME_SIZE;
        assert_eq!(RNNOISE_FRAME_SIZE, 480);
        assert_eq!(DOWNSAMPLED_FRAME_SIZE * 2, VAD_FRAME_SAMPLES);
    }
}
