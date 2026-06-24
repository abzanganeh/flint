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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Drop the first ~300 ms of mic frames after a Listening phase begins.
///
/// TTS playback ends just before the mic stream opens, but on Linux the speaker
/// driver still has ~150-300 ms of decay buffered. RNNoise + Whisper would
/// transcribe that tail as "user speech" and contaminate the answer transcript.
/// The quiet window guarantees the first frames Whisper sees are real silence
/// (or the genuine start of the user's reply).
const POST_TTS_QUIET_MS: u64 = 300;

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
use crate::events::{
    emit_mock_turn_phase, emit_mock_user_transcribed, MockTurnPhasePayload,
    MockUserTranscribedPayload,
};
use crate::transcription::engine::WhisperEngine;

use super::audio_writer::TurnAudioWriter;
use super::turn_phase::{MockMicPhase, TurnSpeechTracker};

// ── Message types ─────────────────────────────────────────────────────────────

/// Commands sent from commands.rs into the capture loop.
pub enum MicCommand {
    /// Open mic and wait for the user to start speaking (no REC/STT yet).
    StartListening {
        turn_n: u32,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Legacy alias — opens mic directly into answering (tests only).
    StartTurn {
        turn_n: u32,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Stop recording, flush audio, return transcript + WAV path + STT confidence via channel.
    EndTurn {
        reply: oneshot::Sender<(String, String, Option<f32>)>,
    },
    /// Discard partial answer and return to listening for the same turn (M12).
    AbortTurn { reply: oneshot::Sender<Result<()>> },
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
    listen_tx: mpsc::Sender<u32>,
    task: JoinHandle<()>,
    cpal_tx: std::sync::mpsc::Sender<CpalControl>,
}

impl MicCapture {
    /// Clone of the channel the conductor uses to begin listening after TTS.
    pub fn listen_trigger(&self) -> mpsc::Sender<u32> {
        self.listen_tx.clone()
    }

    /// Start the mic capture background task without opening the OS audio device.
    ///
    /// Must be called from within the tokio runtime (i.e. from an async fn).
    /// The cpal stream is opened when listening begins for a turn.
    pub async fn start<R: Runtime>(
        app: AppHandle<R>,
        session_id: Uuid,
        audio_dir: PathBuf,
        whisper: Arc<WhisperEngine>,
        mic_recording: Arc<AtomicBool>,
    ) -> Result<Self> {
        let (frame_tx, frame_rx) = mpsc::channel::<Vec<f32>>(512);
        let (cmd_tx, cmd_rx) = mpsc::channel::<MicCommand>(16);
        let (listen_tx, listen_rx) = mpsc::channel::<u32>(8);
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
            listen_rx,
            cpal_tx_for_loop,
            mic_recording,
        ));

        Ok(Self {
            cmd_tx,
            listen_tx,
            task,
            cpal_tx,
        })
    }

