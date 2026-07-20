//! Background Whisper worker for mock interview STT.
//!
//! Decouples inference from the mic frame drain loop. Emits **cumulative**
//! `full_transcript` so the UI replaces text instead of appending overlapping
//! chunk fragments while the worker drains its FIFO backlog.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tauri::{AppHandle, Runtime};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::audio::vad::VadChunk;
use crate::events::{emit_mock_user_transcribed, MockUserTranscribedPayload};
use crate::transcription::engine::WhisperEngine;
use crate::transcription::rolling_context::RollingTranscriptContext;

pub type TurnEpoch = u64;

/// Minimum avg logprob to append decoded text into rolling context (avoids poisoning).
const ROLLING_CONTEXT_LOGPROB_MIN: f32 = -0.85;

pub const WHISPER_FLUSH_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct TurnTranscript {
    pub text: String,
    pub confidence: Option<f32>,
}

enum WorkerCommand {
    ResetContext {
        turn_n: u32,
        epoch: TurnEpoch,
    },
    Transcribe {
        epoch: TurnEpoch,
        turn_n: u32,
        chunk: VadChunk,
    },
    Flush {
        epoch: TurnEpoch,
        turn_n: u32,
        reply: oneshot::Sender<TurnTranscript>,
    },
    Shutdown,
}

pub struct WhisperWorker {
    cmd_tx: mpsc::UnboundedSender<WorkerCommand>,
    task: JoinHandle<()>,
}

impl WhisperWorker {
    pub fn start<R: Runtime>(app: AppHandle<R>, whisper: Arc<WhisperEngine>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(worker_loop(app, whisper, cmd_rx));
        Self { cmd_tx, task }
    }

    pub async fn reset_context(&self, turn_n: u32, epoch: TurnEpoch) -> Result<()> {
        self.cmd_tx
            .send(WorkerCommand::ResetContext { turn_n, epoch })
            .context("whisper worker reset send")
    }

    /// Enqueue transcription without blocking the capture hot path (never drops chunks).
    pub fn enqueue_transcribe(&self, epoch: TurnEpoch, turn_n: u32, chunk: VadChunk) {
        if self
            .cmd_tx
            .send(WorkerCommand::Transcribe {
                epoch,
                turn_n,
                chunk,
            })
            .is_err()
        {
            warn!(turn_n, "mock whisper worker channel closed");
        }
    }

    pub async fn transcribe_blocking(
        &self,
        epoch: TurnEpoch,
        turn_n: u32,
        chunk: VadChunk,
    ) -> Result<()> {
        self.cmd_tx
            .send(WorkerCommand::Transcribe {
                epoch,
                turn_n,
                chunk,
            })
            .context("whisper worker transcribe send")
    }

    pub async fn flush(&self, epoch: TurnEpoch, turn_n: u32, timeout: Duration) -> TurnTranscript {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(WorkerCommand::Flush {
                epoch,
                turn_n,
                reply: reply_tx,
            })
            .is_err()
        {
            return TurnTranscript {
                text: String::new(),
                confidence: None,
            };
        }

        match tokio::time::timeout(timeout, reply_rx).await {
            Ok(Ok(transcript)) => transcript,
            Ok(Err(_)) | Err(_) => {
                warn!("mock whisper flush timed out or reply channel closed");
                TurnTranscript {
                    text: String::new(),
                    confidence: None,
                }
            }
        }
    }

    pub async fn shutdown(self) {
        let _ = self.cmd_tx.send(WorkerCommand::Shutdown);
        let _ = self.task.await;
    }
}

async fn worker_loop<R: Runtime>(
    app: AppHandle<R>,
    whisper: Arc<WhisperEngine>,
    mut cmd_rx: mpsc::UnboundedReceiver<WorkerCommand>,
) {
    let mut active_epoch = TurnEpoch::MAX;
    let mut transcript_buf = String::new();
    let mut rolling = RollingTranscriptContext::default();
    let mut logprob_sum: f32 = 0.0;
    let mut logprob_count: u32 = 0;

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            WorkerCommand::ResetContext { turn_n, epoch } => {
                active_epoch = epoch;
                debug!(turn_n, epoch, "mock whisper worker context reset");
                transcript_buf.clear();
                rolling.clear();
                logprob_sum = 0.0;
                logprob_count = 0;
            }
            WorkerCommand::Transcribe {
                epoch,
                turn_n,
                chunk,
            } => {
                if epoch != active_epoch {
                    continue;
                }
                let ctx = rolling.as_str();
                let w = Arc::clone(&whisper);
                let result =
                    tokio::task::spawn_blocking(move || w.transcribe_with_context(&chunk, &ctx))
                        .await;

                let transcription = match result {
                    Ok(Ok(Some(r))) => r,
                    Ok(Ok(None)) => continue,
                    Ok(Err(e)) => {
                        warn!(error = %e, turn_n, "mock transcription error");
                        continue;
                    }
                    Err(e) => {
                        warn!(error = %e, turn_n, "mock transcription task panicked");
                        continue;
                    }
                };

                if epoch != active_epoch {
                    continue;
                }

                let text = transcription.text.trim().to_string();
                if text.is_empty() {
                    continue;
                }

                if !transcript_buf.is_empty() {
                    transcript_buf.push(' ');
                }
                transcript_buf.push_str(&text);

                if transcription
                    .avg_logprob
                    .is_some_and(|lp| lp >= ROLLING_CONTEXT_LOGPROB_MIN)
                {
                    rolling.append(&text);
                }

                if let Some(lp) = transcription.avg_logprob {
                    logprob_sum += lp;
                    logprob_count += 1;
                }

                emit_mock_user_transcribed(
                    &app,
                    MockUserTranscribedPayload {
                        turn_n,
                        text: text.clone(),
                        full_transcript: transcript_buf.clone(),
                        is_final: false,
                        audio_path: String::new(),
                    },
                );
            }
            WorkerCommand::Flush {
                epoch,
                turn_n,
                reply,
            } => {
                let transcript = if epoch == active_epoch {
                    TurnTranscript {
                        text: transcript_buf.clone(),
                        confidence: if logprob_count > 0 {
                            Some(logprob_sum / logprob_count as f32)
                        } else {
                            None
                        },
                    }
                } else {
                    TurnTranscript {
                        text: String::new(),
                        confidence: None,
                    }
                };

                if epoch == active_epoch && !transcript.text.is_empty() {
                    emit_mock_user_transcribed(
                        &app,
                        MockUserTranscribedPayload {
                            turn_n,
                            text: String::new(),
                            full_transcript: transcript.text.clone(),
                            is_final: true,
                            audio_path: String::new(),
                        },
                    );
                }

                let _ = reply.send(transcript);
            }
            WorkerCommand::Shutdown => break,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn worker_uses_unbounded_queue() {
        let src = include_str!("whisper_worker.rs");
        assert!(
            src.contains("unbounded_channel"),
            "worker must use unbounded channel so transcribe jobs are never dropped"
        );
        assert!(
            src.contains("enqueue_transcribe"),
            "worker must expose non-blocking enqueue API"
        );
    }
}
