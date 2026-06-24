//! Whisper post-processing — hallucination filters, repeat collapse, profanity repair.
//!
//! Whisper.cpp passes the standard `no_speech / compression_ratio / avg_logprob`
//! gates today, but a residual class of failures slips through:
//!
//! - Stock end-of-segment hallucinations on silence ("Thanks for watching",
//!   "Subscribe to my channel", lone "you", lone "the").
//! - Looping ngrams when the audio chunk has long sustained silence inside it
//!   (Whisper repeats a 4-word phrase 5+ times).
//! - Profanity hallucinations when the speaker said "specifically" — Whisper
//!   sometimes splits it into "specific" + an expletive token.
//! - Word/sec ratios that are physically impossible relative to the chunk
//!   duration (more than 5 words/sec sustained over the segment).
//!
//! All filters here are pure functions on `&str` so they are trivial to unit
//! test and reuse from both the Live audio pipeline and the Mock per-turn path.

use std::collections::HashSet;

// ────────────────────────────────────────────────────────────────────────────
// Tunable thresholds
// ────────────────────────────────────────────────────────────────────────────

/// Words per second above which a Whisper segment is treated as a hallucination
/// loop regardless of the numeric thresholds. Conversational English peaks
/// around 4.5 wps; sustained values above 5 are not human speech.
pub const MAX_WORDS_PER_SECOND: f32 = 5.0;

/// Minimum ngram length checked by `collapse_repeated_ngrams`.
const NGRAM_MIN_WORDS: usize = 4;

/// A word phrase (after lowercasing + punctuation strip) that repeats this many
/// times consecutively is collapsed to a single occurrence.
const NGRAM_REPEAT_THRESHOLD: usize = 3;

// ────────────────────────────────────────────────────────────────────────────
// Public surface
// ────────────────────────────────────────────────────────────────────────────

/// Run every Live-friendly post-processor in order: known-hallucination drop,
/// repeat collapse, profanity repair (no script hint), and trim.
///
/// Returns `None` when the segment should be dropped entirely (full match
/// against a known hallucination string).
pub fn sanitize_live_transcript(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if is_known_hallucination(trimmed) {
        return None;
    }
    let collapsed = collapse_repeated_ngrams(trimmed);
    let repaired = sanitize_with_script_hint(&collapsed, None);
    let out = repaired.trim().to_string();
    if out.is_empty() {
        return None;
    }
    Some(out)
}

/// Variant of [`sanitize_live_transcript`] that accepts the prepared script as
/// hint so deliberate profanity in the script is preserved. Used by the mock
/// pipeline where the suggested answer is known.
pub fn sanitize_with_script_hint(transcript: &str, script_hint: Option<&str>) -> String {
    let trimmed = transcript.trim();
    if trimmed.is_empty() {
        return transcript.to_string();
    }

    let script_has_profanity = script_hint.map(text_has_profanity).unwrap_or(false);
    let mut words: Vec<String> = trimmed.split_whitespace().map(str::to_string).collect();

    repair_specifically_profanity_hallucination(&mut words);

    if !script_has_profanity {
        words.retain(|w| !is_profanity_word(w));
    }

    words.join(" ")
}

/// True when the segment text matches a well-documented Whisper hallucination
/// pattern that survives the numeric filters. The list is curated from the
/// upstream OpenAI/whisper hallucination tracker plus our own session logs.
pub fn is_known_hallucination(text: &str) -> bool {
    let normalized = normalize_for_match(text);
    if normalized.is_empty() {
        return true;
    }

    let exact = known_hallucination_set();
    if exact.contains(normalized.as_str()) {
        return true;
    }

    if normalized.starts_with("thanks for watching")
        || normalized.starts_with("thank you for watching")
        || normalized.starts_with("subscribe to my channel")
        || normalized.starts_with("please subscribe")
        || normalized.starts_with("dont forget to subscribe")
        || normalized.starts_with("like and subscribe")
    {
        return true;
    }

    false
}

