//! Canonical focus-tag taxonomy, independent of any single session's bank
//! content. This is the taxonomy `infer_question_tags` (`question_bank.rs`)
//! already implicitly targets — making it an explicit, exposed list lets
//! Session Focus show all 8 tags (with live counts) instead of only the
//! ones a heuristic happened to hit for the current bank.

/// One entry in the canonical focus-tag taxonomy.
pub struct FocusTagDef {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

pub const FOCUS_TAG_TAXONOMY: &[FocusTagDef] = &[
    FocusTagDef {
        id: "self-assessment",
        label: "Self-assessment",
        description: "Tell me about yourself, strengths/weaknesses",
    },
    FocusTagDef {
        id: "motivation",
        label: "Motivation",
        description: "Why this role, why this company",
    },
    FocusTagDef {
        id: "behavioral",
        label: "Behavioral",
        description: "Tell me about a time..., STAR-style",
    },
    FocusTagDef {
        id: "competency",
        label: "Competency",
        description: "Leadership, conflict, prioritization, stakeholders",
    },
    FocusTagDef {
        id: "culture",
        label: "Culture fit",
        description: "Values, team fit, work style",
    },
    FocusTagDef {
        id: "technical",
        label: "Technical",
        description: "System design, coding, architecture",
    },
    FocusTagDef {
        id: "logistics",
        label: "Logistics",
        description: "Location, availability, compensation",
    },
    FocusTagDef {
        id: "general",
        label: "General",
        description: "Process, timeline, other",
    },
];

/// Whether `id` matches a known taxonomy entry.
pub fn is_valid_tag_id(id: &str) -> bool {
    FOCUS_TAG_TAXONOMY.iter().any(|def| def.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taxonomy_has_eight_entries() {
        assert_eq!(FOCUS_TAG_TAXONOMY.len(), 8);
    }

    #[test]
    fn taxonomy_ids_are_unique() {
        let mut ids: Vec<&str> = FOCUS_TAG_TAXONOMY.iter().map(|d| d.id).collect();
        let before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), before);
    }

    #[test]
    fn taxonomy_contains_expected_ids() {
        let ids: Vec<&str> = FOCUS_TAG_TAXONOMY.iter().map(|d| d.id).collect();
        for expected in [
            "self-assessment",
            "motivation",
            "behavioral",
            "competency",
            "culture",
            "technical",
            "logistics",
            "general",
        ] {
            assert!(ids.contains(&expected), "missing tag id: {expected}");
        }
    }

    #[test]
    fn is_valid_tag_id_accepts_known_ids() {
        assert!(is_valid_tag_id("motivation"));
        assert!(is_valid_tag_id("general"));
    }

    #[test]
    fn is_valid_tag_id_rejects_unknown_ids() {
        assert!(!is_valid_tag_id("motivaton"));
        assert!(!is_valid_tag_id(""));
        assert!(!is_valid_tag_id("Motivation"));
    }
}
