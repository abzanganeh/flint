//! Mock interview conductor — sequences questions and fires the suggested-answer
//! LLM thread for each turn.
//!
//! Questions come from `Digest::likely_questions` (pre-warmed, role-specific).
//! The conductor owns the turn state machine:
//!   QuestionAsked → UserAnswering → TurnComplete → (next turn | MockEnded)

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use tauri::{AppHandle, Runtime};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::{info, warn};
use uuid::Uuid;

use crate::digest::Digest;
use crate::events::{
    emit_mock_ended, emit_mock_question_started, emit_mock_suggested_token, MockEndedPayload,
    MockQuestionStartedPayload, MockSuggestedTokenPayload,
};
use crate::interfaces::vector::VectorInterface;
use crate::knowledge::{GlobalKnowledgeBase, PackId};
use crate::llm::failover::FailoverManager;
use crate::llm::provider::CompletionConfig;
use crate::orchestrator::load_prompt;
use crate::rag::embedder::Embedder;
use crate::session::persistence::SessionPersistence;
use crate::session::shuffle::{session_shuffle_seed, shuffle_strings};

use super::rag::{format_digest_context, query_mock_rag};
use super::tts;

// ── Pace & mode ───────────────────────────────────────────────────────────────

/// Controls when the conductor speaks the next question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockPace {
    /// User clicks "Ask question" before each question is spoken.
    Guided,
    /// First question fires immediately; subsequent questions follow each turn.
    Continuous,
}

/// Practice hides the suggested script until after the user answers.
/// Study shows the script during the turn and coaches delivery, not content depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockMode {
    Practice,
    Study,
}

impl MockMode {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "study" => Self::Study,
            _ => Self::Practice,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Practice => "practice",
            Self::Study => "study",
        }
    }
}

// ── Channel message ───────────────────────────────────────────────────────────

/// Sent from `commands.rs` → `Conductor` to advance the turn machine.
pub enum ConductorCommand {
    /// User is ready for the next question (guided mode only).
    AskQuestion,
    /// User has finished answering (or pressed Skip).
    TurnComplete {
        user_text: String,
        audio_path: String,
    },
    /// User cancels — tear down without a summary screen.
    Abort,
    /// User ends early — emit `mock_ended` so the UI can show results.
    FinishEarly,
}

// ── Conductor ─────────────────────────────────────────────────────────────────

pub struct Conductor {
    pub cmd_tx: mpsc::Sender<ConductorCommand>,
}

impl Conductor {
    /// Start the conductor loop and return a handle.
    #[allow(clippy::too_many_arguments)]
    pub fn start<R: Runtime>(
        app: AppHandle<R>,
        session_id: Uuid,
        digest: Arc<Digest>,
        failover: Arc<FailoverManager>,
        persistence: Arc<SessionPersistence>,
        prompts_dir: PathBuf,
        embedder: Arc<Embedder>,
        vector_store: Arc<dyn VectorInterface>,
        global_kb: Arc<GlobalKnowledgeBase>,
        role_packs: Vec<PackId>,
        suggested_buffer: Arc<RwLock<String>>,
        pace: MockPace,
        mode: MockMode,
        shuffle: bool,
        active_turn_n: Arc<AtomicU32>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<ConductorCommand>(8);
        tokio::spawn(conductor_loop(
            app,
            session_id,
            digest,
            failover,
            persistence,
            prompts_dir,
            embedder,
            vector_store,
            global_kb,
            role_packs,
            suggested_buffer,
            pace,
            mode,
            shuffle,
            active_turn_n,
            cmd_rx,
        ));
        Self { cmd_tx }
    }
}

// ── Loop ──────────────────────────────────────────────────────────────────────

/// Max AI-generated follow-up questions appended after the scripted questions.
const MAX_FOLLOW_UPS: usize = 3;

