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
//! WhisperEngine::transcribe_with_context() — rolling ~40-word prior per channel
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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as SyncMutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use tauri::AppHandle;
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use uuid::Uuid;

use crate::audio::capture::{AudioFrame, AudioSource};
use crate::audio::live_whisper_worker::{
    LiveWhisperJob, LiveWhisperJobMeta, LiveWhisperOutcome, LiveWhisperWorker,
};
use crate::audio::rnnoise::{Downsampler, RNNoiseProcessor};
use crate::audio::vad::{VadChunker, WHISPER_MIN_SEGMENT_MS};
use crate::events::{
    emit_audio_quality_status, emit_audio_routing_warning, emit_thread_status,
    emit_transcription_chunk, AudioQualityStatusPayload, AudioRoutingWarningPayload,
    ThreadStatusPayload, TranscriptionChunkPayload,
};

use crate::audio::audit::{AudioAuditCounters, SuppressionReason};
use crate::audio::diarizer::{DiarizerManager, SpeakerRole};
use crate::audio::speaker_classifier::{ClassificationRequest, SpeakerClassifier};
use crate::audio::speaker_heuristic::{rms_dbfs, PhoneHeuristicState};
use crate::session::persistence::{SessionPersistence, TranscriptChunk};
use crate::transcription::engine::WhisperEngine;
use crate::transcription::hybrid::{
    finalize_confirmation, ConfirmPlan, HybridQuestionDetector, SystemTranscriptBuffer,
};
use crate::transcription::rolling_context::ChannelRollingContexts;
use crate::transcription::sanitizer::sanitize_live_transcript;
use crate::transcription::speaker_suspicion::{self, NearDuplicateTracker, SuspicionReason};

/// Default `label_source` value applied to every chunk emitted from this
/// pipeline. The suspicion detector and the manual `relabel_transcript_chunk`
/// command upgrade this to `"heuristic"` or `"user"` respectively.
const LABEL_SOURCE_CHANNEL: &str = "channel";

/// `label_source` applied when a heuristic (text-shape/near-duplicate
/// suspicion, or the phone-mode RMS+pause heuristic) auto-corrects a chunk's
/// speaker and the chunk was NOT enqueued for Tier-2/3 LLM confirmation —
/// either no classifier is configured, or the enqueue policy below decided
/// this chunk didn't need one.
const LABEL_SOURCE_HEURISTIC: &str = "heuristic";

/// `label_source` applied when a heuristic auto-corrects a chunk's speaker
/// AND the chunk has been enqueued onto the Tier-2/3 [`SpeakerClassifier`]
/// for confirmation (Slice 4). Distinguishes an unconfirmed heuristic guess
/// from `"llm"` (Slice 5), which means the classifier already returned a
/// confirmed verdict for this chunk.
const LABEL_SOURCE_HEURISTIC_PENDING: &str = "heuristic_pending";

/// Phone mode: any utterance at or above this word count is always queued
/// for Tier-2/3 classification, regardless of what the cheap heuristics
/// concluded — phone mode has no channel truth at all, so every
/// long-enough utterance is worth the LLM call. Short utterances stay
/// heuristic-only to bound API cost.
const PHONE_CLASSIFIER_MIN_WORDS: usize = 8;

/// Number of `FirstPersonOnSystem` auto-corrections in a session before we warn
/// the user that their loopback is swallowing their own microphone. Below this
/// threshold the occasional bleed is tolerable; above it the System channel is
/// no longer a reliable proxy for "the interviewer".
const MIXED_SOURCE_WARN_THRESHOLD: usize = 5;

// ────────────────────────────────────────────────────────────────────────────
// Public types
// ────────────────────────────────────────────────────────────────────────────

/// A question detected from the System audio channel, ready for the orchestrator.
#[derive(Debug, Clone)]
pub struct DetectedQuestion {
    pub text: String,
    pub session_id: Uuid,
    pub detected_at: Instant,
    /// Provenance — who/what produced this question. The orchestrator
    /// asserts this is never `Microphone` so a future bug that routes the
    /// user's own speech into `question_tx` cannot dispatch responses to
    /// the user's own utterance (M13 S5).
    pub source: DetectedQuestionSource,
}

/// Origin of a [`DetectedQuestion`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedQuestionSource {
    /// Loopback / system audio (the interviewer in non-phone mode).
    System,
    /// Phone-call mode manual confirmation via Ctrl+Q (the single-channel
    /// audio is mixed but the user has marked the end of the interviewer's
    /// question).
    PhoneManual,
    /// React-side `trigger_response` — the user typed or pasted a question.
    UserTriggered,
    /// React-side `trigger_visual_response` (`lpav-s20-visual-classifier`) —
    /// the user manually asked for a diagram on the current/last question,
    /// bypassing the Visual-need classifier. Wired into the two-thread
    /// dispatch in slice 21.
    VisualManual,
    /// Microphone — must NEVER reach the orchestrator. Reserved as a sentinel
    /// for tests / defensive checks.
    Microphone,
}

