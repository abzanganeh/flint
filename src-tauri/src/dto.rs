use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::digest::Digest;
use crate::health::checks::{CheckStatus, HealthCheck, HealthCheckResult};
use crate::health::hardware::{HardwareProfile, LLMConfig};
use crate::interfaces::auth::{Plan, User};

/// Serializable user for the frontend (no secrets).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserDto {
    pub id: String,
    pub email: String,
    pub plan: String,
}

impl From<User> for UserDto {
    fn from(user: User) -> Self {
        Self {
            id: user.id.to_string(),
            email: user.email,
            plan: match user.plan {
                Plan::Free => "free".to_string(),
                Plan::Premium => "premium".to_string(),
            },
        }
    }
}

/// Serializable health check row for the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCheckResultDto {
    pub check: HealthCheck,
    pub status: CheckStatus,
    pub message: String,
    pub fix_instruction: Option<String>,
}

impl From<HealthCheckResult> for HealthCheckResultDto {
    fn from(result: HealthCheckResult) -> Self {
        Self {
            check: result.check,
            status: result.status,
            message: result.message,
            fix_instruction: result.fix_instruction,
        }
    }
}

/// Serializable LLM routing recommendation for the health-check screen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfigDto {
    pub directional: String,
    pub depth: String,
    pub fallback: Option<String>,
    pub cloud_recommended: bool,
}

impl From<LLMConfig> for LlmConfigDto {
    fn from(config: LLMConfig) -> Self {
        Self {
            directional: config.directional,
            depth: config.depth,
            fallback: config.fallback,
            cloud_recommended: config.cloud_recommended,
        }
    }
}

/// Serializable hardware profile for the health-check screen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareProfileDto {
    pub tier: u8,
    pub cpu_cores: usize,
    pub ram_gb: f64,
    pub has_gpu: bool,
    pub gpu_vram_gb: Option<f64>,
    pub os: String,
    pub recommended_whisper_model: String,
    pub recommended_llm_config: LlmConfigDto,
}

impl From<HardwareProfile> for HardwareProfileDto {
    fn from(profile: HardwareProfile) -> Self {
        Self {
            tier: profile.tier,
            cpu_cores: profile.cpu_cores,
            ram_gb: profile.ram_gb,
            has_gpu: profile.has_gpu,
            gpu_vram_gb: profile.gpu_vram_gb,
            os: profile.os,
            recommended_whisper_model: profile.recommended_whisper_model.as_str().to_string(),
            recommended_llm_config: profile.recommended_llm_config.into(),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Session design DTOs (Phase 2)
// ──────────────────────────────────────────────────────────────────────────────

/// Parameters for creating a new session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionConfigDto {
    pub name: String,
    /// e.g. "interview" | "meeting" | "presentation"
    pub session_type: String,
    /// e.g. "software engineering" | "product management"
    pub domain: String,
}

/// Company intelligence extracted from a job description by Smart Resume.
///
/// Optional — only present when Smart Resume successfully extracted signals
/// (mission, values, culture) from the JD text before minting the handoff
/// token.  Flint appends this as structured text to the session context so
/// the digest LLM can surface employer values in `{interviewer_priorities}`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CompanyIntelBlock {
    pub mission: String,
    pub values: Vec<String>,
    pub culture_notes: String,
}

impl CompanyIntelBlock {
    /// True when none of the signal fields contain useful content.
    pub fn is_empty(&self) -> bool {
        self.mission.is_empty() && self.values.is_empty() && self.culture_notes.is_empty()
    }

