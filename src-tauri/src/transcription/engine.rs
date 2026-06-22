//! Whisper.cpp transcription engine — Task 3.5.
//!
//! All decoder parameters are EXACT as specified in §26 and `flint-audio.mdc`.
//! Do not change any constant without updating the design document.
//!
//! ## Pipeline position
//! ```text
//! VadChunker  →  VadChunk (16kHz PCM mono)
//!     WhisperEngine::transcribe()   ← this file
//!     → Some(TranscriptionResult) or None (silence / hallucination)
//! ```
//!
//! ## Security invariant
//! Transcript text MUST NEVER appear in logs at INFO level or above.
//! All per-segment discard reasons log at DEBUG only, and they log
//! probabilities / ratios — never the segment text itself.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use flate2::write::ZlibEncoder;
use flate2::Compression;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::audio::capture::AudioSource;
use crate::audio::vad::VadChunk;
use crate::health::hardware::HardwareTier;

// ────────────────────────────────────────────────────────────────────────────
// Whisper.cpp parameters — §26.  DO NOT change without updating the spec.
// ────────────────────────────────────────────────────────────────────────────

const BEAM_SIZE: i32 = 5;
const TEMPERATURE: f32 = 0.0;
const LANGUAGE: &str = "en";
const NO_SPEECH_THRESHOLD: f32 = 0.6;
const COMPRESSION_RATIO_THRESHOLD: f32 = 2.4;
const LOGPROB_THRESHOLD: f32 = -1.0;
/// §26 fallback — overridden per session via [`WhisperEngine::initial_prompt`].
pub const DEFAULT_INITIAL_PROMPT: &str = "Professional interview conversation.";
/// §26 `max_context: -1` — no context carry-over between VAD chunks.
const MAX_TEXT_CONTEXT_TOKENS: i32 = 0;

// ────────────────────────────────────────────────────────────────────────────
// Public types
// ────────────────────────────────────────────────────────────────────────────

/// Per-word timing extracted from Whisper token timestamps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordTimestamp {
    pub word: String,
    /// Start time in milliseconds relative to the chunk.
    pub start_ms: u32,
    /// End time in milliseconds relative to the chunk.
    pub end_ms: u32,
}

/// Successful transcription of one VAD speech segment.
#[derive(Clone, Debug, PartialEq)]
pub struct TranscriptionResult {
    pub text: String,
    pub source: AudioSource,
    pub word_timestamps: Vec<WordTimestamp>,
    /// Mean `avg_logprob` of surviving segments (for mic quality monitoring).
    pub avg_logprob: Option<f32>,
}

pub struct WhisperEngine {
    model: WhisperContext,
    tier: HardwareTier,
    initial_prompt: Arc<str>,
}

impl WhisperEngine {
    /// Load a Whisper.cpp model from `model_path` and associate it with `tier`.
    ///
    /// Model file selection by tier (§17) is the caller's responsibility —
    /// typically `ggml-{tiny,small,base}.en.bin` from `~/.cache/whisper/`.
    pub fn new(
        model_path: &str,
        tier: HardwareTier,
        initial_prompt: impl Into<Arc<str>>,
    ) -> Result<Self> {
        let path = Path::new(model_path);
        let model = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .with_context(|| format!("Failed to load Whisper model at {}", path.display()))?;

        tracing::info!(
            tier = tier,
            model_path = %path.display(),
            "Whisper engine initialised"
        );

        Ok(Self {
            model,
            tier,
            initial_prompt: initial_prompt.into(),
        })
    }

    /// Session-specific Whisper initial prompt injected on every decode.
    pub fn initial_prompt(&self) -> &str {
        &self.initial_prompt
    }

    /// Transcribe one VAD speech chunk.
    ///
    /// Each Whisper segment within the chunk is evaluated independently:
    /// - Segments with `no_speech_probability > 0.6` are discarded.
    /// - Segments with `compression_ratio > 2.4` are discarded (hallucination).
    /// - Segments with `avg_logprob < -1.0` are discarded (low confidence).
    ///
    /// Surviving segments are joined and returned. `None` only when every
    /// segment in the chunk is discarded.
    ///
    /// Discard events log at DEBUG only — transcript text is never logged.
    pub fn transcribe(&self, chunk: &VadChunk) -> Result<Option<TranscriptionResult>> {
        self.transcribe_with_context(chunk, "")
    }

