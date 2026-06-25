//! Mock-interview transcript sanitiser.
//!
//! The actual cleanup logic lives in [`crate::transcription::sanitizer`] so
//! the Live audio pipeline can reuse it. This module preserves the historical
//! mock-only API surface as a thin wrapper.

pub use crate::transcription::sanitizer::text_has_profanity;
use crate::transcription::sanitizer::{sanitize_with_script_hint, validate_segment};

/// Sanitize a mock turn transcript using optional script context.
///
/// Mock turns have a known suggested answer ("script hint") so deliberate
/// profanity in the script is preserved and only hallucinated expletives are
/// stripped.
pub fn sanitize_mock_transcript(transcript: &str, script_hint: Option<&str>) -> String {
    sanitize_with_script_hint(transcript, script_hint)
}

/// Re-exported so callers that already imported it from `mock::transcript`
/// keep working after the move.
pub fn segment_is_plausible(text: &str, duration_ms: u32) -> bool {
    validate_segment(text, duration_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repairs_specifically_profanity_hallucination() {
        let raw = "A few things stood out about Fisher. specific. F*CK! Fishers take fiduciary";
        let script = "A few things stood out about Fisher specifically.";
        let out = sanitize_mock_transcript(raw, Some(script));
        assert!(!text_has_profanity(&out));
        assert!(out.contains("specifically"));
        assert!(!out.contains("F*CK"));
    }

    #[test]
    fn keeps_profanity_when_script_contains_it() {
        let raw = "That was a fucking disaster";
        let script = "That was a fucking disaster in prod";
        let out = sanitize_mock_transcript(raw, Some(script));
        assert!(text_has_profanity(&out));
    }

    #[test]
    fn drops_isolated_profanity_without_script_match() {
        let raw = "We shipped on time fuck yeah";
        let out = sanitize_mock_transcript(raw, None);
        assert!(!text_has_profanity(&out));
        assert!(out.contains("shipped"));
    }
}
