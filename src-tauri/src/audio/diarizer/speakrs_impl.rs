//! Unix diarization backend — local ONNX via `speakrs`.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use speakrs::{ExecutionMode, ModelBundle, ModelManager, OwnedDiarizationPipeline};
use tracing::{info, warn};

use super::types::{
    parse_speaker_id, DiarizedSegment, DiarizerStatus, SpeakerRole, DIARIZER_BATCH_INTERVAL,
};

const MODEL_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(900);

pub struct DiarizerManager {
    models_dir: Option<PathBuf>,
    pipeline: Option<OwnedDiarizationPipeline>,
    load_failed: bool,
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
        let models_dir = resolve_models_dir();
        let status = if models_dir.is_some() {
            DiarizerStatus::Unavailable
        } else {
            DiarizerStatus::ModelsMissing
        };
        Self {
            models_dir,
            pipeline: None,
            load_failed: false,
            status,
            pcm_buffer: Vec::new(),
            window_secs: 3.0,
            last_batch_at: Instant::now() - DIARIZER_BATCH_INTERVAL,
            session_offset_ms: 0,
            sample_text_by_speaker: std::collections::HashMap::new(),
        }
    }

    /// Load the ONNX pipeline off the hot path (Settings download / session pre-warm).
    pub fn warm_pipeline(&mut self) {
        let _ = self.ensure_pipeline_loaded();
    }

    fn ensure_pipeline_loaded(&mut self) -> bool {
        if self.pipeline.is_some() {
            return true;
        }
        if self.load_failed {
            return false;
        }
        let Some(dir) = self.models_dir.clone() else {
            return false;
        };
        match OwnedDiarizationPipeline::from_dir(&dir, ExecutionMode::Cpu) {
            Ok(p) => {
                info!("speakrs diarization pipeline loaded from {}", dir.display());
                self.pipeline = Some(p);
                true
            }
            Err(e) => {
                warn!(error = %e, "speakrs pipeline init failed");
                self.load_failed = true;
                self.status = DiarizerStatus::Failed;
                self.pipeline = None;
                false
            }
        }
    }

    pub fn status(&self) -> &DiarizerStatus {
        &self.status
    }

    pub fn models_ready(&self) -> bool {
        self.pipeline.is_some()
    }

    pub fn ingest_pcm(&mut self, samples: &[f32], sample_rate: u32) {
        if sample_rate != 16_000 {
            return;
        }
        if !self.ensure_pipeline_loaded() {
            return;
        }
        let Some(pipeline) = self.pipeline.as_mut() else {
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

        let window_start_ms = self
            .session_offset_ms
            .saturating_sub(((window.len() as u64) * 1000) / sample_rate as u64);

        match pipeline.run(&window) {
            Ok(result) => self.apply_diarization_result(&result.segments, window_start_ms),
            Err(e) => {
                warn!(error = %e, "speakrs diarization batch failed");
                self.status = DiarizerStatus::Failed;
                self.load_failed = true;
                self.pipeline = None;
            }
        }
    }

    pub fn note_transcript(&mut self, speaker_id: u8, text: &str) {
        let entry = self.sample_text_by_speaker.entry(speaker_id).or_default();
        if !entry.is_empty() {
            entry.push(' ');
        }
        entry.push_str(text.trim());
        if entry.len() > 240 {
            *entry = entry.chars().take(240).collect();
        }
    }

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
                matches!(
                    self.role_at_offset_ms(offset_ms),
                    Some(SpeakerRole::Interviewer)
                )
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
            DiarizerStatus::ModelsMissing => {
                Err("Speaker models not installed. Use Ctrl+Q to mark question boundaries.".into())
            }
            DiarizerStatus::Failed => Err("Speaker separation unavailable. Use Ctrl+Q.".into()),
            DiarizerStatus::Assigned { .. } => Ok(()),
            DiarizerStatus::Unavailable => Err("Diarization not active for this session.".into()),
        }
    }

    fn apply_diarization_result(&mut self, segments: &[speakrs::Segment], window_start_ms: u64) {
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

        self.status = DiarizerStatus::AwaitingAssignment { segments: diarized };
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
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(download_models_inner());
    });
    match rx.recv_timeout(MODEL_DOWNLOAD_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(
            "Model download timed out after 15 minutes. Check your network and retry.".into(),
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("Model download stopped unexpectedly. Retry from Settings.".into())
        }
    }
}

fn download_models_inner() -> Result<PathBuf, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::diarizer::types::parse_speaker_id;

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
        if matches!(mgr.status(), DiarizerStatus::ModelsMissing) {
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
        assert_eq!(mgr.role_at_offset_ms(500), Some(SpeakerRole::Interviewer));
        assert_eq!(mgr.role_at_offset_ms(3000), Some(SpeakerRole::User));
    }

    #[test]
    fn parse_speaker_label_numeric_suffix() {
        assert_eq!(parse_speaker_id("SPEAKER_00"), 0);
        assert_eq!(parse_speaker_id("SPEAKER_01"), 1);
    }

    #[test]
    fn new_does_not_eagerly_load_pipeline_when_models_present() {
        if !models_downloaded() {
            return;
        }
        let mgr = DiarizerManager::new();
        assert!(!mgr.models_ready());
    }
}
