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
    AwaitingAssignment {
        segments: Vec<DiarizedSegment>,
    },
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

pub const DIARIZER_BATCH_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) fn parse_speaker_id(label: &str) -> u8 {
    label
        .rsplit('_')
        .next()
        .and_then(|n| n.parse::<u16>().ok())
        .map(|n| n.min(u8::MAX as u16) as u8)
        .unwrap_or(0)
}

pub fn flint_models_base() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".flint")
        .join("models")
        .join("speakrs")
}
