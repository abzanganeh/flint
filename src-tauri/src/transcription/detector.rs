//! Two-pass question detection — Task 3.6.
//!
//! ## Design
//! Pass 1 is a deterministic rule-based classifier (~0ms).
//! Pass 2 is an Ollama Llama 3.2 1B binary classifier (Tier 2+ only, 250ms timeout).
//!
//! Pass 2 is only invoked when Pass 1 returns `Ambiguous`. On Tier 1 the
//! ambiguous result resolves to `false` immediately without calling Ollama.
//!
//! ## Latency enforcement
//! A rolling window of the last 100 detect() call durations is maintained.
//! When the P95 of that window exceeds 200ms, Pass 2 is disabled until the
//! rolling P95 recovers below 150ms. This matches NFR-28.
//!
//! ## Prompt
//! Loaded from `/prompts/question_detection/llama.txt` at construction.
//! Template variable: `{utterance}`. NEVER inline prompts as string literals.
//!
//! ## Security
//! Utterance text is injected into the `[Statement]` block only. It is
//! treated as data — never interpolated into the system role.

#![allow(dead_code)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::Mutex;

use crate::health::hardware::HardwareTier;
use crate::llm::provider::{CompletionConfig, LLMProvider};

// ────────────────────────────────────────────────────────────────────────────
// Constants
// ────────────────────────────────────────────────────────────────────────────

/// Ollama timeout for Pass 2 (ms). Falls back to Pass 1 if exceeded.
const PASS2_TIMEOUT_MS: u64 = 250;

/// Rolling window size for P95 latency tracking.
const LATENCY_WINDOW: usize = 100;

/// Minimum samples in the window before P95 enforcement activates.
/// Below this count the estimate is too noisy to act on.
const P95_MIN_SAMPLES: usize = 20;

/// NFR-28: if rolling P95 exceeds this, disable Pass 2.
const P95_DISABLE_THRESHOLD_MS: u128 = 200;

/// Pass 2 re-enables when P95 drops below this.
/// The 80ms gap provides hysteresis — widen to 120/200 if toggling is observed.
const P95_REENABLE_THRESHOLD_MS: u128 = 120;

// ────────────────────────────────────────────────────────────────────────────
// Pass 1 keyword sets
// ────────────────────────────────────────────────────────────────────────────

/// Utterance-opening phrases that reliably signal a question or request.
/// Matched case-insensitively against the normalised utterance prefix.
const QUESTION_PREFIXES: &[&str] = &[
    "what ",
    "what's ",
    "how ",
    "why ",
    "when ",
    "where ",
    "who ",
    "which ",
    "whose ",
    "whom ",
    "can you ",
    "could you ",
    "would you ",
    "should you ",
    "tell me ",
    "tell us ",
    "walk me ",
    "walk us ",
    "describe ",
    "explain ",
    "help me understand ",
    "give me ",
    "give us ",
    "share with ",
    "talk to me about ",
    "let's talk about ",
    "let's chat about ",
    "let's discuss ",
    "maybe we can ",
    "i'd love to hear ",
    "i would love to hear ",
    "i'd like to hear ",
    "i'd like to know ",
    "i'm curious ",
    "i am curious ",
    "we'd like to know ",
];

/// Mid-utterance phrases that signal a conversational invitation even when
/// the utterance does not OPEN with a question pattern. Interviewers often
/// soften asks with filler: "So, um, maybe we can chat a bit about what you
/// enjoy most in your work." These run only on the SYSTEM (interviewer)
/// channel, so the false-positive cost is an extra suggestion — far cheaper
/// than silently dropping a real question.
const INVITATION_PHRASES: &[&str] = &[
    "chat a bit about",
    "chat about what",
    "talk a bit about",
    "talk about what",
    "tell me about",
    "tell us about",
    "hear about your",
    "hear more about",
    "curious about",
    "love to hear",
    "like to know",
    "like to hear",
    "interested to hear",
    "interested in hearing",
    "walk me through",
    "walk us through",
];

// ────────────────────────────────────────────────────────────────────────────
// Public types
// ────────────────────────────────────────────────────────────────────────────

/// Internal classification used by Pass 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionResult {
    Question,
    NotAQuestion,
    /// Pass 1 cannot determine; Pass 2 is needed.
    Ambiguous,
}

pub struct QuestionDetector {
    ollama: Option<Arc<dyn LLMProvider>>,
    tier: HardwareTier,
    prompt_template: String,
    /// Rolling latency samples (detect() wall-clock ms).
    latency_window: Arc<Mutex<Vec<u128>>>,
    /// True when Pass 2 is suppressed due to rolling P95 > threshold.
    pass2_bypassed: Arc<AtomicBool>,
}

