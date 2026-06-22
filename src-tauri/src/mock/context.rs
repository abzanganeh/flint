//! Format Session Design fields for mock coach / suggested-answer prompts.

use crate::session::persistence::SessionContextFields;

/// Company-facing sections from Session Design for mock LLM prompts.
pub fn format_company_context_for_prompt(fields: &SessionContextFields) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(3);
    let mut push = |label: &str, text: &str| {
        let t = text.trim();
        if !t.is_empty() {
            parts.push(format!("[{label}]\n{t}"));
        }
    };
    push("COMPANY OVERVIEW", &fields.company_overview);
    push("LEADERSHIP PRINCIPLES", &fields.leadership_principles);
    push("ROLE EXPECTATIONS", &fields.role_expectations);
    if parts.is_empty() {
        "(No company-specific context provided — use role context and profile only.)".to_string()
    } else {
        parts.join("\n\n")
    }
}

/// Human-readable coaching instruction from the user's speaking-style preference.
pub fn format_speaking_style_for_prompt(style: &str) -> &'static str {
    match style.trim().to_ascii_lowercase().as_str() {
        "natural" => {
            "Natural voice — conversational and authentic. Coach for clarity and structure, \
             not corporate polish. Minor filler is OK if the substance is strong."
        }
        _ => {
            "Polished professional voice — confident, structured, concise. \
             Flag vague hedging and reward crisp specificity."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_company_context_includes_non_empty_sections() {
        let fields = SessionContextFields {
            company_overview: "Mission: help clients retire with dignity.".to_string(),
            leadership_principles: "Client-first, long-term thinking.".to_string(),
            role_expectations: "Own IAM architecture end-to-end.".to_string(),
            ..Default::default()
        };
        let text = format_company_context_for_prompt(&fields);
        assert!(text.contains("[COMPANY OVERVIEW]"));
        assert!(text.contains("Mission: help clients retire"));
        assert!(text.contains("[LEADERSHIP PRINCIPLES]"));
        assert!(text.contains("[ROLE EXPECTATIONS]"));
    }

    #[test]
    fn format_company_context_fallback_when_empty() {
        let text = format_company_context_for_prompt(&SessionContextFields::default());
        assert!(text.contains("No company-specific context"));
    }

    #[test]
    fn speaking_style_natural_vs_polished() {
        assert!(format_speaking_style_for_prompt("natural").contains("Natural"));
        assert!(format_speaking_style_for_prompt("polished").contains("Polished"));
        assert!(format_speaking_style_for_prompt("").contains("Polished"));
    }
}
