//! Structured local metrics — `~/.flint/metrics.log` per performance rules.

use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;

fn metrics_log_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    home.join(".flint").join("metrics.log")
}

fn append_json_line(value: &impl Serialize) -> Result<()> {
    let path = metrics_log_path();
    if let Some(parent) = path.parent() {
        create_dir_all(parent).context("create metrics dir")?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open metrics log at {}", path.display()))?;
    let line = serde_json::to_string(value).context("serialize metrics event")?;
    writeln!(file, "{line}").context("append metrics line")?;
    Ok(())
}

#[derive(Serialize)]
pub struct AudioQualityCalibrationEvent<'a> {
    pub timestamp: String,
    pub level: &'static str,
    pub service: &'static str,
    pub event: &'static str,
    pub wer_phase1: f32,
    pub wer_phase2: f32,
    pub passed: bool,
    pub device_id: &'a str,
    pub forced: bool,
}

pub fn log_audio_quality_calibration(
    device_id: &str,
    wer_phase1: f32,
    wer_phase2: f32,
    passed: bool,
    forced: bool,
) {
    let payload = AudioQualityCalibrationEvent {
        timestamp: Utc::now().to_rfc3339(),
        level: "INFO",
        service: "calibration",
        event: "audio_quality_calibration",
        wer_phase1,
        wer_phase2,
        passed,
        device_id,
        forced,
    };
    if let Err(e) = append_json_line(&payload) {
        tracing::warn!(error = %e, "failed to write audio quality metrics");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_event_serializes_required_fields() {
        let json = serde_json::to_string(&AudioQualityCalibrationEvent {
            timestamp: "2026-06-19T00:00:00Z".into(),
            level: "INFO",
            service: "calibration",
            event: "audio_quality_calibration",
            wer_phase1: 0.1,
            wer_phase2: 0.15,
            passed: true,
            device_id: "abc123",
            forced: false,
        })
        .unwrap();
        assert!(json.contains("audio_quality_calibration"));
        assert!(json.contains("wer_phase1"));
        assert!(json.contains("device_id"));
    }
}
