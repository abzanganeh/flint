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
use crate::audio::vad::{VadChunker, VadChunk};
use crate::mock::tts;
use crate::transcription::engine::WhisperEngine;

const MIC_CALIBRATION_TIMEOUT: Duration = Duration::from_secs(45);
const SYSTEM_CALIBRATION_TIMEOUT: Duration = Duration::from_secs(35);

/// Samples of 16 kHz mono audio per calibration window fed to Whisper.
///
/// 8 seconds × 16 000 samples/s = 128 000 samples.  Keeps chunks well under
/// the 30-second Whisper limit while being long enough to capture a complete
/// calibration sentence.
const CALIB_WINDOW_SAMPLES: usize = 16_000 * 8;

/// Collect audio frames, transcribe in fixed 8-second windows using VAD
/// to determine speech boundaries, and return the joined transcript.
///
/// Uses VAD for speech detection but enforces a maximum window size so
/// Whisper.cpp never receives a 30-second monolithic chunk (which its
/// internal "single timestamp ending" heuristic discards).
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
    // Accumulate VAD-emitted speech samples into a window; flush to Whisper
    // every CALIB_WINDOW_SAMPLES (8 s) so Whisper never sees a 30-second block.
    let mut window: Vec<f32> = Vec::with_capacity(CALIB_WINDOW_SAMPLES);
    let mut parts: Vec<String> = Vec::new();

    macro_rules! flush_window {
        () => {
            if window.len() >= 1_600 {
                // 100 ms minimum — skip shorter noise bursts.
                let n = window.len();
                let samples = std::mem::take(&mut window);
                let duration_ms = (n as u32) / 16; // 16 samples = 1 ms at 16 kHz
                let chunk = VadChunk { samples, source, duration_ms };
                if let Ok(Some(r)) = whisper.transcribe_greedy(&chunk) {
                    let t = r.text.trim().to_owned();
                    if !t.is_empty() {
                        parts.push(t);
                    }
                }
            } else {
                window.clear();
            }
        };
    }

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let frame = match timeout(remaining, frame_rx.recv()).await {
            Ok(Some(f)) => f,
            _ => break,
        };

        if frame.len() != FRAME_SAMPLES {
            continue;
        }

        let mut proc = frame;
        if let Some(rnn) = rnnoise.as_mut() {
            let _ = rnn.process_frame(&mut proc);
        }
        let Ok(downsampled) = downsampler.process(&proc) else {
            continue;
        };

        for chunk_frame in downsampled.chunks(160) {
            if let Some(vad_chunk) = chunker.process_frame(chunk_frame, source) {
                window.extend_from_slice(&vad_chunk.samples);
                if window.len() >= CALIB_WINDOW_SAMPLES {
                    flush_window!();
                }
            }
        }
    }

    // Flush any remaining VAD-buffered audio after timeout.
    flush_window!();

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

/// Generate speech for the system audio calibration clip and route it through
/// PipeWire to the default sink so the monitor source captures it.
///
/// Priority:
///   1. Piper (neural TTS) — produces natural speech that Whisper transcribes
///      accurately.  If Piper is available, use it so the WER test reflects
///      the audio pipeline quality, not TTS quality.
///   2. espeak-ng → paplay — always available fallback.  WER will be elevated
///      when using espeak because Whisper mis-recognises robotic phonemes even
///      on a perfect audio path; a WER warning in this case is expected.
#[cfg(target_os = "linux")]
async fn speak_via_pipewire(text: &str) -> Result<()> {
    // --- Piper path ----------------------------------------------------------
    // Use the same discovery logic as mock interview TTS so the calibration
    // voice matches what the user hears during interviews.
    if let Some(()) = try_speak_piper_via_pipewire(text).await {
        return Ok(());
    }

    // --- espeak-ng + paplay fallback -----------------------------------------
    use std::process::Stdio;
    use tokio::process::Command;

    let wav_path = std::env::temp_dir().join("flint_calib_tts.wav");

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

/// Try to generate speech with Piper and play it via paplay.
/// Returns `Some(())` on success, `None` if Piper is not installed or fails.
#[cfg(target_os = "linux")]
async fn try_speak_piper_via_pipewire(text: &str) -> Option<()> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    let piper_bin = crate::mock::tts::find_piper_bin()?;
    let model = crate::mock::tts::find_piper_model_path()?;
    let wav_path = std::env::temp_dir().join("flint_calib_tts.wav");

    let mut child = Command::new(&piper_bin)
        .args([
            "--model",
            model.to_str()?,
            "--output_file",
            wav_path.to_str()?,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).await.ok()?;
    }

    let out = child.wait_with_output().await.ok()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        tracing::warn!(piper_stderr = %stderr.trim(), "piper failed in calibration TTS");
        return None;
    }

    let status = Command::new("paplay")
        .arg(&wav_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .ok()?;

    if status.success() { Some(()) } else { None }
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
