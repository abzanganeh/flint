//! Mic-only audio capture for mock interview answer recording.
//!
//! This is a lighter version of `audio/pipeline.rs` — it captures only the
//! microphone (no system loopback), runs RNNoise + VAD + Whisper, and emits
//! `MockUserTranscribed` events.  Audio samples are forwarded to the
//! `TurnAudioWriter` so each answer is persisted as a WAV file.
//!
//! Lifecycle:
//!   1. `MicCapture::start()` — spawns a background task.
//!   2. Each VAD chunk that passes Whisper yields a `mock_user_transcribed` event.
//!   3. `MicCapture::stop()` — signals the task, awaits drain, returns final transcript.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, StreamConfig};
use tauri::{AppHandle, Runtime};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::audio::capture::{AudioSource, FRAME_SAMPLES};
use crate::audio::rnnoise::{Downsampler, RNNoiseProcessor};
use crate::audio::vad::{VadChunk, VadChunker};
use crate::events::{emit_mock_user_transcribed, MockUserTranscribedPayload};
use crate::transcription::engine::WhisperEngine;

use super::audio_writer::TurnAudioWriter;

// ── Message types ─────────────────────────────────────────────────────────────

/// Commands sent from commands.rs into the capture loop.
pub enum MicCommand {
    /// Begin recording the user's answer for this turn.
    StartTurn { turn_n: u32 },
    /// Stop recording, flush audio, return transcript + WAV path via channel.
    EndTurn {
        reply: oneshot::Sender<(String, String)>,
    },
    /// Shut down the capture task entirely.
    Shutdown,
}

// ── Public handle ─────────────────────────────────────────────────────────────

/// Handle returned by `MicCapture::start()`.  Drop to stop the capture thread.
pub struct MicCapture {
    cmd_tx: mpsc::Sender<MicCommand>,
    task: JoinHandle<()>,
    /// Signal channel into the cpal OS thread. The cpal thread blocks on the
    /// receiver; dropping the sender (which happens when `MicCapture` is
    /// dropped) wakes the thread so it can drop the `cpal::Stream` and exit
    /// cleanly. Without this, the previous `thread::park()` implementation
    /// leaked one `Stream` per `start_mock` → `stop_mock` cycle.
    _cpal_stop_tx: std::sync::mpsc::Sender<()>,
}

impl MicCapture {
    /// Start the mic capture background task.
    pub fn start<R: Runtime>(
        app: AppHandle<R>,
        session_id: Uuid,
        audio_dir: PathBuf,
        whisper: Arc<WhisperEngine>,
    ) -> Result<Self> {
        let (frame_tx, frame_rx) = mpsc::channel::<Vec<f32>>(512);
        let (cmd_tx, cmd_rx) = mpsc::channel::<MicCommand>(16);

        // cpal must run on a real thread (not a tokio task) because its
        // callback is sync and may be called from an OS audio thread.
        let frame_tx_clone = frame_tx.clone();
        let (stream_ready_tx, stream_ready_rx) = oneshot::channel::<Result<()>>();
        let (cpal_stop_tx, cpal_stop_rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            if let Err(e) = run_cpal_mic_thread(frame_tx_clone, stream_ready_tx, cpal_stop_rx) {
                error!(error = %e, "mock mic cpal thread failed");
            }
        });

        // Block briefly to confirm the stream opened before we store the handle.
        // We use a blocking recv on a dedicated thread to avoid blocking the
        // tokio executor.
        let ready = tauri::async_runtime::block_on(stream_ready_rx)
            .unwrap_or(Err(anyhow::anyhow!("cpal thread died before ready")));
        ready.context("mock mic capture stream init")?;

        let task = tokio::spawn(capture_loop(
            app, session_id, audio_dir, whisper, frame_rx, cmd_rx,
        ));

        Ok(Self {
            cmd_tx,
            task,
            _cpal_stop_tx: cpal_stop_tx,
        })
    }

    /// Begin recording the user's answer for `turn_n`.
    pub async fn start_turn(&self, turn_n: u32) -> Result<()> {
        self.cmd_tx
            .send(MicCommand::StartTurn { turn_n })
            .await
            .context("send StartTurn")
    }

    /// Stop recording and await the transcript + audio path for the turn.
    pub async fn end_turn(&self, timeout: Duration) -> Result<(String, String)> {
        let reply_rx = self.send_end_turn().await?;
        await_end_turn_reply(reply_rx, timeout).await
    }

    /// Send the `EndTurn` command and return the reply receiver without
    /// awaiting it. Callers that hold a session-wide mutex use this to drop
    /// the guard before awaiting the (potentially long) recording shutdown
    /// so concurrent commands (e.g. `stop_mock`) are not blocked.
    pub async fn send_end_turn(&self) -> Result<oneshot::Receiver<(String, String)>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(MicCommand::EndTurn { reply: reply_tx })
            .await
            .context("send EndTurn")?;
        Ok(reply_rx)
    }

    /// Shut down the capture loop and release audio resources.
    pub async fn shutdown(self) {
        let _ = self.cmd_tx.send(MicCommand::Shutdown).await;
        let _ = self.task.await;
        // Dropping `self` releases `_cpal_stop_tx`, which is the signal the
        // cpal OS thread waits on (see `run_cpal_mic_thread`).
    }
}

/// Await the reply from a previously-sent `EndTurn`. Pulled out of
/// `MicCapture::end_turn` so callers that need to release a `Mutex` guard
/// before awaiting can do so via [`MicCapture::send_end_turn`].
pub async fn await_end_turn_reply(
    reply_rx: oneshot::Receiver<(String, String)>,
    timeout: Duration,
) -> Result<(String, String)> {
    tokio::time::timeout(timeout, reply_rx)
        .await
        .context("end_turn timeout")?
        .context("reply channel closed")
}

