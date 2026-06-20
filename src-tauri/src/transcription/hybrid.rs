//! Hybrid question detection — M10 Slice 2.
//!
//! Four layers without a 600MB DeBERTa download:
//!   1. Pass 1 rule classifier with confidence scores
//!   2. VAD silence confirmation (1.5s after candidate)
//!   3. Optional single LLM verify via FailoverManager
//!   4. Manual Ctrl+Q via `signal_question_ended` (bypasses all layers)
//!
//! Guard rails: min 5 new words, 8s LLM cooldown, skip unchanged text,
//! cache generic/UNKNOWN LLM responses for 30s.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tauri::{AppHandle, Runtime};
use tracing::{debug, warn};

use crate::llm::failover::FailoverManager;
use crate::llm::provider::CompletionConfig;
use crate::transcription::detector::{pass1, DetectionResult};

const MIN_WORDS: usize = 5;
const LLM_VERIFY_COOLDOWN: Duration = Duration::from_secs(8);
const UNKNOWN_CACHE: Duration = Duration::from_secs(30);
const SILENCE_CONFIRM_MS: u64 = 1500;
const CONFIDENCE_QUESTION: f32 = 0.9;
const CONFIDENCE_AMBIGUOUS: f32 = 0.5;
const CONFIDENCE_NOT_QUESTION: f32 = 0.1;
const CANDIDATE_THRESHOLD: f32 = 0.7;
const SKIP_THRESHOLD: f32 = 0.3;

/// Rolling System-channel transcript since the last Ctrl+Q signal.
#[derive(Debug, Default, Clone)]
pub struct SystemTranscriptBuffer {
    chunks: Vec<String>,
}

impl SystemTranscriptBuffer {
    pub fn append(&mut self, text: &str) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            self.chunks.push(trimmed.to_string());
        }
    }

    /// Drain and join all chunks accumulated since the last manual signal.
    pub fn drain_since_last_signal(&mut self) -> String {
        let joined = self.chunks.join(" ");
        self.chunks.clear();
        joined
    }

    pub fn accumulated_text(&self) -> String {
        self.chunks.join(" ")
    }
}

fn pass1_confidence(result: DetectionResult) -> f32 {
    match result {
        DetectionResult::Question => CONFIDENCE_QUESTION,
        DetectionResult::Ambiguous => CONFIDENCE_AMBIGUOUS,
        DetectionResult::NotAQuestion => CONFIDENCE_NOT_QUESTION,
    }
}

fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

fn is_generic_llm_response(response: &str) -> bool {
    let upper = response.trim().to_uppercase();
    upper.is_empty()
        || upper.contains("UNKNOWN")
        || upper == "MAYBE"
        || upper == "N/A"
        || upper == "NULL"
}

#[derive(Clone)]
struct Candidate {
    text: String,
    #[allow(dead_code)]
    confidence: f32,
    #[allow(dead_code)]
    marked_at: Instant,
}

