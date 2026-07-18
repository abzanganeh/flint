//! Phone-mode provisional speaker heuristic — Slice 2 (`lpav-s2-phone-heuristic`).
//!
//! Phone-call mode collapses both speakers onto a single mixed audio channel
//! (see [`crate::audio::capture::AudioCapture::start_phone_mode`]), so there
//! is no channel-based speaker proxy the way dual-stream loopback+mic
//! sessions have. This module gives every phone-mode chunk a *provisional*
//! `interviewer | candidate` label using two cheap, fully offline signals:
//!
//! - **Pause-before-utterance**: turn-taking in conversation is dominated by
//!   silence gaps. A long pause before a chunk starts strongly suggests the
//!   previous speaker stopped and the other one started; a short pause (or
//!   none) suggests the same speaker is still talking — most commonly a VAD
//!   segment boundary in the middle of one long answer.
//! - **RMS energy**: a secondary tie-breaker used only in the ambiguous
//!   middle band where the pause is too short to firmly conclude "same
//!   speaker" but too long to confidently conclude "turn changed" either.
//!
//! This label is provisional and cheap — it runs on every phone-mode chunk
//! with zero LLM calls. [`crate::transcription::speaker_suspicion`] (text
//! shape) and [`crate::audio::speaker_classifier`] (Tier-2/3 LLM, Slice 4)
//! can both override it with a more confident verdict.

use std::time::Instant;

use crate::audio::diarizer::SpeakerRole;

/// Below this pause, the chunk is almost certainly a VAD split of the same
/// utterance — same speaker as `prior`.
const CONTINUATION_PAUSE_MS: u64 = 300;

/// At or above this pause, a turn change is likely — flip from `prior`.
const TURN_TAKING_PAUSE_MS: u64 = 900;

/// RMS threshold (dBFS) used only in the ambiguous pause band
/// (`CONTINUATION_PAUSE_MS..TURN_TAKING_PAUSE_MS`). Chunks at or above this
/// level lean toward the candidate speaking directly into the capture
/// device; quieter chunks lean toward the interviewer's phone-line audio,
/// which typically arrives attenuated relative to direct capture. This is a
/// weak, tie-breaking-only signal — never used outside the ambiguous band.
const RMS_CANDIDATE_LEAN_DBFS: f32 = -20.0;

/// Classify one phone-mode utterance as interviewer or candidate.
///
/// `rms_dbfs` is the RMS energy of the utterance (see [`rms_dbfs`]).
/// `pause_before_ms` is the silence duration immediately preceding the
/// utterance's first speech frame. `prior` is the last role assigned to the
/// previous utterance in this session — [`SpeakerRole::Unknown`] only on the
/// very first utterance, when there is no history to anchor a turn-taking
/// decision on.
///
/// Interviews conventionally open with the interviewer (greeting or first
/// question), so the very first utterance defaults to
/// [`SpeakerRole::Interviewer`] regardless of signal values; Tier 2/3 correct
/// this quickly if the guess is wrong.
pub fn classify_phone_utterance(
    rms_dbfs: f32,
    pause_before_ms: u64,
    prior: SpeakerRole,
) -> SpeakerRole {
    if prior == SpeakerRole::Unknown {
        return SpeakerRole::Interviewer;
    }
    if pause_before_ms < CONTINUATION_PAUSE_MS {
        return prior;
    }
    if pause_before_ms >= TURN_TAKING_PAUSE_MS {
        return flip(prior);
    }
    if rms_dbfs >= RMS_CANDIDATE_LEAN_DBFS {
        SpeakerRole::User
    } else {
        SpeakerRole::Interviewer
    }
}

fn flip(role: SpeakerRole) -> SpeakerRole {
    match role {
        SpeakerRole::Interviewer => SpeakerRole::User,
        SpeakerRole::User => SpeakerRole::Interviewer,
        SpeakerRole::Unknown => SpeakerRole::Unknown,
    }
}

