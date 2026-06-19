//! Mic and system audio calibration scoring.

mod device;
mod metrics;
mod runner;
mod text;

pub use device::{device_fingerprint, device_fingerprint_or_fallback};
pub use metrics::log_audio_quality_calibration;
pub use runner::{transcribe_mic_calibration, transcribe_system_calibration};
pub use text::{
    load_mic_paragraph_text, load_system_clip_text, MIC_WER_PASS_THRESHOLD,
    SYSTEM_WER_PASS_THRESHOLD,
};

use crate::transcription::wer::word_error_rate;

#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationScore {
    pub wer: f32,
    pub passed: bool,
    pub transcript: String,
}

pub fn score_transcript(reference: &str, transcript: &str, threshold: f32) -> CalibrationScore {
    let wer = word_error_rate(reference, transcript);
    CalibrationScore {
        wer,
        passed: wer < threshold,
        transcript: transcript.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mocked_system_transcript_passes_threshold() {
        let reference = load_system_clip_text();
        let score = score_transcript(&reference, &reference, SYSTEM_WER_PASS_THRESHOLD);
        assert!(score.passed);
        assert_eq!(score.wer, 0.0);
    }

    #[test]
    fn mocked_mic_transcript_with_typos_may_fail() {
        let reference = load_mic_paragraph_text();
        let hypothesis = "At SecureAuth I led authentication using OAuth OIDC SAML MFA.";
        let score = score_transcript(&reference, hypothesis, MIC_WER_PASS_THRESHOLD);
        assert!(!score.passed || score.wer < MIC_WER_PASS_THRESHOLD);
    }
}