// ────────────────────────────────────────────────────────────────────────────
// Cross-channel echo gate — near-identical duplicates only (M10 Slice 1)
// ────────────────────────────────────────────────────────────────────────────
//
// In loopback mode, channel identity IS speaker identity:
//   System = interviewer (loopback), Mic = user.
//
// Echo is acoustic/hardware bleed of the *same* words on the opposite channel,
// not overlapping conversation. Suppress only when Jaccard ≥ 0.85 within 500ms.
// Concurrent speakers with partial overlap (Jaccard < 0.85) are never dropped.
//
// Phone mode disables this gate entirely (single mixed channel — Slice 7).

const ECHO_WINDOW: Duration = Duration::from_millis(500);
const ECHO_MIN_WORDS: usize = 3;
const ECHO_JACCARD_THRESHOLD: f32 = 0.85;

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

/// Direction of a suppression event.
///
/// `SystemBleedIntoMic` is the dominant case (user without headphones — the
/// interviewer's voice on speakers leaks into the mic). `MicBleedIntoSystem`
/// is rare and indicates a misconfigured loopback (mic feeding back into the
/// system sink); we surface it at INFO+ once per session as a hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SuppressionDirection {
    SystemBleedIntoMic,
    MicBleedIntoSystem,
}

#[derive(Default)]
struct CrossChannelDedup {
    recent_system: Vec<DedupEntry>,
    recent_mic: Vec<DedupEntry>,
    /// Whether we have already logged the user-facing "mic loopback bleed"
    /// hint this session. Avoids log spam while still surfacing the warning.
    mic_to_system_warned: bool,
    /// Count of System-channel chunks auto-corrected to Microphone because they
    /// were clearly the user's own first-person speech. A high count means the
    /// loopback is capturing the mic (e.g. laptop speakers without headphones).
    system_to_mic_relabels: usize,
    /// Whether the mixed-source warning has already been surfaced this session.
    mixed_source_warned: bool,
}

