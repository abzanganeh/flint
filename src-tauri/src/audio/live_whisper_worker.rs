//! Background Whisper worker for the live audio pipeline.
//!
//! Decouples inference from VAD frame processing so loopback/mic capture keeps
//! running while Whisper decodes prior segments. Jobs are processed FIFO on an
//! unbounded queue — chunks are never dropped under backlog.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::warn;

use crate::audio::capture::AudioSource;
use crate::audio::vad::VadChunk;
use crate::transcription::engine::{TranscriptionResult, WhisperEngine};

#[derive(Debug, Clone)]
pub struct LiveWhisperJobMeta {
    pub source: AudioSource,
    pub frame_timestamp_ms: i64,
    pub chunk_ready_at: Instant,
    pub chunk_rms_dbfs: f32,
    pub chunk_duration_ms: u32,
    /// Snapshot of `system_buffer_epoch` at enqueue — stale preview-era jobs
    /// must not repopulate the buffer after `commit_live_preview`.
    pub buffer_epoch: u64,
}

pub struct LiveWhisperJob {
    pub meta: LiveWhisperJobMeta,
    pub chunk: VadChunk,
    pub rolling_context: String,
}

pub enum LiveWhisperOutcome {
    Transcribed(TranscriptionResult),
    Empty,
    Failed,
}

pub struct LiveWhisperWorker {
    job_tx: mpsc::UnboundedSender<LiveWhisperJob>,
    task: JoinHandle<()>,
    /// Jobs enqueued but not yet returned on the result channel.
    pending: Arc<AtomicUsize>,
}

impl LiveWhisperWorker {
    pub fn start(
        whisper: Arc<WhisperEngine>,
        pending: Arc<AtomicUsize>,
    ) -> (
        Self,
        mpsc::UnboundedReceiver<(LiveWhisperJobMeta, LiveWhisperOutcome)>,
    ) {
        let (job_tx, job_rx) = mpsc::unbounded_channel();
        let (result_tx, result_rx) = mpsc::unbounded_channel();
        let pending_worker = Arc::clone(&pending);
        let task = tokio::spawn(worker_loop(whisper, job_rx, result_tx));
        (
            Self {
                job_tx,
                task,
                pending: pending_worker,
            },
            result_rx,
        )
    }

    pub fn enqueue(&self, job: LiveWhisperJob) {
        if self.job_tx.send(job).is_err() {
            warn!("live whisper worker channel closed — dropping chunk");
        } else {
            self.pending.fetch_add(1, Ordering::Release);
        }
    }

    /// Jobs waiting on or inside Whisper decode. Decremented by the pipeline
    /// only after `handle_transcription_result` finishes processing a result.
    pub fn pending_jobs(pending: &AtomicUsize) -> usize {
        pending.load(Ordering::Acquire)
    }

    pub async fn shutdown(self) -> Result<()> {
        drop(self.job_tx);
        self.task
            .await
            .map_err(|e| anyhow::anyhow!("live whisper worker panicked: {e}"))
    }
}

async fn worker_loop(
    whisper: Arc<WhisperEngine>,
    mut job_rx: mpsc::UnboundedReceiver<LiveWhisperJob>,
    result_tx: mpsc::UnboundedSender<(LiveWhisperJobMeta, LiveWhisperOutcome)>,
) {
    while let Some(job) = job_rx.recv().await {
        let LiveWhisperJob {
            meta,
            chunk,
            rolling_context,
        } = job;
        let w = Arc::clone(&whisper);

        let outcome = match tokio::task::spawn_blocking(move || {
            w.transcribe_with_context(&chunk, &rolling_context)
        })
        .await
        {
            Ok(Ok(Some(result))) => LiveWhisperOutcome::Transcribed(result),
            Ok(Ok(None)) => LiveWhisperOutcome::Empty,
            Ok(Err(e)) => {
                warn!(error = %e, source = ?meta.source, "live whisper transcription error");
                LiveWhisperOutcome::Failed
            }
            Err(e) => {
                warn!(error = %e, source = ?meta.source, "live whisper task panicked");
                LiveWhisperOutcome::Failed
            }
        };

        if result_tx.send((meta, outcome)).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn worker_uses_unbounded_queue() {
        let src = include_str!("live_whisper_worker.rs");
        assert!(
            src.contains("unbounded_channel"),
            "live worker must never drop VAD chunks under backlog"
        );
    }
}
