//! Phone-mode speaker diarization via local ONNX (`speakrs`).
//!
//! All inference stays on-device — no audio leaves the machine for diarization.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use speakrs::{ExecutionMode, ModelBundle, ModelManager, OwnedDiarizationPipeline};
use tracing::{info, warn};

/// Label assigned by diarization or the user picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeakerRole {
    Interviewer,
    User,
    Unknown,
}

/// One diarized speech segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiarizedSegment {
    pub speaker_id: u8,
    pub start_ms: u64,
    pub end_ms: u64,
    pub sample_text: String,
}

/// Runtime diarization state for phone-call sessions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DiarizerStatus {
    Unavailable,
    /// Models not downloaded — user should use Ctrl+Q.
    #[default]
    ModelsMissing,
    /// Diarization running but speakers not yet assigned.
    AwaitingAssignment { segments: Vec<DiarizedSegment> },
    /// User picked which speaker is the interviewer.
    Assigned {
        interviewer_id: u8,
        user_id: u8,
        segments: Vec<DiarizedSegment>,
    },
    /// Could not separate voices — Ctrl+Q only.
    Failed,
}

impl DiarizerStatus {
    pub fn role_for_speaker(&self, speaker_id: u8) -> SpeakerRole {
        match self {
            Self::Assigned {
                interviewer_id,
                user_id,
                ..
            } if speaker_id == *interviewer_id => SpeakerRole::Interviewer,
            Self::Assigned {
                interviewer_id: _,
                user_id,
                ..
            } if speaker_id == *user_id => SpeakerRole::User,
            _ => SpeakerRole::Unknown,
        }
    }

    pub fn needs_speaker_picker(&self) -> bool {
        matches!(self, Self::AwaitingAssignment { .. })
    }

    pub fn segments_for_ui(&self) -> Vec<DiarizedSegment> {
        match self {
            Self::AwaitingAssignment { segments } => segments.clone(),
            Self::Assigned { segments, .. } => segments.clone(),
            _ => Vec::new(),
        }
    }
}

enum DiarizerPipeline {
    Missing,
    Ready(Box<OwnedDiarizationPipeline>),
    Failed,
}

/// Rolling-window diarization manager (`speakrs` ONNX, on-device only).
pub struct DiarizerManager {
    pipeline: DiarizerPipeline,
    status: DiarizerStatus,
    pcm_buffer: Vec<f32>,
    window_secs: f64,
    last_batch_at: Instant,
    session_offset_ms: u64,
    sample_text_by_speaker: std::collections::HashMap<u8, String>,
}

impl Default for DiarizerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DiarizerManager {
    pub fn new() -> Self {
        let pipeline = match resolve_models_dir() {
            Some(dir) => match OwnedDiarizationPipeline::from_dir(&dir, ExecutionMode::Cpu) {
                Ok(p) => {
                    info!("speakrs diarization pipeline loaded from {}", dir.display());
                    DiarizerPipeline::Ready(Box::new(p))
                }
                Err(e) => {
                    warn!(error = %e, "speakrs pipeline init failed");
                    DiarizerPipeline::Failed
                }
            },
            None => DiarizerPipeline::Missing,
        };
        let status = match &pipeline {
            DiarizerPipeline::Missing => DiarizerStatus::ModelsMissing,
            DiarizerPipeline::Failed => DiarizerStatus::Failed,
            DiarizerPipeline::Ready(_) => DiarizerStatus::Unavailable,
        };
        Self {
            pipeline,
            status,
            pcm_buffer: Vec::new(),
            window_secs: 3.0,
            last_batch_at: Instant::now() - DIARIZER_BATCH_INTERVAL,
            session_offset_ms: 0,
            sample_text_by_speaker: std::collections::HashMap::new(),
        }
    }

    pub fn status(&self) -> &DiarizerStatus {
        &self.status
    }

    pub fn models_ready(&self) -> bool {
        matches!(self.pipeline, DiarizerPipeline::Ready(_))
    }

