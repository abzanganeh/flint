//! Mic and system audio calibration scoring.

mod device;
mod metrics;
mod runner;
mod text;

pub use device::{device_fingerprint, device_fingerprint_or_fallback};
pub use metrics::log_audio_quality_calibration;
pub use runner::{transcribe_mic_calibration, transcribe_system_calibration};
pub use text::{
    calibration_whisper_prompt, load_mic_paragraph_text, load_system_clip_text,
    MIC_WER_PASS_THRESHOLD, SYSTEM_WER_PASS_THRESHOLD,
};

use crate::transcription::wer::{word_error_rate, word_recall};

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

/// Mic calibration scoring — same WER gate as system audio, but also accepts
/// strong word recall when minor wording differences drive WER up despite clear audio.
pub fn score_mic_calibration(reference: &str, transcript: &str) -> CalibrationScore {
    let wer = word_error_rate(reference, transcript);
    let recall = word_recall(reference, transcript);
    let passed =
        wer < MIC_WER_PASS_THRESHOLD || (recall >= 0.70 && wer < MIC_WER_PASS_THRESHOLD * 2.0);
    CalibrationScore {
        wer,
        passed,
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
        let hypothesis =
            "In my last role I led a team to ship a feature under a deadline with milestones.";
        let score = score_mic_calibration(&reference, hypothesis);
        assert!(!score.passed || score.wer < MIC_WER_PASS_THRESHOLD * 2.0);
    }

    #[test]
    fn mic_calibration_passes_with_high_recall_despite_wer() {
        let reference = load_mic_paragraph_text();
        // Missing opening clause but most content present — typical partial-read recovery.
        let hypothesis =
            "I led a cross functional team to ship a customer facing feature under a six week \
            deadline. We started by writing clear requirements and breaking the work into weekly \
            milestones. When we hit a blocker in testing I coordinated with QA and design to \
            adjust scope without missing the launch date. The feature went live on schedule and \
            improved user retention.";
        let score = score_mic_calibration(&reference, hypothesis);
        assert!(
            score.passed,
            "expected pass via recall gate, wer={}",
            score.wer
        );
    }
}