#[allow(clippy::too_many_arguments)]
async fn conductor_loop<R: Runtime>(
    app: AppHandle<R>,
    session_id: Uuid,
    digest: Arc<Digest>,
    failover: Arc<FailoverManager>,
    persistence: Arc<SessionPersistence>,
    prompts_dir: PathBuf,
    embedder: Arc<Embedder>,
    vector_store: Arc<dyn VectorInterface>,
    global_kb: Arc<GlobalKnowledgeBase>,
    role_packs: Vec<PackId>,
    suggested_buffer: Arc<RwLock<String>>,
    pace: MockPace,
    mode: MockMode,
    shuffle: bool,
    active_turn_n: Arc<AtomicU32>,
    mut cmd_rx: mpsc::Receiver<ConductorCommand>,
) {
    let mut base_questions: Vec<String> = digest
        .likely_questions
        .iter()
        .filter(|q| !persistence.is_question_satisfied(session_id, q))
        .cloned()
        .collect();
    if base_questions.is_empty() && !digest.likely_questions.is_empty() {
        info!(
            session_id = %session_id,
            "all digest questions already practiced satisfactorily — ending mock"
        );
        emit_mock_ended(
            &app,
            MockEndedPayload {
                session_id: session_id.to_string(),
                turns_completed: 0,
            },
        );
        return;
    }
    if shuffle && base_questions.len() > 1 {
        shuffle_strings(&mut base_questions, session_shuffle_seed(session_id));
        info!(
            session_id = %session_id,
            count = base_questions.len(),
            "mock question order shuffled"
        );
    }
    // Follow-ups generated async during each turn; collected into a queue after the
    // scripted questions are exhausted.
    let mut followup_handles: Vec<JoinHandle<Option<String>>> = Vec::new();
    let mut followup_queue: VecDeque<String> = VecDeque::new();

    let mut turns_completed: u32 = 0;
    let mut cancelled = false;

    // Build an iterator over both scripted and (eventually) dynamic questions.
    // We process the scripted list first, then drain followup_queue afterward.
    let scripted_count = base_questions.len();
    let mut question_idx: usize = 0;

    while question_idx < scripted_count || !followup_queue.is_empty() {
        let is_followup = question_idx >= scripted_count;
        let question: String = if !is_followup {
            base_questions[question_idx].clone()
        } else {
            match followup_queue.pop_front() {
                Some(q) => q,
                None => break,
            }
        };

        let turn_n = turns_completed + 1;
        // When in the follow-up phase the current question has already been
        // popped from the queue, so add 1 back to get the real total count.
        let total_questions_now =
            scripted_count as u32 + followup_queue.len() as u32 + u32::from(is_followup);

        if pace == MockPace::Guided {
            match wait_for_ask_question(&mut cmd_rx, session_id).await {
                WaitOutcome::Ask => {}
                WaitOutcome::FinishEarly => break,
                WaitOutcome::Cancelled => {
                    cancelled = true;
                    break;
                }
            }
        }

        if let Ok(mut buf) = suggested_buffer.write() {
            buf.clear();
        }

        let rag_chunks = query_mock_rag(
            session_id,
            &question,
            &embedder,
            vector_store.as_ref(),
            Some((global_kb.as_ref(), &role_packs)),
            8,
        )
        .await;

        let turn_id = match persistence.begin_mock_turn(session_id, turn_n, &question) {
            Ok(id) => id,
            Err(e) => {
                warn!(error = %e, "failed to persist mock turn row");
                Uuid::new_v4()
            }
        };

        active_turn_n.store(turn_n, Ordering::SeqCst);

        emit_mock_question_started(
            &app,
            MockQuestionStartedPayload {
                question: question.clone(),
                turn_n,
                total_questions: total_questions_now,
                mode: mode.as_str().to_string(),
            },
        );
        info!(session_id = %session_id, turn_n, mode = mode.as_str(), "mock question started");

        tts::speak_best_effort(&question).await;

        let suggested_handle = {
            let app_clone = app.clone();
            let failover_clone = Arc::clone(&failover);
            let prompts_dir_clone = prompts_dir.clone();
            let rag_clone = rag_chunks.clone();
            let digest_clone = Arc::clone(&digest);
            let buffer_clone = Arc::clone(&suggested_buffer);
            let q = question.clone();
            tokio::spawn(async move {
                run_suggested_answer(
                    app_clone,
                    session_id,
                    &q,
                    &rag_clone,
                    &digest_clone,
                    &failover_clone,
                    &prompts_dir_clone,
                    mode,
                    buffer_clone,
                )
                .await
                .unwrap_or_default()
            })
        };

        let cmd = cmd_rx.recv().await;
        let suggested_text = suggested_handle.await.unwrap_or_default();

        match cmd {
            Some(ConductorCommand::TurnComplete {
                user_text,
                audio_path,
            }) => {
                if let Err(e) = persistence.update_mock_turn_user_answer(
                    turn_id,
                    &user_text,
                    &audio_path,
                    &suggested_text,
                ) {
                    warn!(error = %e, "failed to persist mock turn user answer");
                }
                turns_completed += 1;

                // Kick off a background follow-up generation if we haven't
                // exceeded the cap and we're still in the scripted phase.
                if question_idx < scripted_count && followup_handles.len() < MAX_FOLLOW_UPS {
                    let fq = question.clone();
                    let fa = user_text.clone();
                    let fd = Arc::clone(&digest);
                    let ff = Arc::clone(&failover);
                    let fp = prompts_dir.clone();
                    let fa_app = app.clone();
                    followup_handles.push(tokio::spawn(async move {
                        generate_follow_up(&fa_app, &fq, &fa, &fd, &ff, &fp)
                            .await
                            .ok()
                            .flatten()
                    }));
                }
            }
            Some(ConductorCommand::AskQuestion) => {
                warn!(session_id = %session_id, "unexpected AskQuestion during turn");
            }
            Some(ConductorCommand::FinishEarly) => {
                info!(session_id = %session_id, "mock interview finish early");
                break;
            }
            Some(ConductorCommand::Abort) | None => {
                info!(session_id = %session_id, "mock interview cancelled");
                cancelled = true;
                break;
            }
        }

        question_idx += 1;

        // When scripted questions are done, collect follow-up results concurrently.
        if question_idx >= scripted_count
            && followup_queue.is_empty()
            && !followup_handles.is_empty()
        {
            let results = futures::future::join_all(followup_handles.drain(..)).await;
            for result in results {
                if let Ok(Some(q)) = result {
                    followup_queue.push_back(q);
                }
            }
            info!(
                session_id = %session_id,
                follow_ups = followup_queue.len(),
                "follow-up questions ready"
            );
        }
    }

    // Cancel any in-flight follow-up generation tasks — they are no longer needed.
    for handle in followup_handles {
        handle.abort();
    }

    if !cancelled {
        emit_mock_ended(
            &app,
            MockEndedPayload {
                session_id: session_id.to_string(),
                turns_completed,
            },
        );
        info!(
            session_id = %session_id,
            turns_completed,
            "mock interview ended"
        );
    }
}

