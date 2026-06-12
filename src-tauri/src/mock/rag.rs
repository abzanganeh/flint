//! Per-question RAG retrieval for mock interview suggested answers and coach.

use uuid::Uuid;

use crate::digest::Digest;
use crate::interfaces::vector::{ScoredChunk, VectorInterface};
use crate::rag::embedder::Embedder;

/// Embed `question` and retrieve the top matching resume/session chunks.
pub async fn query_mock_rag(
    session_id: Uuid,
    question: &str,
    embedder: &Embedder,
    store: &dyn VectorInterface,
    top_k: usize,
) -> Vec<ScoredChunk> {
    let query_vec = embedder.embed_one(question).unwrap_or_default();
    store
        .query(session_id, &query_vec, top_k)
        .await
        .unwrap_or_default()
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
