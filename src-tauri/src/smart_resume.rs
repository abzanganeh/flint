//! Smart Resume cross-product handoff client (Strategy B Phase 1).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::dto::{CompanyIntelBlock, SmartResumeImportDto};

const REDEEM_PATH: &str = "/api/flint/context";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

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

pub async fn redeem_handoff_token(token: &str) -> Result<SmartResumeImportDto, String> {
    validate_token(token)?;

    let url = format!("{}{}", base_url()?, REDEEM_PATH);
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|_| "Could not reach Smart Resume.".to_string())?;

    let response = client
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
