//! Mock interview conductor — sequences questions and fires the suggested-answer
//! LLM thread for each turn.
//!
//! Questions come from `Digest::likely_questions` (pre-warmed, role-specific).
//! The conductor owns the turn state machine:
//!   QuestionAsked → UserAnswering → TurnComplete → (next turn | MockEnded)

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use tauri::{AppHandle, Runtime};
use tokio::sync::mpsc;
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
use crate::session::persistence::{MockTurn, SessionPersistence};

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
    /// User exits mock mode mid-session.
    Abort,
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
            cmd_rx,
        ));
        Self { cmd_tx }
    }
}

// ── Loop ──────────────────────────────────────────────────────────────────────

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
    mut cmd_rx: mpsc::Receiver<ConductorCommand>,
) {
    let questions: Vec<String> = digest.likely_questions.clone();
    let total = questions.len() as u32;
    let mut turns_completed: u32 = 0;

    for (idx, question) in questions.iter().enumerate() {
        let turn_n = idx as u32 + 1;

        if pace == MockPace::Guided && !wait_for_ask_question(&mut cmd_rx, session_id).await {
            break;
        }

        if let Ok(mut buf) = suggested_buffer.write() {
            buf.clear();
        }

        let rag_chunks = query_mock_rag(
            session_id,
            question,
            &embedder,
            vector_store.as_ref(),
            Some((global_kb.as_ref(), &role_packs)),
            8,
        )
        .await;

        let turn = MockTurn {
            id: Uuid::new_v4(),
            session_id,
            turn_n,
            question: question.clone(),
            user_text: String::new(),
            audio_path: String::new(),
            coach_json: String::new(),
            suggested: String::new(),
            score: 0,
        };
        if let Err(e) = persistence.write_mock_turn(&turn) {
            warn!(error = %e, "failed to persist mock turn row");
        }

        emit_mock_question_started(
            &app,
            MockQuestionStartedPayload {
                question: question.clone(),
                turn_n,
                total_questions: total,
                mode: mode.as_str().to_string(),
            },
        );
        info!(session_id = %session_id, turn_n, mode = mode.as_str(), "mock question started");

        tts::speak_best_effort(question).await;

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
                    turn.id,
                    &user_text,
                    &audio_path,
                    &suggested_text,
                ) {
                    warn!(error = %e, "failed to persist mock turn user answer");
                }
                turns_completed += 1;
            }
            Some(ConductorCommand::AskQuestion) => {
                warn!(session_id = %session_id, "unexpected AskQuestion during turn");
            }
            Some(ConductorCommand::Abort) | None => {
                info!(session_id = %session_id, "mock interview aborted");
                break;
            }
        }
    }

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

async fn wait_for_ask_question(
    cmd_rx: &mut mpsc::Receiver<ConductorCommand>,
    session_id: Uuid,
) -> bool {
    loop {
        match cmd_rx.recv().await {
            Some(ConductorCommand::AskQuestion) => return true,
            Some(ConductorCommand::Abort) | None => return false,
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
