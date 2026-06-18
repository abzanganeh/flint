//! Smart Resume cross-product handoff client (Strategy B Phase 1).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::dto::{CompanyIntelBlock, SmartResumeImportDto};

const REDEEM_PATH: &str = "/api/flint/context";
const QUESTIONS_PATH: &str = "/api/interview-questions";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_BANK_FETCH_LIMIT: u32 = 30;
const CONTEXT_EMBED_LIMIT: usize = 15;

#[derive(Debug, Serialize)]
struct RedeemRequest {
    token: String,
}

#[derive(Debug, Deserialize, Default)]
struct CompanyIntelResponse {
    #[serde(default)]
    mission: String,
    #[serde(default)]
    values: Vec<String>,
    #[serde(default)]
    culture_notes: String,
}

/// Curated interview question from the Smart Resume global bank API.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct InterviewQuestionDto {
    pub id: String,
    pub text: String,
    pub domain: String,
    pub category: String,
    #[serde(default)]
    pub canonical_answer: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuestionsResponse {
    questions: Vec<InterviewQuestionDto>,
    #[serde(default)]
    #[allow(dead_code)]
    total: usize,
}

#[derive(Debug, Deserialize)]
struct RedeemResponse {
    session_name: String,
    session_type: String,
    domain: String,
    jd_text: String,
    resume_summary: String,
    smart_resume_session_id: String,
    export_version: u32,
    /// Present when Smart Resume extracted company signals from the JD.
    #[serde(default)]
    company_intel: Option<CompanyIntelResponse>,
}

fn base_url() -> Result<String, String> {
    if let Ok(raw) = std::env::var("FLINT_SMART_RESUME_URL") {
        let trimmed = raw.trim().trim_end_matches('/').to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    #[cfg(debug_assertions)]
    {
        Ok("http://localhost:8000".to_string())
    }

    #[cfg(not(debug_assertions))]
    Err("Smart Resume is not configured. Set FLINT_SMART_RESUME_URL.".to_string())
}

fn validate_token(token: &str) -> Result<(), String> {
    let trimmed = token.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return Err("Invalid import link.".to_string());
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("Invalid import link.".to_string());
    }
    Ok(())
}

fn map_status_error(status: reqwest::StatusCode, body: &str) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(detail) = json.get("detail").and_then(|v| v.as_str()) {
            return match status.as_u16() {
                404 => {
                    "This link has expired. Return to Smart Resume and click Open in Flint again."
                        .to_string()
                }
                429 => "Too many requests. Please wait a moment and try again.".to_string(),
                _ => detail.to_string(),
            };
        }
    }
    match status.as_u16() {
        404 => "This link has expired. Return to Smart Resume and click Open in Flint again."
            .to_string(),
        429 => "Too many requests. Please wait a moment and try again.".to_string(),
        _ => "Could not import from Smart Resume. Please try again.".to_string(),
    }
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|_| "Could not reach Smart Resume.".to_string())
}

/// Fetch curated interview questions for session enrichment (Phase 10).
///
/// Non-fatal at call sites — returns an empty vec when Smart Resume is
/// unreachable or unconfigured in release builds.
pub async fn fetch_interview_questions(
    domain: &str,
    company: &str,
    role: &str,
    limit: u32,
) -> Result<Vec<InterviewQuestionDto>, String> {
    let clamped_limit = limit.clamp(1, 100);
    let mut url = reqwest::Url::parse(&format!("{}{}", base_url()?, QUESTIONS_PATH))
        .map_err(|_| "Invalid Smart Resume URL.".to_string())?;
    {
        let mut pairs = url.query_pairs_mut();
        if !domain.trim().is_empty() {
            pairs.append_pair("domain", domain.trim());
        }
        if !company.trim().is_empty() {
            pairs.append_pair("company", company.trim());
        }
        if !role.trim().is_empty() {
            pairs.append_pair("role", role.trim());
        }
        pairs.append_pair("limit", &clamped_limit.to_string());
    }

    let response = http_client()?.get(url).send().await.map_err(|e| {
        if e.is_timeout() {
            "Smart Resume timed out. Check your connection and try again.".to_string()
        } else {
            "Could not reach Smart Resume. Check FLINT_SMART_RESUME_URL.".to_string()
        }
    })?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(map_status_error(status, &body));
    }

    let parsed: QuestionsResponse = serde_json::from_str(&body)
        .map_err(|_| "Smart Resume returned an invalid question bank response.".to_string())?;

    Ok(parsed.questions)
}

