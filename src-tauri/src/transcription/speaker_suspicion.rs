//! Non-phone-mode speaker-label suspicion detector.
//!
//! The cpal channel that captured a chunk is normally a perfect proxy for who
//! spoke (System = interviewer loopback, Microphone = user). It breaks down
//! when:
//!
//! - The user is on laptop speakers and the mic catches the interviewer's
//!   voice — a question-shaped sentence ends up tagged as `Microphone`.
//! - The loopback is misconfigured and records the user's mic — a
//!   first-person statement ends up tagged as `System`.
//!
//! This module runs cheap, offline regex-based heuristics (no LLM) over the
//! transcript text and channel label to flag the chunk for the UI. The user
//! confirms or fixes the label via `relabel_transcript_chunk`.

use std::sync::OnceLock;

use regex::Regex;

/// Heuristic verdict from [`evaluate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuspicionVerdict {
    pub suggested_speaker: String,
    pub reason: SuspicionReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuspicionReason {
    /// Question-shaped sentence appeared on the Microphone channel.
    QuestionShapeOnMic,
    /// First-person statement appeared on the System channel.
    FirstPersonOnSystem,
}

impl SuspicionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SuspicionReason::QuestionShapeOnMic => "question_shape_on_mic",
            SuspicionReason::FirstPersonOnSystem => "first_person_on_system",
        }
    }
}

/// Minimum number of words required before a chunk is even considered for
/// suspicion. Below this threshold, false positives dominate (single-word
/// "huh?" on mic, "I" alone on system, etc.).
const MIN_WORDS: usize = 4;

/// Run the suspicion check against a chunk that was just persisted with a
/// channel-derived speaker label. Returns `None` when the label looks
/// consistent with the text shape.
pub fn evaluate(speaker: &str, text: &str) -> Option<SuspicionVerdict> {
    let trimmed = text.trim();
    if trimmed.split_whitespace().count() < MIN_WORDS {
        return None;
    }

    match speaker {
        "Microphone" => {
            if looks_like_question(trimmed) {
                Some(SuspicionVerdict {
                    suggested_speaker: "System".into(),
                    reason: SuspicionReason::QuestionShapeOnMic,
                })
            } else {
                None
            }
        }
        "System" => {
            if looks_like_first_person_statement(trimmed) {
                Some(SuspicionVerdict {
                    suggested_speaker: "Microphone".into(),
                    reason: SuspicionReason::FirstPersonOnSystem,
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

fn looks_like_question(text: &str) -> bool {
    let lower = text.to_lowercase();

    if text.trim_end().ends_with('?') {
        return true;
    }

    static INTERROGATIVE: OnceLock<Regex> = OnceLock::new();
    let re = INTERROGATIVE.get_or_init(|| {
        Regex::new(
            r"^\s*(tell me|can you|could you|would you|why|what|when|where|how|do you|did you|have you|are you|will you|describe|walk me through|talk me through|explain)\b",
        )
        .expect("interrogative regex compiles")
    });
    re.is_match(&lower)
}

fn looks_like_first_person_statement(text: &str) -> bool {
    let lower = text.to_lowercase();

    static FIRST_PERSON: OnceLock<Regex> = OnceLock::new();
    let re = FIRST_PERSON.get_or_init(|| {
        Regex::new(
            r"^\s*(i\s|i['']\s*(m|ve|d|ll)\b|my\s|me\s|we\s|in my (last|previous|current) role)\b",
        )
        .expect("first-person regex compiles")
    });

    if !re.is_match(&lower) {
        return false;
    }

    if text.trim_end().ends_with('?') {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn question_on_mic_is_flagged() {
        let verdict = evaluate("Microphone", "Tell me about a time you led a project.").unwrap();
        assert_eq!(verdict.suggested_speaker, "System");
        assert_eq!(verdict.reason, SuspicionReason::QuestionShapeOnMic);
    }

    #[test]
    fn explicit_question_mark_on_mic_is_flagged() {
        let verdict = evaluate("Microphone", "And what would you do differently?").unwrap();
        assert_eq!(verdict.suggested_speaker, "System");
    }

    #[test]
    fn user_normal_answer_on_mic_passes() {
        assert!(evaluate(
            "Microphone",
            "I worked on the identity platform for three years and shipped"
        )
        .is_none());
    }

    #[test]
    fn first_person_on_system_is_flagged() {
        let verdict = evaluate(
            "System",
            "I worked on the identity platform for three years.",
        )
        .unwrap();
        assert_eq!(verdict.suggested_speaker, "Microphone");
        assert_eq!(verdict.reason, SuspicionReason::FirstPersonOnSystem);
    }

    #[test]
    fn first_person_question_on_system_passes() {
        assert!(evaluate("System", "I'm curious — why do you want to work here?").is_none());
    }

    #[test]
    fn short_chunks_skipped() {
        assert!(evaluate("Microphone", "what?").is_none());
        assert!(evaluate("System", "I see.").is_none());
    }

    #[test]
    fn unknown_speaker_returns_none() {
        assert!(evaluate("Unknown", "Tell me about your last project").is_none());
    }

    #[test]
    fn interviewer_question_on_system_passes() {
        assert!(evaluate(
            "System",
            "Tell me about a project you led at your last role."
        )
        .is_none());
    }
}
