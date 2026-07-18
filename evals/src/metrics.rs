//! Rule-based metrics applied locally (no LLM round-trip).
//!
//! These metrics complement the LLM judge (`crate::judge`) and run on every
//! response. They cover the "objective" half of the eval criteria from
//! design doc §20: answer conciseness, visual structure, and latency.

use serde::{Deserialize, Serialize};

/// Maximum sentences an Answer thread response may contain. Matches the
/// "Maximum 4 sentences" instruction in `prompts/answer/*.txt` — the
/// mandatory trailing "Follow-up:" line (slice 17) is itself one sentence,
/// so this is one higher than the retired Directional thread's cap of 3.
const ANSWER_MAX_SENTENCES: usize = 4;

/// A rendered diagram or code fence below this length is almost certainly a
/// truncated stream or an empty block, not a usable visual.
const MIN_VISUAL_BLOCK_CHARS: usize = 15;

/// Result of all rule-based metrics for a single (question, variant)
/// response. Stored alongside judge scores in the per-row report.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RuleScores {
    pub answer_conciseness: ConcisenessOutcome,
    pub visual_structure: VisualStructureOutcome,
    pub latency: LatencyOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ConcisenessOutcome {
    pub sentences: usize,
    pub passed: bool,
}

/// Visual thread's contract is a single complete fenced block (Mermaid
/// diagram or code) — unlike the retired Depth thread's free-form prose,
/// there's no paragraph structure to score. This checks the fence itself
/// extracted, closed, and non-trivial.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VisualStructureOutcome {
    pub has_fenced_block: bool,
    pub block_chars: usize,
    pub passed: bool,
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

pub fn score_conciseness(answer_response: &str) -> ConcisenessOutcome {
    let sentences = count_sentences(answer_response);
    ConcisenessOutcome {
        sentences,
        passed: sentences <= ANSWER_MAX_SENTENCES,
    }
}

/// Mirrors the live orchestrator's fence detection (`orchestrator::visual`)
/// so the eval harness scores the same contract the app actually renders:
/// exactly one fenced block, fully closed.
pub fn score_visual_structure(visual_response: &str) -> VisualStructureOutcome {
    let block = extract_fenced_block(visual_response);
    let block_chars = block.map(str::len).unwrap_or(0);
    let has_fenced_block = block.is_some();
    VisualStructureOutcome {
        has_fenced_block,
        block_chars,
        passed: has_fenced_block && block_chars >= MIN_VISUAL_BLOCK_CHARS,
    }
}

fn extract_fenced_block(buffer: &str) -> Option<&str> {
    const FENCE: &str = "```";
    let start = buffer.find(FENCE)?;
    let after_open = start + FENCE.len();
    let close_offset = buffer[after_open..].find(FENCE)?;
    let after_lang = buffer[after_open..].find('\n').map(|i| after_open + i + 1);
    let content_start = after_lang.unwrap_or(after_open);
    let content_end = after_open + close_offset;
    if content_start >= content_end {
        return Some("");
    }
    Some(buffer[content_start..content_end].trim())
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
    fn score_conciseness_passes_for_four_sentences() {
        // Conclusion + up to 2 reasoning sentences + the mandatory
        // "Follow-up:" line — the Answer prompt's own stated ceiling.
        let out = score_conciseness("First. Second. Third. Follow-up: Fourth?");
        assert_eq!(out.sentences, 4);
        assert!(out.passed);
    }

    #[test]
    fn score_conciseness_fails_for_five_sentences() {
        let out = score_conciseness("One. Two. Three. Four. Five.");
        assert!(!out.passed);
    }

    #[test]
    fn score_visual_structure_fails_when_no_fenced_block_present() {
        let out = score_visual_structure("just some prose, no fence anywhere");
        assert!(!out.has_fenced_block);
        assert!(!out.passed);
    }

    #[test]
    fn score_visual_structure_passes_for_a_closed_mermaid_fence() {
        let out = score_visual_structure("```mermaid\nflowchart TD\nA-->B\n```");
        assert!(out.has_fenced_block);
        assert!(out.passed);
    }

    #[test]
    fn score_visual_structure_fails_for_an_unclosed_fence() {
        let out = score_visual_structure("```mermaid\nflowchart TD\nA-->B");
        assert!(!out.has_fenced_block);
        assert!(!out.passed);
    }

    #[test]
    fn score_visual_structure_fails_for_a_trivially_short_block() {
        let out = score_visual_structure("```\nx\n```");
        assert!(out.has_fenced_block);
        assert!(!out.passed);
    }

    #[test]
    fn score_latency_flags_breaches_correctly() {
        let pass = score_latency(800, 7_500);
        assert!(pass.ttft_under_900ms && pass.stream_under_8s);

        let breach = score_latency(950, 8_500);
        assert!(!breach.ttft_under_900ms && !breach.stream_under_8s);
    }
}
