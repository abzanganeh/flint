//! Per-session question attempt tracking — skip re-asking when the user
//! already got a strong answer (rehearsal confidence or mock coach score).

use crate::confidence::ConfidenceLevel;
use crate::interfaces::vector::QA_EMBED_CONFIDENCE_THRESHOLD;

/// Mock coach score (0–100) at or above which a turn counts as practiced well.
pub const MOCK_COACH_SATISFIED_THRESHOLD: u8 = 70;

/// Normalised key for deduplicating question text within a session.
pub fn normalize_question_key(question: &str) -> String {
    strip_rephrase_prefix(question)
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Rehearsal rephrase turns still map to the original bank question.
pub fn strip_rephrase_prefix(question: &str) -> &str {
    const PREFIX: &str = "Rephrase your previous answer to: ";
    question.strip_prefix(PREFIX).unwrap_or(question)
}

/// Rehearsal turn counts as satisfied when confidence is green/blue at embed threshold.
pub fn rehearsal_attempt_satisfied(score: f32, level: ConfidenceLevel) -> bool {
    if matches!(
        level,
        ConfidenceLevel::Grey | ConfidenceLevel::Red | ConfidenceLevel::AmberLow
    ) {
        return false;
    }
    score >= QA_EMBED_CONFIDENCE_THRESHOLD
        && matches!(level, ConfidenceLevel::Green | ConfidenceLevel::Blue)
}

/// Mock turn counts as satisfied when answered (not skipped) with a strong coach score.
pub fn mock_attempt_satisfied(coach_score: u8, skipped: bool) -> bool {
    !skipped && coach_score >= MOCK_COACH_SATISFIED_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_whitespace_and_case() {
        assert_eq!(
            normalize_question_key("  Tell   Me   About Yourself  "),
            "tell me about yourself"
        );
    }

    #[test]
    fn rephrase_prefix_stripped_for_key() {
        assert_eq!(
            normalize_question_key("Rephrase your previous answer to: Why this role?"),
            "why this role?"
        );
    }

    #[test]
    fn rehearsal_green_is_satisfied() {
        assert!(rehearsal_attempt_satisfied(0.8, ConfidenceLevel::Green));
    }

    #[test]
    fn embed_confidence_threshold_boundary() {
        assert!(
            !rehearsal_attempt_satisfied(0.64, ConfidenceLevel::Green),
            "0.64 must not qualify for Q&A embedding"
        );
        assert!(
            rehearsal_attempt_satisfied(0.65, ConfidenceLevel::Green),
            "0.65 must qualify for Q&A embedding"
        );
    }

    #[test]
    fn rehearsal_amber_not_satisfied() {
        assert!(!rehearsal_attempt_satisfied(0.5, ConfidenceLevel::Amber));
    }

    #[test]
    fn mock_skip_not_satisfied() {
        assert!(!mock_attempt_satisfied(90, true));
    }

    #[test]
    fn mock_low_score_not_satisfied() {
        assert!(!mock_attempt_satisfied(50, false));
    }
}
