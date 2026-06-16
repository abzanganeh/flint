//! Dual-channel audio capture — Tasks 3.2, 3.8, 3.9.
//!
//! SECURITY INVARIANT: Audio data is NEVER written to disk.
//! This file contains no filesystem access, no file creation, no temp files.
//! All audio lives in the fixed ring buffers below and is zeroed on session end.
//!
//! Processing chain produced by this module:
//!   cpal callback → mono conversion → resample to 16kHz → ring buffer
//!   → drain FRAME_SAMPLES → `AudioFrame` sent over tokio mpsc channel

#![allow(dead_code)]

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, StreamConfig};
use rubato::{FftFixedOut, Resampler};
use tokio::sync::mpsc;
use tracing::{debug, error};
#[cfg(target_os = "linux")]
use tracing::info;

// ────────────────────────────────────────────────────────────────────────────
// Public types
// ────────────────────────────────────────────────────────────────────────────

/// Which audio channel a frame originated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSource {
    System,
    Microphone,
}

impl std::fmt::Display for AudioSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioSource::System => write!(f, "System"),
            AudioSource::Microphone => write!(f, "Microphone"),
        }
    }
}

/// One frame of 16kHz PCM mono audio with channel provenance.
///
/// SECURITY: `AudioFrame.samples` is the ONLY audio data structure in this
/// pipeline. Samples live in memory only and are never written to any file,
/// path, or disk.
pub struct AudioFrame {
    pub samples: Vec<f32>,
    pub source: AudioSource,
    pub timestamp: Instant,
}

// ────────────────────────────────────────────────────────────────────────────
// Constants
// ────────────────────────────────────────────────────────────────────────────

/// Ring buffer capacity per channel — Task 3.8: exactly `[f32; 16_384]`.
const RING_CAPACITY: usize = 16_384;

/// Capture sample rate. Hardware almost universally runs at 48kHz natively,
/// so this avoids a conversion in the hot path. RNNoise is designed for 48kHz
/// with 480-sample frames. Downsampling to 16kHz happens once in rnnoise.rs
/// after denoising, before samples reach Whisper and VAD.
pub const TARGET_RATE: u32 = 48_000;

/// Samples per `AudioFrame` sent downstream = 10 ms of 48kHz audio.
/// Matches RNNoise's required frame size exactly (480 samples at 48kHz).
/// rnnoise.rs denoises these 480 samples then downsamples to 160 at 16kHz.
pub const FRAME_SAMPLES: usize = 480;

/// Maximum time allowed for stream reinitialisation (Task 3.9).
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(5);

// ────────────────────────────────────────────────────────────────────────────
// Ring buffer — Task 3.8
// ────────────────────────────────────────────────────────────────────────────

/// Fixed-size circular buffer backed by a heap-allocated `[f32; RING_CAPACITY]`.
///
/// When the buffer is full, incoming writes overwrite the oldest samples (true
/// ring buffer — never grows). Zeroed in-place on session end so no audio
/// data lingers in memory.
struct ChannelRingBuffer {
    buf: Box<[f32; RING_CAPACITY]>,
    /// Index of the oldest unread sample.
    head: usize,
    /// Number of valid samples currently stored (0 ..= RING_CAPACITY).
    count: usize,
}

impl ChannelRingBuffer {
    fn new() -> Self {
        Self {
            buf: Box::new([0.0f32; RING_CAPACITY]),
            head: 0,
            count: 0,
        }
    }

