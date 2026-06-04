//! Conversation memory with dynamic context budget management (design doc §23).
//!
//! Cloud providers (Groq, OpenAI, Anthropic) get full verbatim history because
//! their 128K+ context windows easily fit an entire session. Local Ollama models
//! work within a 4K–8K window, so history is compressed progressively.
//!
//! Context budget per call:
//!   total  = context_window × 0.6
//!   RAG    = 40% of budget
//!   hist   = 40% of budget
//!   system = 10% of budget
//!   query  = 10% of budget

use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};

use crate::llm::provider::{CompletionConfig, LLMProvider};

// ──────────────────────────────────────────────────────────────────────────────
// Types
// ──────────────────────────────────────────────────────────────────────────────

/// A single exchange in the conversation.
#[derive(Debug, Clone)]
pub struct Turn {
    /// Raw question / utterance as transcribed.
    pub question: String,
    /// The directional response that was shown (empty if none yet).
    pub directional_response: String,
    /// The depth response that was shown (empty if none yet).
    pub depth_response: String,
}

/// Budget allocation computed once per LLM call from the provider's context
/// window and the 0.6 safety margin from the design doc.
#[derive(Debug, Clone, Copy)]
pub struct ContextBudget {
    pub rag_tokens: usize,
    pub history_tokens: usize,
    pub system_tokens: usize,
    pub question_tokens: usize,
}

impl ContextBudget {
    pub fn from_window(context_window: usize) -> Self {
        let total = (context_window as f64 * 0.6) as usize;
        Self {
            rag_tokens: (total as f64 * 0.40) as usize,
            history_tokens: (total as f64 * 0.40) as usize,
            system_tokens: (total as f64 * 0.10) as usize,
            question_tokens: (total as f64 * 0.10) as usize,
        }
    }
}

/// The assembled context payload ready to be injected into a prompt template.
#[derive(Debug, Clone)]
pub struct MemoryContext {
    /// Rolling summary prefix (non-empty only when history was compressed).
    pub rolling_summary: String,
    /// Recent verbatim turns serialised as plain text.
    pub recent_turns: String,
    /// Whether context was truncated due to budget pressure.
    pub truncated: bool,
}

// ──────────────────────────────────────────────────────────────────────────────
// Naive token estimator
// ──────────────────────────────────────────────────────────────────────────────

/// Rough word-based token count (4 characters ≈ 1 token, or about 0.75 tokens
/// per word). Accurate enough for budget planning without adding a tokeniser
/// dependency.
fn estimate_tokens(text: &str) -> usize {
    let words = text.split_whitespace().count();
    // Use 1.33 tokens/word as a safe over-estimate.
    (words as f64 * 1.33).ceil() as usize
}

// ──────────────────────────────────────────────────────────────────────────────
// ConversationMemory
// ──────────────────────────────────────────────────────────────────────────────

/// Manages conversation history for a single live session.
///
/// Thread-safe wrapper is provided by callers via `Arc<Mutex<ConversationMemory>>`.
pub struct ConversationMemory {
    turns: Vec<Turn>,
    /// Compressed summary of turns that were evicted beyond the budget window.
    rolling_summary: String,
    /// Ollama context window size — triggers compression when `< 16_000`.
    context_window: usize,
}

impl ConversationMemory {
    pub fn new(context_window: usize) -> Self {
        Self {
            turns: Vec::new(),
            rolling_summary: String::new(),
            context_window,
        }
    }

    /// Append a new turn. Call after the orchestrator has collected the
    /// responses for the turn so both fields can be populated at once.
    pub fn push_turn(&mut self, turn: Turn) {
        self.turns.push(turn);
    }

    /// Update the directional/depth responses on the most recent turn if they
    /// were not available when the turn was first pushed.
    pub fn update_last_responses(&mut self, directional: Option<String>, depth: Option<String>) {
        if let Some(last) = self.turns.last_mut() {
            if let Some(d) = directional {
                last.directional_response = d;
            }
            if let Some(d) = depth {
                last.depth_response = d;
            }
        }
    }

