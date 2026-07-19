//! Interview round-type taxonomy and round-appropriate supplemental
//! questions — Session Focus Robustness bug 3.
//!
//! `likely_questions` extracted from the pasted job description skew toward
//! whatever the JD emphasises (often deep technical detail), even when the
//! actual interview is a lightweight recruiter/HR screen. This module lets a
//! session declare which round it's for and seeds correctly-tagged questions
//! for that round without ever removing the JD-derived set.

use crate::session::question_bank::BankQuestionEntry;

/// Canonical round types as (id, display label) pairs. The id is the wire
/// value persisted on `SessionFocus.round_type`; the label is UI copy.
pub const ROUND_TYPES: &[(&str, &str)] = &[
    ("recruiter_screen", "Recruiter / HR screen"),
    ("technical", "Technical interview"),
    ("hiring_manager", "Hiring manager"),
    ("onsite_panel", "Onsite / panel loop"),
    ("final", "Final / executive round"),
];

/// Heuristic inference from the pasted recruiter brief/agenda — a
/// *suggestion* the user can override, never authoritative on its own.
pub fn infer_round_type(recruiter_brief: &str) -> Option<&'static str> {
    let lower = recruiter_brief.to_lowercase();
    if lower.contains("recruiter")
        || lower.contains("hr screen")
        || lower.contains("phone screen")
        || lower.contains("initial screening")
        || lower.contains("internal recruiter")
    {
        return Some("recruiter_screen");
    }
    if lower.contains("system design")
        || lower.contains("coding")
        || lower.contains("whiteboard")
        || lower.contains("technical interview")
    {
        return Some("technical");
    }
    if lower.contains("hiring manager") {
        return Some("hiring_manager");
    }
    if lower.contains("onsite") || lower.contains("panel") || lower.contains("loop") {
        return Some("onsite_panel");
    }
    if lower.contains("final round") || lower.contains("executive") {
        return Some("final");
    }
    None
}

/// Supplemental, correctly-tagged questions merged into the bank when a
/// round type is confirmed — additive only, never removes JD-derived
/// questions (those stay available for a later round on the same session).
pub fn supplemental_questions_for_round(round_type: &str) -> Vec<BankQuestionEntry> {
    match round_type {
        "recruiter_screen" => vec![
            BankQuestionEntry::new(
                "Tell me about yourself and your background.",
                vec!["self-assessment".into()],
            ),
            BankQuestionEntry::new(
                "Why are you interested in this role?",
                vec!["motivation".into()],
            ),
            BankQuestionEntry::new("Why this company?", vec!["motivation".into()]),
            BankQuestionEntry::new(
                "What are you looking for in your next role?",
                vec!["motivation".into(), "logistics".into()],
            ),
            BankQuestionEntry::new(
                "What's your current location and work setup preference?",
                vec!["logistics".into()],
            ),
            BankQuestionEntry::new(
                "What questions do you have for me?",
                vec!["general".into()],
            ),
        ],
        "hiring_manager" => vec![
            BankQuestionEntry::new(
                "How do you like to work with your manager?",
                vec!["culture".into()],
            ),
            BankQuestionEntry::new(
                "Tell me about a time you disagreed with a decision.",
                vec!["competency".into(), "behavioral".into()],
            ),
        ],
        "onsite_panel" | "final" => vec![
            BankQuestionEntry::new(
                "How do you handle competing priorities across stakeholders?",
                vec!["competency".into()],
            ),
            BankQuestionEntry::new(
                "What does success look like for you in the first 90 days?",
                vec!["motivation".into()],
            ),
        ],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_recruiter_screen() {
        assert_eq!(
            infer_round_type("This will be a 30 minute recruiter screen"),
            Some("recruiter_screen")
        );
        assert_eq!(
            infer_round_type("Initial HR screen with internal recruiter"),
            Some("recruiter_screen")
        );
    }

    #[test]
    fn infers_technical() {
        assert_eq!(
            infer_round_type("Expect a system design and coding round"),
            Some("technical")
        );
        assert_eq!(
            infer_round_type("Whiteboard technical interview with the team"),
            Some("technical")
        );
    }

    #[test]
    fn infers_hiring_manager() {
        assert_eq!(
            infer_round_type("You'll meet with the hiring manager"),
            Some("hiring_manager")
        );
    }

    #[test]
    fn infers_onsite_panel() {
        assert_eq!(
            infer_round_type("Full onsite panel loop with four interviewers"),
            Some("onsite_panel")
        );
    }

    #[test]
    fn infers_final() {
        assert_eq!(
            infer_round_type("This is the final round with the executive team"),
            Some("final")
        );
    }

    #[test]
    fn no_match_returns_none() {
        assert_eq!(infer_round_type("Just a general chat about the role"), None);
        assert_eq!(infer_round_type(""), None);
    }

    #[test]
    fn supplemental_questions_recruiter_screen_are_tagged() {
        let entries = supplemental_questions_for_round("recruiter_screen");
        assert_eq!(entries.len(), 6);
        assert!(entries
            .iter()
            .any(|e| e.tags.contains(&"self-assessment".to_string())));
        assert!(entries
            .iter()
            .any(|e| e.tags.contains(&"motivation".to_string())));
        assert!(entries
            .iter()
            .any(|e| e.tags.contains(&"logistics".to_string())));
        assert!(entries
            .iter()
            .any(|e| e.tags.contains(&"general".to_string())));
    }

    #[test]
    fn supplemental_questions_hiring_manager_are_tagged() {
        let entries = supplemental_questions_for_round("hiring_manager");
        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .any(|e| e.tags.contains(&"culture".to_string())));
        assert!(entries
            .iter()
            .any(|e| e.tags.contains(&"competency".to_string())));
    }

    #[test]
    fn supplemental_questions_onsite_and_final_share_the_same_set() {
        let onsite = supplemental_questions_for_round("onsite_panel");
        let final_round = supplemental_questions_for_round("final");
        assert_eq!(onsite.len(), 2);
        assert_eq!(onsite, final_round);
    }

    #[test]
    fn supplemental_questions_unknown_or_empty_round_type_is_empty() {
        assert!(supplemental_questions_for_round("").is_empty());
        assert!(supplemental_questions_for_round("technical").is_empty());
        assert!(supplemental_questions_for_round("not-a-real-round").is_empty());
    }
}
