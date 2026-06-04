//! Response confidence scoring — computed locally without an LLM round-trip
//! (design doc §21).
//!
//! Formula:
//!   score = 0.50 × rag_grounding
//!         + 0.25 × response_quality
//!         + 0.15 × model_tier
//!         − 0.10 × cache_staleness_penalty
//!
//! The result maps to one of five visual bands used by the UI panels.

// ──────────────────────────────────────────────────────────────────────────────
// Confidence band
// ──────────────────────────────────────────────────────────────────────────────

/// The visual confidence level reported to the React layer via the
/// `confidence_score` Tauri event.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfidenceLevel {
    /// score ≥ 0.75 — Green — "Grounded"
    Green,
    /// 0.55 ≤ score < 0.75 — Blue — "Partial"
    Blue,
    /// 0.35 ≤ score < 0.55 — Amber — "Uncertain"
    Amber,
    /// score < 0.35 — Amber + tooltip — "Limited prep context"
    AmberLow,
    /// No score applicable — Grey — "Clarifying question"
    Grey,
    /// Local Ollama fallback active — Red border
    Red,
}

impl ConfidenceLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Blue => "blue",
            Self::Amber => "amber",
            Self::AmberLow => "amber_low",
            Self::Grey => "grey",
            Self::Red => "red",
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Model tier table (§21)
// ──────────────────────────────────────────────────────────────────────────────

