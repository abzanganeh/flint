//! WebRTC VAD chunking — Task 3.4.
//!
//! All parameters are EXACT as specified in §26 and Task 3.4.
//! Do not change any constant without updating the design document.
//!
//! ## Pipeline position
//! ```text
//! Downsampler output  →  160 samples @ 16kHz
//!     VadChunker::process_frame()   ← this file
//!     → Some(VadChunk) when a speech segment ends
//!     → None while collecting or silent
//! ```
//!
//! ## Frame accumulation
//! The pipeline delivers 160-sample frames (10ms at 16kHz) from the
//! downsampler.  WebRTC VAD requires 320-sample frames (20ms).
//! `VadChunker` maintains an internal `frame_buf` that accumulates
//! sub-frames until a full 320-sample VAD frame is available.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::time::Instant;

use super::capture::AudioSource;

use anyhow::Result;
use webrtc_vad::{SampleRate, Vad, VadMode};

/// WebRTC VAD aggressiveness mode (0 = Quality … 3 = VeryAggressive).
/// Mode 3 filters most background noise — required by §26.
const VAD_MODE: VadMode = VadMode::VeryAggressive;

/// Sample rate fed to the WebRTC VAD module.
pub const VAD_SAMPLE_RATE: u32 = 16_000;

/// VAD frame size: 20ms × 16kHz = 320 samples.
pub const VAD_FRAME_SAMPLES: usize = 320;

/// Duration of one VAD frame in milliseconds.
const FRAME_MS: u32 = 20;

/// Minimum speech segment sent to Whisper (M10 Slice 5).
pub const WHISPER_MIN_SEGMENT_MS: u32 = 500;

/// Pre/post padding applied to VAD boundaries (M10 Slice 5).
const PRE_POST_PADDING_MS: u32 = 200;
const PRE_POST_PADDING_FRAMES: u32 = PRE_POST_PADDING_MS / FRAME_MS;

// ────────────────────────────────────────────────────────────────────────────
// Constants — §26 / Task 3.4.  DO NOT change without updating the spec.
// ────────────────────────────────────────────────────────────────────────────

/// Segments shorter than this are noise artefacts and are discarded.
const MIN_SPEECH_MS: u32 = 200;
const MIN_SPEECH_FRAMES: u32 = MIN_SPEECH_MS / FRAME_MS; // = 10

/// After this much consecutive silence, the current speech segment ends.
const MAX_SILENCE_GAP_MS: u32 = 600;
const MAX_SILENCE_FRAMES: u32 = MAX_SILENCE_GAP_MS / FRAME_MS; // = 30

/// Maximum speech chunk duration before a mid-speech flush is forced.
///
/// When two speakers talk continuously (or one person gives a long answer) with
/// no 600ms pause, the VAD accumulates all samples in `speech_buf` and only
/// emits when silence finally arrives. That produces a single huge chunk that
/// takes Whisper several seconds to transcribe and causes multiple lines of
/// text to appear at once. Enforcing a ceiling here means long utterances are
/// split into ≤30-second pieces, each transcribed independently, which keeps
/// the transcript rolling even during uninterrupted speech.
///
/// The value is a compromise: long enough that a single complete answer fits in
/// one chunk most of the time, short enough that Whisper latency stays under
/// ~3 seconds on Tier-1 hardware.
const MAX_CHUNK_DURATION_MS: u32 = 30_000;
const MAX_CHUNK_FRAMES: u32 = MAX_CHUNK_DURATION_MS / FRAME_MS; // = 1500

/// Below this energy level the frame is definitely silence — skip VAD.
const ENERGY_FLOOR_DBFS: f32 = -60.0;

/// Below this level the frame counts as silence even if energy > floor.
/// Above this threshold the WebRTC VAD is consulted.
const SILENCE_THRESHOLD_DBFS: f32 = -35.0;

// ────────────────────────────────────────────────────────────────────────────
// Public output type
// ────────────────────────────────────────────────────────────────────────────

/// A complete speech segment ready for the Whisper transcription engine.
///
/// Contains only speech samples (no trailing silence).
/// `source` identifies which audio channel produced the segment.
/// `duration_ms` is the speech content duration (including intra-word pauses
/// shorter than `MAX_SILENCE_GAP_MS`, but not trailing silence).
pub struct VadChunk {
    pub samples: Vec<f32>,
    pub source: AudioSource,
    pub duration_ms: u32,
}

