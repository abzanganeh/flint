//! Tagged question bank entries and heuristic tag inference.

use serde::{Deserialize, Serialize};

/// One question in the session bank with optional focus tags.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BankQuestionEntry {
    pub question: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl BankQuestionEntry {
    pub fn new(question: impl Into<String>, tags: Vec<String>) -> Self {
        Self {
            question: question.into(),
            tags,
        }
    }

    pub fn question_only(question: impl Into<String>) -> Self {
        let q = question.into();
        let tags = infer_question_tags(&q);
        Self { question: q, tags }
    }
}

/// Parse bank JSON — accepts legacy `["question", …]` or tagged objects.
pub fn parse_bank_json(json: &str) -> Vec<BankQuestionEntry> {
    if json.trim().is_empty() || json.trim() == "[]" {
        return Vec::new();
    }
    if let Ok(entries) = serde_json::from_str::<Vec<BankQuestionEntry>>(json) {
        return entries
            .into_iter()
            .filter(|e| !e.question.trim().is_empty())
            .collect();
    }
    if let Ok(strings) = serde_json::from_str::<Vec<String>>(json) {
        return strings
            .into_iter()
            .filter(|q| !q.trim().is_empty())
            .map(BankQuestionEntry::question_only)
            .collect();
    }
    Vec::new()
}

pub fn bank_to_json(entries: &[BankQuestionEntry]) -> String {
    serde_json::to_string(entries).unwrap_or_else(|_| "[]".to_string())
}

pub fn bank_questions(entries: &[BankQuestionEntry]) -> Vec<String> {
    entries.iter().map(|e| e.question.clone()).collect()
}

/// Heuristic tags for interview focus filtering (rehearsal / mock only).
pub fn infer_question_tags(question: &str) -> Vec<String> {
    let lower = question.to_lowercase();
    let mut tags: Vec<String> = Vec::new();

    let add = |tags: &mut Vec<String>, tag: &str| {
        if !tags.iter().any(|t| t == tag) {
            tags.push(tag.to_string());
        }
    };

    if lower.contains("tell me about a time")
        || lower.contains("give me an example")
        || lower.contains("describe a situation")
        || lower.contains("walk me through a time")
    {
        add(&mut tags, "behavioral");
    }
    if lower.contains("why do you want")
        || lower.contains("why this role")
        || lower.contains("why our company")
        || lower.contains("motivat")
    {
        add(&mut tags, "motivation");
    }
    if lower.contains("culture")
        || lower.contains("values")
        || lower.contains("team fit")
        || lower.contains("work style")
    {
        add(&mut tags, "culture");
    }
    if lower.contains("salary")
        || lower.contains("compensation")
        || lower.contains("notice period")
        || lower.contains("availability")
    {
        add(&mut tags, "logistics");
    }
    if lower.contains("design")
        || lower.contains("architect")
        || lower.contains("system")
        || lower.contains("scale")
        || lower.contains("debug")
        || lower.contains("algorithm")
        || lower.contains("code")
        || lower.contains("technical")
    {
        add(&mut tags, "technical");
    }
    if lower.contains("leadership")
        || lower.contains("conflict")
        || lower.contains("disagree")
        || lower.contains("stakeholder")
        || lower.contains("priorit")
    {
        add(&mut tags, "competency");
    }
    if lower.contains("strength") || lower.contains("weakness") || lower.contains("yourself") {
        add(&mut tags, "self-assessment");
    }

    if tags.is_empty() {
        add(&mut tags, "general");
    }
    tags
}

/// Map global-bank subdomain strings to focus tags.
pub fn tag_from_subdomain(subdomain: Option<&str>) -> Option<String> {
    let s = subdomain?.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }
    Some(match s.as_str() {
        "behavioral" | "behavior" => "behavioral".to_string(),
        "motivation" | "motivational" => "motivation".to_string(),
        "culture" | "cultural" => "culture".to_string(),
        "technical" | "coding" | "system design" => "technical".to_string(),
        "competency" | "competencies" => "competency".to_string(),
        "logistics" | "hr" => "logistics".to_string(),
        other => other.replace(' ', "-"),
    })
}

/// Filter bank entries to those matching any selected focus tag (OR semantics).
pub fn filter_by_focus_tags(
    entries: &[BankQuestionEntry],
    focus_tags: &[String],
) -> Vec<BankQuestionEntry> {
    if focus_tags.is_empty() {
        return entries.to_vec();
    }
    let wanted: Vec<String> = focus_tags
        .iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if wanted.is_empty() {
        return entries.to_vec();
    }
    entries
        .iter()
        .filter(|e| e.tags.iter().any(|t| wanted.contains(&t.to_lowercase())))
        .cloned()
        .collect()
}

/// Collect unique tags across all bank entries, sorted.
pub fn collect_bank_tags(entries: &[BankQuestionEntry]) -> Vec<String> {
    let mut tags: Vec<String> = entries
        .iter()
        .flat_map(|e| e.tags.iter().cloned())
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_legacy_string_array() {
        let json = r#"["Why IAM?","Tell me about a time you led a project"]"#;
        let entries = parse_bank_json(json);
        assert_eq!(entries.len(), 2);
        assert!(entries[1].tags.contains(&"behavioral".to_string()));
    }

    #[test]
    fn filter_or_semantics() {
        let entries = vec![
            BankQuestionEntry::new("Q1", vec!["behavioral".into()]),
            BankQuestionEntry::new("Q2", vec!["technical".into()]),
        ];
        let filtered = filter_by_focus_tags(&entries, &["technical".into()]);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].question, "Q2");
    }
}
