#![allow(dead_code)]

//! MMR (Maximal Marginal Relevance) retriever.
//!
//! Fetches `2 * top_k` candidates from the vector store, then greedily
//! selects `top_k` results that balance relevance and diversity using the
//! MMR criterion. Inter-chunk similarity is computed as the true dot
//! product of the stored embeddings (`SqliteVecStore` persists and returns
//! the raw embedding alongside each chunk); a geometric-mean fallback on
//! query scores is used only when embeddings are missing.
//!
//! Pairs with cosine similarity ≥ [`NEAR_DUPLICATE_THRESHOLD`] are treated
//! as exact duplicates and are hard-excluded from selection — vanilla MMR
//! with λ > 0.5 cannot otherwise suppress perfect duplicates.

use anyhow::Result;
use uuid::Uuid;

use crate::interfaces::vector::{
    PromptChunks, ScoredChunk, VectorInterface, QA_RETRIEVAL_THRESHOLD,
};

/// Dot product of two equal-length vectors.
///
/// For unit-norm embeddings (bge-small-en-v1.5) this equals cosine similarity.
fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Inter-chunk similarity for the MMR diversity term.
///
/// Uses the real dot product when both chunks have non-empty stored embeddings
/// (the normal case — `SqliteVecStore` persists and returns embeddings).
/// Falls back to the geometric-mean approximation when embeddings are absent.
fn inter_chunk_sim(
    candidate: &crate::interfaces::vector::ScoredChunk,
    selected: &crate::interfaces::vector::ScoredChunk,
) -> f32 {
    let ca = &candidate.chunk.embedding;
    let cb = &selected.chunk.embedding;
    if !ca.is_empty() && ca.len() == cb.len() {
        dot_product(ca, cb)
    } else {
        (candidate.score * selected.score).max(0.0).sqrt()
    }
}

/// Cosine similarity threshold above which two chunks are treated as
/// near-exact duplicates and the second is hard-excluded from selection.
///
/// Standard MMR cannot suppress perfect duplicates with lambda > 0.5 because
/// the relevance gain (λ × 1.0) always exceeds the diversity penalty
/// ((1−λ) × 1.0) when both scores equal 1.0. An explicit dedup threshold is
/// the canonical MMR extension for this case.
const NEAR_DUPLICATE_THRESHOLD: f32 = 0.99;

/// Retrieve the top-k most relevant and diverse chunks for a query using MMR.
///
/// # Algorithm
///
/// 1. Fetch `2 * top_k` candidates from the vector store.
/// 2. Greedily select candidates that maximise:
///    `lambda * relevance - (1 - lambda) * max_similarity_to_already_selected`
/// 3. Return the selected chunks sorted by score descending.
///
/// # Parameters
///
/// - `store`: the vector store to query.
/// - `session_id`: session scope — must never cross sessions.
/// - `query_embedding`: the embedded query vector (unused directly; passed
///   through to the store for candidate retrieval).
/// - `top_k`: maximum number of results to return.
/// - `lambda`: trade-off between relevance (1.0) and diversity (0.0).
pub async fn retrieve(
    store: &dyn VectorInterface,
    session_id: Uuid,
    query_embedding: &[f32],
    top_k: usize,
    lambda: f32,
) -> Result<Vec<ScoredChunk>> {
    if top_k == 0 {
        return Ok(vec![]);
    }

    let candidates = store.query(session_id, query_embedding, 2 * top_k).await?;

    if candidates.is_empty() {
        return Ok(vec![]);
    }

    // Single-candidate fast path.
    if candidates.len() == 1 {
        return Ok(candidates);
    }

    // Track which candidates are still available. `None` means already selected.
    let mut remaining: Vec<Option<ScoredChunk>> = candidates.into_iter().map(Some).collect();
    let mut selected: Vec<ScoredChunk> = Vec::with_capacity(top_k);

    while selected.len() < top_k {
        let mut best_idx: Option<usize> = None;
        let mut best_mmr = f32::NEG_INFINITY;

        for (i, slot) in remaining.iter().enumerate() {
            let Some(candidate) = slot else { continue };

            let max_sim_to_selected = if selected.is_empty() {
                0.0_f32
            } else {
                selected
                    .iter()
                    .map(|sel| inter_chunk_sim(candidate, sel))
                    .fold(0.0_f32, f32::max)
            };

            // Hard-exclude near-exact duplicates. Standard MMR with lambda > 0.5
            // cannot suppress a perfect duplicate (relevance gain always exceeds
            // diversity penalty for identical chunks). See NEAR_DUPLICATE_THRESHOLD.
            let mmr_score = if max_sim_to_selected >= NEAR_DUPLICATE_THRESHOLD {
                f32::NEG_INFINITY
            } else {
                lambda * candidate.score - (1.0 - lambda) * max_sim_to_selected
            };

            if mmr_score > best_mmr {
                best_mmr = mmr_score;
                best_idx = Some(i);
            }
        }

        match best_idx {
            Some(idx) => {
                let chosen = remaining[idx].take().expect("just confirmed Some");
                selected.push(chosen);
            }
            None => break,
        }
    }

    // Sort by original relevance score, highest first.
    selected.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    tracing::debug!(
        top_k,
        lambda,
        candidates = remaining.len(),
        selected = selected.len(),
        "MMR retrieval complete"
    );

    Ok(selected)
}

