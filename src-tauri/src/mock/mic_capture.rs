//! Mic-only audio capture for mock interview answer recording.
//!
//! This is a lighter version of `audio/pipeline.rs` — it captures only the
//! microphone (no system loopback), runs RNNoise + VAD + Whisper, and emits
//! `MockUserTranscribed` events.  Audio samples are forwarded to the
//! `TurnAudioWriter` so each answer is persisted as a WAV file.
//!
//! Lifecycle:
//!   1. `MicCapture::start()` — spawns the async capture loop (no device open).
//!   2. `MicCapture::start_turn()` — opens the cpal stream for one turn only.
//!   3. Each VAD chunk that passes Whisper yields a `mock_user_transcribed` event.
//!   4. `MicCapture::end_turn()` — drains frames, closes the stream, returns transcript.
//!   5. `MicCapture::shutdown()` — tears down the capture loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, StreamTrait};
use tauri::{AppHandle, Runtime};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::audio::capture::{
    build_resampled_mono_stream, find_mock_mic_device, AudioSource, FRAME_SAMPLES,
};
use crate::audio::rnnoise::{Downsampler, RNNoiseProcessor};
use crate::audio::vad::{VadChunk, VadChunker};
use crate::events::{emit_mock_user_transcribed, MockUserTranscribedPayload};
use crate::transcription::engine::WhisperEngine;

use super::audio_writer::TurnAudioWriter;

// ── Message types ─────────────────────────────────────────────────────────────

/// Commands sent from commands.rs into the capture loop.
pub enum MicCommand {
    /// Begin recording the user's answer for this turn.
    StartTurn {
        turn_n: u32,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Stop recording, flush audio, return transcript + WAV path via channel.
    EndTurn {
        reply: oneshot::Sender<(String, String)>,
    },
    /// Shut down the capture task entirely.
    Shutdown,
}

/// Commands sent from the capture loop into the cpal OS thread.
#[derive(Debug)]
enum CpalControl {
    Open { reply: oneshot::Sender<Result<()>> },
    Close { reply: oneshot::Sender<()> },
    Shutdown,
}

// ── Public handle ─────────────────────────────────────────────────────────────

/// Handle returned by `MicCapture::start()`.  Call [`MicCapture::shutdown`] to
/// release resources when the mock session ends.
pub struct MicCapture {
    cmd_tx: mpsc::Sender<MicCommand>,
    task: JoinHandle<()>,
    cpal_tx: std::sync::mpsc::Sender<CpalControl>,
}

impl MicCapture {
    /// Start the mic capture background task without opening the OS audio device.
    ///
    /// Must be called from within the tokio runtime (i.e. from an async fn).
    /// The cpal stream is opened only when [`MicCapture::start_turn`] runs.
    pub async fn start<R: Runtime>(
        app: AppHandle<R>,
        session_id: Uuid,
        audio_dir: PathBuf,
        whisper: Arc<WhisperEngine>,
    ) -> Result<Self> {
        let (frame_tx, frame_rx) = mpsc::channel::<Vec<f32>>(512);
        let (cmd_tx, cmd_rx) = mpsc::channel::<MicCommand>(16);
        let (cpal_tx, cpal_rx) = std::sync::mpsc::channel::<CpalControl>();

        std::thread::spawn(move || {
            if let Err(e) = run_cpal_control_thread(frame_tx, cpal_rx) {
                error!(error = %e, "mock mic cpal thread failed");
            }
        });

        let cpal_tx_for_loop = cpal_tx.clone();
        let task = tokio::spawn(capture_loop(
            app,
            session_id,
            audio_dir,
            whisper,
            frame_rx,
            cmd_rx,
            cpal_tx_for_loop,
        ));

        Ok(Self {
            cmd_tx,
            task,
            cpal_tx,
        })
    }

    /// Begin recording the user's answer for `turn_n`.
    pub async fn start_turn(&self, turn_n: u32) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(MicCommand::StartTurn {
                turn_n,
                reply: reply_tx,
            })
            .await
            .context("send StartTurn")?;
        reply_rx.await.context("StartTurn reply channel closed")?
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

