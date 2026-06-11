//! Prep-mode research: RAG-first answers with optional web search fallback.

pub mod tavily;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tauri::{AppHandle, Runtime};
use tracing::warn;

use crate::interfaces::vector::ScoredChunk;
use crate::interfaces::web_search::{WebSearchProvider, WebSearchResult};
use crate::llm::failover::FailoverManager;
use crate::llm::provider::CompletionConfig;

/// Minimum top-chunk cosine score before we treat RAG as sufficient for prep research.
pub const RAG_SUFFICIENCY_THRESHOLD: f32 = 0.45;

const WEB_FACT_HINTS: &[&str] = &[
    "aum",
    "assets under management",
    "market cap",
    "stock price",
    "share price",
    "revenue",
    "earnings",
    "how much",
    "how many employees",
    "number of employees",
    "ceo",
    "cfo",
    "who is the ceo",
    "press release",
    "recent news",
    "latest news",
    "news about",
    "headquarters",
    "founded in",
    "net worth",
];

const MISSING_CONTEXT_PATTERNS: &[&str] = &[
    "not in the context",
    "is not in the context",
    "don't have enough",
    "do not have enough",
    "don't have access",
    "do not have access",
    "not publicly",
    "web search",
    "tavily",
    "technical prep",
    "i don't have",
    "i do not have",
    "cannot find",
    "can't find",
    "no information in",
    "not enough information",
    "suggest what the user should paste",
    "run a web search",
];

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

const NO_TAVILY_MESSAGE: &str =
    "I don't have enough information in your pasted context to answer that. \
    Add a Tavily API key in Settings → API Keys to search the web during rehearsal prep.";

/// Questions that need live/public facts (AUM, CEO, news) — not answerable from pasted JD alone.
pub fn question_needs_web_research(question: &str) -> bool {
    let q = question.to_lowercase();
    WEB_FACT_HINTS.iter().any(|hint| q.contains(hint))
}

/// True when an LLM answer from RAG-only path admits the context is insufficient.
pub fn response_indicates_missing_context(response: &str) -> bool {
    let lower = response.to_lowercase();
    MISSING_CONTEXT_PATTERNS
        .iter()
        .any(|pat| lower.contains(pat))
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

async fn synthesize_from_rag<R: Runtime>(
    question: &str,
    rag_chunks: &[ScoredChunk],
    failover: &FailoverManager,
    app: &AppHandle<R>,
) -> Result<String> {
    let rag_block = format_rag_block(rag_chunks);
    let template = load_prompt("research/rag_only.txt")?;
    let prompt = fill_template(
        &template,
        &[("pasted_context", &rag_block), ("question", question)],
    );
    failover
        .complete(
            prompt,
            CompletionConfig {
                max_tokens: Some(400),
                temperature: 0.1,
                stream: false,
            },
            app,
            500,
        )
        .await
}

async fn synthesize_from_web<R: Runtime>(
    question: &str,
    rag_chunks: &[ScoredChunk],
    web_sources: &[WebSearchResult],
    failover: &FailoverManager,
    app: &AppHandle<R>,
) -> Result<String> {
    let rag_block = if rag_chunks.is_empty() {
        "(none)".to_string()
    } else {
        format_rag_block(rag_chunks)
    };
    let web_block = format_web_block(web_sources);
    let template = load_prompt("research/web_synthesis.txt")?;
    let prompt = fill_template(
        &template,
        &[
            ("web_results", &web_block),
            ("pasted_context", &rag_block),
            ("question", question),
        ],
    );
    failover
        .complete(
            prompt,
            CompletionConfig {
                max_tokens: Some(500),
                temperature: 0.2,
                stream: false,
            },
            app,
            700,
        )
        .await
}

async fn run_web_search_turn<R: Runtime>(
    question: &str,
    rag_chunks: Vec<ScoredChunk>,
    rag_citations: Vec<String>,
    web_provider: Arc<dyn WebSearchProvider>,
    failover: &FailoverManager,
    app: &AppHandle<R>,
) -> Result<ResearchTurnOutcome> {
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

    let response = synthesize_from_web(question, &rag_chunks, &web_sources, failover, app).await?;

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

/// Run one prep research turn: RAG when sufficient, otherwise web (if configured).
pub async fn run_prep_research_turn<R: Runtime>(
    question: &str,
    rag_chunks: Vec<ScoredChunk>,
    failover: Arc<FailoverManager>,
    web: Option<Arc<dyn WebSearchProvider>>,
    app: AppHandle<R>,
) -> Result<ResearchTurnOutcome> {
    let rag_citations: Vec<String> = rag_chunks.iter().map(|c| c.chunk.text.clone()).collect();
    let needs_web = question_needs_web_research(question);
    let strong_rag = rag_is_sufficient(&rag_chunks);

    // External-fact questions (AUM, CEO, news) skip RAG-only even when JD chunks score high.
    if needs_web {
        if let Some(web_provider) = web {
            return run_web_search_turn(
                question,
                rag_chunks,
                rag_citations,
                web_provider,
                &failover,
                &app,
            )
            .await;
        }
        return Ok(ResearchTurnOutcome {
            response: NO_TAVILY_MESSAGE.to_string(),
            source: ResearchSource::None,
            rag_citations,
            web_sources: Vec::new(),
        });
    }

    if strong_rag {
        let response = synthesize_from_rag(question, &rag_chunks, &failover, &app).await?;
        if !response_indicates_missing_context(&response) {
            return Ok(ResearchTurnOutcome {
                response,
                source: ResearchSource::Rag,
                rag_citations,
                web_sources: Vec::new(),
            });
        }
        // RAG matched JD/profile but couldn't answer — try web if Tavily is configured.
        if let Some(web_provider) = web {
            return run_web_search_turn(
                question,
                rag_chunks,
                rag_citations,
                web_provider,
                &failover,
                &app,
            )
            .await;
        }
        return Ok(ResearchTurnOutcome {
            response: NO_TAVILY_MESSAGE.to_string(),
            source: ResearchSource::None,
            rag_citations,
            web_sources: Vec::new(),
        });
    }

    let Some(web_provider) = web else {
        return Ok(ResearchTurnOutcome {
            response: NO_TAVILY_MESSAGE.to_string(),
            source: ResearchSource::None,
            rag_citations,
            web_sources: Vec::new(),
        });
    };

    run_web_search_turn(
        question,
        rag_chunks,
        rag_citations,
        web_provider,
        &failover,
        &app,
    )
    .await
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
        assert!(!rag_is_sufficient(&[scored(
            "near",
            RAG_SUFFICIENCY_THRESHOLD - 0.001
        )]));
        assert!(rag_is_sufficient(&[scored(
            "exact",
            RAG_SUFFICIENCY_THRESHOLD
        )]));
        assert!(rag_is_sufficient(&[scored("strong", 0.5)]));
    }

    #[test]
    fn question_needs_web_research_for_aum() {
        assert!(question_needs_web_research(
            "What is Fisher Investments' AUM in 2025?"
        ));
        assert!(!question_needs_web_research(
            "Describe a challenging project with technical debt"
        ));
    }

    #[test]
    fn question_needs_web_research_for_recent_news() {
        assert!(question_needs_web_research(
            "Recent news about Fisher Investments AI team?"
        ));
    }

    #[test]
    fn response_indicates_missing_context_detects_rag_only_prompt() {
        assert!(response_indicates_missing_context(
            "The answer is not in the context. Run a web search if Tavily is configured."
        ));
        assert!(!response_indicates_missing_context(
            "Fisher Investments is a fee-only fiduciary firm."
        ));
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
