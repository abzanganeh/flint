//! Supabase `global_question_bank` reader for session enrichment (Phase 11).

use serde::Deserialize;

use crate::smart_resume::InterviewQuestionDto;

const FETCH_TIMEOUT_SECS: u64 = 8;

#[derive(Debug, Deserialize)]
struct BankRow {
    id: String,
    question_text: String,
    domain: String,
    subdomain: Option<String>,
    canonical_answer: Option<String>,
}

fn supabase_rest_config() -> Option<(String, String)> {
    let url = std::env::var("FLINT_SUPABASE_URL").ok().or_else(|| {
        #[cfg(debug_assertions)]
        {
            Some("http://127.0.0.1:54321".to_string())
        }
        #[cfg(not(debug_assertions))]
        {
            None
        }
    })?;
    let key = std::env::var("FLINT_SUPABASE_ANON_KEY").ok()?;
    let base = url.trim().trim_end_matches('/').to_string();
    let anon = key.trim().to_string();
    if base.is_empty() || anon.is_empty() {
        return None;
    }
    Some((base, anon))
}

fn normalize_domain_slug(raw: &str) -> String {
    raw.trim()
        .to_lowercase()
        .replace(' ', "_")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

/// Fetch curated questions from Supabase when configured and table is populated.
/// Returns empty vec on misconfiguration or fetch errors (non-fatal at call sites).
pub async fn fetch_global_bank_questions(
    domain: &str,
    role: &str,
    limit: u32,
) -> Vec<InterviewQuestionDto> {
    let Some((base, anon)) = supabase_rest_config() else {
        return Vec::new();
    };

    let clamped = limit.clamp(1, 100);
    let domain_slug = normalize_domain_slug(domain);
    let filter = if domain_slug.is_empty() || domain_slug == "universal" {
        "review_status=eq.auto_approved&order=quality_score.desc.nullslast".to_string()
    } else {
        format!(
            "or=(domain.eq.universal,domain.eq.{domain_slug})&review_status=eq.auto_approved&order=quality_score.desc.nullslast"
        )
    };

    let url = format!(
        "{base}/rest/v1/global_question_bank?select=id,question_text,domain,subdomain,canonical_answer&{filter}&limit={clamped}"
    );

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, "global_bank client build failed");
            return Vec::new();
        }
    };

    let response = match client
        .get(&url)
        .header("apikey", &anon)
        .header("Authorization", format!("Bearer {anon}"))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(error = %e, "global_bank fetch failed");
            return Vec::new();
        }
    };

    if !response.status().is_success() {
        tracing::debug!(
            status = %response.status(),
            "global_bank fetch non-success"
        );
        return Vec::new();
    }

    let rows: Vec<BankRow> = match response.json().await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(error = %e, "global_bank parse failed");
            return Vec::new();
        }
    };

    if rows.is_empty() {
        return Vec::new();
    }

    let role_lower = role.trim().to_lowercase();
    let mut mapped: Vec<InterviewQuestionDto> = rows
        .into_iter()
        .map(|row| InterviewQuestionDto {
            id: row.id,
            text: row.question_text,
            domain: row.domain,
            category: row.subdomain.unwrap_or_else(|| "general".to_string()),
            canonical_answer: row.canonical_answer,
        })
        .collect();

    if !role_lower.is_empty() && mapped.len() > 1 {
        mapped.sort_by(|a, b| {
            let score_b =
                role_match_score(&b.text, &role_lower) + role_match_score(&b.category, &role_lower);
            let score_a =
                role_match_score(&a.text, &role_lower) + role_match_score(&a.category, &role_lower);
            score_b.cmp(&score_a)
        });
    }

    tracing::debug!(count = mapped.len(), "global_bank fetch ok");
    mapped
}

fn role_match_score(haystack: &str, role: &str) -> i32 {
    role.split_whitespace()
        .filter(|t| t.len() > 2)
        .map(|token| {
            if haystack.to_lowercase().contains(token) {
                1
            } else {
                0
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_domain_slug_maps_spaces() {
        assert_eq!(
            normalize_domain_slug("Software Engineering"),
            "software_engineering"
        );
    }

    #[test]
    fn role_match_score_counts_tokens() {
        assert!(role_match_score("distributed systems engineer", "staff engineer platform") >= 1);
    }
}
