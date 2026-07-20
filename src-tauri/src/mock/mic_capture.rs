//! Mic-only audio capture for mock interview answer recording.
//!
//! Capture (cpal → RNNoise → VAD) runs on a dedicated async loop that never
//! awaits Whisper. Transcription is delegated to [`WhisperWorker`] so long
//! answers cannot block frame drain or falsely trigger turn-level pause.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Drop the first ~300 ms of mic frames after a Listening phase begins.
const POST_TTS_QUIET_MS: u64 = 300;

/// Match live pipeline buffer depth (~10s) so brief Whisper backlog does not drop frames.
const MOCK_FRAME_CHANNEL_DEPTH: usize = 1024;

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
use crate::audio::vad::VadChunker;
use crate::events::{emit_mock_turn_phase, MockTurnPhasePayload};
use crate::transcription::engine::WhisperEngine;

use super::audio_writer::TurnAudioWriter;
use super::turn_phase::{MockMicPhase, TurnSpeechTracker};
use super::whisper_worker::{TurnEpoch, WhisperWorker, WHISPER_FLUSH_TIMEOUT};

// ── Message types ─────────────────────────────────────────────────────────────

pub enum MicCommand {
    StartListening {
        turn_n: u32,
        reply: oneshot::Sender<Result<()>>,
    },
    StartTurn {
        turn_n: u32,
        reply: oneshot::Sender<Result<()>>,
    },
    EndTurn {
        reply: oneshot::Sender<(String, String, Option<f32>)>,
    },
    AbortTurn {
        reply: oneshot::Sender<Result<()>>,
    },
    Shutdown,
}

#[derive(Debug)]
enum CpalControl {
    Open {
        reply: oneshot::Sender<Result<()>>,
    },
    Close {
        reply: oneshot::Sender<()>,
    },
    Shutdown,
}

// ── Public handle ─────────────────────────────────────────────────────────────

pub struct MicCapture {
    cmd_tx: mpsc::Sender<MicCommand>,
    listen_tx: mpsc::Sender<u32>,
    task: JoinHandle<()>,
    cpal_tx: std::sync::mpsc::Sender<CpalControl>,
}

impl MicCapture {
    pub fn listen_trigger(&self) -> mpsc::Sender<u32> {
        self.listen_tx.clone()
    }

