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
use std::time::{Duration, Instant};

use regex::Regex;

use crate::audio::pipeline::{jaccard, tokenize_for_echo};

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
    /// Near-identical text appeared on the opposite channel within
    /// [`NEAR_DUPLICATE_WINDOW`] — loopback bleed that survived the
    /// pipeline's hard 500ms echo-suppression window (Slice 3).
    NearDuplicateCrossChannel,
}

impl SuspicionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SuspicionReason::QuestionShapeOnMic => "question_shape_on_mic",
            SuspicionReason::FirstPersonOnSystem => "first_person_on_system",
            SuspicionReason::NearDuplicateCrossChannel => "near_duplicate_cross_channel",
        }
    }
}

/// Window used by [`NearDuplicateTracker`] — deliberately wider than the
/// pipeline's hard 500ms echo-suppression window
/// (`crate::audio::pipeline::ECHO_WINDOW`). Content that near-duplicates the
/// opposite channel 500ms-1.5s later already survived hard suppression (it
/// was too late to catch), but is still strong evidence the channel label is
/// unreliable for this chunk — worth flagging as suspicious even though it
/// is too late to silently drop.
pub const NEAR_DUPLICATE_WINDOW: Duration = Duration::from_millis(1_500);

/// Same threshold as the pipeline's hard echo suppression — this is the same
/// "near-identical text" bar, just applied over a longer window.
pub const NEAR_DUPLICATE_JACCARD_THRESHOLD: f32 = 0.85;

/// Minimum tokens before two utterances are even compared — short phrases
/// ("yeah", "okay") produce false-positive matches at any threshold.
const NEAR_DUPLICATE_MIN_WORDS: usize = 3;

/// Tracks recent per-channel transcripts so [`NearDuplicateTracker::check`]
/// can spot loopback bleed that survived the pipeline's hard echo
/// suppression — i.e. duplicate content that arrived 500ms-1.5s apart on
/// both channels, too late to be caught as an exact echo but still evidence
/// the channel label is unreliable for this chunk.
///
/// Only meaningful in dual-stream (non-phone) mode — phone-call mode routes
/// all audio onto a single channel, so there is no "opposite channel" to
/// compare against.
#[derive(Default)]
pub struct NearDuplicateTracker {
    recent_system: Vec<(Vec<String>, Instant)>,
    recent_mic: Vec<(Vec<String>, Instant)>,
}

