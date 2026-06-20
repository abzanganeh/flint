//! Phone-mode speaker diarization scaffold (M10 slice 8).
//!
//! Full `speakrs` ONNX integration requires on-device model download (~200MB).
//! This module defines the assignment contract and a stub pipeline so phone
//! mode can fall back to Ctrl+Q until models are present.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Label assigned by diarization or the user picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeakerRole {
    Interviewer,
    User,
    Unknown,
}

/// One diarized speech segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiarizedSegment {
    pub speaker_id: u8,
    pub start_ms: u64,
    pub end_ms: u64,
    pub sample_text: String,
}

/// Runtime diarization state for phone-call sessions.
#[derive(Debug, Clone, Default)]
pub enum DiarizerStatus {
    #[default]
    Unavailable,
    /// Models not downloaded — user should use Ctrl+Q.
    ModelsMissing,
    /// Diarization running but speakers not yet assigned.
    AwaitingAssignment { segments: Vec<DiarizedSegment> },
    /// User picked which speaker is the interviewer.
    Assigned { interviewer_id: u8, user_id: u8 },
    /// Could not separate voices — Ctrl+Q only.
    Failed,
}

impl DiarizerStatus {
    pub fn role_for_speaker(&self, speaker_id: u8) -> SpeakerRole {
        match self {
            Self::Assigned {
                interviewer_id,
                user_id,
            } if speaker_id == *interviewer_id => SpeakerRole::Interviewer,
            Self::Assigned {
                interviewer_id: _,
                user_id,
            } if speaker_id == *user_id => SpeakerRole::User,
            _ => SpeakerRole::Unknown,
        }
    }

    pub fn needs_speaker_picker(&self) -> bool {
        matches!(self, Self::AwaitingAssignment { .. })
    }
}

/// Rolling-window diarization manager (speakrs integration point).
#[derive(Debug, Default)]
pub struct DiarizerManager {
    status: DiarizerStatus,
    window_secs: f64,
}

impl DiarizerManager {
    pub fn new() -> Self {
        Self {
            status: DiarizerStatus::ModelsMissing,
            window_secs: 3.0,
        }
    }

    pub fn status(&self) -> &DiarizerStatus {
        &self.status
    }

    pub fn ingest_pcm(&mut self, samples: &[f32], sample_rate: u32) {
        let _ = (samples.len(), sample_rate, self.window_secs);
        // speakrs batch pipeline.run() on rolling windows — wired when models ship.
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
                };
                Ok(())
            }
            DiarizerStatus::ModelsMissing => {
                Err("Speaker models not installed. Use Ctrl+Q to mark question boundaries.".into())
            }
            DiarizerStatus::Failed => Err("Speaker separation unavailable. Use Ctrl+Q.".into()),
            DiarizerStatus::Assigned { .. } => Ok(()),
            DiarizerStatus::Unavailable => Err("Diarization not active for this session.".into()),
        }
    }

    #[cfg(test)]
    pub fn set_awaiting_assignment(&mut self, segments: Vec<DiarizedSegment>) {
        self.status = DiarizerStatus::AwaitingAssignment { segments };
    }
}

pub const DIARIZER_BATCH_INTERVAL: Duration = Duration::from_secs(2);

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
        assert_eq!(mgr.status().role_for_speaker(0), SpeakerRole::Interviewer);
        assert_eq!(mgr.status().role_for_speaker(1), SpeakerRole::User);
    }

    #[test]
    fn models_missing_returns_ctrl_q_hint() {
        let mut mgr = DiarizerManager::new();
        assert!(mgr.assign_interviewer(0).is_err());
    }
}