    /// Transcribe one VAD chunk with additional rolling-context text appended
    /// to the session `initial_prompt`.
    ///
    /// `rolling_context` should be the last ~40 words of the in-progress
    /// transcript for this turn.  Whisper uses this as a soft prior so that
    /// proper nouns established earlier in the answer are more likely to be
    /// recognised correctly in later chunks.
    pub fn transcribe_with_context(
        &self,
        chunk: &VadChunk,
        rolling_context: &str,
    ) -> Result<Option<TranscriptionResult>> {
        if chunk.samples.is_empty() {
            return Ok(None);
        }

        let prompt = build_context_prompt(&self.initial_prompt, rolling_context);
        let beam = self.decode_chunk_with_prompt(chunk, false, &prompt)?;
        if beam.is_some() {
            return Ok(beam);
        }

        tracing::debug!(
            source = ?chunk.source,
            "beam search produced no valid segments — retrying with greedy decode"
        );
        self.decode_chunk_with_prompt(chunk, true, &prompt)
    }

    /// Greedy-only transcription — skips beam search and the single-timestamp
    /// guard.  Used for calibration where audio windows are pre-sized and
    /// beam search's timestamp heuristics are unreliable.
    pub fn transcribe_greedy(&self, chunk: &VadChunk) -> Result<Option<TranscriptionResult>> {
        if chunk.samples.is_empty() {
            return Ok(None);
        }
        self.decode_chunk_with_prompt(chunk, true, &self.initial_prompt.clone())
    }

    fn decode_chunk_with_prompt(
        &self,
        chunk: &VadChunk,
        greedy: bool,
        prompt: &str,
    ) -> Result<Option<TranscriptionResult>> {
        let mut state = self
            .model
            .create_state()
            .context("Failed to create Whisper state")?;

        let mut params = if greedy {
            build_greedy_params(prompt)
        } else {
            build_full_params(prompt)
        };
        params.set_n_threads(thread_count_for_tier(self.tier));

        state
            .full(params, &chunk.samples)
            .context("Whisper inference failed")?;

        let n_segments = state.full_n_segments();
        if n_segments <= 0 {
            tracing::debug!(source = ?chunk.source, greedy, "Whisper produced no segments");
            return Ok(None);
        }

        if !greedy && is_single_timestamp_ending(&state) {
            tracing::debug!(
                source = ?chunk.source,
                "beam search single timestamp ending — will retry greedy"
            );
            return Ok(None);
        }

        collect_segments(&state, chunk.source, n_segments)
    }

    /// Hardware tier this engine was configured for.
    pub fn tier(&self) -> HardwareTier {
        self.tier
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Parameter builder
// ────────────────────────────────────────────────────────────────────────────

/// Build `FullParams` with exact §26 Whisper.cpp configuration (beam search).
fn build_full_params(initial_prompt: &str) -> FullParams<'static, 'static> {
    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: BEAM_SIZE,
        patience: -1.0,
    });

    apply_common_params(&mut params, initial_prompt);
    params
}

/// Greedy decode fallback when beam search fails (M10 Slice 5).
fn build_greedy_params(initial_prompt: &str) -> FullParams<'static, 'static> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    apply_common_params(&mut params, initial_prompt);
    params
}

fn apply_common_params(params: &mut FullParams<'_, '_>, initial_prompt: &str) {
    params.set_language(Some(LANGUAGE));
    params.set_temperature(TEMPERATURE);
    params.set_no_speech_thold(NO_SPEECH_THRESHOLD);
    params.set_entropy_thold(COMPRESSION_RATIO_THRESHOLD);
    params.set_logprob_thold(LOGPROB_THRESHOLD);
    params.set_token_timestamps(true);
    params.set_initial_prompt(initial_prompt);
    params.set_no_context(true);
    params.set_n_max_text_ctx(MAX_TEXT_CONTEXT_TOKENS);

    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
}