impl NearDuplicateTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record text just accepted on `speaker`'s channel (`"System"` or
    /// `"Microphone"`) so later chunks on the opposite channel can be
    /// checked against it. No-op for unrecognised speaker strings or
    /// too-short text.
    pub fn record(&mut self, speaker: &str, text: &str, now: Instant) {
        let tokens = tokenize_for_echo(text);
        if tokens.len() < NEAR_DUPLICATE_MIN_WORDS {
            return;
        }
        match speaker {
            "System" => self.recent_system.push((tokens, now)),
            "Microphone" => self.recent_mic.push((tokens, now)),
            _ => {}
        }
        self.prune(now);
    }

    /// Check whether `text` on `speaker`'s channel closely duplicates
    /// something recently seen on the OPPOSITE channel within
    /// [`NEAR_DUPLICATE_WINDOW`]. Returns a suspicion verdict suggesting the
    /// opposite channel's speaker if so — the physical channel that captured
    /// `text` is very likely just an echo of the other speaker, not a new
    /// utterance from whoever the channel proxy implies.
    pub fn check(&mut self, speaker: &str, text: &str, now: Instant) -> Option<SuspicionVerdict> {
        self.prune(now);
        let tokens = tokenize_for_echo(text);
        if tokens.len() < NEAR_DUPLICATE_MIN_WORDS {
            return None;
        }
        let (opposite, suggested_speaker) = match speaker {
            "System" => (&self.recent_mic, "Microphone"),
            "Microphone" => (&self.recent_system, "System"),
            _ => return None,
        };
        let matched = opposite.iter().any(|(other_tokens, _)| {
            jaccard(&tokens, other_tokens) >= NEAR_DUPLICATE_JACCARD_THRESHOLD
        });
        if !matched {
            return None;
        }
        Some(SuspicionVerdict {
            suggested_speaker: suggested_speaker.to_string(),
            reason: SuspicionReason::NearDuplicateCrossChannel,
        })
    }

    fn prune(&mut self, now: Instant) {
        if let Some(cutoff) = now.checked_sub(NEAR_DUPLICATE_WINDOW) {
            self.recent_system.retain(|(_, at)| *at >= cutoff);
            self.recent_mic.retain(|(_, at)| *at >= cutoff);
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
            r"(?i)^\s*(?:hi|hello)[,.]?\s*(?:this is|i['']?m|my name is|i am)\b|^\s*(?:i\s|i['']\s*(?:m|ve|d|ll)\b|my\s|me\s|we\s|this is|my name is|in my (?:last|previous|current) role)\b",
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
    fn self_intro_on_system_is_flagged() {
        let verdict = evaluate("System", "Hi, this is Ali Barzio speaking.").unwrap();
        assert_eq!(verdict.suggested_speaker, "Microphone");
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

    // ── NearDuplicateTracker (Slice 3) ──────────────────────────────────────

    /// Table-driven cases: (label, seed_speaker, seed_text, check_speaker,
    /// check_text, gap_ms, expect_match, expect_suggested_speaker).
    #[test]
    fn near_duplicate_table_driven_cases() {
        struct Case {
            label: &'static str,
            seed_speaker: &'static str,
            seed_text: &'static str,
            check_speaker: &'static str,
            check_text: &'static str,
            gap_ms: u64,
            expect_match: bool,
            expect_suggested: Option<&'static str>,
        }

        let cases = [
            Case {
                label: "near-identical text 800ms later on opposite channel matches",
                seed_speaker: "System",
                seed_text: "Why do you like to work with Fisher Investors",
                check_speaker: "Microphone",
                check_text: "Why do you like to work with Fisher Investors",
                gap_ms: 800,
                expect_match: true,
                expect_suggested: Some("System"),
            },
            Case {
                label: "near-identical text just inside the 1.5s window matches",
                seed_speaker: "Microphone",
                seed_text: "I am excited about the AI Engineer opportunity at Fisher",
                check_speaker: "System",
                check_text: "I am excited about the AI Engineer opportunity at Fisher",
                gap_ms: 1_499,
                expect_match: true,
                expect_suggested: Some("Microphone"),
            },
            Case {
                label: "outside the 1.5s window does not match",
                seed_speaker: "System",
                seed_text: "Tell me about a project you led at your company",
                check_speaker: "Microphone",
                check_text: "Tell me about a project you led at your company",
                gap_ms: 1_501,
                expect_match: false,
                expect_suggested: None,
            },
            Case {
                label: "distinct content on the opposite channel does not match",
                seed_speaker: "System",
                seed_text: "Tell me about a project you led at your company",
                check_speaker: "Microphone",
                check_text: "I led the fraud detection platform migration last year",
                gap_ms: 500,
                expect_match: false,
                expect_suggested: None,
            },
            Case {
                label: "same-channel duplicate is not a cross-channel suspicion",
                seed_speaker: "System",
                seed_text: "How are you today and what brings you here",
                check_speaker: "System",
                check_text: "How are you today and what brings you here",
                gap_ms: 500,
                expect_match: false,
                expect_suggested: None,
            },
            Case {
                label: "too-short utterance never matches even if identical",
                seed_speaker: "System",
                seed_text: "okay yeah",
                check_speaker: "Microphone",
                check_text: "okay yeah",
                gap_ms: 300,
                expect_match: false,
                expect_suggested: None,
            },
        ];

        for case in cases {
            let mut tracker = NearDuplicateTracker::new();
            let t0 = Instant::now();
            tracker.record(case.seed_speaker, case.seed_text, t0);

            let t1 = t0 + Duration::from_millis(case.gap_ms);
            let verdict = tracker.check(case.check_speaker, case.check_text, t1);

            assert_eq!(
                verdict.is_some(),
                case.expect_match,
                "case failed: {}",
                case.label
            );
            if case.expect_match {
                assert_eq!(
                    verdict.unwrap().suggested_speaker,
                    case.expect_suggested.unwrap(),
                    "case failed: {}",
                    case.label
                );
            }
        }
    }

    #[test]
    fn near_duplicate_reason_string_matches() {
        assert_eq!(
            SuspicionReason::NearDuplicateCrossChannel.as_str(),
            "near_duplicate_cross_channel"
        );
    }

    #[test]
    fn near_duplicate_unknown_speaker_returns_none() {
        let mut tracker = NearDuplicateTracker::new();
        let now = Instant::now();
        tracker.record("System", "tell me about a time you led a big project", now);
        assert!(tracker
            .check("Unknown", "tell me about a time you led a big project", now)
            .is_none());
    }

    #[test]
    fn near_duplicate_pruning_drops_stale_entries_on_subsequent_checks() {
        let mut tracker = NearDuplicateTracker::new();
        let t0 = Instant::now();
        tracker.record("System", "why do you want to work here at this company", t0);

        // First check well past the window prunes the stale entry.
        let t1 = t0 + NEAR_DUPLICATE_WINDOW + Duration::from_millis(200);
        assert!(tracker
            .check(
                "Microphone",
                "why do you want to work here at this company",
                t1
            )
            .is_none());

        // A fresh record + immediate check on the opposite channel still works.
        tracker.record("System", "why do you want to work here at this company", t1);
        let t2 = t1 + Duration::from_millis(100);
        assert!(tracker
            .check(
                "Microphone",
                "why do you want to work here at this company",
                t2
            )
            .is_some());
    }
}