/// Slot-allocated retrieval for prompt construction (design doc §16.5.3).
///
/// Fills two independent slot budgets from the dual vector stores:
///
/// - **Context slots (6 max):** JD, resume, notes, user-provided Q&A. Always
///   filled first. Subject to MMR de-duplication with λ = 0.7.
/// - **Q&A slots (2 max):** AI-generated Q&A pairs from past turns. Only
///   included when cosine similarity ≥ [`QA_RETRIEVAL_THRESHOLD`] (0.80).
///   Not subject to MMR — they are appended after context.
///
/// If the Q&A store is empty or no pairs clear the threshold, only context
/// chunks are returned and [`PromptChunks::has_qa`] is false (prompt label
/// is suppressed).
pub async fn retrieve_for_prompt(
    store: &dyn VectorInterface,
    session_id: Uuid,
    query_embedding: &[f32],
) -> Result<PromptChunks> {
    const CONTEXT_SLOTS: usize = 6;
    const QA_SLOTS: usize = 2;
    const QA_CANDIDATES: usize = 5;
    const MMR_LAMBDA: f32 = 0.7;

    // Context retrieval — MMR via the existing retrieve() function.
    let context = retrieve(
        store,
        session_id,
        query_embedding,
        CONTEXT_SLOTS,
        MMR_LAMBDA,
    )
    .await?;

    // Q&A retrieval — raw top-k then threshold filter, no MMR (past answers
    // are short and dissimilar from each other; MMR would not add value).
    let qa_candidates = store
        .query_qa(session_id, query_embedding, QA_CANDIDATES)
        .await?;

    let qa: Vec<ScoredChunk> = qa_candidates
        .into_iter()
        .filter(|r| r.score >= QA_RETRIEVAL_THRESHOLD)
        .take(QA_SLOTS)
        .collect();

    tracing::debug!(
        session_id = %session_id,
        context_chunks = context.len(),
        qa_chunks = qa.len(),
        "slot-allocated retrieval complete",
    );

    Ok(PromptChunks { context, qa })
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use uuid::Uuid;

    use super::*;
    use crate::interfaces::vector::Chunk;
    use crate::rag::embedder::Embedder;
    use crate::rag::store::SqliteVecStore;

    static EMBEDDER: OnceLock<Option<Embedder>> = OnceLock::new();

    fn embedder() -> Option<&'static Embedder> {
        EMBEDDER.get_or_init(Embedder::new_if_cached).as_ref()
    }

    macro_rules! require_embedder {
        () => {
            match embedder() {
                Some(e) => e,
                None => {
                    tracing::warn!(
                        "SKIP retriever test: fastembed model not cached (offline or rate-limited)"
                    );
                    return;
                }
            }
        };
    }

    fn make_store() -> SqliteVecStore {
        SqliteVecStore::new(":memory:").expect("store should open")
    }

    fn make_chunk(text: &str, session_id: Uuid, emb: &Embedder) -> Chunk {
        let embedding = emb.embed_one(text).expect("embed failed");
        Chunk {
            id: Uuid::new_v4(),
            text: text.to_string(),
            embedding,
            session_id,
        }
    }

    #[tokio::test]
    async fn test_retrieve_empty_store_returns_empty() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = require_embedder!();
        let query = emb.embed_one("anything").unwrap();
        let results = retrieve(&store, session, &query, 5, 0.7).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_retrieve_top_k_zero_returns_empty() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = require_embedder!();

        let chunk = make_chunk("some text about Rust", session, emb);
        store.ingest(session, vec![chunk]).await.unwrap();

        let query = emb.embed_one("Rust").unwrap();
        let results = retrieve(&store, session, &query, 0, 0.7).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_retrieve_respects_top_k() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = require_embedder!();

        let texts = [
            "distributed systems and fault tolerance",
            "machine learning and neural networks",
            "database indexing and query optimisation",
            "operating system scheduling algorithms",
            "cryptography and public key infrastructure",
            "web server architecture and HTTP",
            "compiler design and abstract syntax trees",
            "containerisation with Docker and Kubernetes",
        ];
        let chunks: Vec<Chunk> = texts.iter().map(|t| make_chunk(t, session, emb)).collect();
        store.ingest(session, chunks).await.unwrap();

        let query = emb.embed_one("computer science fundamentals").unwrap();
        let results = retrieve(&store, session, &query, 3, 0.7).await.unwrap();

        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_mmr_removes_near_duplicates() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = require_embedder!();

        let duplicate_text = "machine learning and deep neural networks for image classification";
        let diverse_texts = [
            "database query optimisation and B-tree indexes",
            "network protocols and TCP/IP stack",
            "operating system virtual memory management",
            "cryptographic hash functions and digital signatures",
        ];

        // Ingest the duplicate twice.
        let dup1 = make_chunk(duplicate_text, session, emb);
        let dup2 = make_chunk(duplicate_text, session, emb);
        let diverse_chunks: Vec<Chunk> = diverse_texts
            .iter()
            .map(|t| make_chunk(t, session, emb))
            .collect();

        store.ingest(session, vec![dup1]).await.unwrap();
        store.ingest(session, vec![dup2]).await.unwrap();
        store.ingest(session, diverse_chunks).await.unwrap();

        let query = emb.embed_one(duplicate_text).unwrap();
        let results = retrieve(&store, session, &query, 4, 0.7).await.unwrap();

        // Count how many results have the duplicate text.
        let dup_count = results
            .iter()
            .filter(|r| r.chunk.text == duplicate_text)
            .count();

        assert_eq!(results.len(), 4);
        assert!(
            dup_count <= 1,
            "MMR should suppress the near-duplicate; got {dup_count} copies in results"
        );
    }

    #[tokio::test]
    async fn test_retrieve_returns_scores_sorted_descending() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = require_embedder!();

        let texts = [
            "Rust ownership and the borrow checker",
            "async/await in Rust with Tokio",
            "Python data science with pandas",
            "JavaScript React component lifecycle",
            "SQL window functions and CTEs",
        ];
        let chunks: Vec<Chunk> = texts.iter().map(|t| make_chunk(t, session, emb)).collect();
        store.ingest(session, chunks).await.unwrap();

        let query = emb.embed_one("Rust programming language").unwrap();
        let results = retrieve(&store, session, &query, 5, 0.7).await.unwrap();

        assert!(!results.is_empty());
        for window in results.windows(2) {
            assert!(
                window[0].score >= window[1].score,
                "scores must be non-increasing: {} < {}",
                window[0].score,
                window[1].score
            );
        }
    }
}
