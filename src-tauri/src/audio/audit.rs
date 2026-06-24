//! Audio pipeline audit telemetry (M13 S6).
//!
//! Aggregates per-chunk counters during a live session and writes a JSON
//! summary line to `~/.flint/metrics.log` on session end. The structured
//! logging schema follows the contract in `.cursor/rules/flint-performance.mdc`.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;

use crate::audio::capture::AudioSource;

/// Reasons a chunk can be suppressed before reaching the orchestrator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressionReason {
    /// Cross-channel echo gate dropped a near-duplicate.
    EchoSystemBleedIntoMic,
    /// Cross-channel echo gate dropped what looks like loopback feedback of
    /// the user's own voice.
    EchoMicBleedIntoSystem,
    /// Engine post-processor dropped the segment as a known-hallucination
    /// string ("Thanks for watching", lone "you", etc.).
    KnownHallucination,
    /// Engine post-processor dropped the segment for an impossible word/sec
    /// rate.
    ImplausibleWordRate,
    /// Live sanitiser stripped the segment to nothing (profanity hallucination
    /// or full content removal).
    SanitizerEmpty,
}

impl SuppressionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SuppressionReason::EchoSystemBleedIntoMic => "echo_system_to_mic",
            SuppressionReason::EchoMicBleedIntoSystem => "echo_mic_to_system",
            SuppressionReason::KnownHallucination => "known_hallucination",
            SuppressionReason::ImplausibleWordRate => "implausible_word_rate",
            SuppressionReason::SanitizerEmpty => "sanitizer_empty",
        }
    }
}

/// Per-source counters for the session-end summary.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct PerSourceCounts {
    pub accepted: u64,
    pub suppressed: u64,
    pub avg_logprob_sum: f64,
    pub avg_logprob_samples: u64,
}

impl PerSourceCounts {
    pub fn mean_logprob(&self) -> Option<f64> {
        if self.avg_logprob_samples == 0 {
            None
        } else {
            Some(self.avg_logprob_sum / self.avg_logprob_samples as f64)
        }
    }
}

/// Suppression-reason counters across the session.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct SuppressionCounts {
    pub echo_system_to_mic: u64,
    pub echo_mic_to_system: u64,
    pub known_hallucination: u64,
    pub implausible_word_rate: u64,
    pub sanitizer_empty: u64,
}

impl SuppressionCounts {
    pub fn record(&mut self, reason: SuppressionReason) {
        match reason {
            SuppressionReason::EchoSystemBleedIntoMic => self.echo_system_to_mic += 1,
            SuppressionReason::EchoMicBleedIntoSystem => self.echo_mic_to_system += 1,
            SuppressionReason::KnownHallucination => self.known_hallucination += 1,
            SuppressionReason::ImplausibleWordRate => self.implausible_word_rate += 1,
            SuppressionReason::SanitizerEmpty => self.sanitizer_empty += 1,
        }
    }

    pub fn total(&self) -> u64 {
        self.echo_system_to_mic
            + self.echo_mic_to_system
            + self.known_hallucination
            + self.implausible_word_rate
            + self.sanitizer_empty
    }
}

/// Chunk-level suspicion counters from the speaker-suspicion detector.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct SuspicionCounts {
    pub question_shape_on_mic: u64,
    pub first_person_on_system: u64,
}

/// Thread-safe aggregator. Held inside `LiveTaskHandles` for the duration of
/// a live session and dumped to `metrics.log` when the session stops.
#[derive(Debug, Default)]
pub struct AudioAuditCounters {
    inner: Mutex<AudioAuditState>,
}

#[derive(Debug, Default)]
struct AudioAuditState {
    pub system: PerSourceCounts,
    pub mic: PerSourceCounts,
    pub suppressions: SuppressionCounts,
    pub suspicions: SuspicionCounts,
}

impl AudioAuditCounters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_chunk(&self, source: AudioSource, avg_logprob: Option<f32>) {
        let mut guard = self.inner.lock().expect("audit mutex poisoned");
        let bucket = match source {
            AudioSource::System => &mut guard.system,
            AudioSource::Microphone => &mut guard.mic,
        };
        bucket.accepted += 1;
        if let Some(lp) = avg_logprob {
            bucket.avg_logprob_sum += lp as f64;
            bucket.avg_logprob_samples += 1;
        }
    }

    pub fn record_suppression(&self, source: AudioSource, reason: SuppressionReason) {
        let mut guard = self.inner.lock().expect("audit mutex poisoned");
        let bucket = match source {
            AudioSource::System => &mut guard.system,
            AudioSource::Microphone => &mut guard.mic,
        };
        bucket.suppressed += 1;
        guard.suppressions.record(reason);
    }

    pub fn record_suspicion_question_on_mic(&self) {
        let mut guard = self.inner.lock().expect("audit mutex poisoned");
        guard.suspicions.question_shape_on_mic += 1;
    }

    pub fn record_suspicion_first_person_on_system(&self) {
        let mut guard = self.inner.lock().expect("audit mutex poisoned");
        guard.suspicions.first_person_on_system += 1;
    }

    /// Snapshot the counters as a serialisable summary. Resets nothing —
    /// the aggregator is meant to live for the whole session.
    pub fn snapshot(&self) -> AudioAuditSummary {
        let guard = self.inner.lock().expect("audit mutex poisoned");
        AudioAuditSummary {
            system: guard.system,
            mic: guard.mic,
            suppressions: guard.suppressions,
            suspicions: guard.suspicions,
            mean_logprob_system: guard.system.mean_logprob(),
            mean_logprob_mic: guard.mic.mean_logprob(),
            suppression_rate: compute_suppression_rate(
                &guard.system,
                &guard.mic,
                &guard.suppressions,
            ),
        }
    }
}