    /// Push `samples` into the ring.  When full, oldest data is overwritten.
    fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            let write_at = (self.head + self.count) % RING_CAPACITY;
            self.buf[write_at] = s;
            if self.count < RING_CAPACITY {
                self.count += 1;
            } else {
                // Full: advance the read head to discard the oldest sample.
                self.head = (self.head + 1) % RING_CAPACITY;
            }
        }
    }

    /// Drain exactly `n` samples into `out`.
    ///
    /// Returns `true` on success. If fewer than `n` samples are available,
    /// nothing is consumed and `false` is returned.
    fn drain_exact(&mut self, out: &mut Vec<f32>, n: usize) -> bool {
        if self.count < n {
            return false;
        }
        out.reserve(n);
        for i in 0..n {
            out.push(self.buf[(self.head + i) % RING_CAPACITY]);
        }
        self.head = (self.head + n) % RING_CAPACITY;
        self.count -= n;
        true
    }

    fn available(&self) -> usize {
        self.count
    }

    /// Zero every sample and reset positions — session-end security guarantee.
    fn zero(&mut self) {
        self.buf.fill(0.0f32);
        self.head = 0;
        self.count = 0;
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Per-stream state shared with the cpal data callback
// ────────────────────────────────────────────────────────────────────────────

/// All mutable state that both the cpal audio thread and the owning
/// `AudioCapture` need to touch.  Always accessed through `Arc<Mutex<_>>`.
pub(super) struct StreamState {
    ring: ChannelRingBuffer,
    /// Software resampler; `None` when the device already delivers 16kHz.
    resampler: Option<FftFixedOut<f32>>,
    /// Accumulates native-rate mono samples until there are enough for one
    /// resampling step.
    native_buf: Vec<f32>,
    /// `input_frames_next()` result cached after every process call.
    native_chunk: usize,
    /// Set by the error callback when cpal reports a stream failure (Task 3.9).
    pub recovering: bool,
    /// Timestamp of the first error within the current recovery episode.
    pub recovery_start: Option<Instant>,
}

impl StreamState {
    fn new(native_rate: u32) -> Result<Self> {
        let (resampler, native_chunk) = if native_rate != TARGET_RATE {
            let rs = FftFixedOut::<f32>::new(
                native_rate as usize,
                TARGET_RATE as usize,
                FRAME_SAMPLES, // desired output chunk = 160 samples at 16kHz
                2,             // sub_chunks
                1,             // mono
            )
            .context("Failed to create audio resampler")?;
            let chunk = rs.input_frames_next();
            (Some(rs), chunk)
        } else {
            (None, 0)
        };

        Ok(Self {
            ring: ChannelRingBuffer::new(),
            resampler,
            native_buf: Vec::new(),
            native_chunk,
            recovering: false,
            recovery_start: None,
        })
    }

    /// Convert multi-channel native-rate samples to 16kHz mono and push to
    /// the ring buffer.
    ///
    /// Steps:
    ///   1. Average all input channels → mono.
    ///   2. If native_rate == 16kHz: push directly.
    ///   3. Otherwise: accumulate in `native_buf`, resample via rubato in
    ///      `FRAME_SAMPLES`-sized output chunks, push resampled output.
    fn ingest(&mut self, raw: &[f32], channels: usize) {
        // ── Step 1: N channels → mono ─────────────────────────────────────
        let mono: Vec<f32> = if channels == 1 {
            raw.to_vec()
        } else {
            raw.chunks_exact(channels)
                .map(|ch| ch.iter().sum::<f32>() / channels as f32)
                .collect()
        };

        // ── Steps 2/3: optional resample → ring ──────────────────────────
        if let Some(rs) = &mut self.resampler {
            self.native_buf.extend_from_slice(&mono);

            while self.native_buf.len() >= self.native_chunk {
                // rubato expects `&[Vec<T>]` — one Vec per channel.
                let input = vec![self.native_buf[..self.native_chunk].to_vec()];
                self.native_buf.drain(..self.native_chunk);

                match rs.process(&input, None) {
                    Ok(output) => {
                        if let Some(ch) = output.first() {
                            self.ring.push(ch);
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "audio resampler error — chunk dropped");
                    }
                }

                // Refresh the required input size for the next iteration.
                self.native_chunk = rs.input_frames_next();
            }
        } else {
            self.ring.push(&mono);
        }
    }

    /// Zero both ring buffer and native accumulation buffer.
    fn zero(&mut self) {
        self.ring.zero();
        self.native_buf.clear();
    }
}

// ────────────────────────────────────────────────────────────────────────────
// AudioCapture — public API
// ────────────────────────────────────────────────────────────────────────────

/// Dual-channel audio capture.  Holds two live `cpal::Stream`s (system/mic)
/// and routes `AudioFrame`s to callers via tokio mpsc channels.
///
/// SECURITY: `stop()` zeroes both ring buffers before returning so that no
/// audio data outlives the session.
pub struct AudioCapture {
    system_stream: Option<cpal::Stream>,
    mic_stream: Option<cpal::Stream>,
    system_tx: mpsc::Sender<AudioFrame>,
    mic_tx: mpsc::Sender<AudioFrame>,
    // ── Ring buffer state (Task 3.8) ──────────────────────────────────────
    system_state: Arc<Mutex<StreamState>>,
    mic_state: Arc<Mutex<StreamState>>,
    // ── Recovery flags (Task 3.9) ─────────────────────────────────────────
    /// Set to `true` by the system stream error callback.
    pub system_recovering: Arc<AtomicBool>,
    /// Set to `true` by the microphone stream error callback.
    pub mic_recovering: Arc<AtomicBool>,
}

impl AudioCapture {
    /// Start dual-channel capture.
    ///
    /// Returns `Err` if the platform's system audio device cannot be found
    /// (e.g. BlackHole not installed on macOS, no PipeWire monitor on Linux).
    pub fn start(
        system_tx: mpsc::Sender<AudioFrame>,
        mic_tx: mpsc::Sender<AudioFrame>,
    ) -> Result<Self> {
        let host = cpal::default_host();

        let system_recovering = Arc::new(AtomicBool::new(false));
        let mic_recovering = Arc::new(AtomicBool::new(false));

        // ── System audio (loopback / monitor) ────────────────────────────
        let sys_dev = find_system_device(&host)?;
        let (sys_cfg, sys_rate) = select_stream_config(&sys_dev)
            .context("Failed to select system audio stream config")?;
        let sys_state = Arc::new(Mutex::new(
            StreamState::new(sys_rate).context("Failed to init system StreamState")?,
        ));
        let sys_stream = build_input_stream(
            &sys_dev,
            &sys_cfg,
            AudioSource::System,
            Arc::clone(&sys_state),
            system_tx.clone(),
            Arc::clone(&system_recovering),
        )
        .context("Failed to build system audio stream")?;
        sys_stream
            .play()
            .context("Failed to start system audio stream")?;

        // ── Microphone ────────────────────────────────────────────────────
        let mic_dev = find_mic_device(&host)?;
        let (mic_cfg, mic_rate) =
            select_stream_config(&mic_dev).context("Failed to select microphone stream config")?;
        let mic_state = Arc::new(Mutex::new(
            StreamState::new(mic_rate).context("Failed to init microphone StreamState")?,
        ));
        let mic_stream = build_input_stream(
            &mic_dev,
            &mic_cfg,
            AudioSource::Microphone,
            Arc::clone(&mic_state),
            mic_tx.clone(),
            Arc::clone(&mic_recovering),
        )
        .context("Failed to build microphone stream")?;
        mic_stream
            .play()
            .context("Failed to start microphone stream")?;

        Ok(Self {
            system_stream: Some(sys_stream),
            mic_stream: Some(mic_stream),
            system_tx,
            mic_tx,
            system_state: sys_state,
            mic_state,
            system_recovering,
            mic_recovering,
        })
    }

    /// Stop capture and zero all ring buffers.
    ///
    /// Drops the cpal streams first (callbacks stop immediately), then zeros
    /// both ring buffers so no audio data remains in memory.
    pub fn stop(mut self) -> Result<()> {
        // Streams must be dropped before zeroing so callbacks can no longer
        // write into the ring buffers.
        drop(self.system_stream.take());
        drop(self.mic_stream.take());

        if let Ok(mut s) = self.system_state.lock() {
            s.zero();
        }
        if let Ok(mut s) = self.mic_state.lock() {
            s.zero();
        }
        debug!("audio ring buffer cleared");
        Ok(())
    }

    // ── Recovery helpers — Task 3.9 ───────────────────────────────────────

    /// Returns `true` if the given channel is currently in recovery mode.
    pub fn is_recovering(&self, source: AudioSource) -> bool {
        match source {
            AudioSource::System => self.system_recovering.load(Ordering::SeqCst),
            AudioSource::Microphone => self.mic_recovering.load(Ordering::SeqCst),
        }
    }

    /// Attempt to reinitialise a failed stream.
    ///
    /// Returns the duration the stream was down on success, or `Err` if:
    ///   - the 5-second recovery window has been exceeded, or
    ///   - the new stream cannot be built.
    ///
    /// Called by `run_audio_pipeline` (Task 3.7) in its recovery loop.
    pub fn try_reinit(&mut self, source: AudioSource) -> Result<Duration> {
        // ── Check timeout ─────────────────────────────────────────────────
        let recovery_start = {
            let state = match source {
                AudioSource::System => &self.system_state,
                AudioSource::Microphone => &self.mic_state,
            };
            state
                .lock()
                .map_err(|_| anyhow!("stream state mutex poisoned"))?
                .recovery_start
                .ok_or_else(|| anyhow!("try_reinit called but recovery_start not set"))?
        };

        let elapsed = recovery_start.elapsed();
        if elapsed > RECOVERY_TIMEOUT {
            return Err(anyhow!(
                "{source} stream recovery timed out after {:.1}s",
                elapsed.as_secs_f32()
            ));
        }

        // ── Drop old stream ───────────────────────────────────────────────
        match source {
            AudioSource::System => drop(self.system_stream.take()),
            AudioSource::Microphone => drop(self.mic_stream.take()),
        }

        // ── Re-enumerate device and rebuild stream ────────────────────────
        let host = cpal::default_host();
        let (dev, tx, state_arc, recovering_flag) = match source {
            AudioSource::System => (
                find_system_device(&host)?,
                self.system_tx.clone(),
                Arc::clone(&self.system_state),
                Arc::clone(&self.system_recovering),
            ),
            AudioSource::Microphone => (
                host.default_input_device()
                    .ok_or_else(|| anyhow!("No default microphone found"))?,
                self.mic_tx.clone(),
                Arc::clone(&self.mic_state),
                Arc::clone(&self.mic_recovering),
            ),
        };

        let (cfg, rate) =
            select_stream_config(&dev).context("Could not select config for reinit")?;

        // Reinitialise the StreamState resampler for the (possibly different) rate.
        {
            let mut st = state_arc
                .lock()
                .map_err(|_| anyhow!("state mutex poisoned during reinit"))?;
            *st = StreamState::new(rate).context("Failed to init reinit StreamState")?;
        }

        let new_stream = build_input_stream(
            &dev,
            &cfg,
            source,
            Arc::clone(&state_arc),
            tx,
            Arc::clone(&recovering_flag),
        )?;
        new_stream
            .play()
            .context("Failed to start reinitialised stream")?;

        // ── Clear recovery flag ───────────────────────────────────────────
        recovering_flag.store(false, Ordering::SeqCst);
        if let Ok(mut st) = state_arc.lock() {
            st.recovering = false;
            st.recovery_start = None;
        }

        match source {
            AudioSource::System => self.system_stream = Some(new_stream),
            AudioSource::Microphone => self.mic_stream = Some(new_stream),
        }

        Ok(elapsed)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Microphone device selection
// ────────────────────────────────────────────────────────────────────────────

/// Choose the microphone input device.
///
/// Resolution order:
///   1. `FLINT_MIC_SOURCE` — device name substring (case-insensitive). Lets
///      users point directly at a PipeWire AEC virtual source set up by
///      `scripts/setup-pipewire-aec.sh`, or any other named device.
///   2. On Linux: auto-detect a running echo-cancel source (PipeWire
///      `module-echo-cancel`) by scanning for a device whose name contains
///      "echo" or "cancel". Suppresses speaker bleed at the OS level.
///   3. System default input device.
fn find_mic_device(host: &cpal::Host) -> Result<Device> {
    // ── Explicit override ─────────────────────────────────────────────────
    if let Ok(target) = std::env::var("FLINT_MIC_SOURCE") {
        let target = target.trim().to_lowercase();
        if !target.is_empty() {
            let devs: Vec<Device> = host
                .input_devices()
                .context("Failed to enumerate input devices")?
                .collect();
            for dev in &devs {
                if dev
                    .name()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&target)
                {
                    debug!(
                        device = %dev.name().unwrap_or_default(),
                        "FLINT_MIC_SOURCE: using named mic device"
                    );
                    return Ok(dev.clone());
                }
            }
            error!(
                target = %target,
                "FLINT_MIC_SOURCE set but no matching input device found — falling back"
            );
        }
    }

    // ── Linux: prefer PipeWire echo-cancel virtual source ─────────────────
    #[cfg(target_os = "linux")]
    if let Some(dev) = find_echo_cancel_device(host) {
        return Ok(dev);
    }

    // ── Default ───────────────────────────────────────────────────────────
    host.default_input_device()
        .ok_or_else(|| anyhow!("No default microphone input device found"))
}

/// Scan input devices for a PipeWire echo-cancel virtual source.
///
/// When `setup-pipewire-aec.sh` has run, PipeWire exposes a source named
/// something like `echo-cancel-source` or `easyeffects_source`. We prefer
/// this over the raw mic so the OS handles AEC before samples reach Flint.
#[cfg(target_os = "linux")]
fn find_echo_cancel_device(host: &cpal::Host) -> Option<Device> {
    let devs = host.input_devices().ok()?;
    for dev in devs {
        let name = dev.name().unwrap_or_default().to_lowercase();
        if name.contains("echo") || name.contains("cancel") || name.contains("aec") {
            info!(device = %dev.name().unwrap_or_default(), "auto-selected PipeWire echo-cancel mic source");
            return Some(dev);
        }
    }
    None
}

// ────────────────────────────────────────────────────────────────────────────
// Platform-specific system audio device selection
// ────────────────────────────────────────────────────────────────────────────

/// Locate the platform's system audio loopback device.
///
/// | Platform | Strategy |
/// |----------|----------|
/// | Linux    | Enumerate input devices; pick first matching "monitor" or "loopback" (PipeWire) |
/// | Windows  | Default output device — WASAPI allows loopback capture from output devices |
/// | macOS    | Enumerate input devices; pick first matching "BlackHole" |
/// On PipeWire/PulseAudio, monitor sources are visible to `pactl` but not as
/// ALSA device names containing "monitor". Route the ALSA `pulse`/`pipewire`
/// plugin to the default sink's `.monitor` source before opening cpal.
#[cfg(target_os = "linux")]
fn configure_pipewire_monitor_source() {
    if std::env::var_os("PULSE_SOURCE").is_some() {
        return;
    }
    let Ok(output) = std::process::Command::new("pactl")
        .args(["get-default-sink"])
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let sink = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sink.is_empty() {
        return;
    }
    std::env::set_var("PULSE_SOURCE", format!("{sink}.monitor"));
    debug!(pulse_source = %format!("{sink}.monitor"), "configured PipeWire monitor source for ALSA capture");
}

#[cfg(target_os = "linux")]
fn find_system_device(host: &cpal::Host) -> Result<Device> {
    configure_pipewire_monitor_source();

    let devs: Vec<Device> = host
        .input_devices()
        .context("Failed to enumerate input devices")?
        .collect();

    for dev in &devs {
        let name = dev.name().unwrap_or_default().to_lowercase();
        if name.contains("monitor") || name.contains("loopback") {
            return Ok(dev.clone());
        }
    }

    // PipeWire exposes sink monitors through the ALSA pulse/pipewire plugins.
    for dev in devs {
        let name = dev.name().unwrap_or_default().to_lowercase();
        if name == "pipewire" || name == "pulse" {
            return Ok(dev);
        }
    }

    Err(anyhow!(
        "No PipeWire monitor source found. Set PULSE_SOURCE to your sink monitor \
         (e.g. `export PULSE_SOURCE=\"$(pactl get-default-sink).monitor\"`) \
         before starting a session. Do NOT run `pactl load-module module-loopback` \
         — that routes your microphone to your speakers."
    ))
}

#[cfg(target_os = "windows")]
fn find_system_device(host: &cpal::Host) -> Result<Device> {
    // WASAPI loopback: the default output device can be used as a loopback
    // input source — cpal's WASAPI backend supports build_input_stream on
    // output devices for this purpose.
    host.default_output_device()
        .ok_or_else(|| anyhow!("No audio output device found for WASAPI loopback capture"))
}

#[cfg(target_os = "macos")]
fn find_system_device(host: &cpal::Host) -> Result<Device> {
    let devs = host
        .input_devices()
        .context("Failed to enumerate input devices")?;
    for dev in devs {
        let name = dev.name().unwrap_or_default();
        if name.to_lowercase().contains("blackhole") {
            return Ok(dev);
        }
    }
    Err(anyhow!(
        "BlackHole virtual audio driver not found. \
         Install BlackHole 2ch from https://existential.audio/blackhole/ \
         then create a Multi-Output Device (Speakers + BlackHole 2ch) \
         in Audio MIDI Setup and set it as your system output."
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn find_system_device(_host: &cpal::Host) -> Result<Device> {
    Err(anyhow!(
        "System audio loopback is not supported on this platform"
    ))
}

// ────────────────────────────────────────────────────────────────────────────
// Stream configuration selection
// ────────────────────────────────────────────────────────────────────────────

/// Choose the best `StreamConfig` for a device.
///
/// Always uses the device's native sample rate and lets `StreamState` resample
/// down to 16 kHz via rubato. Forcing 16 kHz on a shared device (PipeWire
/// monitor on Linux, WASAPI shared mode on Windows) renegotiates the server's
/// global rate, which glitches concurrent playback and other clients (Zoom,
/// browsers). Rubato 48 → 16 kHz is < 1 ms per frame, cheaper than the cost
/// of an audio-server reconfiguration.
///
/// For WASAPI loopback on Windows the device is an output device; we fall back
/// to querying output configs when input configs are empty.
pub(crate) fn select_stream_config(device: &Device) -> Result<(StreamConfig, u32)> {
    let default = device
        .default_input_config()
        .or_else(|_| device.default_output_config())
        .context("Cannot determine device default stream config")?;
    let native = default.sample_rate().0;
    debug!(
        device = ?device.name().unwrap_or_default(),
        native_rate = native,
        "using device-native rate; rubato handles downsample to 16 kHz"
    );
    Ok((default.config(), native))
}

// ────────────────────────────────────────────────────────────────────────────
// Stream builder
// ────────────────────────────────────────────────────────────────────────────

/// Build a cpal input stream that feeds `AudioFrame`s to `tx`.
///
/// The data callback:
///   1. Ingests raw samples into `StreamState` (mono conversion + optional
///      resampling into the ring buffer).
///   2. Drains `FRAME_SAMPLES`-sized blocks as `AudioFrame`s via `tx`.
///      Uses `try_send` to avoid blocking the audio thread; full-channel
///      frames are silently dropped.
///
/// The error callback:
///   1. Logs via `tracing::error!` with `source` and `error` fields.
///   2. Sets `recovering = true` and records `recovery_start` so the
///      pipeline runner can initiate the 5-second recovery loop (Task 3.9).
fn build_input_stream(
    device: &Device,
    config: &StreamConfig,
    source: AudioSource,
    state: Arc<Mutex<StreamState>>,
    tx: mpsc::Sender<AudioFrame>,
    recovering: Arc<AtomicBool>,
) -> Result<cpal::Stream> {
    let channels = config.channels as usize;

    // Each closure gets its own Arc clone.
    let state_d = Arc::clone(&state);
    let tx_d = tx;

    let data_cb = move |data: &[f32], _: &cpal::InputCallbackInfo| {
        let mut st = match state_d.lock() {
            Ok(g) => g,
            Err(_) => return, // poisoned — cannot recover inside a callback
        };

        st.ingest(data, channels);

        // Drain completed FRAME_SAMPLES blocks and forward as AudioFrames.
        loop {
            if st.ring.available() < FRAME_SAMPLES {
                break;
            }
            let mut samples = Vec::with_capacity(FRAME_SAMPLES);
            if !st.ring.drain_exact(&mut samples, FRAME_SAMPLES) {
                break;
            }
            let frame = AudioFrame {
                samples,
                source,
                timestamp: Instant::now(),
            };
            // Non-blocking send: drop frames rather than stall the audio thread.
            if tx_d.try_send(frame).is_err() {
                debug!(source = %source, "audio frame dropped — pipeline channel full");
            }
        }
    };

    let state_e = Arc::clone(&state);
    let recovering_e = Arc::clone(&recovering);

    let error_cb = move |e: cpal::StreamError| {
        error!(source = %source, error = %e, "audio stream error");
        recovering_e.store(true, Ordering::SeqCst);
        if let Ok(mut st) = state_e.lock() {
            // Only record the first error of this recovery episode.
            if st.recovery_start.is_none() {
                st.recovery_start = Some(Instant::now());
            }
            st.recovering = true;
        }
    };

    device
        .build_input_stream(config, data_cb, error_cb, None)
        .context("Failed to build cpal input stream")
}

/// Build a microphone input stream that resamples to 48 kHz mono and emits
/// [`FRAME_SAMPLES`]-sized chunks. Used by mock-interview mic capture.
pub(crate) fn build_resampled_mono_stream(
    device: &Device,
    tx: mpsc::Sender<Vec<f32>>,
) -> Result<cpal::Stream> {
    let (config, native_rate) = select_stream_config(device)?;
    let state = Arc::new(Mutex::new(StreamState::new(native_rate)?));
    let state_d = Arc::clone(&state);
    let channels = config.channels as usize;

    let data_cb = move |data: &[f32], _: &cpal::InputCallbackInfo| {
        let mut st = match state_d.lock() {
            Ok(g) => g,
            Err(_) => return,
        };

        st.ingest(data, channels);

        loop {
            if st.ring.available() < FRAME_SAMPLES {
                break;
            }
            let mut samples = Vec::with_capacity(FRAME_SAMPLES);
            if !st.ring.drain_exact(&mut samples, FRAME_SAMPLES) {
                break;
            }
            if tx.try_send(samples).is_err() {
                debug!("mock mic frame dropped — channel full");
            }
        }
    };

    let error_cb = move |e: cpal::StreamError| {
        error!(error = %e, "mock mic stream error");
    };

    device
        .build_input_stream(&config, data_cb, error_cb, None)
        .context("Failed to build mock mic input stream")
}

// ────────────────────────────────────────────────────────────────────────────
// Tests — Task 3.8
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Ring buffer correctness ───────────────────────────────────────────

    #[test]
    fn ring_push_and_drain_exact() {
        let mut rb = ChannelRingBuffer::new();
        rb.push(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(rb.available(), 5);

        let mut out = Vec::new();
        assert!(rb.drain_exact(&mut out, 3));
        assert_eq!(out, vec![1.0, 2.0, 3.0]);
        assert_eq!(rb.available(), 2);
    }

    #[test]
    fn ring_drain_exact_returns_false_when_insufficient() {
        let mut rb = ChannelRingBuffer::new();
        rb.push(&[1.0, 2.0]);
        let mut out = Vec::new();
        // Requesting 3 samples when only 2 are available — must not consume.
        assert!(!rb.drain_exact(&mut out, 3));
        assert!(out.is_empty());
        assert_eq!(rb.available(), 2); // unchanged
    }

    #[test]
    fn ring_overwrites_oldest_when_full() {
        let mut rb = ChannelRingBuffer::new();
        // Fill to capacity with value 0.5.
        let fill: Vec<f32> = vec![0.5; RING_CAPACITY];
        rb.push(&fill);
        assert_eq!(rb.available(), RING_CAPACITY);

        // Push one more sample (1.0) — oldest 0.5 is overwritten.
        rb.push(&[1.0]);
        assert_eq!(rb.available(), RING_CAPACITY); // count unchanged

        // The 1.0 sample should be at the write tail, i.e. the last one we
        // can read after draining RING_CAPACITY - 1 samples.
        let mut out = Vec::new();
        rb.drain_exact(&mut out, RING_CAPACITY - 1);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < f32::EPSILON));

        let mut tail = Vec::new();
        assert!(rb.drain_exact(&mut tail, 1));
        assert!((tail[0] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn ring_zero_clears_all_samples() {
        let mut rb = ChannelRingBuffer::new();
        rb.push(&[1.0, 2.0, 3.0]);
        rb.zero();
        assert_eq!(rb.available(), 0);
        // Buf should be all zeros — verify a slice of it.
        assert!(rb.buf.iter().all(|&s| s == 0.0));
    }

    // ── Security: no disk writes ──────────────────────────────────────────

    #[test]
    fn capture_rs_contains_no_fs_usage() {
        // Build the banned patterns at runtime so this test's own source
        // does not contain the literal strings and trigger a false positive.
        let src = include_str!("capture.rs");
        let banned = [
            ["std", "::", "fs"].concat(),
            ["File", "::", "create"].concat(),
            "OpenOptions".to_string(),
        ];
        for pattern in &banned {
            let occurrences = src.match_indices(pattern.as_str()).count();
            // The test itself must not appear in the scan window — we count
            // only occurrences outside the tests module.  A simpler and
            // correct approach: if a pattern appears more than once, only
            // appearances that are *not* inside a string literal in the
            // test body count.  We keep it simple: assert zero matches in
            // the non-test portion of the file.
            let test_mod_start = src.find("#[cfg(test)]").unwrap_or(src.len());
            let production_src = &src[..test_mod_start];
            assert!(
                !production_src.contains(pattern.as_str()),
                "capture.rs production code must not contain '{pattern}' (found {occurrences} total occurrences)"
            );
        }
    }
}