/// Generate one targeted follow-up question based on what the user actually said.
/// Returns `None` on LLM failure or if the response is clearly not a question.
async fn generate_follow_up<R: Runtime>(
    app: &AppHandle<R>,
    question: &str,
    user_answer: &str,
    digest: &Digest,
    failover: &Arc<FailoverManager>,
    prompts_dir: &Path,
) -> Result<Option<String>> {
    if user_answer.trim().is_empty() {
        return Ok(None);
    }

    let template = load_prompt(
        "mock_followup",
        failover.active_provider_name(),
        prompts_dir,
    )
    .context("follow-up prompt not found")?;

    let prompt = template
        .replace("{domain}", &digest.domain)
        .replace("{role}", &digest.role)
        .replace("{question}", question)
        .replace("{user_answer}", user_answer);

    let config = CompletionConfig {
        max_tokens: Some(60),
        temperature: 0.6,
        stream: false,
    };

    let text = timeout(
        Duration::from_secs(20),
        failover.complete(prompt, config, app, 60),
    )
    .await
    .context("follow-up generation timed out")?
    .context("follow-up LLM failed")?;

    let cleaned = text.trim().trim_matches('"').trim().to_string();

    if !generate_follow_up_validate(&cleaned) {
        return Ok(None);
    }

    Ok(Some(cleaned))
}