/// Return `false` when the segment violates physical-plausibility checks
/// against `duration_ms`. Used by the Whisper engine to drop hallucination
/// loops that pass the compression ratio threshold (e.g. moderately repetitive
/// long phrases).
pub fn validate_segment(text: &str, duration_ms: u32) -> bool {
    if duration_ms == 0 {
        return false;
    }
    let words = text.split_whitespace().count() as f32;
    if words == 0.0 {
        return false;
    }
    let seconds = (duration_ms as f32) / 1000.0;
    let wps = words / seconds.max(0.05);
    wps <= MAX_WORDS_PER_SECOND
}

/// True when text contains a profanity token (after normalising punctuation).
pub fn text_has_profanity(text: &str) -> bool {
    text.split_whitespace().any(is_profanity_word)
}

/// Collapse any consecutive ngram (length >= [`NGRAM_MIN_WORDS`]) that repeats
/// at least [`NGRAM_REPEAT_THRESHOLD`] times into a single occurrence.
///
/// Whisper occasionally emits "I think the team I think the team I think the
/// team I think the team..." on long silence. The compression ratio gate
/// catches the worst cases; this catches the moderate ones.
pub fn collapse_repeated_ngrams(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < NGRAM_MIN_WORDS * NGRAM_REPEAT_THRESHOLD {
        return text.to_string();
    }

    // Try the largest ngram first so we collapse the longest repeating phrase
    // available; smaller ngrams will not match once the larger one is folded.
    let max_n = words.len() / NGRAM_REPEAT_THRESHOLD;
    for n in (NGRAM_MIN_WORDS..=max_n).rev() {
        if let Some(collapsed) = collapse_for_n(&words, n) {
            return collapsed;
        }
    }
    text.to_string()
}

// ────────────────────────────────────────────────────────────────────────────
// Internals
// ────────────────────────────────────────────────────────────────────────────

fn collapse_for_n(words: &[&str], n: usize) -> Option<String> {
    let mut out: Vec<&str> = Vec::with_capacity(words.len());
    let mut i = 0;
    let mut collapsed = false;

    while i + n <= words.len() {
        let window: Vec<String> = (0..n).map(|k| normalize_word(words[i + k])).collect();
        let mut repeats = 1;
        let mut j = i + n;
        while j + n <= words.len() {
            let next: Vec<String> = (0..n).map(|k| normalize_word(words[j + k])).collect();
            if next == window {
                repeats += 1;
                j += n;
            } else {
                break;
            }
        }

        if repeats >= NGRAM_REPEAT_THRESHOLD {
            // Keep one copy verbatim, skip the rest.
            out.extend(words.iter().skip(i).take(n).copied());
            i = j;
            collapsed = true;
        } else {
            out.push(words[i]);
            i += 1;
        }
    }
    while i < words.len() {
        out.push(words[i]);
        i += 1;
    }

    if collapsed {
        Some(out.join(" "))
    } else {
        None
    }
}

fn known_hallucination_set() -> HashSet<&'static str> {
    [
        "thanks for watching",
        "thank you for watching",
        "thank you",
        "you",
        "the",
        "subscribe",
        "subtitles by the amaraorg community",
        "subtitles by the amara org community",
        "transcribed by",
        "music",
        "applause",
        "silence",
        "bye",
        "okay",
        "thanks",
    ]
    .into_iter()
    .collect()
}

fn normalize_for_match(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

// ── Profanity helpers (formerly in mock::transcript) ─────────────────────────

fn repair_specifically_profanity_hallucination(words: &mut Vec<String>) {
    let mut i = 0;
    while i < words.len() {
        if is_specific_stem(&words[i]) && i + 1 < words.len() && is_profanity_word(&words[i + 1]) {
            words[i] = punctuate_replacement("specifically", &words[i], &words[i + 1]);
            words.remove(i + 1);
            continue;
        }
        i += 1;
    }
}

fn punctuate_replacement(replacement: &str, before: &str, after: &str) -> String {
    if after.contains('!') || after.contains('?') || before.ends_with('.') || before.ends_with(',')
    {
        format!("{replacement},")
    } else {
        replacement.to_string()
    }
}

fn is_specific_stem(word: &str) -> bool {
    normalize_word(word) == "specific"
}

fn normalize_word(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric() || *c == '*')
        .collect::<String>()
        .to_lowercase()
}