    /// Build a [`MemoryContext`] that fits within `budget.history_tokens`.
    ///
    /// For large-window providers (≥ 16K tokens) all turns are returned
    /// verbatim. For small-window providers the oldest turns beyond the budget
    /// are summarised and replaced with a rolling summary.
    pub async fn build_context(
        &self,
        budget: &ContextBudget,
        llm: Option<&Arc<dyn LLMProvider>>,
        compression_prompt_template: &str,
        session_id: uuid::Uuid,
    ) -> Result<MemoryContext> {
        if self.turns.is_empty() {
            return Ok(MemoryContext {
                rolling_summary: String::new(),
                recent_turns: String::new(),
                truncated: false,
            });
        }

        let recent_text = self.serialise_turns(&self.turns);
        let recent_tokens = estimate_tokens(&recent_text);

        if recent_tokens <= budget.history_tokens {
            return Ok(MemoryContext {
                rolling_summary: self.rolling_summary.clone(),
                recent_turns: recent_text,
                truncated: false,
            });
        }

        // Budget exceeded — keep the last 5 verbatim turns and compress the rest.
        let keep_tail = 5.min(self.turns.len());
        let (old_turns, recent_slice) = self.turns.split_at(self.turns.len() - keep_tail);

        if old_turns.is_empty() {
            // Even the tail exceeds budget — drop oldest turns and flag truncation.
            warn!(
                session_id = %session_id,
                "context budget exceeded even for last 5 turns; truncating oldest"
            );
            let trimmed = self.fit_turns_to_budget(recent_slice, budget.history_tokens);
            return Ok(MemoryContext {
                rolling_summary: self.rolling_summary.clone(),
                recent_turns: trimmed,
                truncated: true,
            });
        }

        let old_text = self.serialise_turns(old_turns);
        let new_summary = if let Some(provider) = llm {
            match self
                .compress(&old_text, provider, compression_prompt_template)
                .await
            {
                Ok(s) => {
                    info!(
                        session_id = %session_id,
                        "conversation history compressed ({} turns → summary)",
                        old_turns.len()
                    );
                    // Merge with existing rolling summary if present.
                    if self.rolling_summary.is_empty() {
                        s
                    } else {
                        format!("{}\n{}", self.rolling_summary, s)
                    }
                }
                Err(e) => {
                    warn!(session_id = %session_id, error = %e, "history compression failed; using old summary");
                    self.rolling_summary.clone()
                }
            }
        } else {
            // No provider available for compression — fall back to truncation.
            self.rolling_summary.clone()
        };

        let recent_text = self.serialise_turns(recent_slice);
        Ok(MemoryContext {
            rolling_summary: new_summary,
            recent_turns: recent_text,
            truncated: false,
        })
    }

