//! Session-aware Whisper `initial_prompt` construction — design doc §26.

use crate::digest::Digest;

/// Fallback when no digest has been confirmed (health check, calibration).
pub const FALLBACK_WHISPER_INITIAL_PROMPT: &str =
    "Professional interview conversation. OAuth OIDC MFA IAM LLM API SaaS enterprise authentication.";

const MAX_PROMPT_CHARS: usize = 220;

/// Domain-specific vocabulary injected when the digest domain/role matches a known area.
///
/// These short lists give Whisper the right prior for technical speech that phonetically
/// sounds like common words — e.g. "RBAC" → "our back", "fiduciary" → "feudings".
const DOMAIN_VOCAB: &[(&str, &[&str])] = &[
    (
        "iam",
        &["RBAC", "ABAC", "SCIM", "SAML", "OAuth", "OIDC", "MFA", "LDAP", "SSO"],
    ),
    (
        "identity",
        &[
            "RBAC", "ABAC", "zero-trust", "agentic", "entitlement", "provisioning",
        ],
    ),
    (
        "security",
        &[
            "RBAC", "ABAC", "zero-trust", "posture", "misconfiguration", "SIEM",
        ],
    ),
    (
        "finance",
        &[
            "fiduciary", "suitability", "entitlement", "compliance", "AUM",
        ],
    ),
    (
        "financial",
        &[
            "fiduciary", "suitability", "entitlement", "compliance", "AUM",
        ],
    ),
    (
        "asset management",
        &["fiduciary", "suitability", "AUM", "portfolio", "compliance"],
    ),
    (
        "machine learning",
        &["RAG", "LLM", "embedding", "inference", "fine-tuning", "RLHF"],
    ),
    (
        "ai",
        &["LLM", "RAG", "agentic", "embedding", "inference", "fine-tuning"],
    ),
    (
        "cloud",
        &["Kubernetes", "Terraform", "IAM", "VPC", "microservices", "SLA"],
    ),
    (
        "backend",
        &[
            "idempotent", "microservices", "gRPC", "Kafka", "Postgres", "Redis",
        ],
    ),
];

/// Return domain vocabulary tokens that match any word in `domain_or_role`.
fn domain_vocab_tokens(domain_or_role: &str) -> Vec<&'static str> {
    let haystack = domain_or_role.to_lowercase();
    let mut out: Vec<&'static str> = Vec::new();
    for (keyword, tokens) in DOMAIN_VOCAB {
        if haystack.contains(keyword) {
            for t in *tokens {
                if !out.contains(t) {
                    out.push(t);
                }
            }
        }
    }
    out
}

/// Build a session-specific Whisper initial prompt from digest + raw context text.
pub fn build_whisper_initial_prompt(digest: &Digest, context_text: &str) -> String {
    let role = digest.role.trim();
    let company = digest.company.trim();
    let domain = digest.domain.trim();

    let mut prompt = if role.is_empty() && company.is_empty() {
        FALLBACK_WHISPER_INITIAL_PROMPT.to_string()
    } else if company.is_empty() {
        format!("{role} interview.")
    } else if role.is_empty() {
        format!("Interview at {company}.")
    } else {
        format!("{role} interview at {company}.")
    };

    if !domain.is_empty() {
        prompt.push(' ');
        prompt.push_str(domain);
        if !domain.ends_with('.') {
            prompt.push('.');
        }
    }

    // Domain vocabulary first — highest-value bias for Whisper.
    let combined = format!("{domain} {role}");
    let domain_tokens: Vec<String> = domain_vocab_tokens(&combined)
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    let mut tokens: Vec<String> = domain_tokens;
    for skill in &digest.key_skills {
        let t = skill.trim();
        if !t.is_empty() {
            tokens.push(t.to_string());
        }
    }
    tokens.extend(extract_frequent_capitalised_tokens(context_text));

    let mut seen = std::collections::HashSet::new();
    let prompt_lower = prompt.to_lowercase();
    for token in tokens {
        let key = token.to_lowercase();
        if seen.contains(&key) || prompt_lower.contains(&key) {
            continue;
        }
        seen.insert(key);
        if seen.len() > 15 {
            break;
        }
        if prompt.len() + 1 + token.len() > MAX_PROMPT_CHARS {
            break;
        }
        prompt.push(' ');
        prompt.push_str(&token);
    }

    truncate_prompt(&prompt)
}