enum WaitOutcome {
    Ask,
    FinishEarly,
    Cancelled,
}

async fn wait_for_ask_question(
    cmd_rx: &mut mpsc::Receiver<ConductorCommand>,
    session_id: Uuid,
) -> WaitOutcome {
    loop {
        match cmd_rx.recv().await {
            Some(ConductorCommand::AskQuestion) => return WaitOutcome::Ask,
            Some(ConductorCommand::FinishEarly) => return WaitOutcome::FinishEarly,
            Some(ConductorCommand::Abort) | None => return WaitOutcome::Cancelled,
            Some(ConductorCommand::TurnComplete { .. }) => {
                warn!(
                    session_id = %session_id,
                    "TurnComplete received while waiting for AskQuestion — ignored"
                );
            }
        }
    }
}

// ── Suggested answer LLM ──────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn run_suggested_answer<R: Runtime>(
    app: AppHandle<R>,
    session_id: Uuid,
    question: &str,
    rag_chunks: &[crate::interfaces::vector::ScoredChunk],
    digest: &Digest,
    failover: &Arc<FailoverManager>,
    prompts_dir: &Path,
    mode: MockMode,
    suggested_buffer: Arc<RwLock<String>>,
) -> Result<String> {
    let prompt = build_suggested_prompt(
        question,
        rag_chunks,
        digest,
        failover.active_provider_name(),
        prompts_dir,
    )?;

    let config = CompletionConfig {
        max_tokens: Some(200),
        temperature: 0.3,
        stream: true,
    };

    let mut stream = failover
        .complete_stream(prompt, config, &app, 200)
        .await
        .context("suggested answer stream failed")?;

    let mut full = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);

    while std::time::Instant::now() < deadline {
        match timeout(Duration::from_secs(15), stream.next()).await {
            Ok(Some(Ok(token))) => {
                full.push_str(&token);
                if let Ok(mut buf) = suggested_buffer.write() {
                    buf.push_str(&token);
                }
                // Practice mode: buffer only — reveal after the user finishes answering.
                if mode == MockMode::Study {
                    emit_mock_suggested_token(&app, MockSuggestedTokenPayload { token });
                }
            }
            Ok(Some(Err(e))) => return Err(e).context("suggested token error"),
            Ok(None) => break,
            Err(_) => {
                warn!(session_id = %session_id, "suggested stream stalled");
                break;
            }
        }
    }

    Ok(full)
}

fn build_suggested_prompt(
    question: &str,
    rag_chunks: &[crate::interfaces::vector::ScoredChunk],
    digest: &Digest,
    provider: &str,
    prompts_dir: &Path,
) -> Result<String> {
    let template = load_prompt("mock_suggested", provider, prompts_dir)?;
    let rag_text = rag_chunks
        .iter()
        .take(5)
        .map(|c| c.chunk.text.as_str())
        .collect::<Vec<_>>()
        .join("\n---\n");

    let prompt = template
        .replace("{session_domain}", &digest.domain)
        .replace("{seniority}", &digest.seniority)
        .replace("{company}", &digest.company)
        .replace("{digest_context}", &format_digest_context(digest))
        .replace("{rag_chunks}", &rag_text)
        .replace("{last_n_turns}", "")
        .replace("{question}", question);
    Ok(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_up_rejects_non_questions() {
        assert!(generate_follow_up_validate("Tell me more?"));
        assert!(!generate_follow_up_validate(""));
        assert!(!generate_follow_up_validate("not a question"));
        assert!(!generate_follow_up_validate(&"x".repeat(201)));
    }
}

/// Extracted validation for follow-up LLM output (unit-tested).
fn generate_follow_up_validate(cleaned: &str) -> bool {
    !cleaned.is_empty() && cleaned.len() <= 200 && cleaned.ends_with('?')
}