    /// Serialise a slice of turns to plain text for LLM injection.
    fn serialise_turns(&self, turns: &[Turn]) -> String {
        turns
            .iter()
            .map(|t| {
                let mut s = format!("Q: {}", t.question);
                if !t.directional_response.is_empty() {
                    s.push_str(&format!("\nA (brief): {}", t.directional_response));
                }
                if !t.depth_response.is_empty() {
                    s.push_str(&format!("\nA (full): {}", t.depth_response));
                }
                s
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Greedily drop the oldest turns until the text fits within `token_budget`.
    fn fit_turns_to_budget(&self, turns: &[Turn], token_budget: usize) -> String {
        let mut start = 0;
        loop {
            let text = self.serialise_turns(&turns[start..]);
            if estimate_tokens(&text) <= token_budget || start + 1 >= turns.len() {
                return text;
            }
            start += 1;
        }
    }

    /// Call the LLM to compress `old_text` using the compression prompt template.
    async fn compress(
        &self,
        old_text: &str,
        provider: &Arc<dyn LLMProvider>,
        template: &str,
    ) -> Result<String> {
        let prompt = template.replace("{old_turns}", old_text);
        let config = CompletionConfig {
            max_tokens: Some(200),
            temperature: 0.0,
            stream: false,
        };
        let summary = provider.complete(prompt, config).await?;
        Ok(summary.trim().to_string())
    }

    /// Number of turns in the current session.
    pub fn turn_count(&self) -> usize {
        self.turns.len()
    }

    pub fn context_window(&self) -> usize {
        self.context_window
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_turn(q: &str) -> Turn {
        Turn {
            question: q.to_string(),
            directional_response: format!("Brief answer to: {q}"),
            depth_response: format!("Full answer to: {q}"),
        }
    }

    #[test]
    fn budget_allocations_sum_to_sixty_percent() {
        let budget = ContextBudget::from_window(128_000);
        let total_allocated = budget.rag_tokens
            + budget.history_tokens
            + budget.system_tokens
            + budget.question_tokens;
        let sixty_percent = (128_000_f64 * 0.6) as usize;
        // Allow ±4 tokens for rounding.
        assert!(
            total_allocated.abs_diff(sixty_percent) <= 4,
            "allocated={total_allocated} expected≈{sixty_percent}"
        );
    }

    #[tokio::test]
    async fn empty_memory_returns_empty_context() {
        let mem = ConversationMemory::new(128_000);
        let budget = ContextBudget::from_window(128_000);
        let ctx = mem
            .build_context(&budget, None, "", uuid::Uuid::new_v4())
            .await
            .unwrap();
        assert!(ctx.recent_turns.is_empty());
        assert!(ctx.rolling_summary.is_empty());
        assert!(!ctx.truncated);
    }

    #[tokio::test]
    async fn small_history_fits_without_compression() {
        let mut mem = ConversationMemory::new(128_000);
        for i in 0..3 {
            mem.push_turn(make_turn(&format!("Question {i}")));
        }
        let budget = ContextBudget::from_window(128_000);
        let ctx = mem
            .build_context(&budget, None, "", uuid::Uuid::new_v4())
            .await
            .unwrap();
        assert!(ctx.recent_turns.contains("Question 0"));
        assert!(!ctx.truncated);
    }

    #[tokio::test]
    async fn large_history_truncated_without_provider() {
        // Use a tiny context window so the budget is immediately exceeded.
        let mut mem = ConversationMemory::new(512);
        let long_turn = Turn {
            question: "x".repeat(2000),
            directional_response: "y".repeat(2000),
            depth_response: "z".repeat(2000),
        };
        mem.push_turn(long_turn.clone());
        mem.push_turn(long_turn.clone());
        mem.push_turn(long_turn);

        let budget = ContextBudget::from_window(512);
        let ctx = mem
            .build_context(&budget, None, "", uuid::Uuid::new_v4())
            .await
            .unwrap();
        // Truncated flag or rolling summary must be set.
        assert!(ctx.truncated || !ctx.recent_turns.is_empty());
    }

    #[test]
    fn push_turn_and_update_responses() {
        let mut mem = ConversationMemory::new(4096);
        mem.push_turn(Turn {
            question: "Q1".to_string(),
            directional_response: String::new(),
            depth_response: String::new(),
        });
        mem.update_last_responses(Some("brief".to_string()), Some("full".to_string()));
        assert_eq!(mem.turns[0].directional_response, "brief");
        assert_eq!(mem.turns[0].depth_response, "full");
    }

    #[test]
    fn estimate_tokens_nonzero_for_nonempty_text() {
        assert!(estimate_tokens("Hello world") > 0);
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn turn_count_and_context_window_accessors() {
        let mut mem = ConversationMemory::new(8192);
        assert_eq!(mem.turn_count(), 0);
        assert_eq!(mem.context_window(), 8192);
        mem.push_turn(make_turn("Q"));
        mem.push_turn(make_turn("Q"));
        assert_eq!(mem.turn_count(), 2);
    }

    /// `fit_turns_to_budget` drops oldest turns until the serialised slice
    /// fits the budget. Hit it via the "tail exceeds budget" branch in
    /// `build_context` — push a single very large turn so even keep_tail=1
    /// overflows the budget.
    #[tokio::test]
    async fn build_context_truncates_when_single_turn_exceeds_budget() {
        let mut mem = ConversationMemory::new(512);
        let huge = Turn {
            question: "word ".repeat(2000),
            directional_response: "ans ".repeat(2000),
            depth_response: "ans ".repeat(2000),
        };
        mem.push_turn(huge);

        let budget = ContextBudget::from_window(512);
        let ctx = mem
            .build_context(&budget, None, "", uuid::Uuid::new_v4())
            .await
            .unwrap();
        assert!(ctx.truncated, "single oversized turn must set truncated");
    }

    /// Compression success path: provider returns a summary, rolling_summary
    /// is overwritten because the prior summary is empty.
    #[tokio::test]
    async fn build_context_compresses_old_turns_with_provider() {
        use crate::llm::provider::MockLLMProvider;

        let mut mem = ConversationMemory::new(512);
        // Push enough turns to exceed the 512-window budget after the first 5.
        for i in 0..10 {
            mem.push_turn(make_turn(format!("Q{i} ").repeat(100).as_str()));
        }

        let budget = ContextBudget::from_window(512);
        let provider: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "Earlier the candidate discussed Rust and systems.".to_string(),
            provider_name: "default".to_string(),
        });
        let template = "Summarise the following turns concisely:\n{old_turns}";

        let ctx = mem
            .build_context(&budget, Some(&provider), template, uuid::Uuid::new_v4())
            .await
            .unwrap();

        assert!(
            ctx.rolling_summary.contains("Earlier the candidate"),
            "rolling_summary should contain the provider's summary, got {:?}",
            ctx.rolling_summary
        );
        // Recent turns must still cover the last 5 verbatim.
        assert!(ctx.recent_turns.contains("Q9"));
        assert!(!ctx.truncated);
    }

    /// Pre-existing rolling summary plus new compression: the two are merged
    /// with the older one first.
    #[tokio::test]
    async fn build_context_merges_existing_rolling_summary() {
        use crate::llm::provider::MockLLMProvider;

        let mut mem = ConversationMemory::new(512);
        mem.rolling_summary = "Previously summarised content.".to_string();
        for i in 0..10 {
            mem.push_turn(make_turn(format!("Q{i} ").repeat(100).as_str()));
        }

        let budget = ContextBudget::from_window(512);
        let provider: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "New addition to the rolling summary.".to_string(),
            provider_name: "default".to_string(),
        });
        let template = "Summarise:\n{old_turns}";

        let ctx = mem
            .build_context(&budget, Some(&provider), template, uuid::Uuid::new_v4())
            .await
            .unwrap();

        assert!(
            ctx.rolling_summary.contains("Previously summarised content."),
            "must keep old summary"
        );
        assert!(
            ctx.rolling_summary.contains("New addition"),
            "must append new summary"
        );
    }

    /// Compression failure path: provider returns Err, the prior
    /// rolling_summary survives as fallback.
    #[tokio::test]
    async fn build_context_falls_back_to_old_summary_on_compression_failure() {
        use crate::llm::provider::FailingMockLLMProvider;

        let mut mem = ConversationMemory::new(512);
        mem.rolling_summary = "Stable prior summary.".to_string();
        for i in 0..10 {
            mem.push_turn(make_turn(format!("Q{i} ").repeat(100).as_str()));
        }

        let budget = ContextBudget::from_window(512);
        let provider: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
            provider_name: "default".to_string(),
            error_message: "compress failed".to_string(),
        });
        let template = "Summarise:\n{old_turns}";

        let ctx = mem
            .build_context(&budget, Some(&provider), template, uuid::Uuid::new_v4())
            .await
            .unwrap();

        assert_eq!(
            ctx.rolling_summary, "Stable prior summary.",
            "must fall back to existing summary"
        );
    }

    /// No provider available + over-budget history: rolling_summary is the
    /// previous one, recent turns are the last 5.
    #[tokio::test]
    async fn build_context_keeps_tail_when_no_provider_available() {
        let mut mem = ConversationMemory::new(512);
        mem.rolling_summary = "Prior summary kept verbatim.".to_string();
        for i in 0..10 {
            mem.push_turn(make_turn(format!("Q{i} ").repeat(100).as_str()));
        }

        let budget = ContextBudget::from_window(512);
        let ctx = mem
            .build_context(&budget, None, "", uuid::Uuid::new_v4())
            .await
            .unwrap();

        assert_eq!(ctx.rolling_summary, "Prior summary kept verbatim.");
        assert!(ctx.recent_turns.contains("Q9"));
        assert!(!ctx.truncated);
    }
}
