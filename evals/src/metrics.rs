//! Rule-based metrics applied locally (no LLM round-trip).
//!
//! These metrics complement the LLM judge (`crate::judge`) and run on every
//! response. They cover the "objective" half of the eval criteria from
//! design doc §20: conciseness, depth structure, and latency.

use serde::{Deserialize, Serialize};

/// Maximum sentences a directional response may contain.
const DIRECTIONAL_MAX_SENTENCES: usize = 3;

/// Approximate inverted-pyramid heuristic: the first paragraph should be
/// the shortest (the punchline), and subsequent paragraphs may expand.
const DEPTH_MIN_PARAGRAPHS: usize = 2;

/// Result of all rule-based metrics for a single (question, variant)
/// response. Stored alongside judge scores in the per-row report.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RuleScores {
    pub directional_conciseness: ConcisenessOutcome,
    pub depth_structure: StructureOutcome,
    pub latency: LatencyOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ConcisenessOutcome {
    pub sentences: usize,
    pub passed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StructureOutcome {
    pub paragraphs: usize,
    pub follows_inverted_pyramid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LatencyOutcome {
    pub ttft_ms: u64,
    pub stream_complete_ms: u64,
    pub ttft_under_900ms: bool,
    pub stream_under_8s: bool,
}

/// Count sentences in `text` using punctuation boundaries.
/// Treats a streak of `.!?` as a single sentence terminator.
pub fn count_sentences(text: &str) -> usize {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return 0;
    }
    let mut count = 0usize;
    let mut last_was_terminator = false;
    for ch in trimmed.chars() {
        let is_terminator = matches!(ch, '.' | '!' | '?');
        if is_terminator && !last_was_terminator {
            count += 1;
        }
        last_was_terminator = is_terminator;
    }
    // Trailing text without a terminator still counts as a sentence.
    if !trimmed.ends_with(['.', '!', '?']) {
        count += 1;
    }
    count.max(1)
}

pub fn score_conciseness(directional_response: &str) -> ConcisenessOutcome {
    let sentences = count_sentences(directional_response);
    ConcisenessOutcome {
        sentences,
        passed: sentences <= DIRECTIONAL_MAX_SENTENCES,
    }
}

pub fn score_structure(depth_response: &str) -> StructureOutcome {
    let paragraphs: Vec<&str> = depth_response
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();

    let n = paragraphs.len();
    let follows_inverted_pyramid = if n < DEPTH_MIN_PARAGRAPHS {
        false
    } else {
        let first_len = paragraphs[0].len();
        let rest_avg = paragraphs[1..]
            .iter()
            .map(|p| p.len())
            .sum::<usize>()
            .checked_div(n - 1)
            .unwrap_or(0);
        first_len <= rest_avg
    };

    StructureOutcome {
        paragraphs: n,
        follows_inverted_pyramid,
    }
}

pub fn score_latency(ttft_ms: u64, stream_complete_ms: u64) -> LatencyOutcome {
    const TTFT_BUDGET_MS: u64 = 900;
    const STREAM_BUDGET_MS: u64 = 8_000;
    LatencyOutcome {
        ttft_ms,
        stream_complete_ms,
        ttft_under_900ms: ttft_ms <= TTFT_BUDGET_MS,
        stream_under_8s: stream_complete_ms <= STREAM_BUDGET_MS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_sentences_handles_single_sentence_without_period() {
        assert_eq!(count_sentences("hello world"), 1);
    }

    #[test]
    fn count_sentences_handles_multiple_terminators() {
        assert_eq!(count_sentences("One. Two! Three?"), 3);
    }

    #[test]
    fn count_sentences_treats_ellipsis_as_one_terminator() {
        assert_eq!(count_sentences("Wait... what?"), 2);
    }

    #[test]
    fn score_conciseness_passes_for_three_sentences() {
        let out = score_conciseness("First. Second. Third.");
        assert_eq!(out.sentences, 3);
        assert!(out.passed);
    }

    #[test]
    fn score_conciseness_fails_for_four_sentences() {
        let out = score_conciseness("One. Two. Three. Four.");
        assert!(!out.passed);
    }

    #[test]
    fn score_structure_flags_single_paragraph_as_not_inverted() {
        let out = score_structure("just one paragraph");
        assert!(!out.follows_inverted_pyramid);
    }

    #[test]
    fn score_structure_passes_when_first_paragraph_is_shortest() {
        let depth = "Short summary.\n\nLonger second paragraph with much more detail and context.";
        let out = score_structure(depth);
        assert!(out.follows_inverted_pyramid);
    }

    #[test]
    fn score_latency_flags_breaches_correctly() {
        let pass = score_latency(800, 7_500);
        assert!(pass.ttft_under_900ms && pass.stream_under_8s);

        let breach = score_latency(950, 8_500);
        assert!(!breach.ttft_under_900ms && !breach.stream_under_8s);
    }
}
