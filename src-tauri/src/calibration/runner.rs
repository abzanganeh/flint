//! Short-lived capture paths for mic/system calibration tests.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use cpal::traits::{HostTrait, StreamTrait};
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::audio::capture::{
    build_resampled_mono_stream, find_system_device, AudioSource, FRAME_SAMPLES,
};
use crate::audio::rnnoise::{Downsampler, RNNoiseProcessor};
use crate::audio::vad::VadChunker;
use crate::mock::tts;
use crate::transcription::engine::WhisperEngine;

const MIC_CALIBRATION_TIMEOUT: Duration = Duration::from_secs(45);
const SYSTEM_CALIBRATION_TIMEOUT: Duration = Duration::from_secs(35);

async fn transcribe_from_frames(
    whisper: Arc<WhisperEngine>,
    mut frame_rx: mpsc::Receiver<Vec<f32>>,
    source: AudioSource,
    use_rnnoise: bool,
    deadline: Instant,
) -> Result<String> {
    let mut rnnoise = if use_rnnoise {
        Some(RNNoiseProcessor::new()?)
    } else {
        None
    };
    let mut downsampler = Downsampler::new()?;
    let mut chunker = VadChunker::new()?;
    let mut parts: Vec<String> = Vec::new();

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Some(frame) = timeout(remaining, frame_rx.recv())
            .await
            .context("calibration timed out waiting for audio")?
        else {
            break;
        };

        if frame.len() != FRAME_SAMPLES {
            continue;
        }

        let mut proc = frame;
        if let Some(rnn) = rnnoise.as_mut() {
            rnn.process_frame(&mut proc)?;
        }
        let downsampled = downsampler.process(&proc)?;
        for chunk_frame in downsampled.chunks(160) {
            if let Some(chunk) = chunker.process_frame(chunk_frame, source) {
                if let Some(result) = tokio::task::spawn_blocking({
                    let whisper = Arc::clone(&whisper);
                    move || whisper.transcribe(&chunk)
                })
                .await
                .context("join calibration whisper task")??
                {
                    if !result.text.is_empty() {
                        parts.push(result.text);
                    }
                }
            }
        }
    }

    Ok(parts.join(" "))
}

/// Record mic audio until speech ends or timeout; return concatenated transcript.
pub async fn transcribe_mic_calibration(whisper: Arc<WhisperEngine>) -> Result<String> {
    let (frame_tx, frame_rx) = mpsc::channel::<Vec<f32>>(512);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<()>>();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();

    std::thread::spawn(move || {
        if let Err(e) = run_mic_thread(frame_tx, ready_tx, stop_rx) {
            tracing::warn!(error = %e, "calibration mic thread failed");
        }
    });

    ready_rx
        .await
        .map_err(|_| anyhow::anyhow!("calibration mic thread died before ready"))?
        .context("calibration mic stream init")?;

    let transcript = transcribe_from_frames(
        whisper,
        frame_rx,
        AudioSource::Microphone,
        true,
        Instant::now() + MIC_CALIBRATION_TIMEOUT,
    )
    .await?;

    drop(stop_tx);
    Ok(transcript)
}

/// Play the system clip via TTS and capture loopback; return transcript.
pub async fn transcribe_system_calibration(
    reference_text: &str,
    whisper: Arc<WhisperEngine>,
) -> Result<String> {
    let (frame_tx, frame_rx) = mpsc::channel::<Vec<f32>>(512);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<()>>();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();

    std::thread::spawn(move || {
        if let Err(e) = run_system_thread(frame_tx, ready_tx, stop_rx) {
            tracing::warn!(error = %e, "calibration system thread failed");
        }
    });

    ready_rx
        .await
        .map_err(|_| anyhow::anyhow!("calibration system thread died before ready"))?
        .context("calibration system stream init")?;

    let tts_text = reference_text.to_string();
    let tts_handle = tokio::spawn(async move { tts::speak(&tts_text).await });

    let transcript = transcribe_from_frames(
        whisper,
        frame_rx,
        AudioSource::System,
        false,
        Instant::now() + SYSTEM_CALIBRATION_TIMEOUT,
    )
    .await?;

    let _ = tts_handle.await;
    drop(stop_tx);
    Ok(transcript)
}

fn run_mic_thread(
    frame_tx: mpsc::Sender<Vec<f32>>,
    ready_tx: tokio::sync::oneshot::Sender<Result<()>>,
    stop_rx: std::sync::mpsc::Receiver<()>,
) -> Result<()> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("no default input device for mic calibration")?;
    let stream = build_resampled_mono_stream(&device, frame_tx)?;
    stream.play()?;
    let _ = ready_tx.send(Ok(()));
    let _ = stop_rx.recv();
    drop(stream);
    Ok(())
}

fn run_system_thread(
    frame_tx: mpsc::Sender<Vec<f32>>,
    ready_tx: tokio::sync::oneshot::Sender<Result<()>>,
    stop_rx: std::sync::mpsc::Receiver<()>,
) -> Result<()> {
    let host = cpal::default_host();
    let device = find_system_device(&host)?;
    let stream = build_resampled_mono_stream(&device, frame_tx)?;
    stream.play()?;
    let _ = ready_tx.send(Ok(()));
    let _ = stop_rx.recv();
    drop(stream);
    Ok(())
}