impl CrossChannelDedup {
    /// Returns the suppression direction iff `text` near-duplicates content
    /// recently transcribed on the OPPOSITE channel (cross-channel echo) or
    /// on the SAME channel within the echo window (Whisper VAD producing two
    /// segments for the same utterance, common with overlapping VAD chunks).
    fn should_suppress(
        &mut self,
        source: AudioSource,
        text: &str,
        now: Instant,
    ) -> Option<SuppressionDirection> {
        let tokens = tokenize_for_echo(text);
        if tokens.len() < ECHO_MIN_WORDS {
            return None;
        }
        self.prune(now);

        // Same-channel near-duplicate (Whisper double-emit on contiguous VAD
        // segments). Reusing the SystemBleedIntoMic / MicBleedIntoSystem
        // variants for instrumentation; the suppression effect is the same.
        let same = match source {
            AudioSource::System => &self.recent_system,
            AudioSource::Microphone => &self.recent_mic,
        };
        if same
            .iter()
            .any(|entry| jaccard(&tokens, &entry.tokens) >= ECHO_JACCARD_THRESHOLD)
        {
            return Some(match source {
                AudioSource::System => SuppressionDirection::MicBleedIntoSystem,
                AudioSource::Microphone => SuppressionDirection::SystemBleedIntoMic,
            });
        }

        let opposite = match source {
            AudioSource::Microphone => &self.recent_system,
            AudioSource::System => &self.recent_mic,
        };
        let matched = opposite
            .iter()
            .any(|entry| jaccard(&tokens, &entry.tokens) >= ECHO_JACCARD_THRESHOLD);
        if !matched {
            return None;
        }
        Some(match source {
            // Mic arrived AFTER System spoke the same words → speakers bled
            // into the mic; the System chunk is the truth, drop the Mic copy.
            AudioSource::Microphone => SuppressionDirection::SystemBleedIntoMic,
            // System arrived AFTER Mic spoke the same words → loopback is
            // recording the user's own voice; drop the System copy.
            AudioSource::System => SuppressionDirection::MicBleedIntoSystem,
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

/// Tokenize text for near-duplicate comparison. Shared with
/// [`crate::transcription::speaker_suspicion::NearDuplicateTracker`] (Slice 3)
/// so both the hard 500ms echo-suppression window here and the wider 1.5s
/// suspicion window there agree on what counts as "the same words".
pub(crate) fn tokenize_for_echo(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub(crate) fn jaccard(a: &[String], b: &[String]) -> f32 {
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
            vad: VadChunker::new_for_system()?,
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
/// Whisper inference runs on a background worker so VAD frame processing is
/// never stalled by decode latency. All other per-frame work is synchronous
/// and fast enough to run inline (RNNoise < 5ms, VAD < 1ms per frame).
///
/// The function returns when both input channels are closed (i.e. when
/// `AudioCapture::stop()` has been called and all senders have dropped).
#[allow(clippy::too_many_arguments)]
pub async fn run_audio_pipeline(
    app_handle: AppHandle,
    session_id: Uuid,
    whisper: Arc<WhisperEngine>,
    hybrid: Arc<AsyncMutex<HybridQuestionDetector>>,
    system_buffer: Arc<SyncMutex<SystemTranscriptBuffer>>,
    question_tx: mpsc::Sender<DetectedQuestion>,
    mut system_rx: mpsc::Receiver<AudioFrame>,
    mut mic_rx: mpsc::Receiver<AudioFrame>,
    persistence: Arc<SessionPersistence>,
    mic_quality: Arc<SyncMutex<MicQualityMonitor>>,
    audit: Arc<AudioAuditCounters>,
    echo_suppression_enabled: bool,
    phone_mode_manual_only: bool,
    diarizer: Option<Arc<SyncMutex<DiarizerManager>>>,
    speaker_classifier: Option<Arc<SpeakerClassifier>>,
    whisper_pending: Arc<AtomicUsize>,
    system_buffer_epoch: Arc<AtomicU64>,
) -> Result<()> {
    let mut sys_proc = ChannelProcessor::new_system()?;
    let mut mic_proc = ChannelProcessor::new_mic()?;
    let dedup = SyncMutex::new(CrossChannelDedup::default());
    let near_duplicate = SyncMutex::new(NearDuplicateTracker::new());
    let phone_heuristic = SyncMutex::new(PhoneHeuristicState::new());
    let rolling_contexts = Arc::new(SyncMutex::new(ChannelRollingContexts::default()));
    let (whisper_worker, mut whisper_results) =
        LiveWhisperWorker::start(Arc::clone(&whisper), Arc::clone(&whisper_pending));

    let dispatch_whisper_result = |meta: LiveWhisperJobMeta, outcome: LiveWhisperOutcome| async {
        let result = handle_transcription_result(
            meta,
            outcome,
            &app_handle,
            session_id,
            &hybrid,
            &system_buffer,
            &system_buffer_epoch,
            &question_tx,
            &persistence,
            &dedup,
            &near_duplicate,
            &phone_heuristic,
            &mic_quality,
            &audit,
            echo_suppression_enabled,
            phone_mode_manual_only,
            &diarizer,
            &rolling_contexts,
            &speaker_classifier,
        )
        .await;
        whisper_pending.fetch_sub(1, Ordering::Release);
        result
    };

    loop {
        tokio::select! {
            result = whisper_results.recv() => {
                if let Some((meta, outcome)) = result {
                    if let Err(e) = dispatch_whisper_result(meta, outcome).await {
                        tracing::warn!(error = %e, "transcription result handler error — continuing");
                    }
                }
            }
            f = system_rx.recv() => match f {
                Some(frame) => {
                    if let Err(e) = process_frame(
                        frame,
                        &mut sys_proc,
                        &app_handle,
                        &whisper_worker,
                        &whisper_pending,
                        &system_buffer_epoch,
                        session_id,
                        &hybrid,
                        &system_buffer,
                        &question_tx,
                        &rolling_contexts,
                        phone_mode_manual_only,
                        &diarizer,
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "audio pipeline frame error — continuing");
                    }
                }
                None => break,
            },
            f = mic_rx.recv() => match f {
                Some(frame) => {
                    if let Err(e) = process_frame(
                        frame,
                        &mut mic_proc,
                        &app_handle,
                        &whisper_worker,
                        &whisper_pending,
                        &system_buffer_epoch,
                        session_id,
                        &hybrid,
                        &system_buffer,
                        &question_tx,
                        &rolling_contexts,
                        phone_mode_manual_only,
                        &diarizer,
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "audio pipeline frame error — continuing");
                    }
                }
                None => break,
            },
        }
    }

    while let Some((meta, outcome)) = whisper_results.recv().await {
        if let Err(e) = dispatch_whisper_result(meta, outcome).await {
            tracing::warn!(error = %e, "transcription drain error — continuing");
        }
    }

    whisper_worker.shutdown().await?;

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
    whisper_worker: &LiveWhisperWorker,
    whisper_pending: &AtomicUsize,
    system_buffer_epoch: &AtomicU64,
    session_id: Uuid,
    hybrid: &Arc<AsyncMutex<HybridQuestionDetector>>,
    system_buffer: &Arc<SyncMutex<SystemTranscriptBuffer>>,
    question_tx: &mpsc::Sender<DetectedQuestion>,
    rolling_contexts: &Arc<SyncMutex<ChannelRollingContexts>>,
    phone_mode_manual_only: bool,
    diarizer: &Option<Arc<SyncMutex<DiarizerManager>>>,
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

    if phone_mode_manual_only {
        if let Some(d) = diarizer {
            if let Ok(mut guard) = d.lock() {
                guard.ingest_pcm(&downsampled, 16_000);
            }
        }
    }

    // ── Step 3: VAD ───────────────────────────────────────────────────────
    let silence_ms = if source == AudioSource::System {
        proc.vad.ms_since_last_speech()
    } else {
        0
    };

    let Some(chunk) = proc.vad.process_frame(&downsampled, source) else {
        if source == AudioSource::System && !phone_mode_manual_only {
            if LiveWhisperWorker::pending_jobs(whisper_pending) > 0 {
                return Ok(());
            }
            let uncertain = system_buffer
                .lock()
                .map(|b| b.has_uncertain_speaker())
                .unwrap_or(false);
            if uncertain {
                return Ok(());
            }
            let plan = {
                let mut guard = hybrid.lock().await;
                guard.check_silence(silence_ms)
            };
            let dispatched =
                dispatch_confirm_plan(plan, hybrid, app_handle, question_tx, session_id).await?;
            if dispatched {
                if let Ok(mut buf) = system_buffer.lock() {
                    buf.clear();
                }
                hybrid.lock().await.reset_after_dispatch();
            }
        }
        return Ok(());
    };

    if chunk.duration_ms < WHISPER_MIN_SEGMENT_MS {
        return Ok(());
    }

    // ── Step 4a: enqueue Whisper (background worker — never blocks VAD) ───
    let chunk_duration_ms = chunk.duration_ms;
    let chunk_ready_at = Instant::now();
    let chunk_rms_dbfs = rms_dbfs(&chunk.samples);
    let frame_timestamp_ms = frame.timestamp.elapsed().as_millis() as i64;
    let rolling_context = {
        let guard = rolling_contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("rolling context mutex poisoned"))?;
        guard.context_for(source)
    };

    whisper_worker.enqueue(LiveWhisperJob {
        meta: LiveWhisperJobMeta {
            source,
            frame_timestamp_ms,
            chunk_ready_at,
            chunk_rms_dbfs,
            chunk_duration_ms,
            buffer_epoch: system_buffer_epoch.load(Ordering::Acquire),
        },
        chunk,
        rolling_context,
    });

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_transcription_result(
    meta: LiveWhisperJobMeta,
    outcome: LiveWhisperOutcome,
    app_handle: &AppHandle,
    session_id: Uuid,
    hybrid: &Arc<AsyncMutex<HybridQuestionDetector>>,
    system_buffer: &Arc<SyncMutex<SystemTranscriptBuffer>>,
    system_buffer_epoch: &AtomicU64,
    question_tx: &mpsc::Sender<DetectedQuestion>,
    persistence: &Arc<SessionPersistence>,
    dedup: &SyncMutex<CrossChannelDedup>,
    near_duplicate: &SyncMutex<NearDuplicateTracker>,
    phone_heuristic: &SyncMutex<PhoneHeuristicState>,
    mic_quality: &Arc<SyncMutex<MicQualityMonitor>>,
    audit: &Arc<AudioAuditCounters>,
    echo_suppression_enabled: bool,
    phone_mode_manual_only: bool,
    diarizer: &Option<Arc<SyncMutex<DiarizerManager>>>,
    rolling_contexts: &Arc<SyncMutex<ChannelRollingContexts>>,
    speaker_classifier: &Option<Arc<SpeakerClassifier>>,
) -> Result<()> {
    let source = meta.source;
    let chunk_duration_ms = meta.chunk_duration_ms;
    let chunk_ready_at = meta.chunk_ready_at;
    let chunk_rms_dbfs = meta.chunk_rms_dbfs;
    let timestamp = meta.frame_timestamp_ms;

    let LiveWhisperOutcome::Transcribed(mut result) = outcome else {
        return Ok(());
    };

    // M13 S2: live sanitiser — strip hallucinated profanity / known stock
    // hallucination tails / repeated ngram loops that survived the engine
    // filters. Returns None when the whole utterance should be dropped.
    match sanitize_live_transcript(&result.text) {
        Some(clean) => result.text = clean,
        None => {
            tracing::debug!(
                source = %source,
                "transcript chunk dropped — sanitiser removed entire content"
            );
            audit.record_suppression(source, SuppressionReason::SanitizerEmpty);
            log_chunk_metric(
                source,
                chunk_duration_ms,
                None,
                true,
                Some(SuppressionReason::SanitizerEmpty.as_str()),
                LABEL_SOURCE_CHANNEL,
                false,
            );
            return Ok(());
        }
    }

    let now = Instant::now();
    let channel_speaker = match source {
        AudioSource::System => "System",
        AudioSource::Microphone => "Microphone",
    };

    if echo_suppression_enabled {
        let mut guard = match dedup.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(direction) = guard.should_suppress(source, &result.text, now) {
            let reason = match direction {
                SuppressionDirection::SystemBleedIntoMic => {
                    tracing::debug!(
                        source = %source,
                        "suppressed cross-channel echo (system -> mic)"
                    );
                    SuppressionReason::EchoSystemBleedIntoMic
                }
                SuppressionDirection::MicBleedIntoSystem => {
                    if !guard.mic_to_system_warned {
                        guard.mic_to_system_warned = true;
                        tracing::warn!(
                            "Mic audio appears to be looping back into the system audio \
                             channel. Check your audio routing — Flint expects the system \
                             sink monitor as the loopback, not the mic itself."
                        );
                    } else {
                        tracing::debug!(
                            source = %source,
                            "suppressed cross-channel echo (mic -> system)"
                        );
                    }
                    SuppressionReason::EchoMicBleedIntoSystem
                }
            };
            drop(guard);
            audit.record_suppression(source, reason);
            log_chunk_metric(
                source,
                chunk_duration_ms,
                result.avg_logprob,
                true,
                Some(reason.as_str()),
                LABEL_SOURCE_CHANNEL,
                false,
            );
            return Ok(());
        }
        guard.record(source, &result.text, now);
    }
    // Wider (1.5s) near-duplicate window — dual-stream only (Slice 3). Phone
    // mode routes everything onto one physical channel, so there is no
    // "opposite channel" for loopback bleed to survive onto.
    if echo_suppression_enabled {
        if let Ok(mut guard) = near_duplicate.lock() {
            guard.record(channel_speaker, &result.text, now);
        }
    }

    if let Ok(mut guard) = rolling_contexts.lock() {
        guard.append(source, &result.text);
    }

    // ── Step 4b.0: resolve effective speaker (auto-correct mislabels) ──────
    //
    // The cpal channel is the default speaker proxy, but it is wrong when the
    // loopback captures the user's own voice or the mic captures the
    // interviewer. When the content shape clearly contradicts the channel, we
    // auto-correct the label so every downstream consumer (persistence, UI,
    // question detection, summary) sees the speaker that actually spoke.
    // Phone-call mode collapses both speakers onto one channel, so the
    // heuristic is meaningless there and only runs in dual-stream mode.
    let mut speaker = channel_speaker;
    let mut label_source = LABEL_SOURCE_CHANNEL;
    // True when a heuristic (text-shape or near-duplicate suspicion)
    // overrode the channel label this chunk — Slice 4 will use this to
    // decide which chunks are worth a Tier-2/3 LLM confirmation call.
    let mut was_suspicious = false;

    // Dual-stream: channel is usually reliable; heuristics catch bleed.
    // Phone mode: all audio arrives on System — heuristics are the only way
    // to distinguish interviewer questions from the user's answers.
    if echo_suppression_enabled || phone_mode_manual_only {
        let text_shape_verdict = speaker_suspicion::evaluate(channel_speaker, &result.text);
        let verdict = text_shape_verdict.or_else(|| {
            if echo_suppression_enabled {
                near_duplicate
                    .lock()
                    .ok()
                    .and_then(|mut guard| guard.check(channel_speaker, &result.text, now))
            } else {
                None
            }
        });

        if let Some(verdict) = verdict {
            was_suspicious = true;
            match verdict.reason {
                SuspicionReason::QuestionShapeOnMic => audit.record_suspicion_question_on_mic(),
                SuspicionReason::FirstPersonOnSystem => {
                    audit.record_suspicion_first_person_on_system();
                    if echo_suppression_enabled {
                        maybe_warn_mixed_source(app_handle, dedup);
                    }
                }
                SuspicionReason::NearDuplicateCrossChannel => {}
            }
            tracing::info!(
                from = %channel_speaker,
                to = %verdict.suggested_speaker,
                reason = %verdict.reason.as_str(),
                "auto-corrected suspicious speaker label"
            );
            speaker = if verdict.suggested_speaker == "System" {
                "System"
            } else {
                "Microphone"
            };
            label_source = LABEL_SOURCE_HEURISTIC;
        } else if phone_mode_manual_only {
            // Phone mode has no channel truth at all (both speakers arrive
            // on the same mixed audio) — when the text-shape check above
            // stays silent, fall back to the RMS+pause turn-taking
            // heuristic, the only signal available (Slice 2).
            let role = phone_heuristic
                .lock()
                .map(|mut guard| guard.observe(chunk_rms_dbfs, chunk_duration_ms, chunk_ready_at))
                .unwrap_or(SpeakerRole::Unknown);
            speaker = match role {
                SpeakerRole::Interviewer => "System",
                SpeakerRole::User => "Microphone",
                SpeakerRole::Unknown => channel_speaker,
            };
            label_source = LABEL_SOURCE_HEURISTIC;
        }
    }

    // Slice 4 enqueue policy: phone mode classifies every long-enough
    // utterance (no channel truth exists at all); dual-stream mode only
    // classifies chunks a heuristic already flagged as suspicious.
    let chunk_id = Uuid::new_v4();
    let word_count = result.text.split_whitespace().count();
    let should_enqueue = if phone_mode_manual_only {
        word_count >= PHONE_CLASSIFIER_MIN_WORDS
    } else {
        was_suspicious
    };
    if should_enqueue {
        if let Some(classifier) = speaker_classifier {
            let enqueued = classifier.enqueue(ClassificationRequest {
                chunk_id,
                session_id,
                text: result.text.clone(),
                current_speaker: speaker.to_string(),
            });
            if enqueued {
                label_source = LABEL_SOURCE_HEURISTIC_PENDING;
            }
        }
    }

    // The speaker that actually spoke drives all routing below.
    let mut effective_source = match speaker {
        "System" => AudioSource::System,
        _ => AudioSource::Microphone,
    };

    if phone_mode_manual_only {
        if let Some(d) = diarizer {
            if let Ok(mut guard) = d.lock() {
                if let Some(role) = guard.role_at_offset_ms(timestamp as u64) {
                    effective_source = match role {
                        crate::audio::diarizer::SpeakerRole::Interviewer => AudioSource::System,
                        crate::audio::diarizer::SpeakerRole::User => AudioSource::Microphone,
                        crate::audio::diarizer::SpeakerRole::Unknown => effective_source,
                    };
                    let speaker_id = match role {
                        crate::audio::diarizer::SpeakerRole::Interviewer => {
                            if let crate::audio::diarizer::DiarizerStatus::Assigned {
                                interviewer_id,
                                ..
                            } = guard.status()
                            {
                                *interviewer_id
                            } else {
                                0
                            }
                        }
                        crate::audio::diarizer::SpeakerRole::User => {
                            if let crate::audio::diarizer::DiarizerStatus::Assigned {
                                user_id,
                                ..
                            } = guard.status()
                            {
                                *user_id
                            } else {
                                1
                            }
                        }
                        crate::audio::diarizer::SpeakerRole::Unknown => 0,
                    };
                    guard.note_transcript(speaker_id, &result.text);
                }
            }
        }
    }

    // ── Step 4b: emit + persist transcript chunk ──────────────────────────
    // `chunk_id` was generated above (Slice 4 enqueue policy) so the
    // classifier's callback and this persisted row share one identity.
    emit_transcription_chunk(
        app_handle,
        TranscriptionChunkPayload {
            text: result.text.clone(),
            speaker: speaker.to_string(),
            timestamp,
            chunk_id: chunk_id.to_string(),
            label_source: label_source.to_string(),
        },
    );
    // Persist every chunk immediately — crash-recovery insurance.
    let chunk = TranscriptChunk {
        id: chunk_id,
        session_id,
        speaker: speaker.to_string(),
        text: result.text.clone(),
        timestamp_ms: timestamp,
        label_source: label_source.to_string(),
    };
    if let Err(e) = persistence.write_transcript_chunk(&chunk) {
        tracing::warn!(error = %e, "transcript chunk persist failed — continuing");
    }

    // M13 S6 — record an accepted chunk for the session-end audit summary.
    // Audit reflects the physical capture channel so the loopback health stats
    // stay meaningful regardless of relabelling.
    audit.record_chunk(source, result.avg_logprob);
    log_chunk_metric(
        source,
        chunk_duration_ms,
        result.avg_logprob,
        false,
        None,
        label_source,
        true,
    );

    // ── Steps 4c/4d: hybrid question detection on the interviewer's speech ─
    // Mic-quality monitoring keys off the PHYSICAL mic channel (it measures
    // hardware capture quality, not speaker identity).
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
    }

    // Only the interviewer's speech feeds question detection. Routing by the
    // EFFECTIVE speaker means a mislabeled interviewer question on the mic is
    // still detected, and the user's own speech that bled into the loopback no
    // longer pollutes the detector.
    if effective_source != AudioSource::System {
        return Ok(());
    }

    let buffer_epoch_live = system_buffer_epoch.load(Ordering::Acquire);
    let buffer_current = meta.buffer_epoch == buffer_epoch_live;
    if buffer_current {
        let mut buf = system_buffer
            .lock()
            .map_err(|_| anyhow::anyhow!("system transcript buffer mutex poisoned"))?;
        buf.append_chunk(&result.text, Some(chunk_id.to_string()), label_source);
    }

    if !buffer_current {
        return Ok(());
    }

    if phone_mode_manual_only {
        let allow_auto = diarizer
            .as_ref()
            .and_then(|d| d.lock().ok())
            .map(|g| g.allows_auto_question_detection_at(timestamp as u64))
            .unwrap_or(false);
        if !allow_auto {
            return Ok(());
        }
    }

    let uncertain_speaker = system_buffer
        .lock()
        .map(|b| b.has_uncertain_speaker())
        .unwrap_or(false);
    if uncertain_speaker {
        return Ok(());
    }

    let accumulated = {
        let buf = system_buffer
            .lock()
            .map_err(|_| anyhow::anyhow!("system transcript buffer mutex poisoned"))?;
        buf.accumulated_text()
    };

    // Whisper runs inline and stalls VAD frame processing, so
    // `ms_since_last_speech()` here includes inference latency — not real
    // conversational silence. Stage/update the candidate only; confirmation
    // happens on subsequent live silence ticks via `check_silence`.
    let plan = {
        let mut guard = hybrid.lock().await;
        guard.ingest_transcript(&accumulated, 0)
    };
    let dispatched =
        dispatch_confirm_plan(plan, hybrid, app_handle, question_tx, session_id).await?;
    if dispatched {
        // The confirmed text has been sent to the orchestrator. Clear the
        // System transcript buffer and reset the hybrid detector so the next
        // round of detection starts fresh — otherwise every subsequent chunk
        // re-runs detection over the already-dispatched text plus the new
        // fragment, repeatedly firing answers to ever-growing run-on questions.
        if let Ok(mut buf) = system_buffer.lock() {
            buf.clear();
        }
        hybrid.lock().await.reset_after_dispatch();
    }

    Ok(())
}

/// Track System->Mic relabels and warn the user once per session when the
/// loopback is clearly capturing their own microphone. Recovering speaker
/// separation by hand is impossible mid-session, but headphones fix it
/// instantly, so the hint is actionable.
fn maybe_warn_mixed_source(app_handle: &AppHandle, dedup: &SyncMutex<CrossChannelDedup>) {
    let mut guard = match dedup.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.system_to_mic_relabels += 1;
    if guard.mixed_source_warned || guard.system_to_mic_relabels < MIXED_SOURCE_WARN_THRESHOLD {
        return;
    }
    guard.mixed_source_warned = true;
    drop(guard);
    tracing::warn!("system loopback is capturing the user's own voice — recommending headphones");
    emit_audio_routing_warning(
        app_handle,
        AudioRoutingWarningPayload {
            kind: "loopback_capturing_mic".to_string(),
            message: "Your microphone is bleeding into the system audio. Use headphones so \
                      Flint can tell you and the interviewer apart."
                .to_string(),
        },
    );
}

/// Returns `true` when a question was actually dispatched to the orchestrator
/// so the caller knows to drain the System transcript buffer and reset the
/// hybrid detector. Returns `false` when the plan was `None` or the LLM verify
/// step rejected the candidate.
async fn dispatch_confirm_plan(
    plan: Option<ConfirmPlan>,
    hybrid: &Arc<AsyncMutex<HybridQuestionDetector>>,
    app_handle: &AppHandle,
    question_tx: &mpsc::Sender<DetectedQuestion>,
    session_id: Uuid,
) -> Result<bool> {
    match plan {
        None => Ok(false),
        Some(ConfirmPlan::Immediate(text)) => {
            send_detected_question(question_tx, session_id, text);
            Ok(true)
        }
        Some(ConfirmPlan::WithLlm(text)) => {
            if let Some(q) = finalize_confirmation(hybrid, text, app_handle).await? {
                send_detected_question(question_tx, session_id, q);
                Ok(true)
            } else {
                Ok(false)
            }
        }
    }
}

/// M13 S6 — emit the per-chunk structured log line per
/// `.cursor/rules/flint-performance.mdc`. INFO level so it surfaces in the
/// dev dashboard without requiring debug builds.
fn log_chunk_metric(
    source: AudioSource,
    duration_ms: u32,
    avg_logprob: Option<f32>,
    suppressed: bool,
    suppression_reason: Option<&'static str>,
    label_source: &'static str,
    was_validated: bool,
) {
    tracing::info!(
        target: "flint::audio::chunk",
        source = %source,
        duration_ms,
        avg_logprob = ?avg_logprob,
        suppressed,
        suppression_reason = ?suppression_reason,
        label_source,
        was_validated,
        "transcription_chunk_metric"
    );
}

fn send_detected_question(
    question_tx: &mpsc::Sender<DetectedQuestion>,
    session_id: Uuid,
    text: String,
) {
    let q = DetectedQuestion {
        text,
        session_id,
        detected_at: Instant::now(),
        source: DetectedQuestionSource::System,
    };
    if question_tx.try_send(q).is_err() {
        tracing::warn!(
            session_id = %session_id,
            "question_tx full — detected question dropped"
        );
    }
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
            chunk_id: String::new(),
            label_source: LABEL_SOURCE_CHANNEL.to_string(),
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
            source: DetectedQuestionSource::System,
        };
        assert!(!q.text.is_empty());
    }

    #[test]
    fn jaccard_high_overlap_matches_near_duplicate_transcripts() {
        let a = tokenize_for_echo("Why do you like to work with Fisher Investors?");
        let b = tokenize_for_echo("Why do you like work with Fisher Investors");
        assert!(jaccard(&a, &b) >= ECHO_JACCARD_THRESHOLD);

        let c = tokenize_for_echo("Tell me about a time you led a team");
        assert!(jaccard(&a, &c) < ECHO_JACCARD_THRESHOLD);
    }

    #[test]
    fn concurrent_speakers_with_partial_overlap_not_suppressed() {
        let mut dedup = CrossChannelDedup::default();
        let now = Instant::now();
        dedup.record(
            AudioSource::System,
            "tell me about a project you led at your company",
            now,
        );
        assert!(dedup
            .should_suppress(
                AudioSource::Microphone,
                "I led the fraud detection platform migration last year",
                now + Duration::from_millis(200),
            )
            .is_none());
    }

    #[test]
    fn same_channel_near_duplicate_is_suppressed() {
        let mut dedup = CrossChannelDedup::default();
        let now = Instant::now();
        let text = "How are you today and what brings you here";
        dedup.record(AudioSource::System, text, now);
        // Whisper VAD double-emit: same channel, same text, within window.
        assert!(dedup
            .should_suppress(AudioSource::System, text, now + Duration::from_millis(400))
            .is_some());
    }

    #[test]
    fn exact_echo_suppressed_at_high_jaccard_within_window() {
        let mut dedup = CrossChannelDedup::default();
        let now = Instant::now();
        let text = "I am excited about the AI Engineer opportunity at Fisher Investors";
        dedup.record(AudioSource::Microphone, text, now);
        assert_eq!(
            dedup.should_suppress(AudioSource::System, text, now + Duration::from_millis(300),),
            Some(SuppressionDirection::MicBleedIntoSystem)
        );
    }

    #[test]
    fn dedup_suppresses_mic_echo_when_system_spoke_first() {
        let mut dedup = CrossChannelDedup::default();
        let now = Instant::now();
        let text = "Why do you like to work with Fisher Investors";
        dedup.record(AudioSource::System, text, now);

        assert_eq!(
            dedup.should_suppress(
                AudioSource::Microphone,
                text,
                now + Duration::from_millis(200),
            ),
            Some(SuppressionDirection::SystemBleedIntoMic)
        );
    }

    #[test]
    fn dedup_suppresses_system_echo_when_mic_spoke_first() {
        let mut dedup = CrossChannelDedup::default();
        let now = Instant::now();
        let text = "I am excited about the AI Engineer opportunity at Fisher Investors";
        dedup.record(AudioSource::Microphone, text, now);

        assert_eq!(
            dedup.should_suppress(AudioSource::System, text, now + Duration::from_millis(200),),
            Some(SuppressionDirection::MicBleedIntoSystem)
        );
    }

    #[test]
    fn dedup_does_not_suppress_distinct_content_on_either_channel() {
        let mut dedup = CrossChannelDedup::default();
        let now = Instant::now();
        dedup.record(AudioSource::System, "tell me about a project you led", now);

        assert!(dedup
            .should_suppress(
                AudioSource::Microphone,
                "I led the fraud detection platform migration last year",
                now + Duration::from_millis(500),
            )
            .is_none());
    }

    #[test]
    fn dedup_drops_entries_outside_the_window() {
        let mut dedup = CrossChannelDedup::default();
        let now = Instant::now();
        let text = "alpha bravo charlie delta echo test phrase here";
        dedup.record(AudioSource::System, text, now);
        let later = now + ECHO_WINDOW + Duration::from_millis(1);
        assert!(dedup
            .should_suppress(AudioSource::Microphone, text, later,)
            .is_none());
    }

    #[test]
    fn prefix_match_alone_does_not_suppress_without_high_jaccard() {
        let mut dedup = CrossChannelDedup::default();
        let now = Instant::now();
        dedup.record(
            AudioSource::System,
            "all right lets take our time with this question",
            now,
        );
        assert!(dedup
            .should_suppress(
                AudioSource::Microphone,
                "all right lets say guard over here now",
                now + Duration::from_millis(300),
            )
            .is_none());
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
    fn mic_quality_monitor_emits_low_on_poor_logprob_sequence() {
        let mut monitor = MicQualityMonitor::default();
        assert!(monitor.observe(-0.55).is_none());
        assert!(monitor.observe(-0.6).is_none());
        assert_eq!(monitor.observe(-0.65), Some("low"));
    }
}