pub struct HybridQuestionDetector {
    prompt_template: String,
    failover: Arc<FailoverManager>,
    last_checked_snapshot: String,
    candidate: Option<Candidate>,
    last_llm_verify: Option<Instant>,
    unknown_cached_until: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LlmVerifyOutcome {
    Confirmed,
    Rejected,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmPlan {
    Immediate(String),
    WithLlm(String),
}

impl HybridQuestionDetector {
    pub fn new(failover: Arc<FailoverManager>, prompts_dir: &Path) -> Result<Self> {
        let prompt_path = prompts_dir.join("question_detection").join("llama.txt");
        let prompt_template = std::fs::read_to_string(&prompt_path).with_context(|| {
            format!(
                "Failed to load question detection prompt from {}",
                prompt_path.display()
            )
        })?;

        Ok(Self {
            prompt_template,
            failover,
            last_checked_snapshot: String::new(),
            candidate: None,
            last_llm_verify: None,
            unknown_cached_until: None,
        })
    }

    /// Sync ingest of new System transcript text. Returns a confirmation plan
    /// when Pass1 + VAD silence agree on a candidate question.
    pub(crate) fn ingest_transcript(&mut self, text: &str, silence_ms: u64) -> Option<ConfirmPlan> {
        self.stage_transcript(text, silence_ms)
    }

    /// Sync silence tick — confirm a pending candidate when silence threshold met.
    pub(crate) fn check_silence(&mut self, silence_ms: u64) -> Option<ConfirmPlan> {
        self.plan_confirm(silence_ms)
    }

    fn stage_transcript(&mut self, text: &str, silence_ms: u64) -> Option<ConfirmPlan> {
        let accumulated = text.trim();
        if accumulated.is_empty() || accumulated == self.last_checked_snapshot {
            return None;
        }
        if word_count(accumulated) < MIN_WORDS {
            return None;
        }
        if self
            .unknown_cached_until
            .is_some_and(|until| Instant::now() < until)
        {
            return None;
        }

        let normalized = accumulated.to_lowercase();
        let confidence = pass1_confidence(pass1(&normalized));

        if confidence < SKIP_THRESHOLD {
            self.last_checked_snapshot = accumulated.to_string();
            self.candidate = None;
            return None;
        }

        if confidence >= CANDIDATE_THRESHOLD {
            self.candidate = Some(Candidate {
                text: accumulated.to_string(),
                confidence,
                marked_at: Instant::now(),
            });
        }

        self.plan_confirm(silence_ms)
    }

    fn plan_confirm(&mut self, silence_ms: u64) -> Option<ConfirmPlan> {
        let candidate = self.candidate.clone()?;
        if silence_ms < SILENCE_CONFIRM_MS {
            return None;
        }

        self.last_checked_snapshot = candidate.text.clone();
        self.candidate = None;

        if self.llm_verify_allowed() {
            Some(ConfirmPlan::WithLlm(candidate.text))
        } else {
            Some(ConfirmPlan::Immediate(candidate.text))
        }
    }

    fn build_verify_prompt(&self, utterance: &str) -> String {
        let sanitized = utterance.replace('{', "\u{FF5B}").replace('}', "\u{FF5D}");
        self.prompt_template.replace("{utterance}", &sanitized)
    }

    fn apply_llm_outcome(&mut self, outcome: LlmVerifyOutcome, candidate: &str) -> Option<String> {
        match outcome {
            LlmVerifyOutcome::Confirmed => Some(candidate.to_string()),
            LlmVerifyOutcome::Rejected => None,
            LlmVerifyOutcome::Skipped | LlmVerifyOutcome::Failed => Some(candidate.to_string()),
        }
    }

    fn llm_verify_allowed(&self) -> bool {
        if self.failover.is_using_local() {
            return false;
        }
        match self.last_llm_verify {
            None => true,
            Some(at) => Instant::now().duration_since(at) >= LLM_VERIFY_COOLDOWN,
        }
    }
}

/// Run optional LLM verification without holding the detector mutex during the network call.
pub async fn finalize_confirmation<R: Runtime>(
    hybrid: &Arc<tokio::sync::Mutex<HybridQuestionDetector>>,
    candidate: String,
    app: &AppHandle<R>,
) -> Result<Option<String>> {
    let (prompt, failover, skip_llm) = {
        let guard = hybrid.lock().await;
        if !guard.llm_verify_allowed() {
            return Ok(Some(candidate));
        }
        (
            guard.build_verify_prompt(&candidate),
            Arc::clone(&guard.failover),
            false,
        )
    };

    if skip_llm {
        return Ok(Some(candidate));
    }

    {
        let mut guard = hybrid.lock().await;
        guard.last_llm_verify = Some(Instant::now());
    }

    let config = CompletionConfig {
        max_tokens: Some(4),
        temperature: 0.0,
        stream: false,
    };

    let outcome = match failover.complete(prompt, config, app, 200).await {
        Ok(response) => {
            let answer = response.trim().to_uppercase();
            if is_generic_llm_response(&answer) {
                let mut guard = hybrid.lock().await;
                guard.unknown_cached_until = Some(Instant::now() + UNKNOWN_CACHE);
                debug!("LLM verify returned generic response — cached skip for 30s");
                LlmVerifyOutcome::Skipped
            } else {
                match answer.as_str() {
                    "YES" => LlmVerifyOutcome::Confirmed,
                    "NO" => LlmVerifyOutcome::Rejected,
                    other => {
                        warn!(
                            response = other,
                            "LLM verify unexpected response — treating as skip"
                        );
                        let mut guard = hybrid.lock().await;
                        guard.unknown_cached_until = Some(Instant::now() + UNKNOWN_CACHE);
                        LlmVerifyOutcome::Skipped
                    }
                }
            }
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("rate_limit") || msg.contains("rate limit") {
                debug!("LLM verify skipped — rate limited");
                LlmVerifyOutcome::Skipped
            } else {
                warn!(error = %e, "LLM verify failed — using Pass1+VAD only");
                LlmVerifyOutcome::Failed
            }
        }
    };

    let mut guard = hybrid.lock().await;
    Ok(guard.apply_llm_outcome(outcome, &candidate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::failover::FailoverManager;
    use crate::llm::provider::MockLLMProvider;
    use crate::llm::rate_limiter::RateLimiter;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};

    fn prompts_dir() -> PathBuf {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        PathBuf::from(manifest).join("../prompts")
    }

    fn mock_app_handle() -> tauri::AppHandle<MockRuntime> {
        mock_builder()
            .build(mock_context(noop_assets()))
            .expect("mock app")
            .handle()
            .clone()
    }

    fn mock_failover(response: &str) -> Arc<FailoverManager> {
        let primary: Arc<dyn crate::llm::provider::LLMProvider> = Arc::new(MockLLMProvider {
            response: response.to_string(),
            provider_name: "mock".to_string(),
        });
        let local: Arc<dyn crate::llm::provider::LLMProvider> = Arc::new(MockLLMProvider {
            response: "NO".to_string(),
            provider_name: "local".to_string(),
        });
        let rl = Arc::new(RateLimiter::new("mock", 60, 60_000));
        Arc::new(FailoverManager::new(primary, vec![], local, rl))
    }

    #[test]
    fn system_buffer_drains_since_last_signal() {
        let mut buf = SystemTranscriptBuffer::default();
        buf.append("Tell me");
        buf.append("about yourself.");
        assert_eq!(buf.accumulated_text(), "Tell me about yourself.");
        let drained = buf.drain_since_last_signal();
        assert_eq!(drained, "Tell me about yourself.");
        assert!(buf.accumulated_text().is_empty());
    }

    #[test]
    fn pass1_confidence_maps_correctly() {
        assert!((pass1_confidence(pass1("what motivates you")) - 0.9).abs() < f32::EPSILON);
        assert!((pass1_confidence(pass1("that is interesting")) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn generic_llm_responses_detected() {
        assert!(is_generic_llm_response("UNKNOWN"));
        assert!(is_generic_llm_response("MAYBE"));
        assert!(!is_generic_llm_response("YES"));
    }

    #[test]
    fn no_new_text_skips_detection() {
        let failover = mock_failover("YES");
        let mut detector = HybridQuestionDetector::new(failover, &prompts_dir()).unwrap();
        let text = "what motivates you in your daily work here";
        assert!(detector.ingest_transcript(text, 0).is_none());
        assert!(detector.ingest_transcript(text, 0).is_none());
    }

    #[test]
    fn low_confidence_skips() {
        let failover = mock_failover("YES");
        let mut detector = HybridQuestionDetector::new(failover, &prompts_dir()).unwrap();
        detector.last_checked_snapshot.clear();
        assert!(detector
            .ingest_transcript(
                "okay sure I understand that part completely",
                SILENCE_CONFIRM_MS,
            )
            .is_none());
    }

    #[tokio::test]
    async fn high_confidence_with_silence_triggers_llm_verify() {
        let failover = mock_failover("YES");
        let hybrid = Arc::new(tokio::sync::Mutex::new(
            HybridQuestionDetector::new(failover, &prompts_dir()).unwrap(),
        ));
        let app = mock_app_handle();
        let plan = {
            let mut detector = hybrid.lock().await;
            detector.ingest_transcript(
                "tell me about a challenge you faced recently at work",
                SILENCE_CONFIRM_MS,
            )
        };
        assert!(matches!(plan, Some(ConfirmPlan::WithLlm(_))));
        if let Some(ConfirmPlan::WithLlm(text)) = plan {
            let result = finalize_confirmation(&hybrid, text, &app).await.unwrap();
            assert!(result.is_some());
        }
    }

    #[tokio::test]
    async fn llm_no_rejects_candidate() {
        let failover = mock_failover("NO");
        let hybrid = Arc::new(tokio::sync::Mutex::new(
            HybridQuestionDetector::new(failover, &prompts_dir()).unwrap(),
        ));
        let app = mock_app_handle();
        let plan = {
            let mut detector = hybrid.lock().await;
            detector.ingest_transcript(
                "walk me through your experience with distributed systems",
                SILENCE_CONFIRM_MS,
            )
        };
        if let Some(ConfirmPlan::WithLlm(text)) = plan {
            let result = finalize_confirmation(&hybrid, text, &app).await.unwrap();
            assert!(result.is_none());
        }
    }

    #[tokio::test]
    async fn generic_unknown_response_cached() {
        let failover = mock_failover("UNKNOWN");
        let hybrid = Arc::new(tokio::sync::Mutex::new(
            HybridQuestionDetector::new(failover, &prompts_dir()).unwrap(),
        ));
        let app = mock_app_handle();
        let plan = {
            let mut detector = hybrid.lock().await;
            detector.ingest_transcript(
                "describe your approach to testing production systems",
                SILENCE_CONFIRM_MS,
            )
        };
        if let Some(ConfirmPlan::WithLlm(text)) = plan {
            let _ = finalize_confirmation(&hybrid, text, &app).await.unwrap();
        }
        let detector = hybrid.lock().await;
        assert!(detector.unknown_cached_until.is_some());
    }
}