fn compute_suppression_rate(
    system: &PerSourceCounts,
    mic: &PerSourceCounts,
    suppressions: &SuppressionCounts,
) -> f64 {
    let total = system.accepted + system.suppressed + mic.accepted + mic.suppressed;
    if total == 0 {
        return 0.0;
    }
    suppressions.total() as f64 / total as f64
}

/// Serialisable end-of-session summary written to `~/.flint/metrics.log`.
#[derive(Debug, Clone, Serialize)]
pub struct AudioAuditSummary {
    pub system: PerSourceCounts,
    pub mic: PerSourceCounts,
    pub suppressions: SuppressionCounts,
    pub suspicions: SuspicionCounts,
    pub mean_logprob_system: Option<f64>,
    pub mean_logprob_mic: Option<f64>,
    pub suppression_rate: f64,
}

/// Append the session-end audit summary as a single JSON line to
/// `~/.flint/metrics.log`. Best-effort: failures are logged but never
/// surfaced to the user.
pub fn write_summary_to_metrics_log(session_id: uuid::Uuid, summary: &AudioAuditSummary) {
    let Some(path) = metrics_log_path() else {
        tracing::debug!("audit: no HOME — skipping metrics.log write");
        return;
    };
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            tracing::warn!(path = %dir.display(), error = %e, "audit: create metrics dir failed");
            return;
        }
    }

    let line = match serde_json::to_string(&MetricsLogLine {
        timestamp: chrono::Utc::now().to_rfc3339(),
        level: "INFO",
        service: "audio_pipeline",
        session_id: session_id.to_string(),
        event: "session_audit_summary",
        summary,
    }) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "audit: summary serialise failed");
            return;
        }
    };

    use std::fs::OpenOptions;
    use std::io::Write;
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{line}") {
                tracing::warn!(path = %path.display(), error = %e, "audit: metrics write failed");
            }
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "audit: metrics open failed");
        }
    }
}

#[derive(Serialize)]
struct MetricsLogLine<'a> {
    timestamp: String,
    level: &'static str,
    service: &'static str,
    session_id: String,
    event: &'static str,
    summary: &'a AudioAuditSummary,
}

fn metrics_log_path() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("FLINT_METRICS_LOG") {
        return Some(PathBuf::from(custom));
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".flint").join("metrics.log"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_record_chunks_per_source() {
        let counters = AudioAuditCounters::new();
        counters.record_chunk(AudioSource::System, Some(-0.3));
        counters.record_chunk(AudioSource::System, Some(-0.1));
        counters.record_chunk(AudioSource::Microphone, Some(-0.5));

        let snap = counters.snapshot();
        assert_eq!(snap.system.accepted, 2);
        assert_eq!(snap.mic.accepted, 1);
        assert!((snap.mean_logprob_system.unwrap() - -0.2).abs() < 1e-6);
        assert!((snap.mean_logprob_mic.unwrap() - -0.5).abs() < 1e-6);
    }

    #[test]
    fn counters_record_suppressions_per_reason() {
        let counters = AudioAuditCounters::new();
        counters.record_suppression(
            AudioSource::Microphone,
            SuppressionReason::EchoSystemBleedIntoMic,
        );
        counters.record_suppression(AudioSource::System, SuppressionReason::KnownHallucination);
        counters.record_suppression(AudioSource::System, SuppressionReason::SanitizerEmpty);

        let snap = counters.snapshot();
        assert_eq!(snap.suppressions.echo_system_to_mic, 1);
        assert_eq!(snap.suppressions.known_hallucination, 1);
        assert_eq!(snap.suppressions.sanitizer_empty, 1);
        assert_eq!(snap.suppressions.total(), 3);
        assert_eq!(snap.mic.suppressed, 1);
        assert_eq!(snap.system.suppressed, 2);
    }

    #[test]
    fn suppression_rate_zero_when_no_chunks() {
        let counters = AudioAuditCounters::new();
        assert_eq!(counters.snapshot().suppression_rate, 0.0);
    }

    #[test]
    fn suspicion_counters_increment() {
        let counters = AudioAuditCounters::new();
        counters.record_suspicion_question_on_mic();
        counters.record_suspicion_question_on_mic();
        counters.record_suspicion_first_person_on_system();
        let snap = counters.snapshot();
        assert_eq!(snap.suspicions.question_shape_on_mic, 2);
        assert_eq!(snap.suspicions.first_person_on_system, 1);
    }

    #[test]
    fn metrics_log_path_respects_env_override() {
        // Save and restore the env so the test is hermetic.
        let prev = std::env::var("FLINT_METRICS_LOG").ok();
        std::env::set_var("FLINT_METRICS_LOG", "/tmp/flint-test-metrics.log");
        let path = metrics_log_path().unwrap();
        assert_eq!(path, PathBuf::from("/tmp/flint-test-metrics.log"));
        match prev {
            Some(v) => std::env::set_var("FLINT_METRICS_LOG", v),
            None => std::env::remove_var("FLINT_METRICS_LOG"),
        }
    }
}