/// Static per-model reliability score used in the confidence formula.
pub fn model_tier_score(provider_name: &str) -> f32 {
    match provider_name {
        "claude-3-5-sonnet" | "claude" => 1.00,
        "gpt-4o" => 0.95,
        "llama-3-3-70b-versatile" | "groq" => 0.85,
        "claude-3-5-haiku" => 0.85,
        "gpt-4o-mini" => 0.80,
        "llama3.1:8b" | "ollama-8b" => 0.60,
        "llama3.2:3b" | "ollama-3b" => 0.45,
        // Llama 1B is used for classification only; treat as lowest tier for
        // response generation quality.
        "llama3.2:1b" | "ollama-1b" => 0.30,
        // Unknown provider — conservative default.
        _ => 0.60,
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Input signals
// ──────────────────────────────────────────────────────────────────────────────

/// All signals required to compute a single confidence score.
#[derive(Debug, Clone)]
pub struct ConfidenceSignals {
    /// Mean cosine similarity of the top-3 retrieved RAG chunks (0.0–1.0).
    pub rag_grounding: f32,
    /// Raw response text — used to detect hedging and refusal patterns.
    pub response_text: String,
    /// Top RAG chunk texts — used for lexical overlap scoring.
    pub rag_texts: Vec<String>,
    /// Name of the active LLM provider/model (matched against the tier table).
    pub provider_name: String,
    /// Whether this response was served from the pre-warm cache AND more than
    /// 3 turns have elapsed since the session started (design doc §21 staleness
    /// rule: turns 1–3 are exempt from staleness penalty).
    pub cache_stale: bool,
    /// True when the local Ollama fallback is active.
    pub local_fallback_active: bool,
    /// Current turn number in the session (1-indexed).
    pub turn_number: usize,
}

// ──────────────────────────────────────────────────────────────────────────────
// Hedge / refusal detection
// ──────────────────────────────────────────────────────────────────────────────

const HEDGE_PATTERNS: &[&str] = &[
    "i'm not sure",
    "i am not sure",
    "i don't know",
    "i do not know",
    "it depends",
    "hard to say",
    "it's unclear",
    "i cannot",
    "i can't",
    "unclear",
    "not certain",
    "possibly",
    "perhaps",
    "might be",
];

const REFUSAL_PATTERNS: &[&str] = &[
    "as an ai",
    "as a language model",
    "i cannot provide",
    "i'm unable to",
    "i am unable to",
    "i don't have access",
    "i do not have access",
    "not able to assist",
    "outside my knowledge",
];

/// Compute lexical overlap between response words and RAG chunk vocabulary.
fn lexical_overlap(response: &str, rag_texts: &[String]) -> f32 {
    if rag_texts.is_empty() {
        return 0.5;
    }
    let rag_lower: Vec<String> = rag_texts
        .iter()
        .flat_map(|t| {
            t.to_lowercase()
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    if rag_lower.is_empty() {
        return 0.5;
    }
    let response_lower = response.to_lowercase();
    let resp_words: Vec<&str> = response_lower.split_whitespace().collect();
    if resp_words.is_empty() {
        return 0.0;
    }
    let hits = resp_words
        .iter()
        .filter(|w| rag_lower.iter().any(|r| r == *w))
        .count();
    hits as f32 / resp_words.len() as f32
}

/// Compute a response quality signal (0.0–1.0) from the response text.
///
/// Blends hedge/refusal heuristics with lexical overlap against RAG chunks.
fn response_quality(text: &str, rag_texts: &[String]) -> f32 {
    let lower = text.to_lowercase();

    let hedge_hits = HEDGE_PATTERNS
        .iter()
        .filter(|&&p| lower.contains(p))
        .count();

    let refusal_hits = REFUSAL_PATTERNS
        .iter()
        .filter(|&&p| lower.contains(p))
        .count();

    let hedge_penalty = (hedge_hits as f32 * 0.15).min(0.30);
    let refusal_penalty = (refusal_hits as f32 * 0.40).min(0.40);

    let hedge_score = (1.0_f32 - hedge_penalty - refusal_penalty).max(0.0);
    let overlap = lexical_overlap(text, rag_texts);
    (0.6 * hedge_score + 0.4 * overlap).clamp(0.0, 1.0)
}

// ──────────────────────────────────────────────────────────────────────────────
// Main scorer
// ──────────────────────────────────────────────────────────────────────────────

/// Compute the confidence score and resolve it to a [`ConfidenceLevel`].
pub fn compute_confidence(signals: &ConfidenceSignals) -> (f32, ConfidenceLevel) {
    if signals.local_fallback_active {
        return (0.0, ConfidenceLevel::Red);
    }

    let staleness_penalty = if signals.cache_stale && signals.turn_number > 3 {
        0.10
    } else {
        0.0
    };

    let rq = response_quality(&signals.response_text, &signals.rag_texts);
    let tier = model_tier_score(&signals.provider_name);

    let score = (0.50 * signals.rag_grounding)
        + (0.25 * rq)
        + (0.15 * tier)
        - staleness_penalty;

    let score = score.clamp(0.0, 1.0);

    let level = if score >= 0.75 {
        ConfidenceLevel::Green
    } else if score >= 0.55 {
        ConfidenceLevel::Blue
    } else if score >= 0.35 {
        ConfidenceLevel::Amber
    } else {
        ConfidenceLevel::AmberLow
    };

    (score, level)
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Test signal builder — responses overlap with RAG vocabulary so that
    /// `lexical_overlap` is high (1.0 in these fixtures).
    fn signals(rag: f32, response: &str, provider: &str, cache_stale: bool, turn: usize) -> ConfidenceSignals {
        ConfidenceSignals {
            rag_grounding: rag,
            response_text: response.to_string(),
            rag_texts: vec![response.to_string()],
            provider_name: provider.to_string(),
            cache_stale,
            local_fallback_active: false,
            turn_number: turn,
        }
    }

    #[test]
    fn green_band_high_quality_groq() {
        // rag=0.90, quality blended (hedge=1.0 × 0.6 + overlap=1.0 × 0.4 = 1.0),
        // tier=0.85; score = 0.45 + 0.25 + 0.1275 = 0.8275
        let (score, level) = compute_confidence(&signals(0.90, "The answer is X.", "groq", false, 1));
        assert!(score >= 0.75, "score={score}");
        assert_eq!(level, ConfidenceLevel::Green);
    }

    #[test]
    fn blue_band_medium_rag() {
        let (score, level) = compute_confidence(&signals(0.60, "The answer is Y.", "groq", false, 2));
        assert!((0.55..0.75).contains(&score), "score={score}");
        assert_eq!(level, ConfidenceLevel::Blue);
    }

    #[test]
    fn amber_band_hedged_response() {
        // rag=0.20, hedge_score=0.70 (two hedges), overlap=1.0,
        // quality = 0.6×0.70 + 0.4×1.0 = 0.82
        // score = 0.50×0.20 + 0.25×0.82 + 0.15×0.85 = 0.10 + 0.205 + 0.1275 = 0.4325
        let (score, level) =
            compute_confidence(&signals(0.20, "I'm not sure, it depends.", "groq", false, 1));
        assert!((0.35..0.55).contains(&score), "score={score}");
        assert_eq!(level, ConfidenceLevel::Amber);
    }

    #[test]
    fn amber_low_band_refusal() {
        let (score, level) = compute_confidence(&signals(
            0.05,
            "As an AI language model, I cannot provide this.",
            "groq",
            false,
            1,
        ));
        assert!(score < 0.35, "score={score}");
        assert_eq!(level, ConfidenceLevel::AmberLow);
    }

    #[test]
    fn lexical_overlap_zero_when_response_misses_rag_vocab() {
        let mut sig = signals(0.50, "The answer is irrelevant.", "groq", false, 1);
        sig.rag_texts = vec!["completely different vocabulary here".to_string()];
        let (score, _) = compute_confidence(&sig);
        // Lower than the matching-overlap green-test because overlap drags quality down.
        assert!(score < 0.65, "expected reduced score with no overlap, got {score}");
    }

    #[test]
    fn red_band_local_fallback() {
        let mut sig = signals(0.90, "Great answer.", "ollama-8b", false, 1);
        sig.local_fallback_active = true;
        let (_, level) = compute_confidence(&sig);
        assert_eq!(level, ConfidenceLevel::Red);
    }

    #[test]
    fn staleness_penalty_applied_after_turn_3() {
        // Without penalty: rag=0.80, quality=1.0, tier=0.85 → 0.40 + 0.25 + 0.1275 = 0.7775
        // With penalty (−0.10): 0.6775
        let fresh = compute_confidence(&signals(0.80, "Answer.", "groq", true, 2));
        let stale = compute_confidence(&signals(0.80, "Answer.", "groq", true, 4));
        assert!(fresh.0 > stale.0, "staleness penalty not applied");
        assert!((fresh.0 - stale.0 - 0.10).abs() < 0.01, "penalty magnitude wrong");
    }

    #[test]
    fn staleness_penalty_not_applied_on_turn_3_or_earlier() {
        let at_turn_3 = compute_confidence(&signals(0.80, "Answer.", "groq", true, 3));
        let no_cache = compute_confidence(&signals(0.80, "Answer.", "groq", false, 3));
        assert_eq!(at_turn_3.0, no_cache.0, "turn 3 should not incur staleness penalty");
    }

    #[test]
    fn model_tier_known_providers() {
        assert_eq!(model_tier_score("groq"), 0.85);
        assert_eq!(model_tier_score("claude"), 1.00);
        assert_eq!(model_tier_score("gpt-4o"), 0.95);
        assert_eq!(model_tier_score("ollama-8b"), 0.60);
    }

    #[test]
    fn score_clamped_to_unit_range() {
        // Artificially high inputs should not exceed 1.0.
        let (score, _) = compute_confidence(&signals(1.0, "Perfect.", "claude", false, 1));
        assert!(score <= 1.0);
        assert!(score >= 0.0);
    }
}