fn is_profanity_word(word: &str) -> bool {
    let normalized = normalize_word(word);
    if normalized.is_empty() {
        return false;
    }

    let collapsed = normalized.replace('*', "");
    if collapsed.contains("fuck") || collapsed == "fck" || collapsed.starts_with("fck") {
        return true;
    }

    matches!(
        collapsed.as_str(),
        "shit" | "bitch" | "asshole" | "damn" | "cunt" | "bastard"
    )
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_hallucination_thanks_for_watching() {
        assert!(is_known_hallucination("Thanks for watching!"));
        assert!(is_known_hallucination("THANK YOU FOR WATCHING"));
        assert!(is_known_hallucination("Thanks for watching, see you next time"));
    }

    #[test]
    fn known_hallucination_lone_filler_words() {
        assert!(is_known_hallucination("you"));
        assert!(is_known_hallucination("the"));
        assert!(is_known_hallucination(" Thanks "));
    }

    #[test]
    fn known_hallucination_does_not_drop_real_speech() {
        assert!(!is_known_hallucination(
            "Tell me about a time you led a project."
        ));
        assert!(!is_known_hallucination("I worked on the team for three years"));
    }

    #[test]
    fn validate_segment_rejects_impossible_word_rate() {
        // 60 words in 250 ms = 240 wps — clearly hallucinated.
        let text = "word ".repeat(60);
        assert!(!validate_segment(&text, 250));
    }

    #[test]
    fn validate_segment_accepts_normal_speech() {
        // ~3 wps over 4 seconds.
        let text = "I worked on the identity platform for three years and shipped";
        assert!(validate_segment(text, 4_000));
    }

    #[test]
    fn validate_segment_rejects_zero_duration() {
        assert!(!validate_segment("hello world", 0));
    }

    #[test]
    fn collapse_repeated_ngrams_folds_three_repeats() {
        let raw = "I think the team I think the team I think the team is excellent";
        let out = collapse_repeated_ngrams(raw);
        assert_eq!(out, "I think the team is excellent");
    }

    #[test]
    fn collapse_repeated_ngrams_leaves_normal_text_untouched() {
        let raw = "Tell me about a time you faced a hard problem at work";
        assert_eq!(collapse_repeated_ngrams(raw), raw);
    }

    #[test]
    fn collapse_repeated_ngrams_keeps_two_repeats() {
        let raw = "see you later see you later then";
        assert_eq!(collapse_repeated_ngrams(raw), raw);
    }

    #[test]
    fn sanitize_live_drops_pure_hallucination() {
        assert!(sanitize_live_transcript("Thanks for watching!").is_none());
    }

    #[test]
    fn sanitize_live_strips_profanity_with_no_script() {
        let out = sanitize_live_transcript("We shipped on time fuck yeah").unwrap();
        assert!(!text_has_profanity(&out));
        assert!(out.contains("shipped"));
    }

    #[test]
    fn sanitize_live_repairs_specifically_pattern() {
        let out =
            sanitize_live_transcript("A few things stood out about Fisher specific. F*CK! Fishers")
                .unwrap();
        assert!(out.contains("specifically"));
        assert!(!text_has_profanity(&out));
    }

    #[test]
    fn sanitize_with_script_hint_keeps_intentional_profanity() {
        let out = sanitize_with_script_hint(
            "That was a fucking disaster",
            Some("That was a fucking disaster in prod"),
        );
        assert!(text_has_profanity(&out));
    }
}
