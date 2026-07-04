//! Phone-mode speaker diarization.
//!
//! Linux/macOS use local ONNX via `speakrs`. Windows uses a stub (Ctrl+Q only)
//! because `speakrs`/Intel MKL triggers a rustc archive ICE on CI.

mod types;

#[cfg(not(target_os = "windows"))]
mod speakrs_impl;
#[cfg(not(target_os = "windows"))]
use speakrs_impl as backend;

#[cfg(target_os = "windows")]
mod stub;
#[cfg(target_os = "windows")]
use stub as backend;

pub use backend::{
    download_models, models_downloaded, resolve_models_dir, speakrs_models_dir, DiarizerManager,
};
pub use types::{DiarizedSegment, DiarizerStatus, SpeakerRole, DIARIZER_BATCH_INTERVAL};

#[cfg(test)]
mod tests {
    use super::types::parse_speaker_id;

    #[test]
    fn parse_speaker_label_numeric_suffix() {
        assert_eq!(parse_speaker_id("SPEAKER_00"), 0);
        assert_eq!(parse_speaker_id("SPEAKER_01"), 1);
    }
}