    pub async fn start<R: Runtime>(
        app: AppHandle<R>,
        session_id: Uuid,
        audio_dir: PathBuf,
        whisper: Arc<WhisperEngine>,
        mic_recording: Arc<AtomicBool>,
    ) -> Result<Self> {
        let (frame_tx, frame_rx) = mpsc::channel::<Vec<f32>>(MOCK_FRAME_CHANNEL_DEPTH);
        let (cmd_tx, cmd_rx) = mpsc::channel::<MicCommand>(16);
        let (listen_tx, listen_rx) = mpsc::channel::<u32>(8);
        let (cpal_tx, cpal_rx) = std::sync::mpsc::channel::<CpalControl>();

        std::thread::spawn(move || {
            if let Err(e) = run_cpal_control_thread(frame_tx, cpal_rx) {
                error!(error = %e, "mock mic cpal thread failed");
            }
        });

        let worker = WhisperWorker::start(app.clone(), whisper);
        let cpal_tx_for_loop = cpal_tx.clone();
        let task = tokio::spawn(capture_loop(
            app,
            session_id,
            audio_dir,
            Some(worker),
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

    pub async fn end_turn(&self, timeout: Duration) -> Result<(String, String, Option<f32>)> {
        let reply_rx = self.send_end_turn().await?;
        await_end_turn_reply(reply_rx, timeout).await
    }

    pub async fn send_end_turn(&self) -> Result<oneshot::Receiver<(String, String, Option<f32>)>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(MicCommand::EndTurn { reply: reply_tx })
            .await
            .context("send EndTurn")?;
        Ok(reply_rx)
    }

    pub async fn abort_turn(&self) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(MicCommand::AbortTurn { reply: reply_tx })
            .await
            .context("send AbortTurn")?;
        reply_rx.await.context("AbortTurn reply channel closed")?
    }

    pub async fn shutdown(self) {
        let _ = self.cpal_tx.send(CpalControl::Shutdown);
        let _ = self.cmd_tx.send(MicCommand::Shutdown).await;
        let _ = self.task.await;
    }
}

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
    mut worker: Option<WhisperWorker>,
    mut frame_rx: mpsc::Receiver<Vec<f32>>,
    mut cmd_rx: mpsc::Receiver<MicCommand>,
    mut listen_rx: mpsc::Receiver<u32>,
    cpal_tx: std::sync::mpsc::Sender<CpalControl>,
    mic_recording: Arc<AtomicBool>,
) {
    let shutdown_worker = async |worker: &mut Option<WhisperWorker>| {
        if let Some(w) = worker.take() {
            w.shutdown().await;
        }
    };

    let mut rnnoise = match RNNoiseProcessor::new() {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "failed to init RNNoise for mock capture");
            shutdown_worker(&mut worker).await;
            return;
        }
    };
    let mut downsampler = match Downsampler::new() {
        Ok(d) => d,
        Err(e) => {
            error!(error = %e, "failed to init downsampler for mock capture");
            shutdown_worker(&mut worker).await;
            return;
        }
    };
    let mut vad = match VadChunker::new() {
        Ok(v) => v,
        Err(e) => {
            error!(error = %e, "failed to init VAD for mock capture");
            shutdown_worker(&mut worker).await;
            return;
        }
    };

    let worker_ref = worker.as_ref().expect("whisper worker");

    let mut current_turn: Option<u32> = None;
    let mut epoch: TurnEpoch = 0;
    let mut mock_phase = MockMicPhase::Off;
    let mut audio_writer: Option<TurnAudioWriter> = None;
    let mut stream_open = false;
    let mut speech_tracker = TurnSpeechTracker::default();
    let mut quiet_until: Option<Instant> = None;

    loop {
        tokio::select! {
            Some(turn_n) = listen_rx.recv() => {
                if let Err(e) = begin_listening(
                    turn_n,
                    &app,
                    worker_ref,
                    &cpal_tx,
                    &mut frame_rx,
                    &mut stream_open,
                    &mut rnnoise,
                    &mut downsampler,
                    &mut vad,
                    &mut current_turn,
                    &mut epoch,
                    &mut mock_phase,
                    &mut audio_writer,
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
                            worker_ref,
                            &cpal_tx,
                            &mut frame_rx,
                            &mut stream_open,
                            &mut rnnoise,
                            &mut downsampler,
                            &mut vad,
                            &mut current_turn,
                            &mut epoch,
                            &mut mock_phase,
                            &mut audio_writer,
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
                            worker_ref,
                            &cpal_tx,
                            &mut frame_rx,
                            &mut stream_open,
                            &mut rnnoise,
                            &mut downsampler,
                            &mut vad,
                            &mut current_turn,
                            &mut epoch,
                            &mut mock_phase,
                            &mut audio_writer,
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
                        if stream_open {
                            close_cpal_stream(&cpal_tx).await;
                            stream_open = false;
                        }

                        let (text, path, confidence) = if mock_phase.captures_speech() {
                            if let Some(turn_n) = current_turn {
                                drain_remaining_frames(
                                    worker_ref,
                                    epoch,
                                    turn_n,
                                    &mut frame_rx,
                                    session_id,
                                    &audio_dir,
                                    &app,
                                    &mut mock_phase,
                                    &mut audio_writer,
                                    &mut rnnoise,
                                    &mut downsampler,
                                    &mut vad,
                                    &mut speech_tracker,
                                    &mic_recording,
                                    &mut quiet_until,
                                );

                                if let Some(tail) = vad.force_end_segment() {
                                    let _ = worker_ref
                                        .transcribe_blocking(epoch, turn_n, tail)
                                        .await;
                                }

                                let transcript =
                                    worker_ref.flush(epoch, WHISPER_FLUSH_TIMEOUT).await;
                                let writer = audio_writer.take();
                                let path = writer
                                    .map(|w| w.finish().unwrap_or_default())
                                    .unwrap_or_default();
                                (transcript.text, path, transcript.confidence)
                            } else {
                                (String::new(), String::new(), None)
                            }
                        } else {
                            let path = audio_writer
                                .take()
                                .map(|w| w.finish().unwrap_or_default())
                                .unwrap_or_default();
                            (String::new(), path, None)
                        };

                        current_turn = None;
                        mock_phase = MockMicPhase::Off;
                        speech_tracker.reset();
                        mic_recording.store(false, Ordering::SeqCst);
                        let _ = reply.send((text, path, confidence));
                    }
                    MicCommand::AbortTurn { reply } => {
                        let result = abort_active_turn(
                            &app,
                            worker_ref,
                            &mut mock_phase,
                            &mut audio_writer,
                            &mut speech_tracker,
                            &mut vad,
                            current_turn,
                            &mut epoch,
                            &mic_recording,
                        )
                        .await;
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
                        worker_ref,
                        epoch,
                        frame,
                        turn_n,
                        session_id,
                        &audio_dir,
                        &app,
                        &mut mock_phase,
                        &mut audio_writer,
                        &mut rnnoise,
                        &mut downsampler,
                        &mut vad,
                        &mut speech_tracker,
                        &mic_recording,
                        &mut quiet_until,
                    );
                }
            }
            else => break,
        }
    }

    if stream_open {
        close_cpal_stream(&cpal_tx).await;
    }
    shutdown_worker(&mut worker).await;
}