fn collect_segments(
    state: &whisper_rs::WhisperState,
    source: AudioSource,
    n_segments: i32,
) -> Result<Option<TranscriptionResult>> {
    let mut text_parts: Vec<String> = Vec::new();
    let mut word_timestamps: Vec<WordTimestamp> = Vec::new();
    let mut discarded: u32 = 0;
    let mut logprob_sum: f32 = 0.0;
    let mut logprob_count: u32 = 0;

    for segment in state.as_iter() {
        let no_speech_prob = segment.no_speech_probability();
        if no_speech_prob > NO_SPEECH_THRESHOLD {
            tracing::debug!(
                source = ?source,
                no_speech_prob = no_speech_prob,
                "Whisper segment discarded — no speech"
            );
            discarded += 1;
            continue;
        }

        let segment_text = segment
            .to_str()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        if segment_text.is_empty() {
            discarded += 1;
            continue;
        }

        let ratio = compression_ratio(&segment_text);
        if ratio > COMPRESSION_RATIO_THRESHOLD {
            tracing::debug!(
                source = ?source,
                compression_ratio = ratio,
                "Whisper segment discarded — repetition hallucination"
            );
            discarded += 1;
            continue;
        }

        let mut seg_logprob_sum: f32 = 0.0;
        let mut seg_logprob_count: u32 = 0;
        let mut seg_words: Vec<WordTimestamp> = Vec::new();

        let n_tokens = segment.n_tokens();
        for token_idx in 0..n_tokens {
            let Some(token) = segment.get_token(token_idx) else {
                continue;
            };

            let word = token.to_str_lossy().unwrap_or_default().into_owned();
            let word = word.trim().to_string();
            if word.is_empty() || is_special_token(&word) {
                continue;
            }

            let data = token.token_data();
            seg_logprob_sum += data.plog;
            seg_logprob_count += 1;

            let start_ms = centiseconds_to_ms(data.t0);
            let end_ms = centiseconds_to_ms(data.t1).max(start_ms);
            seg_words.push(WordTimestamp {
                word,
                start_ms,
                end_ms,
            });
        }

        if seg_logprob_count > 0 {
            let avg_logprob = seg_logprob_sum / seg_logprob_count as f32;
            if avg_logprob < LOGPROB_THRESHOLD {
                tracing::debug!(
                    source = ?source,
                    avg_logprob = avg_logprob,
                    "Whisper segment discarded — low confidence"
                );
                discarded += 1;
                continue;
            }
            logprob_sum += avg_logprob;
            logprob_count += 1;
        }

        text_parts.push(segment_text);
        word_timestamps.extend(seg_words);
    }

    let total = n_segments as u32;
    if text_parts.is_empty() {
        tracing::debug!(
            source = ?source,
            discarded = discarded,
            total = total,
            "All Whisper segments discarded"
        );
        return Ok(None);
    }

    if discarded > 0 {
        tracing::debug!(
            source = ?source,
            discarded = discarded,
            total = total,
            "Some Whisper segments discarded — partial transcript kept"
        );
    }

    let text = text_parts.join(" ").trim().to_string();
    let avg_logprob = if logprob_count > 0 {
        Some(logprob_sum / logprob_count as f32)
    } else {
        None
    };
    Ok(Some(TranscriptionResult {
        text,
        source,
        word_timestamps,
        avg_logprob,
    }))
}

/// True when every token in every segment shares the same end timestamp.
fn is_single_timestamp_ending(state: &whisper_rs::WhisperState) -> bool {
    let mut saw_word = false;
    let mut single_end: Option<i64> = None;

    for segment in state.as_iter() {
        for token_idx in 0..segment.n_tokens() {
            let Some(token) = segment.get_token(token_idx) else {
                continue;
            };
            let word = token.to_str_lossy().unwrap_or_default();
            let word = word.trim();
            if word.is_empty() || is_special_token(word) {
                continue;
            }
            saw_word = true;
            let end = token.token_data().t1;
            match single_end {
                None => single_end = Some(end),
                Some(prev) if prev == end => {}
                _ => return false,
            }
        }
    }

    saw_word && single_end.is_some()
}

// ────────────────────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────────────────────

/// Combine the static session `initial_prompt` with the last ≤40 words of the
/// in-progress transcript so that proper nouns established in earlier chunks
/// are available as a soft prior for the current chunk.
///
/// Total length is capped at 400 characters — well within Whisper's token
/// budget for `initial_prompt`.
fn build_context_prompt(session_prompt: &str, rolling_context: &str) -> String {
    let tail = rolling_context.trim();
    if tail.is_empty() {
        return session_prompt.to_string();
    }
    // Keep last 40 words of rolling context to avoid prompt bloat.
    let words: Vec<&str> = tail.split_whitespace().collect();
    let keep_from = words.len().saturating_sub(40);
    let tail_trimmed = words[keep_from..].join(" ");

    let combined = format!("{session_prompt} | {tail_trimmed}");
    if combined.len() <= 400 {
        combined
    } else {
        combined[..400].to_string()
    }
}

fn thread_count_for_tier(tier: HardwareTier) -> i32 {
    // On macOS with Core ML enabled, whisper.cpp delegates inference to the
    // ANE/GPU and ignores n_threads. The value set here only affects CPU-bound
    // paths (Linux, Windows, and macOS without Core ML).
    let hw = std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(4);
    match tier {
        1 => 2,
        2 => hw.min(4),
        _ => hw.min(4),
    }
}

/// OpenAI-compatible per-segment compression ratio: `len(text) / len(zlib(text))`.
///
/// Mirrors the hallucination filter in `openai/whisper` `transcribe.py`.
/// Applied per segment, not per chunk, so a repetitive segment cannot cause
/// valid adjacent segments to be discarded.
fn compression_ratio(text: &str) -> f32 {
    let text_bytes = text.as_bytes();
    if text_bytes.is_empty() {
        return 0.0;
    }

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    if encoder.write_all(text_bytes).is_err() {
        return 0.0;
    }
    let compressed = encoder.finish().unwrap_or_default();
    let compressed_len = compressed.len().max(1) as f32;
    text_bytes.len() as f32 / compressed_len
}

