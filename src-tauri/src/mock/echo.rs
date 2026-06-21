//! Smarter echo detection for mock practice mode — reduces false positives when
//! the candidate uses the same domain vocabulary as the suggested script.

use std::collections::HashSet;

const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "can", "had", "her", "was",
    "one", "our", "out", "day", "get", "has", "him", "his", "how", "its", "may", "new",
    "now", "old", "see", "two", "way", "who", "boy", "did", "let", "put", "say", "she",
    "too", "use", "that", "this", "with", "have", "from", "they", "been", "were", "said",
    "each", "which", "their", "will", "other", "about", "many", "then", "them", "these",
    "some", "would", "make", "like", "into", "time", "very", "when", "come", "could",
    "more", "also", "what", "your", "work", "team", "role", "just", "well", "than",
];

/// Minimum filtered word Jaccard to flag scripted reading (after stop-word removal).
const FILTERED_WORD_JACCARD_THRESHOLD: f32 = 0.62;
/// Bigram overlap required alongside moderate word overlap for short answers.
const BIGRAM_JACCARD_THRESHOLD: f32 = 0.55;
const BIGRAM_WORD_JACCARD_FLOOR: f32 = 0.48;
/// Short answers need higher word overlap — bigrams are sparse.
const SHORT_ANSWER_WORD_THRESHOLD: f32 = 0.72;
const SHORT_ANSWER_MAX_WORDS: usize = 25;

/// Domain tokens present in both company context and the suggested script —
/// shared vocabulary should not inflate echo overlap scores.
pub fn collect_shared_vocab_terms(company_context: &str, suggested_answer: &str) -> HashSet<String> {
    let context_set: HashSet<String> = tokenize_content_words(company_context).into_iter().collect();
    tokenize_content_words(suggested_answer)
        .into_iter()
        .filter(|t| context_set.contains(t))
        .collect()
}

/// Combined echo signal in `[0, 1]`. Caller decides threshold / caps.
pub fn echo_overlap_score(
    user_answer: &str,
    suggested_answer: &str,
    exclude_terms: &HashSet<String>,
) -> f32 {
    if user_answer.trim().is_empty() || suggested_answer.trim().is_empty() {
        return 0.0;
    }

    let user_words = filter_tokens(tokenize_content_words(user_answer), exclude_terms);
    let suggested_words = filter_tokens(tokenize_content_words(suggested_answer), exclude_terms);

    if user_words.is_empty() || suggested_words.is_empty() {
        return 0.0;
    }

    let word_j = jaccard(&user_words, &suggested_words);
    let user_word_count = count_words(user_answer);

    if user_word_count <= SHORT_ANSWER_MAX_WORDS && word_j >= SHORT_ANSWER_WORD_THRESHOLD {
        return word_j;
    }

    if word_j >= FILTERED_WORD_JACCARD_THRESHOLD {
        return word_j;
    }

    let user_bigrams = bigrams(&user_words);
    let suggested_bigrams = bigrams(&suggested_words);
    if user_bigrams.is_empty() || suggested_bigrams.is_empty() {
        return word_j;
    }

    let bigram_j = jaccard(&user_bigrams, &suggested_bigrams);
    if bigram_j >= BIGRAM_JACCARD_THRESHOLD && word_j >= BIGRAM_WORD_JACCARD_FLOOR {
        return word_j.max(bigram_j * 0.85);
    }

    word_j
}

pub fn should_cap_echo_score(
    user_answer: &str,
    suggested_answer: &str,
    exclude_terms: &HashSet<String>,
) -> bool {
    let overlap = echo_overlap_score(user_answer, suggested_answer, exclude_terms);
    let word_count = count_words(user_answer);
    if word_count < 12 {
        return overlap >= SHORT_ANSWER_WORD_THRESHOLD;
    }
    overlap >= FILTERED_WORD_JACCARD_THRESHOLD
        || (overlap >= BIGRAM_WORD_JACCARD_FLOOR
            && bigrams(&filter_tokens(
                tokenize_content_words(user_answer),
                exclude_terms,
            ))
            .len()
                >= 3)
}

fn count_words(text: &str) -> usize {
    text.split_whitespace().filter(|w| !w.is_empty()).count()
}

fn tokenize_content_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .filter(|w| !STOP_WORDS.contains(w))
        .map(str::to_string)
        .collect()
}

fn filter_tokens(tokens: Vec<String>, exclude: &HashSet<String>) -> Vec<String> {
    tokens
        .into_iter()
        .filter(|t| !exclude.contains(t))
        .collect()
}

fn jaccard(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let set_a: HashSet<_> = a.iter().collect();
    let set_b: HashSet<_> = b.iter().collect();
    let inter = set_a.intersection(&set_b).count() as f32;
    let union = set_a.union(&set_b).count() as f32;
    if union <= f32::EPSILON {
        0.0
    } else {
        inter / union
    }
}

fn bigrams(words: &[String]) -> Vec<String> {
    words
        .windows(2)
        .map(|w| format!("{} {}", w[0], w[1]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_iam_vocab_does_not_trigger_echo_cap() {
        let suggested = "I designed RBAC and ABAC policies for OAuth OIDC federation across enterprise IAM.";
        let user = "At my last role I implemented RBAC with OAuth and OIDC for our IAM platform using ABAC rules.";
        let exclude = collect_shared_vocab_terms(
            "Leadership: Client-first. Role: IAM architect, OAuth OIDC MFA.",
            suggested,
        );
        assert!(
            !should_cap_echo_score(user, suggested, &exclude),
            "domain overlap should not cap score"
        );
    }

    #[test]
    fn verbatim_reading_triggers_echo_cap() {
        let script = "I led a cross-functional team to migrate our monolith to microservices, reducing deploy time by forty percent.";
        let user = "I led a cross-functional team to migrate our monolith to microservices, reducing deploy time by forty percent.";
        let exclude = collect_shared_vocab_terms("", script);
        assert!(should_cap_echo_score(user, script, &exclude));
    }

    #[test]
    fn paraphrase_does_not_trigger_echo_cap() {
        let script = "I led a cross-functional team to migrate our monolith to microservices, reducing deploy time by forty percent.";
        let user = "We moved from a monolith to microservices on my team and cut releases from weekly to daily.";
        let exclude = collect_shared_vocab_terms("", script);
        assert!(!should_cap_echo_score(user, script, &exclude));
    }
}