impl QuestionDetector {
    /// Build a `QuestionDetector`.
    ///
    /// `prompts_dir` is the path to the `prompts/` directory. The prompt file
    /// is loaded eagerly so missing files fail at startup, not at runtime.
    ///
    /// `ollama` may be `None` when the caller chooses not to configure a local
    /// provider (e.g. when running tests). On Tier 1 or when `ollama` is
    /// `None`, only Pass 1 runs regardless of tier.
    pub fn new(
        tier: HardwareTier,
        ollama: Option<Arc<dyn LLMProvider>>,
        prompts_dir: &Path,
    ) -> Result<Self> {
        let prompt_path = prompts_dir.join("question_detection").join("llama.txt");
        let prompt_template = std::fs::read_to_string(&prompt_path).with_context(|| {
            format!(
                "Failed to load question detection prompt from {}",
                prompt_path.display()
            )
        })?;

        Ok(Self {
            ollama,
            tier,
            prompt_template,
            latency_window: Arc::new(Mutex::new(Vec::with_capacity(LATENCY_WINDOW))),
            pass2_bypassed: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Detect whether `utterance` is a question.
    ///
    /// Returns `true` if the utterance is a question or request for information,
    /// `false` otherwise.
    ///
    /// Latency target: P95 ≤ 100ms (NFR). Pass 2 is auto-bypassed if the
    /// rolling P95 of this method exceeds 200ms (NFR-28).
    ///
    /// The wall-clock time recorded for P95 tracking includes the full
    /// `detect_inner` duration — Pass 1 + optional Pass 2 + optional timeout
    /// wait. This ensures the 200ms P95 gate reflects what the calling
    /// pipeline actually waited for.
    pub async fn detect(&self, utterance: &str) -> Result<bool> {
        let start = Instant::now();
        let result = self.detect_inner(utterance).await?;
        let elapsed_ms = start.elapsed().as_millis();

        self.record_latency(elapsed_ms).await;

        Ok(result)
    }

    // ── Internal ──────────────────────────────────────────────────────────

    async fn detect_inner(&self, utterance: &str) -> Result<bool> {
        let normalized = utterance.trim().to_lowercase();

        match pass1(&normalized) {
            DetectionResult::Question => return Ok(true),
            DetectionResult::NotAQuestion => return Ok(false),
            DetectionResult::Ambiguous => {}
        }

        // Tier 1 always falls back to Pass 1 result for ambiguous cases.
        if self.tier < 2 {
            return Ok(false);
        }

        // Pass 2 may be bypassed due to rolling latency degradation.
        if self.pass2_bypassed.load(Ordering::Relaxed) {
            tracing::warn!("Pass 2 bypassed — rolling P95 above threshold; using Pass 1 fallback");
            return Ok(false);
        }

        if let Some(ollama) = &self.ollama {
            match self.run_pass2(utterance, ollama.as_ref()).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    tracing::warn!(error = %e, "Pass 2 failed; using Pass 1 fallback");
                    return Ok(false);
                }
            }
        }

        // No Ollama provider configured — treat ambiguous as not a question.
        Ok(false)
    }

    async fn run_pass2(&self, utterance: &str, provider: &dyn LLMProvider) -> Result<bool> {
        // Sanitize the utterance before template substitution. Brace
        // characters could accidentally match other template variables if
        // the utterance contains a literal `{variable}` pattern, corrupting
        // the prompt structure. Replace bare braces with their lookalike
        // Unicode alternatives so the text is preserved but is inert.
        let sanitized = utterance
            .replace('{', "\u{FF5B}") // ｛ FULLWIDTH LEFT CURLY BRACKET
            .replace('}', "\u{FF5D}"); // ｝ FULLWIDTH RIGHT CURLY BRACKET
        let prompt = self.prompt_template.replace("{utterance}", &sanitized);

        let config = CompletionConfig {
            max_tokens: Some(4),
            temperature: 0.0,
            stream: false,
        };

        let timeout = Duration::from_millis(PASS2_TIMEOUT_MS);
        let response = tokio::time::timeout(timeout, provider.complete(prompt, config))
            .await
            .context("Pass 2 timed out")?
            .context("Pass 2 completion failed")?;

        let answer = response.trim().to_uppercase();
        match answer.as_str() {
            "YES" => Ok(true),
            "NO" => Ok(false),
            other => {
                // Unexpected response — fall back to Pass 1 result (ambiguous = false).
                tracing::warn!(
                    response = other,
                    "Pass 2 returned unexpected response; using Pass 1 fallback"
                );
                Ok(false)
            }
        }
    }

    async fn record_latency(&self, elapsed_ms: u128) {
        let mut window = self.latency_window.lock().await;

        if window.len() == LATENCY_WINDOW {
            window.remove(0);
        }
        window.push(elapsed_ms);

        if window.len() < P95_MIN_SAMPLES {
            return;
        }

        let p95 = percentile_95(&window);
        let currently_bypassed = self.pass2_bypassed.load(Ordering::Relaxed);

        if !currently_bypassed && p95 > P95_DISABLE_THRESHOLD_MS {
            self.pass2_bypassed.store(true, Ordering::Relaxed);
            tracing::warn!(
                p95_ms = p95,
                "Question detector Pass 2 disabled — rolling P95 exceeded {}ms",
                P95_DISABLE_THRESHOLD_MS
            );
        } else if currently_bypassed && p95 < P95_REENABLE_THRESHOLD_MS {
            self.pass2_bypassed.store(false, Ordering::Relaxed);
            tracing::info!(
                p95_ms = p95,
                "Question detector Pass 2 re-enabled — rolling P95 recovered below {}ms",
                P95_REENABLE_THRESHOLD_MS
            );
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Pass 1 — rule-based classifier
// ────────────────────────────────────────────────────────────────────────────

/// Rule-based question classification.
///
/// Input must be pre-normalised (trimmed, lowercased).
///
/// Decision tree:
/// 1. Ends with `?` → `Question`
/// 2. Starts with a known question prefix → `Question`
/// 3. Contains a conversational invitation phrase → `Question`
/// 4. Otherwise → `Ambiguous`
///
/// This pass never returns `NotAQuestion` — the rule set covers known-question
/// patterns but cannot rule out statement-form questions, so unknown cases are
/// escalated to Pass 2 rather than rejected.
fn pass1(normalized: &str) -> DetectionResult {
    if normalized.ends_with('?') {
        return DetectionResult::Question;
    }

    for prefix in QUESTION_PREFIXES {
        if normalized.starts_with(prefix) {
            return DetectionResult::Question;
        }
    }

    for phrase in INVITATION_PHRASES {
        if normalized.contains(phrase) {
            return DetectionResult::Question;
        }
    }

    DetectionResult::Ambiguous
}

// ────────────────────────────────────────────────────────────────────────────
// Latency helpers
// ────────────────────────────────────────────────────────────────────────────

/// P95 of a non-empty slice (nearest rank method).
fn percentile_95(samples: &[u128]) -> u128 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let idx = ((sorted.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
    sorted[idx.min(sorted.len() - 1)]
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::MockLLMProvider;
    use std::path::PathBuf;
    use std::sync::Arc;

    // ── Helpers ───────────────────────────────────────────────────────────

    fn prompts_dir() -> PathBuf {
        // Cargo sets CARGO_MANIFEST_DIR to the crate root during tests.
        let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
        PathBuf::from(manifest).join("../prompts")
    }

    fn detector_tier1() -> QuestionDetector {
        QuestionDetector::new(1, None, &prompts_dir()).expect("Failed to build detector")
    }

    fn detector_with_mock(response: &str) -> QuestionDetector {
        let mock = Arc::new(MockLLMProvider {
            response: response.to_string(),
            provider_name: "llama".to_string(),
        });
        QuestionDetector::new(2, Some(mock), &prompts_dir()).expect("Failed to build detector")
    }

    // ── Pass 1 — explicit question mark ──────────────────────────────────

    #[test]
    fn question_mark_detected() {
        assert_eq!(pass1("tell me about yourself?"), DetectionResult::Question);
        assert_eq!(
            pass1("what is your experience with rust?"),
            DetectionResult::Question
        );
    }

    // ── Pass 1 — prefix matching ──────────────────────────────────────────

    #[test]
    fn question_prefix_what_detected() {
        assert_eq!(pass1("what motivates you"), DetectionResult::Question);
    }

    #[test]
    fn question_prefix_how_detected() {
        assert_eq!(pass1("how did you handle that"), DetectionResult::Question);
    }

    #[test]
    fn question_prefix_tell_me_detected() {
        assert_eq!(
            pass1("tell me about a challenge"),
            DetectionResult::Question
        );
    }

    #[test]
    fn question_prefix_walk_me_detected() {
        assert_eq!(
            pass1("walk me through your decision"),
            DetectionResult::Question
        );
    }

    #[test]
    fn question_prefix_describe_detected() {
        assert_eq!(pass1("describe your team"), DetectionResult::Question);
    }

    #[test]
    fn question_prefix_explain_detected() {
        assert_eq!(
            pass1("explain your approach to testing"),
            DetectionResult::Question
        );
    }

    #[test]
    fn statement_is_ambiguous() {
        assert_eq!(pass1("that is interesting"), DetectionResult::Ambiguous);
        assert_eq!(pass1("i see, go on"), DetectionResult::Ambiguous);
        assert_eq!(pass1("okay"), DetectionResult::Ambiguous);
    }

    // ── Pass 1 — conversational invitations ──────────────────────────────

    #[test]
    fn invitation_maybe_we_can_chat_detected() {
        // Real missed question from a live session.
        assert_eq!(
            pass1(
                "maybe we can chat a bit about what you enjoy most in your \
                 work and what keeps you motivated day to day"
            ),
            DetectionResult::Question
        );
    }

    #[test]
    fn invitation_with_leading_filler_detected() {
        // Filler before the invitation defeats prefix matching; the
        // contains-based pass must still catch it.
        assert_eq!(
            pass1("so, um, i'd be interested to hear about your last project"),
            DetectionResult::Question
        );
        assert_eq!(
            pass1("great, now tell me about a time you failed"),
            DetectionResult::Question
        );
    }

    #[test]
    fn invitation_prefixes_detected() {
        assert_eq!(
            pass1("let's talk about your leadership style"),
            DetectionResult::Question
        );
        assert_eq!(
            pass1("i'm curious how you handle conflict"),
            DetectionResult::Question
        );
        assert_eq!(
            pass1("share with us an example of a difficult decision"),
            DetectionResult::Question
        );
    }

    // ── Tier 1 — ambiguous resolves to false ─────────────────────────────

    #[tokio::test]
    async fn tier1_ambiguous_returns_false() {
        let detector = detector_tier1();
        let result = detector.detect("that is interesting").await.unwrap();
        assert!(!result, "Tier 1 ambiguous should return false");
    }

    #[tokio::test]
    async fn tier1_clear_question_returns_true() {
        let detector = detector_tier1();
        let result = detector.detect("what motivates you").await.unwrap();
        assert!(result);
    }

    // ── Tier 2 — Pass 2 called on ambiguous ──────────────────────────────

    #[tokio::test]
    async fn tier2_pass2_yes_response_returns_true() {
        let detector = detector_with_mock("YES");
        let result = detector
            .detect("that is quite the challenge")
            .await
            .unwrap();
        assert!(result, "Pass 2 YES should return true");
    }

    #[tokio::test]
    async fn tier2_pass2_no_response_returns_false() {
        let detector = detector_with_mock("NO");
        let result = detector.detect("i see, thank you").await.unwrap();
        assert!(!result, "Pass 2 NO should return false");
    }

    #[tokio::test]
    async fn tier2_pass2_unexpected_response_falls_back_to_false() {
        let detector = detector_with_mock("MAYBE");
        let _result = detector.detect("so tell us something").await.unwrap();
        // "tell us something" → Pass 1 Question (starts with "tell ") — confirmed true
        // because Pass 1 catches it before Pass 2 is even called.
        // Use a truly ambiguous utterance for this test.
        let result2 = detector.detect("that sounds great").await.unwrap();
        assert!(
            !result2,
            "Unexpected Pass 2 response should fall back to false"
        );
    }

    #[tokio::test]
    async fn tier2_clear_question_resolved_by_pass1_without_pass2() {
        // Even on Tier 2, a clear "?" question never reaches Pass 2.
        let detector = detector_with_mock("NO"); // Pass 2 would say NO — but is never called
        let result = detector
            .detect("can you describe your approach?")
            .await
            .unwrap();
        assert!(
            result,
            "Clear question should be true regardless of mock response"
        );
    }

    // ── P95 latency tracking ──────────────────────────────────────────────

    #[test]
    fn percentile_95_basic() {
        let samples: Vec<u128> = (1..=100).collect();
        let p95 = percentile_95(&samples);
        assert_eq!(p95, 95, "P95 of 1..=100 should be 95");
    }

    #[test]
    fn percentile_95_single_element() {
        assert_eq!(percentile_95(&[42]), 42);
    }

    #[test]
    fn percentile_95_all_same() {
        let samples = vec![10u128; 50];
        assert_eq!(percentile_95(&samples), 10);
    }
}