// ── cpal thread ───────────────────────────────────────────────────────────────

fn run_cpal_mic_thread(
    frame_tx: mpsc::Sender<Vec<f32>>,
    ready_tx: oneshot::Sender<Result<()>>,
    stop_rx: std::sync::mpsc::Receiver<()>,
) -> Result<()> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("no default input device for mock mic")?;

    let (config, _rate) = select_mic_config(&device).context("select mic config")?;

    let tx = frame_tx;
    let stream = device
        .build_input_stream(
            &config,
            move |data: &[f32], _| {
                // Chunk into FRAME_SAMPLES-sized slices.
                for chunk in data.chunks(FRAME_SAMPLES) {
                    let _ = tx.try_send(chunk.to_vec());
                }
            },
            |err| warn!(error = %err, "mock mic stream error"),
            None,
        )
        .context("build mock mic stream")?;
    stream.play().context("start mock mic stream")?;

    let _ = ready_tx.send(Ok(()));
    info!("mock mic stream running");

    // Park here until `MicCapture` is dropped (or `shutdown()` is awaited),
    // which drops `_cpal_stop_tx` and makes this recv return `Err`. The
    // explicit `drop(stream)` below releases the OS audio device so the next
    // `start_mock` does not contend with a leaked handle.
    let _ = stop_rx.recv();
    drop(stream);
    info!("mock mic stream stopped");
    Ok(())
}

fn select_mic_config(device: &Device) -> Result<(StreamConfig, u32)> {
    let default_cfg = device
        .default_input_config()
        .context("get default input config")?;
    let rate = default_cfg.sample_rate().0;
    Ok((default_cfg.into(), rate))
}

// ── Async capture loop ────────────────────────────────────────────────────────

async fn capture_loop<R: Runtime>(
    app: AppHandle<R>,
    session_id: Uuid,
    audio_dir: PathBuf,
    whisper: Arc<WhisperEngine>,
    mut frame_rx: mpsc::Receiver<Vec<f32>>,
    mut cmd_rx: mpsc::Receiver<MicCommand>,
) {
    let mut rnnoise = match RNNoiseProcessor::new() {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "failed to init RNNoise for mock capture");
            return;
        }
    };
    let mut downsampler = match Downsampler::new() {
        Ok(d) => d,
        Err(e) => {
            error!(error = %e, "failed to init downsampler for mock capture");
            return;
        }
    };
    let mut vad = match VadChunker::new() {
        Ok(v) => v,
        Err(e) => {
            error!(error = %e, "failed to init VAD for mock capture");
            return;
        }
    };

    let mut current_turn: Option<u32> = None;
    let mut audio_writer: Option<TurnAudioWriter> = None;
    let mut transcript_buf = String::new();

    loop {
        tokio::select! {
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    MicCommand::StartTurn { turn_n } => {
                        current_turn = Some(turn_n);
                        audio_writer = Some(TurnAudioWriter::new(
                            session_id,
                            turn_n,
                            &audio_dir,
                        ));
                        transcript_buf.clear();
                        info!(turn_n, "mock mic: recording started");
                    }
                    MicCommand::EndTurn { reply } => {
                        let writer = audio_writer.take();
                        let path = writer
                            .map(|w| w.finish().unwrap_or_default())
                            .unwrap_or_default();
                        let text = std::mem::take(&mut transcript_buf);
                        current_turn = None;
                        let _ = reply.send((text, path));
                    }
                    MicCommand::Shutdown => {
                        info!("mock mic: shutdown requested");
                        break;
                    }
                }
            }
            Some(frame) = frame_rx.recv() => {
                if current_turn.is_none() {
                    continue; // not recording — discard frames
                }

                // RNNoise expects exactly 480 f32 samples.
                let mut proc = frame.clone();
                if proc.len() == 480 {
                    let _ = rnnoise.process_frame(&mut proc);
                }

                let downsampled = match downsampler.process(&proc) {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                // Store raw 16kHz samples for the WAV writer.
                if let Some(w) = &mut audio_writer {
                    w.push_samples(&downsampled);
                }

                // Feed into VAD.
                for chunk_frame in downsampled.chunks(160) {
                    if let Some(chunk) = vad.process_frame(chunk_frame, AudioSource::Microphone) {
                        dispatch_chunk(&app, &whisper, chunk, current_turn.unwrap_or(0), &mut transcript_buf).await;
                    }
                }
            }
            else => break,
        }
    }
}

async fn dispatch_chunk<R: Runtime>(
    app: &AppHandle<R>,
    whisper: &Arc<WhisperEngine>,
    chunk: VadChunk,
    turn_n: u32,
    buf: &mut String,
) {
    let w = Arc::clone(whisper);
    let result = tokio::task::spawn_blocking(move || w.transcribe(&chunk)).await;

    let text = match result {
        Ok(Ok(Some(r))) => r.text,
        Ok(Ok(None)) => return,
        Ok(Err(e)) => {
            warn!(error = %e, "mock transcription error");
            return;
        }
        Err(e) => {
            warn!(error = %e, "mock transcription task panicked");
            return;
        }
    };

    if text.trim().is_empty() {
        return;
    }

    if !buf.is_empty() {
        buf.push(' ');
    }
    buf.push_str(text.trim());

    emit_mock_user_transcribed(
        app,
        MockUserTranscribedPayload {
            turn_n,
            text: text.trim().to_owned(),
            audio_path: String::new(), // updated after EndTurn
        },
    );
}