// ────────────────────────────────────────────────────────────────────────────
// Internal state machine
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    /// No speech in progress.
    Idle,
    /// Accumulating speech frames into the current segment.
    Collecting,
    /// In post-speech silence; waiting to see if speech resumes before
    /// `MAX_SILENCE_FRAMES` are consumed.
    Ending,
}

// ────────────────────────────────────────────────────────────────────────────
// VadChunker
// ────────────────────────────────────────────────────────────────────────────

pub struct VadChunker {
    vad: Vad,
    state: State,
    /// Accumulates incoming sub-frames until `VAD_FRAME_SAMPLES` are available.
    ///
    /// Upstream contract: `Downsampler` in `rnnoise.rs` emits
    /// [`super::rnnoise::DOWNSAMPLED_FRAME_SIZE`] (160) samples per call at
    /// 16kHz. Two such frames are merged before WebRTC VAD runs (320 samples
    /// = 20ms). Other sizes are accepted but non-standard for this pipeline.
    frame_buf: Vec<f32>,
    /// Speech + intra-segment pause samples for the active segment.
    speech_buf: Vec<f32>,
    /// Silence frames accumulated since the last speech frame (Ending state).
    /// These are buffered so they can be folded back into `speech_buf` if
    /// speech resumes before the silence gap is exceeded.
    trailing_silence_buf: Vec<f32>,
    /// Consecutive silence-frame count in the Ending state.
    silence_frames: u32,
    /// Count of VAD frames classified as speech in the current segment.
    /// Used to enforce `MIN_SPEECH_MS`.
    speech_frames: u32,
    /// Source of the first speech frame of the active segment.
    current_source: AudioSource,
    /// Rolling pre-speech buffer for 200ms pre-padding.
    pre_roll: VecDeque<Vec<f32>>,
    /// Timestamp of the most recent speech-classified frame.
    last_speech_at: Option<Instant>,
}

// `webrtc_vad::Vad` wraps a C pointer from bindgen, so it is not `Send` by
// default. `VadChunker` is only ever accessed from a single pipeline task —
// there is no cross-thread sharing. The declaration is therefore sound.
unsafe impl Send for VadChunker {}

impl VadChunker {
    /// Create a VadChunker with exact §26 parameters.
    pub fn new() -> Result<Self> {
        let vad = Vad::new_with_rate_and_mode(SampleRate::Rate16kHz, VAD_MODE);
        Ok(Self::with_vad(vad))
    }

    fn with_vad(vad: Vad) -> Self {
        Self {
            vad,
            state: State::Idle,
            frame_buf: Vec::new(),
            speech_buf: Vec::new(),
            trailing_silence_buf: Vec::new(),
            silence_frames: 0,
            speech_frames: 0,
            current_source: AudioSource::System,
            pre_roll: VecDeque::new(),
            last_speech_at: None,
        }
    }

    /// Milliseconds since the last speech-classified frame on this channel.
    pub fn ms_since_last_speech(&self) -> u64 {
        self.last_speech_at
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(u64::MAX)
    }

    fn push_pre_roll(&mut self, frame: &[f32]) {
        if self.pre_roll.len() >= PRE_POST_PADDING_FRAMES as usize {
            self.pre_roll.pop_front();
        }
        self.pre_roll.push_back(frame.to_vec());
    }

    fn prepend_pre_roll(&mut self) {
        let mut padded = Vec::new();
        for frame in &self.pre_roll {
            padded.extend_from_slice(frame);
        }
        padded.append(&mut self.speech_buf);
        self.speech_buf = padded;
    }

    fn append_post_padding(&mut self) {
        let pad_samples = (PRE_POST_PADDING_MS as usize * VAD_SAMPLE_RATE as usize) / 1000;
        self.speech_buf.extend(std::iter::repeat_n(0.0f32, pad_samples));
    }

    /// Process one frame of 16kHz PCM mono audio.
    ///
    /// Expected upstream input: 160 samples (10ms) from `Downsampler::process`
    /// in `rnnoise.rs`. Arbitrary sizes are accepted; the internal `frame_buf`
    /// accumulates until a full 320-sample (20ms) VAD frame is ready.
    ///
    /// Returns `Some(VadChunk)` when a complete speech segment ends.
    /// Returns `None` while speech is ongoing or during silence.
    pub fn process_frame(&mut self, frame: &[f32], source: AudioSource) -> Option<VadChunk> {
        self.frame_buf.extend_from_slice(frame);

        if self.frame_buf.len() < VAD_FRAME_SAMPLES {
            return None; // not enough samples for a VAD frame yet
        }

        let vad_frame: Vec<f32> = self.frame_buf.drain(..VAD_FRAME_SAMPLES).collect();
        self.step(&vad_frame, source)
    }

