//! Background Whisper worker for mock interview STT.
//!
//! Decouples inference from the mic frame drain loop so capture never blocks
//! on whisper.cpp decode (which caused channel overflow, false pause, and
//! truncated transcripts on long answers).

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

/// Monotonic per-attempt identifier; stale jobs tagged with an old epoch are dropped.
pub type TurnEpoch = u64;

const WORKER_QUEUE_DEPTH: usize = 8;
pub const WHISPER_FLUSH_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct TurnTranscript {
    pub text: String,
    pub confidence: Option<f32>,
}

enum WorkerCommand {
    ResetContext { turn_n: u32, epoch: TurnEpoch },
    Transcribe {
        epoch: TurnEpoch,
        turn_n: u32,
        chunk: VadChunk,
    },
    Flush {
        epoch: TurnEpoch,
        reply: oneshot::Sender<TurnTranscript>,
    },
    Shutdown,
}

pub struct WhisperWorker {
    cmd_tx: mpsc::Sender<WorkerCommand>,
    task: JoinHandle<()>,
}

impl WhisperWorker {
    pub fn start<R: Runtime>(app: AppHandle<R>, whisper: Arc<WhisperEngine>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel(WORKER_QUEUE_DEPTH);
        let task = tokio::spawn(worker_loop(app, whisper, cmd_rx));
        Self { cmd_tx, task }
    }

    pub async fn reset_context(&self, turn_n: u32, epoch: TurnEpoch) -> Result<()> {
        self.cmd_tx
            .send(WorkerCommand::ResetContext { turn_n, epoch })
            .await
            .context("whisper worker reset send")
    }

    /// Enqueue transcription without blocking the capture hot path.
    pub fn try_transcribe(&self, epoch: TurnEpoch, turn_n: u32, chunk: VadChunk) -> bool {
        match self.cmd_tx.try_send(WorkerCommand::Transcribe {
            epoch,
            turn_n,
            chunk,
        }) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!(turn_n, "mock whisper worker queue full — dropping VAD chunk");
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    /// Blocking enqueue for EndTurn tail flush (capture loop is already shutting down).
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
            .await
            .context("whisper worker transcribe send")
    }

    pub async fn flush(&self, epoch: TurnEpoch, timeout: Duration) -> TurnTranscript {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(WorkerCommand::Flush {
                epoch,
                reply: reply_tx,
            })
            .await
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
        let _ = self.cmd_tx.send(WorkerCommand::Shutdown).await;
        let _ = self.task.await;
    }
}

async fn worker_loop<R: Runtime>(
    app: AppHandle<R>,
    whisper: Arc<WhisperEngine>,
    mut cmd_rx: mpsc::Receiver<WorkerCommand>,
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

                // AbortTurn may have bumped epoch while Whisper was running.
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
                rolling.append(&text);

                if let Some(lp) = transcription.avg_logprob {
                    logprob_sum += lp;
                    logprob_count += 1;
                }

                emit_mock_user_transcribed(
                    &app,
                    MockUserTranscribedPayload {
                        turn_n,
                        text: text.clone(),
                        audio_path: String::new(),
                    },
                );
            }
            WorkerCommand::Flush { epoch, reply } => {
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
                let _ = reply.send(transcript);
            }
            WorkerCommand::Shutdown => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_queue_depth_is_bounded() {
        assert!(WORKER_QUEUE_DEPTH >= 4 && WORKER_QUEUE_DEPTH <= 32);
    }

    #[test]
    fn flush_timeout_covers_end_turn_contract() {
        assert!(WHISPER_FLUSH_TIMEOUT >= Duration::from_secs(30));
    }
}
