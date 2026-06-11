//! Per-turn WAV writer for mock interview answers.
//!
//! Collects raw 16kHz mono f32 samples from the mic VAD pipeline and
//! flushes them to a numbered WAV file under `{app_data}/mock_audio/`
//! when `finish()` is called.
//!
//! Uses `hound` for writing — it produces standard RIFF/WAV files playable
//! in every OS media player without additional decoding.

use anyhow::{Context, Result};
use hound::{SampleFormat, WavSpec, WavWriter};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const SAMPLE_RATE: u32 = 16_000;
const NUM_CHANNELS: u16 = 1;
const BITS_PER_SAMPLE: u16 = 16;

/// Accumulates PCM samples in memory and writes a WAV when `finish` is called.
pub struct TurnAudioWriter {
    session_id: Uuid,
    turn_n: u32,
    audio_dir: PathBuf,
    samples: Vec<i16>,
}

impl TurnAudioWriter {
    /// Create a new writer.  `audio_dir` must already exist.
    pub fn new(session_id: Uuid, turn_n: u32, audio_dir: impl AsRef<Path>) -> Self {
        Self {
            session_id,
            turn_n,
            audio_dir: audio_dir.as_ref().to_owned(),
            samples: Vec::new(),
        }
    }

    /// Append a chunk of f32 samples (16kHz mono) from the VAD pipeline.
    pub fn push_samples(&mut self, samples: &[f32]) {
        // Convert f32 [-1, 1] to i16 for storage efficiency.
        let converted = samples
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
        self.samples.extend(converted);
    }

    /// Flush buffered samples to disk and return the absolute WAV path.
    ///
    /// Returns an empty string without error if no samples were captured
    /// (e.g. user pressed Skip before speaking).
    pub fn finish(self) -> Result<String> {
        if self.samples.is_empty() {
            return Ok(String::new());
        }

        let filename = format!(
            "session_{}_turn_{:03}.wav",
            self.session_id.simple(),
            self.turn_n
        );
        let path = self.audio_dir.join(filename);

        let spec = WavSpec {
            channels: NUM_CHANNELS,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: BITS_PER_SAMPLE,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::create(&path, spec)
            .with_context(|| format!("create WAV file {}", path.display()))?;
        for sample in &self.samples {
            writer.write_sample(*sample).context("write WAV sample")?;
        }
        writer.finalize().context("finalize WAV file")?;

        Ok(path.to_string_lossy().into_owned())
    }

    /// Total captured duration in seconds.
    pub fn duration_secs(&self) -> f64 {
        self.samples.len() as f64 / SAMPLE_RATE as f64
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn finish_empty_returns_empty_path() {
        let dir = tempdir().unwrap();
        let writer = TurnAudioWriter::new(Uuid::new_v4(), 1, dir.path());
        let path = writer.finish().unwrap();
        assert!(path.is_empty());
    }

    #[test]
    fn finish_with_samples_creates_wav() {
        let dir = tempdir().unwrap();
        let mut writer = TurnAudioWriter::new(Uuid::new_v4(), 1, dir.path());
        let samples: Vec<f32> = (0..SAMPLE_RATE as usize)
            .map(|i| (i as f32).sin() * 0.5)
            .collect();
        writer.push_samples(&samples);
        let path = writer.finish().unwrap();
        assert!(!path.is_empty());
        assert!(std::path::Path::new(&path).exists());

        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_rate, SAMPLE_RATE);
        assert_eq!(reader.spec().channels, NUM_CHANNELS);
    }

    #[test]
    fn duration_tracks_pushed_samples() {
        let dir = tempdir().unwrap();
        let mut writer = TurnAudioWriter::new(Uuid::new_v4(), 2, dir.path());
        writer.push_samples(&vec![0.0f32; SAMPLE_RATE as usize]);
        assert!((writer.duration_secs() - 1.0).abs() < 0.01);
    }
}
