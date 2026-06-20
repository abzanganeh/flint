//! Short-lived capture paths for mic/system calibration tests.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
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
///
/// On Linux, TTS is forced through PipeWire/PulseAudio (`paplay` or
/// `espeak-ng --stdout | paplay`) so the audio definitely reaches the
/// default sink monitor source that the loopback capture is listening to.
/// If `espeak-ng` writes directly to an ALSA hw: device, the monitor source
/// never sees the audio and the capture times out.
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
    let tts_handle = tokio::spawn(async move {
        #[cfg(target_os = "linux")]
        {
            // Prefer PipeWire-routed playback so the monitor source captures it.
            if speak_via_pipewire(&tts_text).await.is_ok() {
                return Ok(());
            }
        }
        tts::speak(&tts_text).await
    });

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

/// Generate speech with espeak-ng, save WAV to a temp file, then play it with
/// `paplay` which routes through PipeWire to the default sink.
///
/// This guarantees the calibration clip reaches the default output device and
/// is therefore captured by its PipeWire monitor source.
#[cfg(target_os = "linux")]
async fn speak_via_pipewire(text: &str) -> Result<()> {
    use std::process::Stdio;
    use tokio::process::Command;

    let wav_path = std::env::temp_dir().join("flint_calib_tts.wav");

    // espeak-ng --stdout writes a 16-bit PCM WAV; save to temp file first.
    let status = Command::new("espeak-ng")
        .args(["-s", "150", "--stdout", text])
        .stdout(Stdio::from(
            std::fs::File::create(&wav_path).context("create TTS wav temp file")?,
        ))
        .stderr(Stdio::null())
        .status()
        .await
        .context("spawn espeak-ng --stdout")?;

    anyhow::ensure!(
        status.success(),
        "espeak-ng exited with {:?}",
        status.code()
    );

    // paplay routes through PipeWire to the default sink.
    let status = Command::new("paplay")
        .arg(&wav_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("spawn paplay")?;

    anyhow::ensure!(status.success(), "paplay exited with {:?}", status.code());

    Ok(())
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
    tracing::info!(
        device = %device.name().unwrap_or_else(|_| "unknown".into()),
        "system calibration: loopback capture device selected"
    );
    let stream = build_resampled_mono_stream(&device, frame_tx)?;
    stream.play()?;
    let _ = ready_tx.send(Ok(()));
    let _ = stop_rx.recv();
    drop(stream);
    Ok(())
}