    /// Open mic in listen mode for `turn_n` (no REC until speech is detected).
    pub async fn start_listening(&self, turn_n: u32) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(MicCommand::StartListening {
                turn_n,
                reply: reply_tx,
            })
            .await
            .context("send StartListening")?;
        reply_rx
            .await
            .context("StartListening reply channel closed")?
    }

    /// Begin recording the user's answer for `turn_n` (legacy — auto flow uses listening).
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

    /// Stop recording and await the transcript + audio path + confidence for the turn.
    pub async fn end_turn(&self, timeout: Duration) -> Result<(String, String, Option<f32>)> {
        let reply_rx = self.send_end_turn().await?;
        await_end_turn_reply(reply_rx, timeout).await
    }

    /// Send the `EndTurn` command and return the reply receiver without
    /// awaiting it. Callers that hold a session-wide mutex use this to drop
    /// the guard before awaiting the (potentially long) recording shutdown
    /// so concurrent commands (e.g. `stop_mock`) are not blocked.
    pub async fn send_end_turn(&self) -> Result<oneshot::Receiver<(String, String, Option<f32>)>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(MicCommand::EndTurn { reply: reply_tx })
            .await
            .context("send EndTurn")?;
        Ok(reply_rx)
    }

    /// Discard the in-progress answer and reopen listen mode for the active turn.
    pub async fn abort_turn(&self) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(MicCommand::AbortTurn { reply: reply_tx })
            .await
            .context("send AbortTurn")?;
        reply_rx.await.context("AbortTurn reply channel closed")?
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
    reply_rx: oneshot::Receiver<(String, String, Option<f32>)>,
    timeout: Duration,
) -> Result<(String, String, Option<f32>)> {
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

#[allow(clippy::too_many_arguments)]
async fn capture_loop<R: Runtime>(
    app: AppHandle<R>,
    session_id: Uuid,
    audio_dir: PathBuf,
    whisper: Arc<WhisperEngine>,
    mut frame_rx: mpsc::Receiver<Vec<f32>>,
    mut cmd_rx: mpsc::Receiver<MicCommand>,
    mut listen_rx: mpsc::Receiver<u32>,
    cpal_tx: std::sync::mpsc::Sender<CpalControl>,
    mic_recording: Arc<AtomicBool>,
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
    let mut mock_phase = MockMicPhase::Off;
    let mut audio_writer: Option<TurnAudioWriter> = None;
    let mut transcript_buf = String::new();
    let mut rolling_context = String::new();
    let mut logprob_sum: f32 = 0.0;
    let mut logprob_count: u32 = 0;
    let mut stream_open = false;
    let mut speech_tracker = TurnSpeechTracker::default();
    let mut quiet_until: Option<Instant> = None;

    loop {
        tokio::select! {
            Some(turn_n) = listen_rx.recv() => {
                if let Err(e) = begin_listening(
                    turn_n,
                    &app,
                    &cpal_tx,
                    &mut frame_rx,
                    &mut stream_open,
                    &mut rnnoise,
                    &mut downsampler,
                    &mut vad,
                    &mut current_turn,
                    &mut mock_phase,
                    &mut audio_writer,
                    &mut transcript_buf,
                    &mut rolling_context,
                    &mut logprob_sum,
                    &mut logprob_count,
                    &mut speech_tracker,
                    &mic_recording,
                    &mut quiet_until,
                ).await {
                    warn!(error = %e, turn_n, "mock mic: listen trigger failed");
                }
            }
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    MicCommand::StartListening { turn_n, reply } => {
                        let result = begin_listening(
                            turn_n,
                            &app,
                            &cpal_tx,
                            &mut frame_rx,
                            &mut stream_open,
                            &mut rnnoise,
                            &mut downsampler,
                            &mut vad,
                            &mut current_turn,
                            &mut mock_phase,
                            &mut audio_writer,
                            &mut transcript_buf,
                            &mut rolling_context,
                            &mut logprob_sum,
                            &mut logprob_count,
                            &mut speech_tracker,
                            &mic_recording,
                            &mut quiet_until,
                        ).await;
                        if result.is_ok() {
                            info!(turn_n, "mock mic: listening started");
                        } else if let Err(ref e) = result {
                            error!(error = %e, turn_n, "mock mic: listening failed");
                        }
                        let _ = reply.send(result);
                    }
                    MicCommand::StartTurn { turn_n, reply } => {
                        let result = begin_listening(
                            turn_n,
                            &app,
                            &cpal_tx,
                            &mut frame_rx,
                            &mut stream_open,
                            &mut rnnoise,
                            &mut downsampler,
                            &mut vad,
                            &mut current_turn,
                            &mut mock_phase,
                            &mut audio_writer,
                            &mut transcript_buf,
                            &mut rolling_context,
                            &mut logprob_sum,
                            &mut logprob_count,
                            &mut speech_tracker,
                            &mic_recording,
                            &mut quiet_until,
                        ).await;
                        if result.is_ok() {
                            enter_answering(
                                turn_n,
                                session_id,
                                &audio_dir,
                                &app,
                                &mut mock_phase,
                                &mut audio_writer,
                                &mic_recording,
                            );
                            info!(turn_n, "mock mic: recording started (legacy StartTurn)");
                        }
                        let _ = reply.send(result);
                    }
                    MicCommand::EndTurn { reply } => {
                        if mock_phase == MockMicPhase::Answering || mock_phase == MockMicPhase::Paused {
                            if let Some(turn_n) = current_turn {
                                drain_audio_frames(
                                    &app,
                                    &whisper,
                                    turn_n,
                                    &mut frame_rx,
                                    Duration::from_millis(300),
                                    &mut audio_writer,
                                    &mut transcript_buf,
                                    &mut rolling_context,
                                    &mut logprob_sum,
                                    &mut logprob_count,
                                    &mut rnnoise,
                                    &mut downsampler,
                                    &mut vad,
                                )
                                .await;
                            }
                        }

                        if stream_open {
                            close_cpal_stream(&cpal_tx).await;
                            stream_open = false;
                        }

                        if mock_phase == MockMicPhase::Answering || mock_phase == MockMicPhase::Paused {
                            if let Some(turn_n) = current_turn {
                                drain_audio_frames(
                                    &app,
                                    &whisper,
                                    turn_n,
                                    &mut frame_rx,
                                    Duration::from_millis(150),
                                    &mut audio_writer,
                                    &mut transcript_buf,
                                    &mut rolling_context,
                                    &mut logprob_sum,
                                    &mut logprob_count,
                                    &mut rnnoise,
                                    &mut downsampler,
                                    &mut vad,
                                )
                                .await;
                            }
                        }

                        let writer = audio_writer.take();
                        let path = writer
                            .map(|w| w.finish().unwrap_or_default())
                            .unwrap_or_default();
                        let text = std::mem::take(&mut transcript_buf);
                        let confidence = if logprob_count > 0 {
                            Some(logprob_sum / logprob_count as f32)
                        } else {
                            None
                        };
                        rolling_context.clear();
                        logprob_sum = 0.0;
                        logprob_count = 0;
                        current_turn = None;
                        mock_phase = MockMicPhase::Off;
                        speech_tracker.reset();
                        mic_recording.store(false, Ordering::SeqCst);
                        let _ = reply.send((text, path, confidence));
                    }
                    MicCommand::AbortTurn { reply } => {
                        let result = abort_active_turn(
                            &app,
                            &mut mock_phase,
                            &mut audio_writer,
                            &mut transcript_buf,
                            &mut rolling_context,
                            &mut logprob_sum,
                            &mut logprob_count,
                            &mut speech_tracker,
                            &mut vad,
                            current_turn,
                            &mic_recording,
                        );
                        let _ = reply.send(result);
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
                    process_mock_frame(
                        &app,
                        &whisper,
                        frame,
                        turn_n,
                        session_id,
                        &audio_dir,
                        &mut mock_phase,
                        &mut audio_writer,
                        &mut transcript_buf,
                        &mut rolling_context,
                        &mut logprob_sum,
                        &mut logprob_count,
                        &mut rnnoise,
                        &mut downsampler,
                        &mut vad,
                        &mut speech_tracker,
                        &mic_recording,
                        &mut quiet_until,
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
async fn begin_listening<R: Runtime>(
    turn_n: u32,
    app: &AppHandle<R>,
    cpal_tx: &std::sync::mpsc::Sender<CpalControl>,
    frame_rx: &mut mpsc::Receiver<Vec<f32>>,
    stream_open: &mut bool,
    rnnoise: &mut RNNoiseProcessor,
    downsampler: &mut Downsampler,
    vad: &mut VadChunker,
    current_turn: &mut Option<u32>,
    mock_phase: &mut MockMicPhase,
    audio_writer: &mut Option<TurnAudioWriter>,
    transcript_buf: &mut String,
    rolling_context: &mut String,
    logprob_sum: &mut f32,
    logprob_count: &mut u32,
    speech_tracker: &mut TurnSpeechTracker,
    mic_recording: &Arc<AtomicBool>,
    quiet_until: &mut Option<Instant>,
) -> Result<()> {
    if *stream_open {
        close_cpal_stream(cpal_tx).await;
        *stream_open = false;
    }

    discard_stale_frames(frame_rx);

    open_cpal_stream(cpal_tx).await?;
    *stream_open = true;

    if let Ok(r) = RNNoiseProcessor::new() {
        *rnnoise = r;
    }
    if let Ok(d) = Downsampler::new() {
        *downsampler = d;
    }
    if let Ok(v) = VadChunker::new() {
        *vad = v;
    }

    *current_turn = Some(turn_n);
    *mock_phase = MockMicPhase::Listening;
    audio_writer.take();
    transcript_buf.clear();
    rolling_context.clear();
    *logprob_sum = 0.0;
    *logprob_count = 0;
    speech_tracker.reset();
    mic_recording.store(false, Ordering::SeqCst);
    *quiet_until = Some(Instant::now() + Duration::from_millis(POST_TTS_QUIET_MS));

    emit_mock_turn_phase(
        app,
        MockTurnPhasePayload {
            turn_n,
            phase: MockMicPhase::Listening.as_str().to_string(),
        },
    );

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn abort_active_turn<R: Runtime>(
    app: &AppHandle<R>,
    mock_phase: &mut MockMicPhase,
    audio_writer: &mut Option<TurnAudioWriter>,
    transcript_buf: &mut String,
    rolling_context: &mut String,
    logprob_sum: &mut f32,
    logprob_count: &mut u32,
    speech_tracker: &mut TurnSpeechTracker,
    vad: &mut VadChunker,
    current_turn: Option<u32>,
    mic_recording: &Arc<AtomicBool>,
) -> Result<()> {
    if !mock_phase.allows_mid_answer_abort() {
        anyhow::bail!("Cannot retry — start speaking before using Retry.");
    }
    let turn_n = current_turn.ok_or_else(|| anyhow::anyhow!("No active mock turn."))?;

    audio_writer.take();
    transcript_buf.clear();
    rolling_context.clear();
    *logprob_sum = 0.0;
    *logprob_count = 0;
    speech_tracker.reset();
    if let Ok(v) = VadChunker::new() {
        *vad = v;
    }
    *mock_phase = MockMicPhase::Listening;
    mic_recording.store(false, Ordering::SeqCst);
    emit_mock_turn_phase(
        app,
        MockTurnPhasePayload {
            turn_n,
            phase: MockMicPhase::Listening.as_str().to_string(),
        },
    );
    Ok(())
}

fn enter_answering<R: Runtime>(
    turn_n: u32,
    session_id: Uuid,
    audio_dir: &PathBuf,
    app: &AppHandle<R>,
    mock_phase: &mut MockMicPhase,
    audio_writer: &mut Option<TurnAudioWriter>,
    mic_recording: &Arc<AtomicBool>,
) {
    if *mock_phase == MockMicPhase::Answering {
        return;
    }
    *audio_writer = Some(TurnAudioWriter::new(session_id, turn_n, audio_dir));
    *mock_phase = MockMicPhase::Answering;
    mic_recording.store(true, Ordering::SeqCst);
    emit_mock_turn_phase(
        app,
        MockTurnPhasePayload {
            turn_n,
            phase: MockMicPhase::Answering.as_str().to_string(),
        },
    );
}

#[allow(clippy::too_many_arguments)]
async fn process_mock_frame<R: Runtime>(
    app: &AppHandle<R>,
    whisper: &Arc<WhisperEngine>,
    frame: Vec<f32>,
    turn_n: u32,
    session_id: Uuid,
    audio_dir: &PathBuf,
    mock_phase: &mut MockMicPhase,
    audio_writer: &mut Option<TurnAudioWriter>,
    transcript_buf: &mut String,
    rolling_context: &mut String,
    logprob_sum: &mut f32,
    logprob_count: &mut u32,
    rnnoise: &mut RNNoiseProcessor,
    downsampler: &mut Downsampler,
    vad: &mut VadChunker,
    speech_tracker: &mut TurnSpeechTracker,
    mic_recording: &Arc<AtomicBool>,
    quiet_until: &mut Option<Instant>,
) {
    if frame.len() != FRAME_SAMPLES {
        return;
    }

    // Drop frames captured during the post-TTS quiet window. Speakers may
    // still be decaying for ~300 ms after the TTS subprocess exits; running
    // RNNoise + VAD + Whisper on that tail produces phantom answers.
    if let Some(deadline) = *quiet_until {
        if Instant::now() < deadline {
            return;
        }
        *quiet_until = None;
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

    let record_audio = *mock_phase == MockMicPhase::Answering;
    if record_audio {
        if let Some(w) = audio_writer {
            w.push_samples(&downsampled);
        }
    }

    for chunk_frame in downsampled.chunks(160) {
        let chunk = vad.process_frame(chunk_frame, AudioSource::Microphone);

        if vad.speech_in_progress() {
            match *mock_phase {
                MockMicPhase::Listening => {
                    enter_answering(
                        turn_n,
                        session_id,
                        audio_dir,
                        app,
                        mock_phase,
                        audio_writer,
                        mic_recording,
                    );
                    if let Some(w) = audio_writer {
                        w.push_samples(chunk_frame);
                    }
                }
                MockMicPhase::Paused => {
                    *mock_phase = MockMicPhase::Answering;
                    emit_mock_turn_phase(
                        app,
                        MockTurnPhasePayload {
                            turn_n,
                            phase: MockMicPhase::Answering.as_str().to_string(),
                        },
                    );
                    if let Some(w) = audio_writer {
                        w.push_samples(chunk_frame);
                    }
                }
                MockMicPhase::Answering => {
                    speech_tracker.on_speech_frame();
                }
                MockMicPhase::Off => {}
            }
        }

        if *mock_phase == MockMicPhase::Answering {
            if let Some(vad_chunk) = chunk {
                if let Some((text, lp)) =
                    dispatch_chunk(app, whisper, vad_chunk, turn_n, rolling_context).await
                {
                    if !transcript_buf.is_empty() {
                        transcript_buf.push(' ');
                    }
                    transcript_buf.push_str(&text);
                    append_rolling_context(rolling_context, &text);
                    *logprob_sum += lp;
                    *logprob_count += 1;
                }
            }

            if speech_tracker.should_pause(vad.ms_since_last_speech()) {
                *mock_phase = MockMicPhase::Paused;
                emit_mock_turn_phase(
                    app,
                    MockTurnPhasePayload {
                        turn_n,
                        phase: MockMicPhase::Paused.as_str().to_string(),
                    },
                );
            }
        }
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
    rolling_context: &mut String,
    logprob_sum: &mut f32,
    logprob_count: &mut u32,
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
                    rolling_context,
                    logprob_sum,
                    logprob_count,
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
    rolling_context: &mut String,
    logprob_sum: &mut f32,
    logprob_count: &mut u32,
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
            if let Some((text, lp)) =
                dispatch_chunk(app, whisper, chunk, turn_n, rolling_context).await
            {
                if !transcript_buf.is_empty() {
                    transcript_buf.push(' ');
                }
                transcript_buf.push_str(&text);
                append_rolling_context(rolling_context, &text);
                *logprob_sum += lp;
                *logprob_count += 1;
            }
        }
    }
}

/// Transcribe one VAD chunk using the rolling-context-aware engine, emit a
/// `mock_user_transcribed` event, and return the recognised text alongside its
/// average log-probability so the caller can track STT confidence for the turn.
///
/// Returns `None` on silence, engine error, or empty output.
async fn dispatch_chunk<R: Runtime>(
    app: &AppHandle<R>,
    whisper: &Arc<WhisperEngine>,
    chunk: VadChunk,
    turn_n: u32,
    rolling_context: &str,
) -> Option<(String, f32)> {
    let w = Arc::clone(whisper);
    let ctx = rolling_context.to_string();
    let result = tokio::task::spawn_blocking(move || w.transcribe_with_context(&chunk, &ctx)).await;

    let transcription = match result {
        Ok(Ok(Some(r))) => r,
        Ok(Ok(None)) => return None,
        Ok(Err(e)) => {
            warn!(error = %e, "mock transcription error");
            return None;
        }
        Err(e) => {
            warn!(error = %e, "mock transcription task panicked");
            return None;
        }
    };

    let text = transcription.text.trim().to_string();
    if text.is_empty() {
        return None;
    }

    let avg_logprob = transcription.avg_logprob.unwrap_or(-0.5);

    emit_mock_user_transcribed(
        app,
        MockUserTranscribedPayload {
            turn_n,
            text: text.clone(),
            audio_path: String::new(),
        },
    );

    Some((text, avg_logprob))
}

/// Keep the rolling context to the last 40 words so Whisper's `initial_prompt`
/// stays well within its token budget.
fn append_rolling_context(context: &mut String, new_text: &str) {
    let combined = if context.is_empty() {
        new_text.to_string()
    } else {
        format!("{context} {new_text}")
    };
    let words: Vec<&str> = combined.split_whitespace().collect();
    let keep_from = words.len().saturating_sub(40);
    *context = words[keep_from..].join(" ");
}
