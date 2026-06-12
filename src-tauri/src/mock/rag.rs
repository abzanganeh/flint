//! Per-question RAG retrieval for mock interview suggested answers and coach.
//!
//! Each call embeds the current question and queries two sources in parallel:
//!   1. Session RAG — user's resume, JD, and prep research (personalised).
//!   2. Global knowledge base — static domain packs (algorithms, system design, etc.).
//!
//! Results from both stores are merged and re-ranked by cosine similarity so
//! the LLM receives the most relevant chunks regardless of source.

use uuid::Uuid;

use crate::digest::Digest;
use crate::interfaces::vector::{ScoredChunk, VectorInterface};
use crate::knowledge::{GlobalKnowledgeBase, PackId};
use crate::rag::embedder::Embedder;

/// Retrieve the top `top_k` chunks relevant to `question`.
///
/// Queries two sources and merges the results:
///   - Session RAG: up to `top_k` personalised chunks (resume, JD, prep notes).
///   - Global KB: up to `top_k / 2` domain-knowledge chunks from the role packs.
///
/// The union is sorted by cosine similarity and truncated to `top_k`, so the
/// best chunks from either source rise to the top.  When `global_kb` is `None`
/// (embedder not ready, packs empty) only session RAG is returned.
pub async fn query_mock_rag(
    session_id: Uuid,
    question: &str,
    embedder: &Embedder,
    session_store: &dyn VectorInterface,
    global_kb: Option<(&GlobalKnowledgeBase, &[PackId])>,
    top_k: usize,
) -> Vec<ScoredChunk> {
    let query_vec = match embedder.embed_one(question) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    // Session RAG: user-specific context (resume, JD, prep notes).
    // Full top_k budget — personalised content is most valuable.
    let mut session_chunks = session_store
        .query(session_id, &query_vec, top_k.max(1))
        .await
        .unwrap_or_default();

    // Global knowledge base: static domain packs.
    let kb_top_k = (top_k / 2).max(2);
    let kb_chunks = if let Some((kb, packs)) = global_kb {
        kb.query_packs(packs, &query_vec, kb_top_k).await
    } else {
        vec![]
    };

    if kb_chunks.is_empty() {
        // No KB results — return session chunks directly.
        session_chunks.truncate(top_k);
        return session_chunks;
    }

    // Merge + re-rank: combine both sets, sort by score, deduplicate by chunk id.
    session_chunks.extend(kb_chunks);
    session_chunks
        .sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    let mut seen = std::collections::HashSet::new();
    session_chunks.retain(|c| seen.insert(c.chunk.id));
    session_chunks.truncate(top_k);
    session_chunks
}

/// Structured digest fields for grounding suggested answers in the candidate's profile.
pub fn format_digest_context(digest: &Digest) -> String {
    let skills = digest.key_skills.join(", ");
    let avoid = if digest.topics_to_avoid.is_empty() {
        "none".to_string()
    } else {
        digest.topics_to_avoid.join(", ")
    };
    format!(
        "Role: {}\nCompany target: {}\nDomain: {}\nSeniority: {}\nKey skills: {}\nTopics to avoid: {}",
        digest.role, digest.company, digest.domain, digest.seniority, skills, avoid
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_digest_context_includes_skills() {
        let digest = Digest {
            role: "AI Engineer".into(),
            company: "Acme".into(),
            domain: "fintech".into(),
            key_skills: vec!["Rust".into(), "RAG".into()],
            seniority: "senior".into(),
            likely_questions: vec![],
            topics_to_avoid: vec!["salary".into()],
        };
        let ctx = format_digest_context(&digest);
        assert!(ctx.contains("Rust"));
        assert!(ctx.contains("salary"));
    }
}
