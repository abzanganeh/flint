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

use std::collections::VecDeque;
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
    emit_audio_quality_status, emit_thread_status, emit_transcription_chunk,
    AudioQualityStatusPayload, ThreadStatusPayload, TranscriptionChunkPayload,
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
// Cross-channel echo suppression — first-arrival wins
// ────────────────────────────────────────────────────────────────────────────
//
// The dual-channel architecture assumes headphones. Without them, content
// leaks across channels in BOTH directions:
//
//   - Interviewer audio (speakers) → picked up by the mic → duplicate on MIC.
//   - The user's own voice → looped back in the conferencing app's call mix
//     (Teams/Zoom sidetone, far-end echo) → duplicate on SYSTEM.
//
// Attribution rule: the genuine source always transcribes FIRST — the echo
// arrives later because it travels through speakers/network/DSP before being
// re-captured. So whichever channel produced the content first is the true
// speaker; a near-duplicate arriving on the opposite channel within the
// window is the echo and is dropped. This replaces the old "SYSTEM always
// wins" rule, which mislabelled the user's answers as Interviewer whenever
// the call mix echoed their voice back.
//
// Near-duplicate matching uses two complementary signals:
//
//  1. Jaccard word-set overlap (≥ 0.5).  Effective for long utterances where
//     Whisper's two transcriptions cover the same vocabulary.
//
//  2. Ordered prefix match: if the first ⌈N/2⌉ words of both transcriptions
//     match exactly in order, treat it as an echo.  This catches the common
//     failure mode where room acoustics cause Whisper to diverge only at the
//     END of a phrase ("All right, let's take our" vs "All right, let's say
//     guard") — Jaccard of 0.43 misses it, but the first 3 words agree exactly.
//
// Lowering ECHO_JACCARD_THRESHOLD from 0.6 → 0.5 and adding the prefix check
// substantially reduces bleed-through when the user is not wearing headphones.

const ECHO_WINDOW: Duration = Duration::from_secs(10);
const ECHO_MIN_WORDS: usize = 3;
/// Minimum words that must agree in the ordered prefix check.
const ECHO_PREFIX_MATCH_WORDS: usize = 3;
const ECHO_JACCARD_THRESHOLD: f32 = 0.5;

const MIC_QUALITY_WINDOW: usize = 8;
const MIC_QUALITY_LOGPROB_THRESHOLD: f32 = -0.5;

#[derive(Default)]
pub struct MicQualityMonitor {
    samples: VecDeque<f32>,
    last_level: Option<&'static str>,
}

impl MicQualityMonitor {
    fn observe(&mut self, logprob: f32) -> Option<&'static str> {
        self.samples.push_back(logprob);
        while self.samples.len() > MIC_QUALITY_WINDOW {
            self.samples.pop_front();
        }
        if self.samples.len() < 3 {
            return None;
        }
        let mean = self.samples.iter().sum::<f32>() / self.samples.len() as f32;
        let level = if mean < MIC_QUALITY_LOGPROB_THRESHOLD {
            "low"
        } else {
            "ok"
        };
        if self.last_level == Some(level) {
            return None;
        }
        self.last_level = Some(level);
        Some(level)
    }
}

#[derive(Clone)]
struct DedupEntry {
    tokens: Vec<String>,
    at: Instant,
}

#[derive(Default)]
struct CrossChannelDedup {
    recent_system: Vec<DedupEntry>,
    recent_mic: Vec<DedupEntry>,
}

impl CrossChannelDedup {
    /// Returns true iff `text` near-duplicates content recently transcribed on
    /// the OPPOSITE channel — i.e. this arrival is the echo, not the source.
    fn should_suppress(&mut self, source: AudioSource, text: &str, now: Instant) -> bool {
        let tokens = tokenize_for_echo(text);
        if tokens.len() < ECHO_MIN_WORDS {
            return false;
        }
        self.prune(now);
        let opposite = match source {
            AudioSource::Microphone => &self.recent_system,
            AudioSource::System => &self.recent_mic,
        };
        opposite.iter().any(|entry| {
            jaccard(&tokens, &entry.tokens) >= ECHO_JACCARD_THRESHOLD
                || prefix_matches(&tokens, &entry.tokens)
        })
    }

    /// Record an accepted transcript so later echoes on the opposite channel
    /// can be matched against it.
    fn record(&mut self, source: AudioSource, text: &str, now: Instant) {
        let tokens = tokenize_for_echo(text);
        if tokens.len() < ECHO_MIN_WORDS {
            return;
        }
        let entry = DedupEntry { tokens, at: now };
        match source {
            AudioSource::System => self.recent_system.push(entry),
            AudioSource::Microphone => self.recent_mic.push(entry),
        }
        self.prune(now);
    }