    /// Format as a compact block for appending to the session context text.
    ///
    /// The block is structured so the digest LLM naturally picks up the
    /// employer's values as candidate `key_skills` / `{interviewer_priorities}`.
    ///
    /// NOTE: The equivalent formatting is duplicated in `buildContextText` in
    /// `src/App.tsx`. Both must stay in sync if the block format changes.
    /// Used directly by unit tests and available for any future Rust-side
    /// context assembly path.
    pub fn render_for_context(&self) -> String {
        let mut lines: Vec<String> = vec![
            "--- COMPANY CONTEXT (from Smart Resume) ---".to_string(),
        ];
        if !self.mission.is_empty() {
            lines.push(format!("Company Mission: {}", self.mission));
        }
        if !self.values.is_empty() {
            lines.push(format!("Core Values: {}", self.values.join(", ")));
        }
        if !self.culture_notes.is_empty() {
            lines.push(format!("Culture: {}", self.culture_notes));
        }
        lines.push("---".to_string());
        lines.join("\n")
    }
}

/// Payload redeemed from a Smart Resume handoff token (Strategy B Phase 1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartResumeImportDto {
    pub session_name: String,
    pub session_type: String,
    pub domain: String,
    pub jd_text: String,
    pub resume_summary: String,
    pub smart_resume_session_id: String,
    pub export_version: u32,
    /// Company intelligence from Smart Resume — None when unavailable or empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub company_intel: Option<CompanyIntelBlock>,
}

/// Serialisable view of a [`Digest`] for React. All fields are editable on the
/// DigestReview screen before the user confirms and triggers pre-warming.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigestDto {
    pub role: String,
    pub company: String,
    pub domain: String,
    pub key_skills: Vec<String>,
    pub seniority: String,
    pub likely_questions: Vec<String>,
    pub topics_to_avoid: Vec<String>,
}

impl From<Digest> for DigestDto {
    fn from(d: Digest) -> Self {
        Self {
            role: d.role,
            company: d.company,
            domain: d.domain,
            key_skills: d.key_skills,
            seniority: d.seniority,
            likely_questions: d.likely_questions,
            topics_to_avoid: d.topics_to_avoid,
        }
    }
}

impl From<DigestDto> for Digest {
    fn from(d: DigestDto) -> Self {
        Self {
            role: d.role,
            company: d.company,
            domain: d.domain,
            key_skills: d.key_skills,
            seniority: d.seniority,
            likely_questions: d.likely_questions,
            topics_to_avoid: d.topics_to_avoid,
        }
    }
}

/// Full current state snapshot returned by `get_session_snapshot`. React uses
/// this to resync after missed events (e.g. after window focus regained).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshotDto {
    pub session_id: Option<Uuid>,
    /// Canonical SCREAMING_SNAKE_CASE state name (matches `session_state_change` event).
    pub state: String,
    pub digest: Option<DigestDto>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Session list DTOs (Phase 6)
// ──────────────────────────────────────────────────────────────────────────────

/// One row in the session list screen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummaryDto {
    pub id: String,
    pub state: String,
    pub created_at: i64,
    /// Seconds until this session's data expires (negative = already expired).
    pub expires_in_secs: i64,
    /// If `true`, this session is pinned and won't be auto-deleted at expiry.
    pub promoted: bool,
    /// User-specified session name (may be empty for legacy rows).
    pub name: String,
    /// e.g. "interview" | "meeting"
    pub session_type: String,
    /// e.g. "software engineering" | "product management"
    pub domain: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn company_intel_block_is_empty_when_all_fields_blank() {
        let block = CompanyIntelBlock::default();
        assert!(block.is_empty());
    }

    #[test]
    fn company_intel_block_not_empty_when_mission_set() {
        let block = CompanyIntelBlock {
            mission: "Build great products".to_string(),
            ..Default::default()
        };
        assert!(!block.is_empty());
    }

    #[test]
    fn render_for_context_includes_all_fields() {
        let block = CompanyIntelBlock {
            mission: "Empower teams".to_string(),
            values: vec!["Bias for Action".to_string(), "Customer Obsession".to_string()],
            culture_notes: "Fast-paced".to_string(),
        };
        let rendered = block.render_for_context();
        assert!(rendered.contains("Company Mission: Empower teams"));
        assert!(rendered.contains("Core Values: Bias for Action, Customer Obsession"));
        assert!(rendered.contains("Culture: Fast-paced"));
    }

    #[test]
    fn render_for_context_skips_empty_fields() {
        let block = CompanyIntelBlock {
            mission: String::new(),
            values: vec!["Ownership".to_string()],
            culture_notes: String::new(),
        };
        let rendered = block.render_for_context();
        assert!(!rendered.contains("Company Mission:"));
        assert!(rendered.contains("Core Values: Ownership"));
        assert!(!rendered.contains("Culture:"));
    }
}
