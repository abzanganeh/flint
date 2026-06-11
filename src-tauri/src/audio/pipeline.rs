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

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
// Cross-channel echo suppression
// ────────────────────────────────────────────────────────────────────────────
//
// The dual-channel architecture assumes headphones. When the user has speakers
// on, the microphone re-captures system audio (YouTube, the interviewer's
// voice, etc.) and both channels transcribe the same content with slight
// per-pass variation. We compare normalised word sets via Jaccard overlap
// and, when the same content is detected on both channels within a sliding
// window, always drop the MIC copy in favour of SYSTEM (cleaner signal).
//
// The window is wider than fix 1's old 4 s because Whisper inference on a
// large chunk can lag the original audio by several seconds.

const ECHO_WINDOW: Duration = Duration::from_secs(10);
const ECHO_MIN_WORDS: usize = 3;
const ECHO_JACCARD_THRESHOLD: f32 = 0.6;

#[derive(Clone)]
struct DedupEntry {
    tokens: Vec<String>,
    at: Instant,
}

#[derive(Default)]
struct CrossChannelDedup {
    // Only SYSTEM entries are stored — MIC is always the suppressed side.
    recent_system: Vec<DedupEntry>,
}

impl CrossChannelDedup {
    /// Returns true iff `source` is MIC and its text overlaps a recent SYSTEM entry.
    fn should_suppress(&mut self, source: AudioSource, text: &str, now: Instant) -> bool {
        // Only MIC is ever suppressed — SYSTEM is the authoritative channel.
        if source != AudioSource::Microphone {
            return false;
        }
        let tokens = tokenize_for_echo(text);
        if tokens.len() < ECHO_MIN_WORDS {
            return false;
        }
        self.prune(now);
        self.recent_system
            .iter()
            .any(|entry| jaccard(&tokens, &entry.tokens) >= ECHO_JACCARD_THRESHOLD)
    }

    /// Record a SYSTEM transcript so future MIC transcripts can be checked against it.
    fn record_system(&mut self, text: &str, now: Instant) {
        let tokens = tokenize_for_echo(text);
        if tokens.len() < ECHO_MIN_WORDS {
            return;
        }
        self.recent_system.push(DedupEntry { tokens, at: now });
        self.prune(now);
    }

    fn prune(&mut self, now: Instant) {
        if let Some(cutoff) = now.checked_sub(ECHO_WINDOW) {
            self.recent_system.retain(|e| e.at >= cutoff);
        }
    }
}

fn tokenize_for_echo(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn jaccard(a: &[String], b: &[String]) -> f32 {
    use std::collections::HashSet;
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let set_a: HashSet<&String> = a.iter().collect();
    let set_b: HashSet<&String> = b.iter().collect();
    let intersection = set_a.intersection(&set_b).count() as f32;
    let union = set_a.union(&set_b).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        intersection / union
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Per-channel processing state
// ────────────────────────────────────────────────────────────────────────────

/// All stateful processors that run sequentially on one audio channel.
///
/// `rnnoise` is `None` for the SYSTEM channel — the digital loopback signal
/// should not be run through the speech-trained denoiser.
struct ChannelProcessor {
    rnnoise: Option<RNNoiseProcessor>,
    downsampler: Downsampler,
    vad: VadChunker,
}

impl ChannelProcessor {
    fn new_mic() -> Result<Self> {
        Ok(Self {
            rnnoise: Some(RNNoiseProcessor::new()?),
            downsampler: Downsampler::new()?,
            vad: VadChunker::new()?,
        })
    }

    fn new_system() -> Result<Self> {
        Ok(Self {
            rnnoise: None,
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
    let mut sys_proc = ChannelProcessor::new_system()?;
    let mut mic_proc = ChannelProcessor::new_mic()?;
    let dedup = Mutex::new(CrossChannelDedup::default());

    loop {
        // No `biased` — fair scheduling prevents MIC starvation under heavy
        // SYSTEM load (e.g. continuous YouTube audio).
        let frame = tokio::select! {
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
            &dedup,
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
    dedup: &Mutex<CrossChannelDedup>,
) -> Result<()> {
    let source = frame.source;

    // ── Step 1: RNNoise ───────────────────────────────────────────────────
    //
    // Only the MIC channel has RNNoise allocated (ChannelProcessor::new_mic).
    // SYSTEM is a clean digital loopback — running it through the speech-
    // trained denoiser damages non-speech spectra (music, multi-speaker
    // dialogue) and degrades Whisper accuracy on that channel.
    if let Some(rnn) = &mut proc.rnnoise {
        rnn.process_frame(&mut frame.samples)?;
    }

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

    let now = Instant::now();
    {
        let mut guard = match dedup.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.should_suppress(source, &result.text, now) {
            tracing::debug!(text = %result.text, "suppressed mic echo of system audio");
            return Ok(());
        }
        // Record SYSTEM transcripts so subsequent MIC chunks can be compared.
        if source == AudioSource::System {
            guard.record_system(&result.text, now);
        }
    }

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
        let mic = ChannelProcessor::new_mic();
        assert!(mic.is_ok(), "mic ChannelProcessor failed: {:?}", mic.err());
        assert!(mic.unwrap().rnnoise.is_some());

        let sys = ChannelProcessor::new_system();
        assert!(sys.is_ok(), "system ChannelProcessor failed: {:?}", sys.err());
        assert!(sys.unwrap().rnnoise.is_none());
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
    fn jaccard_word_overlap_matches_near_duplicate_transcripts() {
        let a = tokenize_for_echo("Why do you like to work with Fisher Investors?");
        let b = tokenize_for_echo("Why do you like work with Fisher Investors");
        assert!(jaccard(&a, &b) >= ECHO_JACCARD_THRESHOLD);

        let c = tokenize_for_echo("Tell me about a time you led a team");
        assert!(jaccard(&a, &c) < ECHO_JACCARD_THRESHOLD);
    }

    #[test]
    fn dedup_suppresses_only_mic_when_echoing_system() {
        let mut dedup = CrossChannelDedup::default();
        let now = Instant::now();
        dedup.record_system("Why do you like to work with Fisher Investors?", now);

        assert!(dedup.should_suppress(
            AudioSource::Microphone,
            "Why do you like work with Fisher Investors",
            now + Duration::from_millis(500),
        ));

        assert!(!dedup.should_suppress(
            AudioSource::System,
            "Why do you like work with Fisher Investors",
            now + Duration::from_millis(500),
        ));
    }

    #[test]
    fn dedup_drops_entries_outside_the_window() {
        let mut dedup = CrossChannelDedup::default();
        let now = Instant::now();
        dedup.record_system("alpha bravo charlie delta", now);
        let later = now + ECHO_WINDOW + Duration::from_secs(1);
        assert!(!dedup.should_suppress(
            AudioSource::Microphone,
            "alpha bravo charlie delta",
            later,
        ));
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
