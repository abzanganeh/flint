//! Post-process Whisper mock-interview transcripts before persistence and coaching.
//!
//! Whisper sometimes hallucinates profanity — especially after partial word matches
//! (e.g. "specifically" → "specific" + expletive). We repair known patterns and
//! drop isolated profanity when the prepared script contains none.

/// Sanitize a mock turn transcript using optional script context.
pub fn sanitize_mock_transcript(transcript: &str, script_hint: Option<&str>) -> String {
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

/// True when text contains a profanity token (after normalizing punctuation).
pub fn text_has_profanity(text: &str) -> bool {
    text.split_whitespace().any(is_profanity_word)
}

/// Whisper often emits "specific" + profanity when the speaker said "specifically".
fn repair_specifically_profanity_hallucination(words: &mut Vec<String>) {
    let mut i = 0;
    while i < words.len() {
        if is_specific_stem(&words[i]) {
            if i + 1 < words.len() && is_profanity_word(&words[i + 1]) {
                words[i] = punctuate_replacement("specifically", &words[i], &words[i + 1]);
                words.remove(i + 1);
                continue;
            }
        }
        i += 1;
    }
}

fn punctuate_replacement(replacement: &str, before: &str, after: &str) -> String {
    if after.contains('!')
        || after.contains('?')
        || before.ends_with('.')
        || before.ends_with(',')
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
