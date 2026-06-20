//! Per-session question attempt tracking — skip re-asking when the user
//! already got a strong answer (rehearsal confidence or mock coach score).

use crate::confidence::ConfidenceLevel;
use crate::interfaces::vector::QA_EMBED_CONFIDENCE_THRESHOLD;

/// Mock coach score (0–100) at or above which a turn counts as practiced well.
pub const MOCK_COACH_SATISFIED_THRESHOLD: u8 = 70;

/// Cosine similarity threshold for matching rephrased questions to saved preferred answers.
/// Same threshold as the pre-warm cache (§13 / flint-performance NFR).
pub const PREFERRED_ANSWER_MATCH_THRESHOLD: f32 = 0.85;

/// Dot product of two same-length unit-norm vectors equals cosine similarity.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return f32::NEG_INFINITY;
    }
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Pick the preferred answer whose stored question embedding best matches `query`.
pub fn best_preferred_semantic_match<'a>(
    query: &[f32],
    candidates: impl IntoIterator<Item = (&'a [f32], &'a str)>,
) -> Option<&'a str> {
    let (best_answer, best_sim) = candidates.into_iter().fold(
        (None, f32::NEG_INFINITY),
        |(best_a, best_s), (stored, answer)| {
            let sim = cosine_similarity(query, stored);
            if sim > best_s {
                (Some(answer), sim)
            } else {
                (best_a, best_s)
            }
        },
    );
    if best_sim >= PREFERRED_ANSWER_MATCH_THRESHOLD {
        best_answer
    } else {
        None
    }
}

pub fn encode_embedding_blob(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|v| v.to_le_bytes()).collect()
}

pub fn decode_embedding_blob(bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        let arr: [u8; 4] = chunk.try_into().ok()?;
        out.push(f32::from_le_bytes(arr));
    }
    Some(out)
}

/// Normalised key for deduplicating question text within a session.
///
/// Collapses case/whitespace, strips trailing punctuation, and removes common
/// interviewer lead-ins so preferred answers match minor rephrasings.
pub fn normalize_question_key(question: &str) -> String {
    let mut q = strip_rephrase_prefix(question).trim().to_lowercase();

    const LEAD_INS: &[&str] = &[
        "could you please ",
        "can you please ",
        "would you please ",
        "could you ",
        "can you ",
        "would you ",
        "please ",
    ];
    for lead in LEAD_INS {
        if let Some(rest) = q.strip_prefix(lead) {
            q = rest.trim().to_string();
            break;
        }
    }

    q.split_whitespace()
        .map(strip_word_punctuation)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_word_punctuation(word: &str) -> &str {
    word.trim_matches(|c: char| c.is_ascii_punctuation())
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

/// Behavioral / intro questions need spoken first-person answers, not resume blocks.
pub fn is_behavioral_question(question: &str) -> bool {
    let q = strip_rephrase_prefix(question).trim().to_lowercase();
    if q.is_empty() {
        return false;
    }
    const PHRASES: &[&str] = &[
        "tell me about yourself",
        "walk me through your background",
        "walk me through your resume",
        "introduce yourself",
        "why are you interested",
        "why this role",
        "why do you want",
        "why fisher",
        "why our company",
        "greatest strength",
        "greatest weakness",
        "weakness",
        "where do you see yourself",
        "why should we hire",
        "tell me about a time",
        "describe a time",
        "describe a situation",
        "how do you handle conflict",
        "tell me about a challenge",
    ];
    PHRASES.iter().any(|p| q.contains(p))
}

/// Prompt injection for directional/depth threads — behavioral vs technical tone.
pub fn answer_style_instructions(question: &str, for_depth: bool) -> String {
    if is_behavioral_question(question) {
        if for_depth {
            "Write a spoken answer the candidate can read aloud in first person. \
             Use short paragraphs, not bullet lists. For STAR-style questions, cover situation, \
             your action, and result. Ground every detail in [Supporting context] only — \
             never invent employers, dates, metrics, or certifications. \
             For \"tell me about yourself\": name and location first, then recent roles in order, \
             close with why this role fits. Maximum 150 words."
                .to_string()
        } else {
            "Speak in first person as the candidate (\"I\", not \"the candidate\"). \
             Sound natural read aloud — short sentences, no bullet lists. \
             Use only facts from [Supporting context]; never invent employers, metrics, or certs. \
             For \"tell me about yourself\": name + location, 2–3 recent roles, why this role. \
             Maximum 3 sentences."
                .to_string()
        }
    } else if for_depth {
        "Answer in first person as the candidate. Inverted pyramid: lead with the direct answer, \
         then reasoning and one concrete example from [Supporting context]. \
         Do not invent tools, metrics, or project names. Maximum 150 words."
            .to_string()
    } else {
        "Answer in first person as the candidate. Be specific using [Supporting context] only. \
         Do not invent metrics, tools, or project names. Maximum 3 sentences."
            .to_string()
    }
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
    fn normalize_strips_trailing_punctuation() {
        assert_eq!(
            normalize_question_key("Tell me about yourself?"),
            "tell me about yourself"
        );
        assert_eq!(
            normalize_question_key("Tell me about yourself."),
            "tell me about yourself"
        );
    }

    #[test]
    fn normalize_strips_common_lead_ins() {
        assert_eq!(
            normalize_question_key("Can you tell me about yourself?"),
            "tell me about yourself"
        );
        assert_eq!(
            normalize_question_key("Could you please walk me through your background?"),
            "walk me through your background"
        );
    }

    #[test]
    fn normalize_does_not_merge_distinct_questions() {
        assert_ne!(
            normalize_question_key("Why do you want this role?"),
            normalize_question_key("Why this role?")
        );
    }

    #[test]
    fn rephrase_prefix_stripped_for_key() {
        assert_eq!(
            normalize_question_key("Rephrase your previous answer to: Why this role?"),
            "why this role"
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

    #[test]
    fn behavioral_detects_tmay() {
        assert!(is_behavioral_question("Tell me about yourself"));
        assert!(!is_behavioral_question(
            "How would you design Okta SSO for 50k users?"
        ));
    }

    #[test]
    fn semantic_preferred_match_at_threshold() {
        let stored = [1.0_f32, 0.0, 0.0];
        let query = [0.986_f32, 0.164, 0.0]; // cos ≈ 0.986
        let answer = best_preferred_semantic_match(&query, [(&stored[..], "my script")]);
        assert_eq!(answer, Some("my script"));
    }

    #[test]
    fn semantic_preferred_miss_below_threshold() {
        let stored = [1.0_f32, 0.0, 0.0];
        let query = [0.5_f32, 0.866, 0.0]; // cos = 0.5
        assert!(best_preferred_semantic_match(&query, [(&stored[..], "my script")]).is_none());
    }

    #[test]
    fn embedding_blob_round_trip() {
        let v = vec![0.1_f32, -0.2, 3.0];
        let blob = encode_embedding_blob(&v);
        assert_eq!(decode_embedding_blob(&blob), Some(v));
    }
}