    /// Shut down the capture loop and release any held audio resources.
    pub async fn shutdown(self) {
        let _ = self.cpal_tx.send(CpalControl::Shutdown);
        let _ = self.cmd_tx.send(MicCommand::Shutdown).await;
        let _ = self.task.await;
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

// ── cpal control thread ───────────────────────────────────────────────────────

fn run_cpal_control_thread(
    frame_tx: mpsc::Sender<Vec<f32>>,
    control_rx: std::sync::mpsc::Receiver<CpalControl>,
) -> Result<()> {
    info!("mock mic cpal control thread started");

    while let Ok(cmd) = control_rx.recv() {
        match cmd {
            CpalControl::Open { reply } => {
                if let Err(e) = open_mic_stream(&frame_tx, &control_rx, reply) {
                    error!(error = %e, "mock mic stream open failed");
                }
            }
            CpalControl::Close { reply } => {
                let _ = reply.send(());
            }
            CpalControl::Shutdown => break,
        }
    }

    info!("mock mic cpal control thread stopped");
    Ok(())
}

fn open_mic_stream(
    frame_tx: &mpsc::Sender<Vec<f32>>,
    control_rx: &std::sync::mpsc::Receiver<CpalControl>,
    reply: oneshot::Sender<Result<()>>,
) -> Result<()> {
    let host = cpal::default_host();
    let device = find_mock_mic_device(&host).context("no mock mic input device")?;
    info!(
        device = %device.name().unwrap_or_else(|_| "unknown".into()),
        "mock mic stream opening"
    );

    let stream =
        build_resampled_mono_stream(&device, frame_tx.clone()).context("build mock mic stream")?;
    stream.play().context("start mock mic stream")?;
    let _ = reply.send(Ok(()));
    info!("mock mic stream open for turn");

    match control_rx.recv() {
        Ok(CpalControl::Close { reply: close_reply }) => {
            drop(stream);
            info!("mock mic stream closed — device released");
            let _ = close_reply.send(());
        }
        Ok(CpalControl::Shutdown) | Err(_) => {
            drop(stream);
            info!("mock mic stream closed on shutdown");
        }
        Ok(other) => {
            warn!(?other, "unexpected cpal control while stream open");
            drop(stream);
        }
    }

    Ok(())
}

async fn open_cpal_stream(cpal_tx: &std::sync::mpsc::Sender<CpalControl>) -> Result<()> {
    let (reply_tx, reply_rx) = oneshot::channel();
    cpal_tx
        .send(CpalControl::Open { reply: reply_tx })
        .map_err(|_| anyhow::anyhow!("cpal control thread exited"))?;
    reply_rx.await.context("cpal open reply channel closed")?
}

async fn close_cpal_stream(cpal_tx: &std::sync::mpsc::Sender<CpalControl>) {
    let (reply_tx, reply_rx) = oneshot::channel();
    if cpal_tx
        .send(CpalControl::Close { reply: reply_tx })
        .is_err()
    {
        return;
    }
    let _ = reply_rx.await;
}

fn discard_stale_frames(frame_rx: &mut mpsc::Receiver<Vec<f32>>) {
    while frame_rx.try_recv().is_ok() {}
}

// ── Async capture loop ────────────────────────────────────────────────────────

async fn capture_loop<R: Runtime>(
    app: AppHandle<R>,
    session_id: Uuid,
    audio_dir: PathBuf,
    whisper: Arc<WhisperEngine>,
    mut frame_rx: mpsc::Receiver<Vec<f32>>,
    mut cmd_rx: mpsc::Receiver<MicCommand>,
    cpal_tx: std::sync::mpsc::Sender<CpalControl>,
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
    let mut stream_open = false;

    loop {
        tokio::select! {
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    MicCommand::StartTurn { turn_n, reply } => {
                        if stream_open {
                            close_cpal_stream(&cpal_tx).await;
                            stream_open = false;
                        }

                        discard_stale_frames(&mut frame_rx);

                        match open_cpal_stream(&cpal_tx).await {
                            Ok(()) => {
                                stream_open = true;
                                if let Ok(r) = RNNoiseProcessor::new() {
                                    rnnoise = r;
                                }
                                if let Ok(d) = Downsampler::new() {
                                    downsampler = d;
                                }
                                if let Ok(v) = VadChunker::new() {
                                    vad = v;
                                }
                                current_turn = Some(turn_n);
                                audio_writer = Some(TurnAudioWriter::new(
                                    session_id,
                                    turn_n,
                                    &audio_dir,
                                ));
                                transcript_buf.clear();
                                info!(turn_n, "mock mic: recording started");
                                let _ = reply.send(Ok(()));
                            }
                            Err(e) => {
                                error!(error = %e, turn_n, "mock mic: failed to open stream");
                                let _ = reply.send(Err(e));
                            }
                        }
                    }
                    MicCommand::EndTurn { reply } => {
                        if let Some(turn_n) = current_turn {
                            drain_audio_frames(
                                &app,
                                &whisper,
                                turn_n,
                                &mut frame_rx,
                                Duration::from_millis(300),
                                &mut audio_writer,
                                &mut transcript_buf,
                                &mut rnnoise,
                                &mut downsampler,
                                &mut vad,
                            )
                            .await;
                        }

                        if stream_open {
                            close_cpal_stream(&cpal_tx).await;
                            stream_open = false;
                        }

                        if let Some(turn_n) = current_turn {
                            drain_audio_frames(
                                &app,
                                &whisper,
                                turn_n,
                                &mut frame_rx,
                                Duration::from_millis(150),
                                &mut audio_writer,
                                &mut transcript_buf,
                                &mut rnnoise,
                                &mut downsampler,
                                &mut vad,
                            )
                            .await;
                        }

                        let writer = audio_writer.take();
                        let path = writer
                            .map(|w| w.finish().unwrap_or_default())
                            .unwrap_or_default();
                        let text = std::mem::take(&mut transcript_buf);
                        current_turn = None;
                        let _ = reply.send((text, path));
                    }
                    MicCommand::Shutdown => {
                        if stream_open {
                            close_cpal_stream(&cpal_tx).await;
                            stream_open = false;
                        }
                        info!("mock mic: shutdown requested");
                        break;
                    }
                }
            }
            Some(frame) = frame_rx.recv(), if current_turn.is_some() => {
                if let Some(turn_n) = current_turn {
                    process_audio_frame(
                        &app,
                        &whisper,
                        frame,
                        turn_n,
                        &mut audio_writer,
                        &mut transcript_buf,
                        &mut rnnoise,
                        &mut downsampler,
                        &mut vad,
                    )
                    .await;
                }
            }
            else => break,
        }
    }

