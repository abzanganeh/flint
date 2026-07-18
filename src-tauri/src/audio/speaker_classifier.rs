//! Tier-2/3 async LLM speaker classifier — Slice 4 (`lpav-s4-speaker-classifier`).
//!
//! Confirms or overrides provisional speaker labels for chunks the earlier,
//! cheap heuristics (channel proxy, RMS+pause, text-shape/near-duplicate
//! suspicion) could not resolve with confidence. Enqueue policy lives in
//! [`crate::audio::pipeline`] (phone mode: every utterance with >= 8 words;
//! dual-stream mode: suspicious chunks only) — this module only owns the
//! queue, the LLM call, and verdict parsing.
//!
//! Runs entirely off the hot audio-pipeline path: [`SpeakerClassifier::enqueue`]
//! is a non-blocking `try_send` onto a bounded channel drained by a single
//! background worker task. A full queue drops the request (with a warning)
//! rather than ever blocking transcription or question detection — matching
//! `.cursor/rules/flint-rust.mdc`'s "never a silent hang" HTTP contract.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::llm::provider::{CompletionConfig, LLMProvider};

/// Bounded queue capacity. Sized generously above expected suspicious-chunk
/// rate (a handful per minute even in phone mode) so backpressure drops are
/// a genuine overload signal, not routine queueing.
const QUEUE_CAPACITY: usize = 32;

/// Hard timeout on one classification call. The classifier never blocks the
/// live path directly, but an unbounded hang would starve the queue for
/// every chunk behind it.
const CLASSIFIER_TIMEOUT: Duration = Duration::from_secs(5);

const PROMPT_CATEGORY: &str = "speaker_classification";

// ────────────────────────────────────────────────────────────────────────────
// Verdict
// ────────────────────────────────────────────────────────────────────────────

/// Confirmed (or corrected) speaker role for one chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifierVerdict {
    Interviewer,
    Candidate,
    /// The model could not confidently pick a role, or the call failed. The
    /// caller should leave the existing provisional label untouched.
    Uncertain,
}

impl ClassifierVerdict {
    /// The chunk-payload speaker string this verdict corresponds to, or
    /// `None` for [`ClassifierVerdict::Uncertain`] (no change to apply).
    pub fn as_speaker(&self) -> Option<&'static str> {
        match self {
            ClassifierVerdict::Interviewer => Some("System"),
            ClassifierVerdict::Candidate => Some("Microphone"),
            ClassifierVerdict::Uncertain => None,
        }
    }

    /// Parse the raw LLM completion. Deliberately lenient (substring match,
    /// case-insensitive) since small/fast models rarely return exactly one
    /// bare word even when instructed to.
    fn parse(raw: &str) -> Self {
        let lower = raw.trim().to_lowercase();
        if lower.contains("interviewer") {
            Self::Interviewer
        } else if lower.contains("candidate") {
            Self::Candidate
        } else {
            Self::Uncertain
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Queue payloads
// ────────────────────────────────────────────────────────────────────────────

/// One chunk queued for Tier-2/3 classification.
#[derive(Debug, Clone)]
pub struct ClassificationRequest {
    pub chunk_id: Uuid,
    pub session_id: Uuid,
    pub text: String,
    /// Current best-guess speaker label (`"System"` / `"Microphone"`),
    /// included in the prompt as context — never trusted as ground truth,
    /// since it is exactly the label the classifier exists to double-check.
    pub current_speaker: String,
}

/// Result delivered to the caller-supplied callback once classification
/// completes.
#[derive(Debug, Clone)]
pub struct ClassificationResult {
    pub chunk_id: Uuid,
    pub session_id: Uuid,
    pub verdict: ClassifierVerdict,
}

// ────────────────────────────────────────────────────────────────────────────
// SpeakerClassifier
// ────────────────────────────────────────────────────────────────────────────

/// Async speaker classifier backed by a bounded mpsc queue and a single
/// background worker task.
pub struct SpeakerClassifier {
    tx: mpsc::Sender<ClassificationRequest>,
}

impl SpeakerClassifier {
    /// Spawn the background worker and return a handle. `on_result` is
    /// invoked (on the worker task) for every completed classification,
    /// including [`ClassifierVerdict::Uncertain`] — callers wire this to
    /// persistence + the `speaker_refined` event (Slice 5) and should treat
    /// `Uncertain` as a no-op.
    ///
    /// Dropping the returned [`SpeakerClassifier`] drops `tx`, which ends
    /// the worker loop (`rx.recv()` returns `None`) — no explicit shutdown
    /// signal or `JoinHandle` needed.
    pub fn spawn<F>(provider: Arc<dyn LLMProvider>, prompts_dir: PathBuf, on_result: F) -> Self
    where
        F: Fn(ClassificationResult) + Send + Sync + 'static,
    {
        let (tx, mut rx) = mpsc::channel::<ClassificationRequest>(QUEUE_CAPACITY);

        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let verdict = classify_one(provider.as_ref(), &prompts_dir, &req).await;
                on_result(ClassificationResult {
                    chunk_id: req.chunk_id,
                    session_id: req.session_id,
                    verdict,
                });
            }
            debug!("speaker classifier worker exited — sender dropped");
        });

        Self { tx }
    }

    /// Non-blocking enqueue. Returns `true` if the request was accepted,
    /// `false` if it was dropped (queue full or worker gone) — never blocks
    /// the audio pipeline.
    pub fn enqueue(&self, request: ClassificationRequest) -> bool {
        match self.tx.try_send(request) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!("speaker classifier queue full — dropping classification request");
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                warn!("speaker classifier worker gone — dropping classification request");
                false
            }
        }
    }
}