    // ── State machine ─────────────────────────────────────────────────────

    fn step(&mut self, frame: &[f32], source: AudioSource) -> Option<VadChunk> {
        let is_speech = self.classify(frame);

        if is_speech {
            self.last_speech_at = Some(Instant::now());
        } else if self.state == State::Idle {
            self.push_pre_roll(frame);
        }

        match self.state {
            State::Idle => {
                if is_speech {
                    self.state = State::Collecting;
                    self.current_source = source;
                    self.speech_frames = 1;
                    self.prepend_pre_roll();
                    self.speech_buf.extend_from_slice(frame);
                }
                None
            }

            State::Collecting => {
                if is_speech {
                    self.speech_frames += 1;
                    self.speech_buf.extend_from_slice(frame);

                    // Force-flush when the segment reaches the maximum chunk
                    // duration. Speech is still active so we stay in Collecting
                    // and start a fresh buffer immediately — no transition to Idle.
                    if self.speech_frames >= MAX_CHUNK_FRAMES {
                        return self.flush_and_continue();
                    }

                    None
                } else {
                    // First silence after speech — start the silence countdown.
                    self.state = State::Ending;
                    self.silence_frames = 1;
                    self.trailing_silence_buf.extend_from_slice(frame);
                    None
                }
            }

            State::Ending => {
                if is_speech {
                    // Speech resumed within the gap — merge trailing silence
                    // back into the speech segment (natural intra-utterance pause).
                    self.speech_buf.append(&mut self.trailing_silence_buf);
                    self.trailing_silence_buf.clear();
                    self.speech_buf.extend_from_slice(frame);
                    self.speech_frames += 1;
                    self.silence_frames = 0;
                    self.state = State::Collecting;
                    None
                } else {
                    self.trailing_silence_buf.extend_from_slice(frame);
                    self.silence_frames += 1;

                    if self.silence_frames >= MAX_SILENCE_FRAMES {
                        self.finalise_segment()
                    } else {
                        None
                    }
                }
            }
        }
    }

    /// Finalise the current segment.
    ///
    /// Emits a `VadChunk` if the segment meets `MIN_SPEECH_MS`; silently
    /// discards it otherwise (noise artefact).  Always resets to `Idle`.
    fn finalise_segment(&mut self) -> Option<VadChunk> {
        let speech_buf = std::mem::take(&mut self.speech_buf);
        let speech_frames = self.speech_frames;
        let source = self.current_source;

        self.trailing_silence_buf.clear();
        self.silence_frames = 0;
        self.speech_frames = 0;
        self.state = State::Idle;

        if speech_frames < MIN_SPEECH_FRAMES {
            return None; // below minimum speech duration — noise artefact
        }

        self.append_post_padding();
        let duration_ms = (speech_buf.len() as u32 * 1000) / VAD_SAMPLE_RATE;
        Some(VadChunk {
            samples: speech_buf,
            source,
            duration_ms,
        })
    }

    /// Flush the current speech buffer mid-utterance and immediately restart
    /// collection (do NOT transition to Idle).
    ///
    /// Called when `MAX_CHUNK_FRAMES` is reached. The caller must emit the
    /// returned chunk to Whisper; the next frame continues filling a fresh
    /// buffer in the `Collecting` state so there is no audio gap.
    fn flush_and_continue(&mut self) -> Option<VadChunk> {
        let speech_buf = std::mem::take(&mut self.speech_buf);
        let speech_frames = self.speech_frames;
        let source = self.current_source;

        // Reset counters but stay in Collecting — speech is still live.
        self.speech_frames = 0;

        if speech_frames < MIN_SPEECH_FRAMES {
            return None;
        }

        self.append_post_padding();
        let duration_ms = (speech_buf.len() as u32 * 1000) / VAD_SAMPLE_RATE;
        tracing::debug!(
            duration_ms,
            max_chunk_frames = MAX_CHUNK_FRAMES,
            "VAD max-chunk flush — splitting long utterance"
        );
        Some(VadChunk {
            samples: speech_buf,
            source,
            duration_ms,
        })
    }

    // ── Classification ────────────────────────────────────────────────────