#[allow(clippy::too_many_arguments)]
async fn begin_listening<R: Runtime>(
    turn_n: u32,
    app: &AppHandle<R>,
    worker: &WhisperWorker,
    cpal_tx: &std::sync::mpsc::Sender<CpalControl>,
    frame_rx: &mut mpsc::Receiver<Vec<f32>>,
    stream_open: &mut bool,
    rnnoise: &mut RNNoiseProcessor,
    downsampler: &mut Downsampler,
    vad: &mut VadChunker,
    current_turn: &mut Option<u32>,
    epoch: &mut TurnEpoch,
    mock_phase: &mut MockMicPhase,
    audio_writer: &mut Option<TurnAudioWriter>,
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

    *epoch = epoch.saturating_add(1);
    worker.reset_context(turn_n, *epoch).await?;

    *current_turn = Some(turn_n);
    *mock_phase = MockMicPhase::Listening;
    audio_writer.take();
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
async fn abort_active_turn<R: Runtime>(
    app: &AppHandle<R>,
    worker: &WhisperWorker,
    mock_phase: &mut MockMicPhase,
    audio_writer: &mut Option<TurnAudioWriter>,
    speech_tracker: &mut TurnSpeechTracker,
    vad: &mut VadChunker,
    current_turn: Option<u32>,
    epoch: &mut TurnEpoch,
    mic_recording: &Arc<AtomicBool>,
) -> Result<()> {
    if !mock_phase.allows_mid_answer_abort() {
        anyhow::bail!("Cannot retry — start speaking before using Retry.");
    }
    let turn_n = current_turn.ok_or_else(|| anyhow::anyhow!("No active mock turn."))?;

    audio_writer.take();
    speech_tracker.reset();
    if let Ok(v) = VadChunker::new() {
        *vad = v;
    }

    *epoch = epoch.saturating_add(1);
    worker.reset_context(turn_n, *epoch).await?;

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
fn process_mock_frame<R: Runtime>(
    worker: &WhisperWorker,
    epoch: TurnEpoch,
    frame: Vec<f32>,
    turn_n: u32,
    session_id: Uuid,
    audio_dir: &PathBuf,
    app: &AppHandle<R>,
    mock_phase: &mut MockMicPhase,
    audio_writer: &mut Option<TurnAudioWriter>,
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

    if mock_phase.captures_speech() {
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

        if mock_phase.captures_speech() {
            if let Some(vad_chunk) = chunk {
                worker.try_transcribe(epoch, turn_n, vad_chunk);
            }

            if *mock_phase == MockMicPhase::Answering
                && speech_tracker.should_pause(vad.ms_since_last_speech())
            {
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
fn drain_remaining_frames<R: Runtime>(
    worker: &WhisperWorker,
    epoch: TurnEpoch,
    turn_n: u32,
    frame_rx: &mut mpsc::Receiver<Vec<f32>>,
    session_id: Uuid,
    audio_dir: &PathBuf,
    app: &AppHandle<R>,
    mock_phase: &mut MockMicPhase,
    audio_writer: &mut Option<TurnAudioWriter>,
    rnnoise: &mut RNNoiseProcessor,
    downsampler: &mut Downsampler,
    vad: &mut VadChunker,
    speech_tracker: &mut TurnSpeechTracker,
    mic_recording: &Arc<AtomicBool>,
    quiet_until: &mut Option<Instant>,
) {
    while let Ok(frame) = frame_rx.try_recv() {
        process_mock_frame(
            worker,
            epoch,
            frame,
            turn_n,
            session_id,
            audio_dir,
            app,
            mock_phase,
            audio_writer,
            rnnoise,
            downsampler,
            vad,
            speech_tracker,
            mic_recording,
            quiet_until,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_frame_channel_matches_live_depth() {
        assert_eq!(MOCK_FRAME_CHANNEL_DEPTH, 1024);
    }

    #[test]
    fn end_turn_uses_force_end_segment_in_protocol() {
        let src = include_str!("mic_capture.rs");
        let start = src
            .find("MicCommand::EndTurn { reply }")
            .expect("EndTurn handler");
        let body = &src[start..start + 2500];
        assert!(
            body.contains("force_end_segment"),
            "EndTurn must flush trailing VAD segment"
        );
        assert!(
            body.contains("worker_ref.flush"),
            "EndTurn must barrier-flush whisper worker"
        );
        assert!(
            !body.contains("Duration::from_millis(300)"),
            "EndTurn must not use legacy timing drain"
        );
    }

    #[test]
    fn hot_path_does_not_await_whisper() {
        let src = include_str!("mic_capture.rs");
        let start = src
            .find("fn process_mock_frame")
            .expect("process_mock_frame");
        let end = src[start..]
            .find("fn drain_remaining_frames")
            .map(|i| start + i)
            .expect("drain_remaining_frames");
        let body = &src[start..end];
        assert!(
            !body.contains(".await"),
            "process_mock_frame must stay synchronous"
        );
        assert!(
            body.contains("try_transcribe"),
            "process_mock_frame must enqueue STT without blocking"
        );
    }
}
