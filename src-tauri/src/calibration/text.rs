//! Static calibration reference texts bundled with the app.

pub const SYSTEM_CLIP_TEXT: &str =
    "Tell me about a recent project where you worked with a small team to deliver \
     something on a tight schedule. Walk me through how you gathered requirements, \
     planned the work, communicated progress, and handled unexpected problems along \
     the way.";

pub const MIC_PARAGRAPH_TEXT: &str =
    "In my last role, I led a cross-functional team to ship a customer-facing feature \
     under a six-week deadline. We started by writing clear requirements and breaking \
     the work into weekly milestones. When we hit a blocker in testing, I coordinated \
     with QA and design to adjust scope without missing the launch date. The feature \
     went live on schedule and improved user retention.";

pub const SYSTEM_WER_PASS_THRESHOLD: f32 = 0.20;
pub const MIC_WER_PASS_THRESHOLD: f32 = 0.25;

/// Cap matches session Whisper initial_prompt budget (§26).
const CALIBRATION_PROMPT_CHAR_CAP: usize = 220;

pub fn calibration_whisper_prompt(reference: &str) -> String {
    reference
        .chars()
        .take(CALIBRATION_PROMPT_CHAR_CAP)
        .collect()
}

pub fn calibration_resources_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/calibration")
}

pub fn load_system_clip_text() -> String {
    let path = calibration_resources_dir().join("system_clip.txt");
    std::fs::read_to_string(path).unwrap_or_else(|_| SYSTEM_CLIP_TEXT.to_string())
}

pub fn load_mic_paragraph_text() -> String {
    let path = calibration_resources_dir().join("mic_paragraph.txt");
    std::fs::read_to_string(path).unwrap_or_else(|_| MIC_PARAGRAPH_TEXT.to_string())
}

#[cfg(test)]
mod tests {
    use super::{calibration_whisper_prompt, load_mic_paragraph_text, load_system_clip_text};

    #[test]
    fn bundled_texts_are_non_empty() {
        assert!(!load_system_clip_text().is_empty());
        assert!(!load_mic_paragraph_text().is_empty());
    }

    #[test]
    fn calibration_prompt_is_capped() {
        let reference = load_mic_paragraph_text();
        let prompt = calibration_whisper_prompt(&reference);
        assert!(!prompt.is_empty());
        assert!(prompt.len() <= 220);
    }
}