async fn classify_one(
    provider: &dyn LLMProvider,
    prompts_dir: &Path,
    req: &ClassificationRequest,
) -> ClassifierVerdict {
    let template = match load_prompt(prompts_dir, provider.name()) {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "speaker classifier prompt load failed");
            return ClassifierVerdict::Uncertain;
        }
    };
    let prompt = build_prompt(&template, req);

    let config = CompletionConfig {
        max_tokens: Some(8),
        temperature: 0.0,
        stream: false,
    };

    match timeout(CLASSIFIER_TIMEOUT, provider.complete(prompt, config)).await {
        Ok(Ok(raw)) => ClassifierVerdict::parse(&raw),
        Ok(Err(e)) => {
            debug!(error = %e, "speaker classifier LLM call failed");
            ClassifierVerdict::Uncertain
        }
        Err(_) => {
            debug!("speaker classifier LLM call timed out");
            ClassifierVerdict::Uncertain
        }
    }
}

fn build_prompt(template: &str, req: &ClassificationRequest) -> String {
    template
        .replace("{utterance}", &req.text)
        .replace("{current_label}", &req.current_speaker)
}

fn load_prompt(prompts_dir: &Path, provider_name: &str) -> Result<String> {
    let category_dir = prompts_dir.join(PROMPT_CATEGORY);
    let specific = category_dir.join(format!("{provider_name}.txt"));
    if specific.exists() {
        return std::fs::read_to_string(&specific)
            .with_context(|| format!("read {}", specific.display()));
    }
    let default = category_dir.join("default.txt");
    std::fs::read_to_string(&default).with_context(|| format!("read {}", default.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{FailingMockLLMProvider, MockLLMProvider};
    use std::time::Duration as StdDuration;
    use tokio::sync::Mutex as AsyncMutex;

    fn write_prompts(dir: &Path) {
        let category = dir.join(PROMPT_CATEGORY);
        std::fs::create_dir_all(&category).unwrap();
        std::fs::write(
            category.join("default.txt"),
            "Utterance: {utterance}\nCurrent label: {current_label}\nAnswer interviewer, candidate, or uncertain.",
        )
        .unwrap();
    }

    #[test]
    fn verdict_as_speaker_maps_correctly() {
        assert_eq!(ClassifierVerdict::Interviewer.as_speaker(), Some("System"));
        assert_eq!(
            ClassifierVerdict::Candidate.as_speaker(),
            Some("Microphone")
        );
        assert_eq!(ClassifierVerdict::Uncertain.as_speaker(), None);
    }

    #[test]
    fn verdict_parse_is_lenient_and_case_insensitive() {
        assert_eq!(
            ClassifierVerdict::parse("Interviewer"),
            ClassifierVerdict::Interviewer
        );
        assert_eq!(
            ClassifierVerdict::parse("  the CANDIDATE  \n"),
            ClassifierVerdict::Candidate
        );
        assert_eq!(
            ClassifierVerdict::parse("not sure"),
            ClassifierVerdict::Uncertain
        );
        assert_eq!(ClassifierVerdict::parse(""), ClassifierVerdict::Uncertain);
    }

    #[test]
    fn build_prompt_substitutes_template_vars() {
        let template = "Text: {utterance} | Label: {current_label}";
        let req = ClassificationRequest {
            chunk_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            text: "tell me about your last role".to_string(),
            current_speaker: "System".to_string(),
        };
        let prompt = build_prompt(template, &req);
        assert_eq!(prompt, "Text: tell me about your last role | Label: System");
    }

    #[test]
    fn load_prompt_prefers_provider_specific_file() {
        let dir = tempfile::tempdir().unwrap();
        let category = dir.path().join(PROMPT_CATEGORY);
        std::fs::create_dir_all(&category).unwrap();
        std::fs::write(category.join("default.txt"), "default template").unwrap();
        std::fs::write(category.join("groq.txt"), "groq template").unwrap();

        assert_eq!(load_prompt(dir.path(), "groq").unwrap(), "groq template");
        assert_eq!(
            load_prompt(dir.path(), "ollama").unwrap(),
            "default template"
        );
    }

    #[test]
    fn load_prompt_errors_when_neither_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(PROMPT_CATEGORY)).unwrap();
        assert!(load_prompt(dir.path(), "groq").is_err());
    }

    #[tokio::test]
    async fn classify_one_parses_mock_provider_response() {
        let dir = tempfile::tempdir().unwrap();
        write_prompts(dir.path());
        let provider: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "interviewer".to_string(),
            provider_name: "mock".to_string(),
        });
        let req = ClassificationRequest {
            chunk_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            text: "tell me about a time you led a project".to_string(),
            current_speaker: "Microphone".to_string(),
        };
        let verdict = classify_one(provider.as_ref(), dir.path(), &req).await;
        assert_eq!(verdict, ClassifierVerdict::Interviewer);
    }

    #[tokio::test]
    async fn classify_one_returns_uncertain_on_provider_failure() {
        let dir = tempfile::tempdir().unwrap();
        write_prompts(dir.path());
        let provider: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
            provider_name: "mock".to_string(),
            error_message: "boom".to_string(),
        });
        let req = ClassificationRequest {
            chunk_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            text: "some utterance".to_string(),
            current_speaker: "System".to_string(),
        };
        let verdict = classify_one(provider.as_ref(), dir.path(), &req).await;
        assert_eq!(verdict, ClassifierVerdict::Uncertain);
    }

    #[tokio::test]
    async fn classify_one_returns_uncertain_when_prompt_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(PROMPT_CATEGORY)).unwrap();
        let provider: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "interviewer".to_string(),
            provider_name: "mock".to_string(),
        });
        let req = ClassificationRequest {
            chunk_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            text: "some utterance".to_string(),
            current_speaker: "System".to_string(),
        };
        let verdict = classify_one(provider.as_ref(), dir.path(), &req).await;
        assert_eq!(verdict, ClassifierVerdict::Uncertain);
    }

    #[tokio::test]
    async fn enqueue_and_worker_deliver_result_via_callback() {
        let dir = tempfile::tempdir().unwrap();
        write_prompts(dir.path());
        let provider: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "candidate".to_string(),
            provider_name: "mock".to_string(),
        });

        let results: Arc<AsyncMutex<Vec<ClassificationResult>>> =
            Arc::new(AsyncMutex::new(Vec::new()));
        let results_clone = Arc::clone(&results);
        let classifier = SpeakerClassifier::spawn(provider, dir.path().to_path_buf(), move |r| {
            let results = Arc::clone(&results_clone);
            tokio::spawn(async move {
                results.lock().await.push(r);
            });
        });

        let chunk_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        assert!(classifier.enqueue(ClassificationRequest {
            chunk_id,
            session_id,
            text: "I led the migration project last year".to_string(),
            current_speaker: "System".to_string(),
        }));

        // Poll briefly for the async worker + callback spawn to land — bounded
        // to keep the test fast and non-flaky under CI load.
        let mut delivered = false;
        for _ in 0..50 {
            tokio::time::sleep(StdDuration::from_millis(20)).await;
            if results.lock().await.iter().any(|r| r.chunk_id == chunk_id) {
                delivered = true;
                break;
            }
        }
        assert!(delivered, "classification result was never delivered");

        let guard = results.lock().await;
        let result = guard.iter().find(|r| r.chunk_id == chunk_id).unwrap();
        assert_eq!(result.session_id, session_id);
        assert_eq!(result.verdict, ClassifierVerdict::Candidate);
    }

    #[test]
    fn enqueue_drops_without_panic_when_queue_is_full() {
        // Build a classifier with no worker draining the channel (rx dropped
        // immediately) — every enqueue after the channel closes must be a
        // graceful `false`, never a panic.
        let (tx, rx) = mpsc::channel::<ClassificationRequest>(1);
        drop(rx);
        let classifier = SpeakerClassifier { tx };
        let accepted = classifier.enqueue(ClassificationRequest {
            chunk_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            text: "test".to_string(),
            current_speaker: "System".to_string(),
        });
        assert!(
            !accepted,
            "enqueue onto a closed channel must return false, not panic"
        );
    }
}