    /// Append 16 kHz mono PCM and run a rolling diarization batch when due.
    pub fn ingest_pcm(&mut self, samples: &[f32], sample_rate: u32) {
        if sample_rate != 16_000 {
            return;
        }
        let DiarizerPipeline::Ready(pipeline) = &mut self.pipeline else {
            return;
        };
        if matches!(
            self.status,
            DiarizerStatus::Failed | DiarizerStatus::ModelsMissing
        ) {
            return;
        }

        self.pcm_buffer.extend_from_slice(samples);
        self.session_offset_ms += ((samples.len() as u64) * 1000) / sample_rate as u64;

        if self.last_batch_at.elapsed() < DIARIZER_BATCH_INTERVAL {
            return;
        }

        let window_samples = (self.window_secs * sample_rate as f64) as usize;
        let min_samples = sample_rate as usize * 2;
        if self.pcm_buffer.len() < min_samples {
            return;
        }

        let window = if self.pcm_buffer.len() > window_samples {
            self.pcm_buffer[self.pcm_buffer.len() - window_samples..].to_vec()
        } else {
            self.pcm_buffer.clone()
        };

        self.last_batch_at = Instant::now();

        let window_start_ms = self.session_offset_ms.saturating_sub(
            ((window.len() as u64) * 1000) / sample_rate as u64,
        );

        match pipeline.run(&window) {
            Ok(result) => self.apply_diarization_result(&result.segments, window_start_ms),
            Err(e) => {
                warn!(error = %e, "speakrs diarization batch failed");
                self.status = DiarizerStatus::Failed;
                self.pipeline = DiarizerPipeline::Failed;
            }
        }
    }

    pub fn note_transcript(&mut self, speaker_id: u8, text: &str) {
        let entry = self
            .sample_text_by_speaker
            .entry(speaker_id)
            .or_default();
        if !entry.is_empty() {
            entry.push(' ');
        }
        entry.push_str(text.trim());
        if entry.len() > 240 {
            *entry = entry.chars().take(240).collect();
        }
    }

    /// When speakers are assigned, map transcript offset to interviewer/user role.
    pub fn role_at_offset_ms(&self, offset_ms: u64) -> Option<SpeakerRole> {
        let DiarizerStatus::Assigned { segments, .. } = &self.status else {
            return None;
        };
        let t = offset_ms as f64 / 1000.0;
        for seg in segments {
            let start = seg.start_ms as f64 / 1000.0;
            let end = seg.end_ms as f64 / 1000.0;
            if t >= start && t <= end {
                return Some(self.status.role_for_speaker(seg.speaker_id));
            }
        }
        None
    }

    pub fn allows_auto_question_detection_at(&self, offset_ms: u64) -> bool {
        match &self.status {
            DiarizerStatus::Assigned { .. } => {
                matches!(self.role_at_offset_ms(offset_ms), Some(SpeakerRole::Interviewer))
            }
            _ => false,
        }
    }

    pub fn assign_interviewer(&mut self, speaker_id: u8) -> Result<(), String> {
        match &self.status {
            DiarizerStatus::AwaitingAssignment { segments } => {
                let other = segments
                    .iter()
                    .map(|s| s.speaker_id)
                    .find(|id| *id != speaker_id)
                    .unwrap_or(speaker_id ^ 1);
                self.status = DiarizerStatus::Assigned {
                    interviewer_id: speaker_id,
                    user_id: other,
                    segments: segments.clone(),
                };
                Ok(())
            }
            DiarizerStatus::ModelsMissing => Err(
                "Speaker models not installed. Use Ctrl+Q to mark question boundaries.".into(),
            ),
            DiarizerStatus::Failed => Err("Speaker separation unavailable. Use Ctrl+Q.".into()),
            DiarizerStatus::Assigned { .. } => Ok(()),
            DiarizerStatus::Unavailable => {
                Err("Diarization not active for this session.".into())
            }
        }
    }

    fn apply_diarization_result(
        &mut self,
        segments: &[speakrs::Segment],
        window_start_ms: u64,
    ) {
        if segments.is_empty() {
            return;
        }

        let mut diarized = Vec::new();
        let mut speaker_ids = std::collections::BTreeSet::new();

        for seg in segments {
            let speaker_id = parse_speaker_id(&seg.speaker);
            speaker_ids.insert(speaker_id);
            diarized.push(DiarizedSegment {
                speaker_id,
                start_ms: window_start_ms + (seg.start * 1000.0).round() as u64,
                end_ms: window_start_ms + (seg.end * 1000.0).round() as u64,
                sample_text: self
                    .sample_text_by_speaker
                    .get(&speaker_id)
                    .cloned()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| format!("Speaker {}", speaker_id + 1)),
            });
        }

        if speaker_ids.len() < 2 {
            return;
        }

        if matches!(self.status, DiarizerStatus::Assigned { .. }) {
            if let DiarizerStatus::Assigned {
                interviewer_id,
                user_id,
                ..
            } = &self.status
            {
                self.status = DiarizerStatus::Assigned {
                    interviewer_id: *interviewer_id,
                    user_id: *user_id,
                    segments: diarized,
                };
            }
            return;
        }

