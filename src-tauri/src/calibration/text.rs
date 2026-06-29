//! Static calibration reference texts bundled with the app.

pub const SYSTEM_CLIP_TEXT: &str =
    "Tell me about your experience at SecureAuth building adaptive authentication and \
     ML-based risk engines. We use OAuth, OIDC, SAML, MFA, and IAM across multi-tenant \
     SaaS platforms. Describe your work with Kerberos, LDAP, and enterprise identity \
     federation for LLM and API security.";

pub const MIC_PARAGRAPH_TEXT: &str =
    "At SecureAuth, I led the design of an adaptive authentication system using ML-based \
     risk scoring. The platform supported OAuth 2.0 and OIDC federation across multi-tenant \
     SaaS customers. I integrated step-up MFA triggers with identity-aware policy \
     enforcement — including Kerberos and LDAP for enterprise directories. My most recent \
     work at IdMe24 focused on agentic AI identity: autonomous agents requiring \
     just-in-time credential provisioning with zero-standing privilege.";

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