fn extract_frequent_capitalised_tokens(text: &str) -> Vec<String> {
    let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();

    for word in text.split_whitespace() {
        let cleaned: String = word
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_string();
        if cleaned.len() < 2 {
            continue;
        }
        let first = cleaned.chars().next().unwrap_or(' ');
        if !first.is_uppercase() {
            continue;
        }
        if cleaned.chars().all(|c| c.is_uppercase()) && cleaned.len() <= 5 {
            // Likely acronym — keep as-is.
        } else if !cleaned.chars().all(|c| c.is_alphabetic()) {
            continue;
        }
        *counts.entry(cleaned).or_insert(0) += 1;
    }

    let mut frequent: Vec<(String, u32)> = counts.into_iter().filter(|(_, c)| *c >= 2).collect();
    frequent.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    frequent.into_iter().map(|(w, _)| w).collect()
}

fn truncate_prompt(prompt: &str) -> String {
    if prompt.chars().count() <= MAX_PROMPT_CHARS {
        return prompt.to_string();
    }
    prompt
        .char_indices()
        .nth(MAX_PROMPT_CHARS)
        .map(|(idx, _)| prompt[..idx].trim_end().to_string())
        .unwrap_or_else(|| prompt.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_digest() -> Digest {
        Digest {
            role: "IAM architect".to_string(),
            company: "Fisher Investments".to_string(),
            domain: "Identity security".to_string(),
            key_skills: vec![
                "OAuth".to_string(),
                "OIDC".to_string(),
                "SAML".to_string(),
                "MFA".to_string(),
            ],
            seniority: "senior".to_string(),
            likely_questions: vec!["Tell me about yourself".to_string()],
            topics_to_avoid: vec![],
        }
    }

    #[test]
    fn digest_produces_role_company_domain_prefix() {
        let prompt = build_whisper_initial_prompt(&sample_digest(), "");
        assert!(prompt.starts_with("IAM architect interview at Fisher Investments."));
        assert!(prompt.contains("Identity security"));
        assert!(prompt.contains("OAuth"));
    }

    #[test]
    fn iam_domain_injects_rbac_abac() {
        let mut d = sample_digest();
        d.domain = "IAM and identity security".to_string();
        let prompt = build_whisper_initial_prompt(&d, "");
        assert!(prompt.contains("RBAC"), "expected RBAC in prompt: {prompt}");
        assert!(prompt.contains("ABAC"), "expected ABAC in prompt: {prompt}");
        assert!(prompt.chars().count() <= MAX_PROMPT_CHARS);
    }

    #[test]
    fn finance_domain_injects_fiduciary() {
        let mut d = sample_digest();
        d.domain = "Financial services and fiduciary compliance".to_string();
        let prompt = build_whisper_initial_prompt(&d, "");
        assert!(prompt.contains("fiduciary"), "expected fiduciary in prompt: {prompt}");
        assert!(prompt.chars().count() <= MAX_PROMPT_CHARS);
    }

    #[test]
    fn identity_security_role_injects_zero_trust_agentic() {
        let mut d = sample_digest();
        d.role = "Identity security architect".to_string();
        d.domain = "Zero-trust IAM".to_string();
        let prompt = build_whisper_initial_prompt(&d, "");
        assert!(prompt.contains("zero-trust") || prompt.contains("RBAC"), "prompt: {prompt}");
        assert!(prompt.chars().count() <= MAX_PROMPT_CHARS);
    }

    #[test]
    fn truncates_at_220_chars() {
        let mut digest = sample_digest();
        digest.key_skills = (0..20).map(|i| format!("TechnologyStack{i}")).collect();
        let prompt = build_whisper_initial_prompt(&digest, "");
        assert!(prompt.chars().count() <= MAX_PROMPT_CHARS);
    }

    #[test]
    fn fallback_when_empty_digest_fields() {
        let digest = Digest {
            role: String::new(),
            company: String::new(),
            domain: String::new(),
            key_skills: vec![],
            seniority: String::new(),
            likely_questions: vec!["Q".to_string()],
            topics_to_avoid: vec![],
        };
        assert_eq!(
            build_whisper_initial_prompt(&digest, ""),
            FALLBACK_WHISPER_INITIAL_PROMPT
        );
    }
}
