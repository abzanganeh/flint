//! Windows stub — `speakrs`/Intel MKL triggers a rustc archive ICE on CI.
//! Phone mode on Windows uses Ctrl+Q for question boundaries.

use std::path::PathBuf;

use super::types::{DiarizedSegment, DiarizerStatus, SpeakerRole};

pub struct DiarizerManager {
    status: DiarizerStatus,
}

impl Default for DiarizerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DiarizerManager {
    pub fn new() -> Self {
        Self {
            status: DiarizerStatus::ModelsMissing,
        }
    }

    pub fn status(&self) -> &DiarizerStatus {
        &self.status
    }

    pub fn models_ready(&self) -> bool {
        false
    }

    pub fn ingest_pcm(&mut self, _samples: &[f32], _sample_rate: u32) {}

    pub fn note_transcript(&mut self, _speaker_id: u8, _text: &str) {}

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
        matches!(self.status, DiarizerStatus::Assigned { .. })
            && matches!(
                self.role_at_offset_ms(offset_ms),
                Some(SpeakerRole::Interviewer)
            )
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
            DiarizerStatus::ModelsMissing => {
                Err("Speaker diarization is not available on Windows in v1. Use Ctrl+Q.".into())
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

pub fn speakrs_models_dir() -> PathBuf {
    super::types::flint_models_base()
}

pub fn models_downloaded() -> bool {
    false
}

pub fn resolve_models_dir() -> Option<PathBuf> {
    None
}

pub fn download_models() -> Result<PathBuf, String> {
    Err("Speaker diarization is not available on Windows in v1. Use Ctrl+Q.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::diarizer::types::DiarizedSegment;

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
