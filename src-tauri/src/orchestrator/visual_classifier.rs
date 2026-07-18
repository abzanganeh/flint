//! Visual-need classifier — `lpav-s20-visual-classifier`.
//!
//! Cheap Tier-1 regex/keyword heuristic deciding whether a question is
//! likely to benefit from a diagram (system design, algorithms, data
//! models, whiteboard walkthroughs) versus a purely verbal answer
//! (behavioral, opinion, "tell me about yourself" style questions).
//!
//! Runs synchronously on the hot dispatch path (`orchestrator::mod::run_turn`,
//! wired in slice 21) — must stay well under the question-detection P95
//! budget (100ms, `.cursor/rules/flint-performance.mdc`), so no LLM call
//! happens here by default. A tiny-LLM Tier-2 pass for genuinely ambiguous
//! questions is deliberately not implemented in this milestone — the regex
//! heuristic below is tuned to be permissive (prefer a false-positive Visual
//! spawn, which just costs one extra parallel LLM call, over a false
//! negative that silently withholds a diagram the user wanted).

/// Keywords whose presence strongly implies a diagram would help the answer.
/// Matched case-insensitively against the whole question text.
const VISUAL_KEYWORDS: &[&str] = &[
    // System design / architecture
    "system design",
    "design a system",
    "design an api",
    "design a rate limiter",
    "architecture",
    "microservice",
    "high level design",
    "high-level design",
    "component diagram",
    // Data flow / sequencing
    "sequence diagram",
    "flowchart",
    "flow chart",
    "data flow",
    "request flow",
    "call flow",
    // Data modelling
    "database schema",
    "data model",
    "entity relationship",
    "er diagram",
    "class diagram",
    "class hierarchy",
    "object model",
    // State / lifecycle
    "state machine",
    "state diagram",
    "lifecycle",
    // Algorithms / whiteboard
    "algorithm",
    "data structure",
    "whiteboard",
    "draw",
    "sketch",
    "diagram",
    "pseudocode",
    "big o",
    "time complexity",
    "walk me through how you would design",
    "how would you design",
    "how would you architect",
];

/// Returns `true` when the question is likely to benefit from a Visual
/// thread response (a Mermaid diagram or code block) rather than — or in
/// addition to — a purely verbal Answer.
pub fn needs_visual(question: &str) -> bool {
    let lower = question.to_lowercase();
    VISUAL_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Table-driven cases per the task's required coverage: system design,
    /// algorithm, and whiteboard questions must trigger the Visual thread;
    /// purely verbal/behavioral questions must not.
    #[test]
    fn needs_visual_table() {
        let cases: &[(&str, bool)] = &[
            (
                "Walk me through how you would design a rate limiter for a public API.",
                true,
            ),
            (
                "Can you design a URL shortener system, including the database schema?",
                true,
            ),
            (
                "What's the time complexity of this sorting algorithm?",
                true,
            ),
            (
                "Can you whiteboard the sequence diagram for a checkout flow?",
                true,
            ),
            (
                "Explain the class hierarchy you'd use for a plugin system.",
                true,
            ),
            (
                "Tell me about a time you disagreed with a senior engineer.",
                false,
            ),
            ("Why do you want to work here?", false),
            ("What are your salary expectations?", false),
            ("How do you handle tight deadlines?", false),
            ("Describe your greatest strength.", false),
        ];

        for (question, expected) in cases {
            assert_eq!(
                needs_visual(question),
                *expected,
                "needs_visual({question:?}) expected {expected}"
            );
        }
    }

    #[test]
    fn needs_visual_is_case_insensitive() {
        assert!(needs_visual("DESIGN A SYSTEM for a chat application."));
        assert!(needs_visual("please Draw the Sequence Diagram"));
    }

    #[test]
    fn needs_visual_empty_question_returns_false() {
        assert!(!needs_visual(""));
    }
}
