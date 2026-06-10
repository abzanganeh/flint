//! Prep-mode research: RAG-first answers with optional web search fallback.

pub mod tavily;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::warn;

use crate::interfaces::vector::ScoredChunk;
use crate::interfaces::web_search::{WebSearchProvider, WebSearchResult};
use crate::llm::provider::{CompletionConfig, LLMProvider};

/// Minimum top-chunk cosine score before we treat RAG as sufficient for prep research.
pub const RAG_SUFFICIENCY_THRESHOLD: f32 = 0.45;

/// How a research turn was answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResearchSource {
    Rag,
    Web,
    RagAndWeb,
    None,
}

impl ResearchSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rag => "rag",
            Self::Web => "web",
            Self::RagAndWeb => "rag_and_web",
            Self::None => "none",
        }
    }
}

/// Outcome of one prep research turn.
#[derive(Debug, Clone)]
pub struct ResearchTurnOutcome {
    pub response: String,
    pub source: ResearchSource,
    pub rag_citations: Vec<String>,
    pub web_sources: Vec<WebSearchResult>,
}

/// Whether retrieved chunks are strong enough to answer without web search.
pub fn rag_is_sufficient(chunks: &[ScoredChunk]) -> bool {
    if chunks.is_empty() {
        return false;
    }
    let max_score = chunks
        .iter()
        .map(|c| c.score)
        .fold(f32::NEG_INFINITY, f32::max);
    max_score >= RAG_SUFFICIENCY_THRESHOLD
}

fn prompts_base_dir() -> PathBuf {
    std::env::var("FLINT_PROMPTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("prompts")
        })
}

fn load_prompt(relative: &str) -> Result<String> {
    let path = prompts_base_dir().join(relative);
    std::fs::read_to_string(&path).with_context(|| format!("load prompt {relative}"))
}

fn format_rag_block(chunks: &[ScoredChunk]) -> String {
    chunks
        .iter()
        .enumerate()
        .map(|(i, c)| format!("[{}] {}", i + 1, c.chunk.text))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_web_block(results: &[WebSearchResult]) -> String {
    results
        .iter()
        .enumerate()
        .map(|(i, r)| format!("[{}] {} ({})\n{}", i + 1, r.title, r.url, r.snippet))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn fill_template(template: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (key, value) in pairs {
        out = out.replace(&format!("{{{key}}}"), value);
    }
    out
}

/// Run one prep research turn: RAG when sufficient, otherwise web (if configured).
pub async fn run_prep_research_turn(
    question: &str,
    rag_chunks: Vec<ScoredChunk>,
    llm: Arc<dyn LLMProvider>,
    web: Option<Arc<dyn WebSearchProvider>>,
) -> Result<ResearchTurnOutcome> {
    let rag_citations: Vec<String> = rag_chunks.iter().map(|c| c.chunk.text.clone()).collect();

    if rag_is_sufficient(&rag_chunks) {
        let rag_block = format_rag_block(&rag_chunks);
        let template = load_prompt("research/rag_only.txt")?;
        let prompt = fill_template(
            &template,
            &[("pasted_context", &rag_block), ("question", question)],
        );
        let response = llm
            .complete(
                prompt,
                CompletionConfig {
                    max_tokens: Some(400),
                    temperature: 0.1,
                    stream: false,
                },
            )
            .await?;
        return Ok(ResearchTurnOutcome {
            response,
            source: ResearchSource::Rag,
            rag_citations,
            web_sources: Vec::new(),
        });
    }

    let Some(web_provider) = web else {
        return Ok(ResearchTurnOutcome {
            response: "I don't have enough information in your pasted context to answer that. \
                Add a Tavily API key in Settings → API Keys to search the web during rehearsal prep."
                .to_string(),
            source: ResearchSource::None,
            rag_citations,
            web_sources: Vec::new(),
        });
    };

    let web_sources = match web_provider.search(question, 5).await {
        Ok(results) if !results.is_empty() => results,
        Ok(_) => {
            return Ok(ResearchTurnOutcome {
                response: "Web search returned no results for that question. Try rephrasing or \
                    paste background notes into Technical Prep first."
                    .to_string(),
                source: ResearchSource::None,
                rag_citations,
                web_sources: Vec::new(),
            });
        }
        Err(e) => {
            warn!(error = %e, "prep web search failed");
            return Ok(ResearchTurnOutcome {
                response: format!(
                    "Web search failed ({e}). Check your Tavily API key in Settings and try again."
                ),
                source: ResearchSource::None,
                rag_citations,
                web_sources: Vec::new(),
            });
        }
    };

    let rag_block = if rag_chunks.is_empty() {
        "(none)".to_string()
    } else {
        format_rag_block(&rag_chunks)
    };
    let web_block = format_web_block(&web_sources);
    let template = load_prompt("research/web_synthesis.txt")?;
    let prompt = fill_template(
        &template,
        &[
            ("web_results", &web_block),
            ("pasted_context", &rag_block),
            ("question", question),
        ],
    );

    let response = llm
        .complete(
            prompt,
            CompletionConfig {
                max_tokens: Some(500),
                temperature: 0.2,
                stream: false,
            },
        )
        .await?;

    let source = if rag_chunks.is_empty() {
        ResearchSource::Web
    } else {
        ResearchSource::RagAndWeb
    };

    Ok(ResearchTurnOutcome {
        response,
        source,
        rag_citations,
        web_sources,
    })
}

/// Build a labelled blob for appending web research into session context / RAG.
pub fn format_research_append_block(
    question: &str,
    answer: &str,
    web_sources: &[WebSearchResult],
) -> String {
    let mut block = format!("[WEB RESEARCH — {question}]\n{answer}");
    if !web_sources.is_empty() {
        block.push_str("\n\nSources:\n");
        for source in web_sources {
            block.push_str(&format!("- {} ({})\n", source.title, source.url));
        }
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    use crate::interfaces::vector::{Chunk, ScoredChunk};

    fn scored(text: &str, score: f32) -> ScoredChunk {
        ScoredChunk {
            chunk: Chunk {
                id: Uuid::new_v4(),
                text: text.to_string(),
                embedding: vec![],
                session_id: Uuid::new_v4(),
            },
            score,
        }
    }

    #[test]
    fn rag_is_sufficient_when_top_score_meets_threshold() {
        assert!(!rag_is_sufficient(&[]));
        assert!(!rag_is_sufficient(&[scored("weak", 0.2)]));
        // just below threshold
        assert!(!rag_is_sufficient(&[scored("near", RAG_SUFFICIENCY_THRESHOLD - 0.001)]));
        // exactly at threshold
        assert!(rag_is_sufficient(&[scored("exact", RAG_SUFFICIENCY_THRESHOLD)]));
        // above threshold
        assert!(rag_is_sufficient(&[scored("strong", 0.5)]));
    }

    #[test]
    fn format_research_append_block_includes_sources() {
        let block = format_research_append_block(
            "What is NIM?",
            "NIM is a deployment microservice.",
            &[WebSearchResult {
                title: "NVIDIA NIM".to_string(),
                url: "https://example.com".to_string(),
                snippet: "snippet".to_string(),
            }],
        );
        assert!(block.contains("[WEB RESEARCH"));
        assert!(block.contains("https://example.com"));
    }
}