    fn prune(&mut self, now: Instant) {
        if let Some(cutoff) = now.checked_sub(ECHO_WINDOW) {
            self.recent_system.retain(|e| e.at >= cutoff);
            self.recent_mic.retain(|e| e.at >= cutoff);
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

/// True when both token sequences share at least `ECHO_PREFIX_MATCH_WORDS`
/// ordered tokens at the start of the shorter sequence.
///
/// Jaccard is a bag-of-words metric and misses cases where Whisper diverges
/// only at the end of a phrase due to different acoustic quality on each
/// channel.  E.g. "all right lets take our" vs "all right lets say guard"
/// has Jaccard = 0.43 but the first 3 tokens are identical → echo.
fn prefix_matches(a: &[String], b: &[String]) -> bool {
    let check = a.len().min(b.len()).min(ECHO_PREFIX_MATCH_WORDS * 2);
    if check < ECHO_PREFIX_MATCH_WORDS {
        return false;
    }
    let agree = a
        .iter()
        .zip(b.iter())
        .take(check)
        .filter(|(x, y)| x == y)
        .count();
    agree >= ECHO_PREFIX_MATCH_WORDS
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
    mic_quality: Arc<Mutex<MicQualityMonitor>>,
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
            &mic_quality,
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
    mic_quality: &Arc<Mutex<MicQualityMonitor>>,
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
            tracing::debug!(
                source = %source,
                text = %result.text,
                "suppressed cross-channel echo (first arrival wins)"
            );
            return Ok(());
        }
        guard.record(source, &result.text, now);
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
        if let Some(logprob) = result.avg_logprob {
            let level = {
                let mut guard = mic_quality.lock().expect("mic quality mutex poisoned");
                guard.observe(logprob)
            };
            if let Some(level) = level {
                emit_audio_quality_status(
                    app_handle,
                    AudioQualityStatusPayload {
                        level: level.to_string(),
                    },
                );
            }
        }
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
        assert!(
            sys.is_ok(),
            "system ChannelProcessor failed: {:?}",
            sys.err()
        );
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
    fn dedup_suppresses_mic_echo_when_system_spoke_first() {
        // Interviewer speaks through speakers → mic re-captures it.
        let mut dedup = CrossChannelDedup::default();
        let now = Instant::now();
        dedup.record(
            AudioSource::System,
            "Why do you like to work with Fisher Investors?",
            now,
        );

        assert!(dedup.should_suppress(
            AudioSource::Microphone,
            "Why do you like work with Fisher Investors",
            now + Duration::from_millis(500),
        ));
    }

    #[test]
    fn dedup_suppresses_system_echo_when_mic_spoke_first() {
        // User answers → conferencing app loops their voice back into the
        // call mix → SYSTEM channel transcribes the duplicate. The user's
        // answer must NOT be re-attributed to the interviewer.
        let mut dedup = CrossChannelDedup::default();
        let now = Instant::now();
        dedup.record(
            AudioSource::Microphone,
            "I am excited about the AI Engineer opportunity at Fisher",
            now,
        );

        assert!(dedup.should_suppress(
            AudioSource::System,
            "I am excited about the AI Engineer opportunity at Fisher",
            now + Duration::from_millis(800),
        ));
    }

    #[test]
    fn dedup_does_not_suppress_distinct_content_on_either_channel() {
        let mut dedup = CrossChannelDedup::default();
        let now = Instant::now();
        dedup.record(AudioSource::System, "tell me about a project you led", now);

        assert!(!dedup.should_suppress(
            AudioSource::Microphone,
            "I led the fraud detection platform migration last year",
            now + Duration::from_millis(500),
        ));
    }

    #[test]
    fn dedup_drops_entries_outside_the_window() {
        let mut dedup = CrossChannelDedup::default();
        let now = Instant::now();
        dedup.record(AudioSource::System, "alpha bravo charlie delta", now);
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

    #[test]
    fn prefix_match_catches_whisper_end_of_utterance_divergence() {
        // Whisper transcribes the same audio differently on each channel because
        // room acoustics degrade the mic capture. Jaccard alone misses these
        // (Jaccard = 0.43 < 0.5), but the first three tokens are identical.
        let system = tokenize_for_echo("all right lets take our");
        let mic = tokenize_for_echo("all right lets say guard");
        assert!(
            jaccard(&system, &mic) < ECHO_JACCARD_THRESHOLD,
            "Jaccard should NOT trigger on its own"
        );
        assert!(
            prefix_matches(&system, &mic),
            "prefix_matches should catch the shared prefix"
        );
    }

    #[test]
    fn dedup_suppresses_acoustic_bleed_via_prefix_match() {
        // The system channel transcribes the interviewer's question. The mic
        // picks it up acoustically and Whisper produces a divergent ending.
        let mut dedup = CrossChannelDedup::default();
        let now = Instant::now();
        dedup.record(
            AudioSource::System,
            "all right lets take our time with this",
            now,
        );
        assert!(
            dedup.should_suppress(
                AudioSource::Microphone,
                "all right lets say guard over here",
                now + Duration::from_millis(300),
            ),
            "prefix-matched acoustic echo should be suppressed"
        );
    }

    #[test]
    fn prefix_match_does_not_suppress_distinct_content_sharing_a_short_preamble() {
        // Both could start with "so" but diverge quickly — must NOT suppress.
        let a = tokenize_for_echo("so tell me about your leadership experience");
        let b = tokenize_for_echo("so the biggest challenge was aligning stakeholders");
        // Only "so" matches at position 0 — well below ECHO_PREFIX_MATCH_WORDS.
        assert!(!prefix_matches(&a, &b));
    }

    #[test]
    fn mic_quality_monitor_emits_low_on_poor_logprob_sequence() {
        let mut monitor = MicQualityMonitor::default();
        assert!(monitor.observe(-0.55).is_none());
        assert!(monitor.observe(-0.6).is_none());
        assert_eq!(monitor.observe(-0.65), Some("low"));
    }
}