fn centiseconds_to_ms(cs: i64) -> u32 {
    cs.max(0) as u32 * 10
}

fn is_special_token(token: &str) -> bool {
    token.starts_with('[') || token.starts_with('<') || token.starts_with('_')
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Compression ratio ─────────────────────────────────────────────────

    #[test]
    fn compression_ratio_normal_text_below_threshold() {
        let ratio = compression_ratio("Tell me about your experience with Rust.");
        assert!(
            ratio < COMPRESSION_RATIO_THRESHOLD,
            "Normal text ratio {ratio} should be < {COMPRESSION_RATIO_THRESHOLD}"
        );
    }

    #[test]
    fn compression_ratio_repetitive_text_above_threshold() {
        let repetitive = "the the the the the the the the the the the the the the the \
                          the the the the the the the the the the the the the the the \
                          the the the the the the the the the the the the the the the";
        let ratio = compression_ratio(repetitive);
        assert!(
            ratio > COMPRESSION_RATIO_THRESHOLD,
            "Repetitive text ratio {ratio} should be > {COMPRESSION_RATIO_THRESHOLD}"
        );
    }

    #[test]
    fn compression_ratio_empty_text_is_zero() {
        assert_eq!(compression_ratio(""), 0.0);
    }

    // ── Per-segment filter contract ───────────────────────────────────────

    /// Verify that a normal sentence and a repetitive sentence are classified
    /// correctly by `compression_ratio` independently — the contract the
    /// per-segment loop relies on.
    #[test]
    fn per_segment_filter_does_not_cross_contaminate() {
        let normal = "Tell me about a time you led a project.";
        let repetitive = "and and and and and and and and and and and and and and and \
                          and and and and and and and and and and and and and and and \
                          and and and and and and and and and and and and and and and";

        let r_normal = compression_ratio(normal);
        let r_repetitive = compression_ratio(repetitive);

        assert!(
            r_normal < COMPRESSION_RATIO_THRESHOLD,
            "Normal segment should pass: {r_normal}"
        );
        assert!(
            r_repetitive > COMPRESSION_RATIO_THRESHOLD,
            "Repetitive segment should fail: {r_repetitive}"
        );
    }

    // ── Parameter builder ─────────────────────────────────────────────────

    #[test]
    fn build_full_params_does_not_panic() {
        let params = build_full_params(DEFAULT_INITIAL_PROMPT);
        drop(params);
    }

    #[test]
    fn build_greedy_params_does_not_panic() {
        let params = build_greedy_params(DEFAULT_INITIAL_PROMPT);
        drop(params);
    }

    #[test]
    fn single_timestamp_ending_detects_collapsed_timestamps() {
        assert!(is_single_timestamp_ending_mock(&[(100, 100), (100, 100)]));
        assert!(!is_single_timestamp_ending_mock(&[(100, 150), (100, 200)]));
    }

    fn is_single_timestamp_ending_mock(ends: &[(i64, i64)]) -> bool {
        let mut single_end: Option<i64> = None;
        for &(_start, end) in ends {
            match single_end {
                None => single_end = Some(end),
                Some(prev) if prev == end => {}
                _ => return false,
            }
        }
        single_end.is_some()
    }

    // ── Utility functions ─────────────────────────────────────────────────

    #[test]
    fn special_tokens_are_filtered() {
        assert!(is_special_token("[_BEG_]"));
        assert!(is_special_token("<|endoftext|>"));
        assert!(!is_special_token("hello"));
        assert!(!is_special_token("world"));
    }

    #[test]
    fn centiseconds_convert_to_milliseconds() {
        assert_eq!(centiseconds_to_ms(150), 1500);
        assert_eq!(centiseconds_to_ms(0), 0);
        assert_eq!(centiseconds_to_ms(-5), 0);
    }

    // ── Rolling context prompt ────────────────────────────────────────────

    #[test]
    fn build_context_prompt_empty_rolling_returns_session_prompt() {
        let result = build_context_prompt("IAM interview at Fisher.", "");
        assert_eq!(result, "IAM interview at Fisher.");
    }

    #[test]
    fn build_context_prompt_appends_tail_with_separator() {
        let result = build_context_prompt("IAM interview.", "fiduciary fee-only");
        assert!(result.contains("IAM interview."));
        assert!(result.contains("fiduciary fee-only"));
        assert!(result.contains(" | "));
    }

    #[test]
    fn build_context_prompt_trims_to_last_40_words() {
        let many_words = (0..60)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let result = build_context_prompt("prompt.", &many_words);
        let tail_part = result.split(" | ").nth(1).unwrap_or("");
        assert!(tail_part.split_whitespace().count() <= 40);
    }

    #[test]
    fn build_context_prompt_caps_at_400_chars() {
        let long = "a".repeat(350);
        let result = build_context_prompt(&long, &long);
        assert!(result.len() <= 400);
    }
}