    if stream_open {
        close_cpal_stream(&cpal_tx).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn drain_audio_frames<R: Runtime>(
    app: &AppHandle<R>,
    whisper: &Arc<WhisperEngine>,
    turn_n: u32,
    frame_rx: &mut mpsc::Receiver<Vec<f32>>,
    max_wait: Duration,
    audio_writer: &mut Option<TurnAudioWriter>,
    transcript_buf: &mut String,
    rnnoise: &mut RNNoiseProcessor,
    downsampler: &mut Downsampler,
    vad: &mut VadChunker,
) {
    let deadline = tokio::time::Instant::now() + max_wait;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(50), frame_rx.recv()).await {
            Ok(Some(frame)) => {
                process_audio_frame(
                    app,
                    whisper,
                    frame,
                    turn_n,
                    audio_writer,
                    transcript_buf,
                    rnnoise,
                    downsampler,
                    vad,
                )
                .await;
            }
            _ => break,
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_audio_frame<R: Runtime>(
    app: &AppHandle<R>,
    whisper: &Arc<WhisperEngine>,
    frame: Vec<f32>,
    turn_n: u32,
    audio_writer: &mut Option<TurnAudioWriter>,
    transcript_buf: &mut String,
    rnnoise: &mut RNNoiseProcessor,
    downsampler: &mut Downsampler,
    vad: &mut VadChunker,
) {
    if frame.len() != FRAME_SAMPLES {
        return;
    }

    let mut proc = frame;
    if let Err(e) = rnnoise.process_frame(&mut proc) {
        warn!(error = %e, "mock mic RNNoise error");
        return;
    }

    let downsampled = match downsampler.process(&proc) {
        Ok(d) => d,
        Err(e) => {
            warn!(error = %e, "mock mic downsampler error");
            return;
        }
    };

    if let Some(w) = audio_writer {
        w.push_samples(&downsampled);
    }

    for chunk_frame in downsampled.chunks(160) {
        if let Some(chunk) = vad.process_frame(chunk_frame, AudioSource::Microphone) {
            dispatch_chunk(app, whisper, chunk, turn_n, transcript_buf).await;
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
            audio_path: String::new(),
        },
    );
}