/// Merge remote bank questions into a local digest bank without duplicates.
/// Local digest questions always win — remote entries are appended only.
pub fn merge_question_bank(local: &mut Vec<String>, remote: &[InterviewQuestionDto]) {
    let existing: std::collections::HashSet<String> =
        local.iter().map(|q| q.trim().to_lowercase()).collect();
    for question in remote {
        let text = question.text.trim();
        if text.is_empty() {
            continue;
        }
        let key = text.to_lowercase();
        if existing.contains(&key) {
            continue;
        }
        local.push(text.to_string());
    }
}

/// Format global bank entries for context-store embedding at session setup.
pub fn bank_entries_for_context_embed(questions: &[InterviewQuestionDto]) -> Vec<String> {
    questions
        .iter()
        .take(CONTEXT_EMBED_LIMIT)
        .map(|q| {
            let answer = q
                .canonical_answer
                .as_deref()
                .filter(|a| !a.trim().is_empty())
                .unwrap_or(
                    "Prepare a personal answer grounded in your resume and this role's context.",
                );
            format!(
                "[Global interview question bank]\nQ: {}\nA: {}",
                q.text.trim(),
                answer.trim()
            )
        })
        .collect()
}

pub async fn redeem_handoff_token(token: &str) -> Result<SmartResumeImportDto, String> {
    validate_token(token)?;

    let url = format!("{}{}", base_url()?, REDEEM_PATH);

    let response = http_client()?
        .post(&url)
        .json(&RedeemRequest {
            token: token.trim().to_string(),
        })
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "Smart Resume timed out. Check your connection and try again.".to_string()
            } else {
                "Could not reach Smart Resume. Check FLINT_SMART_RESUME_URL.".to_string()
            }
        })?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(map_status_error(status, &body));
    }

    let parsed: RedeemResponse = serde_json::from_str(&body)
        .map_err(|_| "Smart Resume returned an invalid response.".to_string())?;

    let company_intel = parsed.company_intel.and_then(|ci| {
        let block = CompanyIntelBlock {
            mission: ci.mission,
            values: ci.values,
            culture_notes: ci.culture_notes,
        };
        if block.is_empty() {
            None
        } else {
            Some(block)
        }
    });

    Ok(SmartResumeImportDto {
        session_name: parsed.session_name,
        session_type: parsed.session_type,
        domain: parsed.domain,
        jd_text: parsed.jd_text,
        resume_summary: parsed.resume_summary,
        smart_resume_session_id: parsed.smart_resume_session_id,
        export_version: parsed.export_version,
        company_intel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_question_bank_preserves_local_and_dedupes_remote() {
        let mut local = vec![
            "Tell me about yourself.".to_string(),
            "Why this role?".to_string(),
        ];
        let remote = vec![
            InterviewQuestionDto {
                id: "uni-1".into(),
                text: "Tell me about yourself.".into(),
                domain: "universal".into(),
                category: "introduction".into(),
                canonical_answer: None,
            },
            InterviewQuestionDto {
                id: "swe-1".into(),
                text: "Explain CAP theorem trade-offs.".into(),
                domain: "software_engineering".into(),
                category: "technical".into(),
                canonical_answer: None,
            },
        ];
        merge_question_bank(&mut local, &remote);
        assert_eq!(local.len(), 3);
        assert!(local.contains(&"Explain CAP theorem trade-offs.".to_string()));
    }

    #[test]
    fn bank_entries_for_context_embed_caps_at_fifteen() {
        let questions: Vec<InterviewQuestionDto> = (0..20)
            .map(|i| InterviewQuestionDto {
                id: format!("q-{i}"),
                text: format!("Question {i}?"),
                domain: "universal".into(),
                category: "general".into(),
                canonical_answer: Some("Framework answer.".into()),
            })
            .collect();
        let chunks = bank_entries_for_context_embed(&questions);
        assert_eq!(chunks.len(), CONTEXT_EMBED_LIMIT);
        assert!(chunks[0].contains("Question 0?"));
        assert!(chunks[0].contains("Framework answer."));
    }
}