/// RMS energy of a PCM buffer in dBFS (0 dBFS = full scale).
///
/// Mirrors the per-frame `energy_dbfs` helper in [`crate::audio::vad`], but
/// operates on a full utterance buffer (the whole [`crate::audio::vad::VadChunk`])
/// rather than a single 20ms VAD frame — the heuristic needs one energy
/// figure per utterance, not per frame. Returns `-100.0` for an empty buffer
/// to avoid `-inf` in logs.
pub fn rms_dbfs(samples: &[f32]) -> f32 {
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

/// Tracks turn-taking state across an entire phone-mode session so
/// [`classify_phone_utterance`] has a real `prior` role and a real pause
/// duration to reason about, instead of being called with synthetic state
/// on every chunk.
///
/// One instance lives for the lifetime of a phone-mode `run_audio_pipeline`
/// call — see the `phone_heuristic` state threaded through
/// [`crate::audio::pipeline::process_frame`].
pub struct PhoneHeuristicState {
    last_chunk_end_at: Option<Instant>,
    last_role: SpeakerRole,
}

impl Default for PhoneHeuristicState {
    fn default() -> Self {
        Self {
            last_chunk_end_at: None,
            last_role: SpeakerRole::Unknown,
        }
    }
}

impl PhoneHeuristicState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Classify the utterance that just finished VAD chunking and advance
    /// the internal turn-taking state.
    ///
    /// `chunk_duration_ms` is the speech-content duration of the
    /// just-completed segment (`VadChunk::duration_ms`). `now` should be
    /// sampled immediately after VAD emits the chunk (before the
    /// potentially slow Whisper transcription step), so the pause estimate
    /// is not inflated by inference latency.
    ///
    /// The pause before the utterance is derived as
    /// `elapsed_since_last_chunk_end - chunk_duration_ms`, floored at zero:
    /// the two chunk-end timestamps bracket `[pause] + [this utterance's
    /// duration]`, and the duration is already known, so subtracting it
    /// recovers the pause without needing to track each utterance's start
    /// time separately.
    pub fn observe(&mut self, rms_dbfs_value: f32, chunk_duration_ms: u32, now: Instant) -> SpeakerRole {
        let pause_before_ms = self
            .last_chunk_end_at
            .map(|prev| {
                let elapsed_ms = now.duration_since(prev).as_millis() as u64;
                elapsed_ms.saturating_sub(chunk_duration_ms as u64)
            })
            .unwrap_or(u64::MAX);

        let role = classify_phone_utterance(rms_dbfs_value, pause_before_ms, self.last_role);
        self.last_role = role;
        self.last_chunk_end_at = Some(now);
        role
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // ── classify_phone_utterance ────────────────────────────────────────────

    #[test]
    fn first_utterance_defaults_to_interviewer_regardless_of_signals() {
        assert_eq!(
            classify_phone_utterance(-40.0, 0, SpeakerRole::Unknown),
            SpeakerRole::Interviewer
        );
        assert_eq!(
            classify_phone_utterance(-5.0, 10_000, SpeakerRole::Unknown),
            SpeakerRole::Interviewer
        );
    }

    #[test]
    fn short_pause_continues_prior_speaker() {
        assert_eq!(
            classify_phone_utterance(-30.0, 100, SpeakerRole::User),
            SpeakerRole::User
        );
        assert_eq!(
            classify_phone_utterance(-5.0, 299, SpeakerRole::Interviewer),
            SpeakerRole::Interviewer
        );
    }

    #[test]
    fn long_pause_flips_speaker() {
        assert_eq!(
            classify_phone_utterance(-30.0, 900, SpeakerRole::Interviewer),
            SpeakerRole::User
        );
        assert_eq!(
            classify_phone_utterance(-30.0, 5_000, SpeakerRole::User),
            SpeakerRole::Interviewer
        );
    }

    #[test]
    fn ambiguous_band_leans_on_rms() {
        // Pause in [300, 900) ms — RMS breaks the tie.
        assert_eq!(
            classify_phone_utterance(-10.0, 500, SpeakerRole::Interviewer),
            SpeakerRole::User,
            "loud mid-band utterance leans candidate"
        );
        assert_eq!(
            classify_phone_utterance(-35.0, 500, SpeakerRole::User),
            SpeakerRole::Interviewer,
            "quiet mid-band utterance leans interviewer"
        );
    }

    #[test]
    fn ambiguous_band_boundary_is_inclusive_toward_candidate() {
        assert_eq!(
            classify_phone_utterance(-20.0, 600, SpeakerRole::Interviewer),
            SpeakerRole::User
        );
    }

    #[test]
    fn flip_is_symmetric_and_leaves_unknown_alone() {
        assert_eq!(flip(SpeakerRole::Interviewer), SpeakerRole::User);
        assert_eq!(flip(SpeakerRole::User), SpeakerRole::Interviewer);
        assert_eq!(flip(SpeakerRole::Unknown), SpeakerRole::Unknown);
    }

    // ── rms_dbfs ─────────────────────────────────────────────────────────────

    #[test]
    fn rms_dbfs_empty_buffer_is_floor() {
        assert_eq!(rms_dbfs(&[]), -100.0);
    }

    #[test]
    fn rms_dbfs_silence_is_floor() {
        assert_eq!(rms_dbfs(&[0.0; 320]), -100.0);
    }

    #[test]
    fn rms_dbfs_louder_signal_is_higher() {
        let quiet = vec![0.01f32; 320];
        let loud = vec![0.5f32; 320];
        assert!(rms_dbfs(&loud) > rms_dbfs(&quiet));
    }

    #[test]
    fn rms_dbfs_full_scale_is_near_zero() {
        let full_scale = vec![1.0f32; 320];
        assert!(rms_dbfs(&full_scale) > -1.0);
    }

    // ── PhoneHeuristicState — pipeline-shaped sequence test ────────────────
    //
    // Simulates the sequence of calls `process_frame` makes as VAD chunks
    // arrive in phone mode: interviewer opens, candidate answers after a
    // real pause, interviewer follows up quickly split across two VAD
    // segments (short pause -> must not flip mid-answer).

    #[test]
    fn state_sequence_first_utterance_is_interviewer() {
        let mut state = PhoneHeuristicState::new();
        let now = Instant::now();
        let role = state.observe(-25.0, 1_500, now);
        assert_eq!(role, SpeakerRole::Interviewer);
    }

    #[test]
    fn state_sequence_flips_after_real_pause_then_holds_through_split_segment() {
        let mut state = PhoneHeuristicState::new();
        let t0 = Instant::now();

        // Interviewer's opening question, 1.5s long.
        let role0 = state.observe(-25.0, 1_500, t0);
        assert_eq!(role0, SpeakerRole::Interviewer);

        // Candidate starts answering 1.2s after the question ended.
        let t1 = t0 + Duration::from_millis(1_500 + 1_200);
        let role1 = state.observe(-15.0, 2_000, t1);
        assert_eq!(role1, SpeakerRole::User, "long pause must flip to candidate");

        // Whisper's VAD splits the candidate's long answer into a second
        // chunk with almost no gap (100ms) — must NOT flip back.
        let t2 = t1 + Duration::from_millis(2_000 + 100);
        let role2 = state.observe(-15.0, 3_000, t2);
        assert_eq!(
            role2,
            SpeakerRole::User,
            "short intra-answer pause must hold the same speaker"
        );
    }

    #[test]
    fn state_sequence_missing_history_treated_as_infinite_pause() {
        let mut state = PhoneHeuristicState::new();
        // First call always resolves via the Unknown-prior branch, so the
        // synthetic u64::MAX pause is never actually exercised by
        // classify_phone_utterance — assert the resulting role anyway to
        // pin the observable behaviour.
        assert_eq!(state.observe(-50.0, 100, Instant::now()), SpeakerRole::Interviewer);
    }
}