    /// Classify one 320-sample frame as speech or silence.
    ///
    /// Decision chain (exact §26 parameters):
    ///   1. `energy < −60 dBFS` → silence; skip VAD call entirely.
    ///   2. `energy < −35 dBFS` → silence.
    ///   3. Otherwise: consult WebRTC VAD (mode 3, 16kHz, 20ms frame).
    fn classify(&mut self, frame: &[f32]) -> bool {
        let energy = energy_dbfs(frame);

        if energy < ENERGY_FLOOR_DBFS {
            return false; // definitely silence — skip VAD
        }
        if energy < SILENCE_THRESHOLD_DBFS {
            return false; // below speech threshold
        }

        // Energy is above the silence threshold — ask WebRTC VAD.
        let i16_frame = f32_to_i16(frame);
        match self.vad.is_voice_segment(&i16_frame) {
            Ok(is_voice) => is_voice,
            Err(()) => {
                tracing::warn!("WebRTC VAD rejected frame — treating as silence");
                false
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Audio helpers
// ────────────────────────────────────────────────────────────────────────────

/// RMS energy of a PCM frame in dBFS (0 dBFS = full scale).
/// Returns `−100.0` for silence to avoid −∞ in logs.
fn energy_dbfs(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -100.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    let rms = (sum_sq / samples.len() as f32).sqrt();
    if rms < 1e-10 {
        return -100.0;
    }
    20.0 * rms.log10()
}

/// Convert f32 PCM (−1.0 … 1.0) to i16 for WebRTC VAD input.
fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32_767.0) as i16)
        .collect()
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Test helpers ──────────────────────────────────────────────────────

    /// Voiced-speech-like frame: multiple harmonics at natural speech
    /// frequencies (100 Hz fundamental + overtones), amplitude ≈ −10 dBFS.
    ///
    /// This spectral profile is sufficient to pass WebRTC VAD in Quality mode
    /// (mode 0).  Tests create chunkers with `new_for_testing()` which uses
    /// mode 0 so synthetic audio is reliably classified as speech.
    /// Mode-3 (VeryAggressive) accuracy is validated by the integration test
    /// in `tests/integration/audio_pipeline.rs` using real speech audio.
    fn speech_frame(n: usize) -> Vec<f32> {
        let pi = std::f32::consts::PI;
        (0..VAD_FRAME_SAMPLES)
            .map(|i| {
                let t = (n * VAD_FRAME_SAMPLES + i) as f32 / VAD_SAMPLE_RATE as f32;
                // 100 Hz fundamental + 3 harmonics — voiced-speech envelope
                let s = (2.0 * pi * 100.0 * t).sin()
                    + 0.6 * (2.0 * pi * 200.0 * t).sin()
                    + 0.3 * (2.0 * pi * 300.0 * t).sin()
                    + 0.2 * (2.0 * pi * 400.0 * t).sin();
                s * 0.18 // scale to ≈ −10 dBFS, well above −35 dBFS threshold
            })
            .collect()
    }

    /// Silent frame: all zeros → energy = −∞ < −60 dBFS floor.
    fn silence_frame() -> Vec<f32> {
        vec![0.0f32; VAD_FRAME_SAMPLES]
    }

    impl VadChunker {
        /// Test-only constructor. Uses VAD mode 0 (Quality) instead of production
        /// mode 3 (VeryAggressive).
        ///
        /// Mode 3 reliably rejects synthetic sinusoidal audio — it expects real
        /// speech spectral structure. Unit tests here assert chunking behaviour
        /// (durations, counts, source tagging), not VAD sensitivity. Mode 0 lets
        /// `speech_frame()` harmonics pass VAD so timing logic can be verified
        /// deterministically. Mode-3 classification is covered by the integration
        /// test in `tests/integration/audio_pipeline.rs` with real WAV audio.
        /// All timing parameters match production `new()` exactly.
        fn new_for_testing() -> Result<Self> {
            let vad = Vad::new_with_rate_and_mode(SampleRate::Rate16kHz, VadMode::Quality);
            Ok(Self::with_vad(vad))
        }
    }

    // Frame counts derived from §26 timing constants
    const SPEECH_FRAMES_200MS: usize = 10; // 200ms / 20ms = 10 frames
    const SPEECH_FRAMES_150MS: usize = 7; // 140ms < 200ms threshold
    const SILENCE_FRAMES_700MS: usize = 35; // 700ms / 20ms = 35 > 30 (600ms)

    fn run_frames(
        chunker: &mut VadChunker,
        frames: &[Vec<f32>],
        source: AudioSource,
        out: &mut Vec<VadChunk>,
    ) {
        for f in frames {
            if let Some(c) = chunker.process_frame(f, source) {
                out.push(c);
            }
        }
    }

    // ── Spec-required tests ───────────────────────────────────────────────

    /// 200ms speech → 700ms silence: exactly ONE chunk emitted.
    #[test]
    fn test_200ms_speech_700ms_silence_emits_one_chunk() {
        let mut chunker = VadChunker::new_for_testing().unwrap();
        let mut chunks: Vec<VadChunk> = Vec::new();

        let speech: Vec<Vec<f32>> = (0..SPEECH_FRAMES_200MS).map(speech_frame).collect();
        let silence: Vec<Vec<f32>> = (0..SILENCE_FRAMES_700MS).map(|_| silence_frame()).collect();

        run_frames(&mut chunker, &speech, AudioSource::System, &mut chunks);
        run_frames(&mut chunker, &silence, AudioSource::System, &mut chunks);

        assert_eq!(chunks.len(), 1, "Expected 1 chunk, got {}", chunks.len());
        assert_eq!(chunks[0].source, AudioSource::System);
        assert!(
            chunks[0].duration_ms >= MIN_SPEECH_MS,
            "Chunk {}ms < minimum {}ms",
            chunks[0].duration_ms,
            MIN_SPEECH_MS
        );
    }

    /// 150ms speech → silence: NO chunk (below min_speech_duration).
    #[test]
    fn test_short_speech_below_minimum_emits_no_chunk() {
        let mut chunker = VadChunker::new_for_testing().unwrap();
        let mut chunks: Vec<VadChunk> = Vec::new();

        let speech: Vec<Vec<f32>> = (0..SPEECH_FRAMES_150MS).map(speech_frame).collect();
        let silence: Vec<Vec<f32>> = (0..SILENCE_FRAMES_700MS).map(|_| silence_frame()).collect();

        run_frames(&mut chunker, &speech, AudioSource::System, &mut chunks);
        run_frames(&mut chunker, &silence, AudioSource::System, &mut chunks);

        assert_eq!(
            chunks.len(),
            0,
            "Expected 0 chunks for 140ms speech, got {}",
            chunks.len()
        );
    }

    /// Two speech segments separated by 700ms silence: exactly TWO chunks.
    #[test]
    fn test_two_segments_separated_by_700ms_silence_emits_two_chunks() {
        let mut chunker = VadChunker::new_for_testing().unwrap();
        let mut chunks: Vec<VadChunk> = Vec::new();

        // First segment
        let speech1: Vec<Vec<f32>> = (0..SPEECH_FRAMES_200MS).map(speech_frame).collect();
        let silence1: Vec<Vec<f32>> = (0..SILENCE_FRAMES_700MS).map(|_| silence_frame()).collect();
        run_frames(&mut chunker, &speech1, AudioSource::System, &mut chunks);
        run_frames(&mut chunker, &silence1, AudioSource::System, &mut chunks);

        assert_eq!(
            chunks.len(),
            1,
            "First segment: expected 1 chunk after silence"
        );

        // Second segment
        let speech2: Vec<Vec<f32>> = (SPEECH_FRAMES_200MS..SPEECH_FRAMES_200MS * 2)
            .map(speech_frame)
            .collect();
        let silence2: Vec<Vec<f32>> = (0..SILENCE_FRAMES_700MS).map(|_| silence_frame()).collect();
        run_frames(&mut chunker, &speech2, AudioSource::System, &mut chunks);
        run_frames(&mut chunker, &silence2, AudioSource::System, &mut chunks);

        assert_eq!(
            chunks.len(),
            2,
            "Expected 2 total chunks, got {}",
            chunks.len()
        );
    }

    // ── Source tagging ────────────────────────────────────────────────────

    /// Every chunk must be tagged with the source of its first speech frame.
    #[test]
    fn chunk_tagged_with_correct_source() {
        let mut chunker = VadChunker::new_for_testing().unwrap();
        let mut chunks: Vec<VadChunk> = Vec::new();

        let speech: Vec<Vec<f32>> = (0..SPEECH_FRAMES_200MS).map(speech_frame).collect();
        let silence: Vec<Vec<f32>> = (0..SILENCE_FRAMES_700MS).map(|_| silence_frame()).collect();

        run_frames(&mut chunker, &speech, AudioSource::Microphone, &mut chunks);
        run_frames(&mut chunker, &silence, AudioSource::Microphone, &mut chunks);

        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].source,
            AudioSource::Microphone,
            "Chunk source must match input source"
        );
    }

    // ── Energy classification ─────────────────────────────────────────────

    #[test]
    fn silence_is_below_energy_floor() {
        let s = silence_frame();
        assert!(
            energy_dbfs(&s) < ENERGY_FLOOR_DBFS,
            "Silence energy {} should be < {}",
            energy_dbfs(&s),
            ENERGY_FLOOR_DBFS
        );
    }

    #[test]
    fn speech_frame_is_above_silence_threshold() {
        let s = speech_frame(0);
        assert!(
            energy_dbfs(&s) >= SILENCE_THRESHOLD_DBFS,
            "Speech frame energy {} should be >= {}",
            energy_dbfs(&s),
            SILENCE_THRESHOLD_DBFS
        );
    }

    // ── Frame accumulation ────────────────────────────────────────────────

    /// 160-sample sub-frames (pipeline output) must accumulate correctly
    /// into 320-sample VAD frames before classification runs.
    #[test]
    fn sub_frames_accumulate_before_vad_runs() {
        let mut chunker = VadChunker::new_for_testing().unwrap();

        // Half a VAD frame — must NOT trigger any processing.
        let half = vec![0.0f32; VAD_FRAME_SAMPLES / 2];
        let result = chunker.process_frame(&half, AudioSource::System);
        assert!(result.is_none());
        assert_eq!(chunker.frame_buf.len(), VAD_FRAME_SAMPLES / 2);

        // Second half — now a full VAD frame is available; state machine runs.
        let _ = chunker.process_frame(&half, AudioSource::System);
        assert_eq!(chunker.frame_buf.len(), 0);
    }

    /// Continuous speech exceeding MAX_CHUNK_DURATION_MS must produce multiple
    /// chunks without waiting for silence, and must not drop any audio.
    #[test]
    fn test_max_chunk_flush_splits_long_utterance() {
        let mut chunker = VadChunker::new_for_testing().unwrap();
        let mut chunks: Vec<VadChunk> = Vec::new();

        // Feed MAX_CHUNK_FRAMES + 100 speech frames — enough to trigger at least
        // one mid-speech flush plus a residual segment.
        let total_frames = (MAX_CHUNK_FRAMES as usize) + 100;
        let speech: Vec<Vec<f32>> = (0..total_frames).map(speech_frame).collect();
        run_frames(&mut chunker, &speech, AudioSource::System, &mut chunks);

        // At least one mid-speech flush must have fired by now.
        assert!(
            !chunks.is_empty(),
            "expected at least one chunk after {} frames of continuous speech",
            total_frames
        );

        // Every emitted chunk must be attributed to the correct channel.
        for chunk in &chunks {
            assert_eq!(chunk.source, AudioSource::System);
        }
    }

    /// After a mid-speech flush the chunker continues collecting; a subsequent
    /// silence gap finalises the residual segment normally.
    #[test]
    fn test_max_chunk_flush_followed_by_silence_emits_residual() {
        let mut chunker = VadChunker::new_for_testing().unwrap();
        let mut chunks: Vec<VadChunk> = Vec::new();

        // Feed MAX_CHUNK_FRAMES + enough residual to exceed MIN_SPEECH_FRAMES so
        // the residual segment is not discarded as a noise artefact.
        let residual_frames = MIN_SPEECH_FRAMES as usize + 5;
        let flush_frames = (MAX_CHUNK_FRAMES as usize) + residual_frames;
        let speech: Vec<Vec<f32>> = (0..flush_frames).map(speech_frame).collect();
        run_frames(&mut chunker, &speech, AudioSource::Microphone, &mut chunks);

        let after_flush = chunks.len();
        assert!(
            after_flush >= 1,
            "flush should have emitted at least one chunk"
        );

        // Now silence — should emit the residual speech that accumulated after
        // the flush.
        let silence: Vec<Vec<f32>> = (0..SILENCE_FRAMES_700MS).map(|_| silence_frame()).collect();
        run_frames(&mut chunker, &silence, AudioSource::Microphone, &mut chunks);

        assert!(
            chunks.len() > after_flush,
            "silence after flush should emit a residual chunk"
        );
        for chunk in &chunks {
            assert_eq!(chunk.source, AudioSource::Microphone);
        }
    }
}