        self.status = DiarizerStatus::AwaitingAssignment {
            segments: diarized,
        };
    }

    #[cfg(test)]
    pub fn set_awaiting_assignment(&mut self, segments: Vec<DiarizedSegment>) {
        self.status = DiarizerStatus::AwaitingAssignment { segments };
    }
}

pub const DIARIZER_BATCH_INTERVAL: Duration = Duration::from_secs(2);

pub fn speakrs_models_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".flint")
        .join("models")
        .join("speakrs")
}

pub fn models_downloaded() -> bool {
    resolve_models_dir().is_some()
}

pub fn resolve_models_dir() -> Option<PathBuf> {
    let marker = speakrs_models_dir().join("model_dir.txt");
    let raw = std::fs::read_to_string(marker).ok()?;
    let dir = PathBuf::from(raw.trim());
    let bundle = ModelBundle::from_dir(&dir);
    if bundle.segmentation_path().is_file() && bundle.embedding_path().is_file() {
        Some(dir)
    } else {
        None
    }
}

pub fn download_models() -> Result<PathBuf, String> {
    let base = speakrs_models_dir();
    std::fs::create_dir_all(&base).map_err(|e| format!("create models dir: {e}"))?;
    let cache = base.join("hf");
    std::fs::create_dir_all(&cache).map_err(|e| format!("create hf cache dir: {e}"))?;
    let manager =
        ModelManager::with_cache_dir(cache).map_err(|e| format!("model manager init: {e}"))?;
    let snapshot = manager
        .ensure(ExecutionMode::Cpu)
        .map_err(|e| format!("model download failed: {e}"))?;
    write_model_dir_marker(&base, &snapshot)?;
    info!(path = %snapshot.display(), "speakrs models ready");
    Ok(snapshot)
}

fn write_model_dir_marker(base: &Path, snapshot: &Path) -> Result<(), String> {
    std::fs::write(
        base.join("model_dir.txt"),
        snapshot.to_string_lossy().as_bytes(),
    )
    .map_err(|e| format!("write model_dir marker: {e}"))
}

fn parse_speaker_id(label: &str) -> u8 {
    label
        .rsplit('_')
        .next()
        .and_then(|n| n.parse::<u16>().ok())
        .map(|n| n.min(u8::MAX as u16) as u8)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assign_interviewer_from_awaiting_segments() {
        let mut mgr = DiarizerManager::new();
        mgr.set_awaiting_assignment(vec![
            DiarizedSegment {
                speaker_id: 0,
                start_ms: 0,
                end_ms: 2300,
                sample_text: "Tell me about yourself".into(),
            },
            DiarizedSegment {
                speaker_id: 1,
                start_ms: 2500,
                end_ms: 4800,
                sample_text: "Sure, I led the platform team".into(),
            },
        ]);
        mgr.assign_interviewer(0).unwrap();
        assert_eq!(
            mgr.status().role_for_speaker(0),
            SpeakerRole::Interviewer
        );
        assert_eq!(mgr.status().role_for_speaker(1), SpeakerRole::User);
    }

    #[test]
    fn models_missing_returns_ctrl_q_hint() {
        let mut mgr = DiarizerManager::new();
        if matches!(mgr.pipeline, DiarizerPipeline::Missing) {
            assert!(mgr.assign_interviewer(0).is_err());
        }
    }

    #[test]
    fn role_at_offset_resolves_assigned_segments() {
        let mut mgr = DiarizerManager::new();
        mgr.set_awaiting_assignment(vec![
            DiarizedSegment {
                speaker_id: 0,
                start_ms: 0,
                end_ms: 2000,
                sample_text: "Question".into(),
            },
            DiarizedSegment {
                speaker_id: 1,
                start_ms: 2100,
                end_ms: 4000,
                sample_text: "Answer".into(),
            },
        ]);
        mgr.assign_interviewer(0).unwrap();
        assert_eq!(
            mgr.role_at_offset_ms(500),
            Some(SpeakerRole::Interviewer)
        );
        assert_eq!(mgr.role_at_offset_ms(3000), Some(SpeakerRole::User));
    }

    #[test]
    fn parse_speaker_label_numeric_suffix() {
        assert_eq!(parse_speaker_id("SPEAKER_00"), 0);
        assert_eq!(parse_speaker_id("SPEAKER_01"), 1);
    }
}
