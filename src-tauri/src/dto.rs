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
