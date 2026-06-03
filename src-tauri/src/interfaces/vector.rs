//! Vector store trait and associated types (design doc §27).
//!
//! Implementations swap without touching the orchestrator or RAG pipeline.
//! The production implementation is `SqliteVecStore` in `rag::store`.
//! Session isolation is a hard invariant — every query is scoped to a single
//! session_id and must never return chunks from another session.

#![allow(dead_code)]

use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

/// A single text chunk with its precomputed embedding, ready for ingestion.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Stable identifier for this chunk (generated at chunking time).
    pub id: Uuid,
    /// Raw text content.
    pub text: String,
    /// bge-small-en-v1.5 embedding (384 dimensions, unit-norm).
    pub embedding: Vec<f32>,
    /// Session this chunk belongs to.
    pub session_id: Uuid,
}

/// A retrieved chunk together with its similarity score (0.0–1.0).
///
/// For unit-norm embeddings from bge-small-en-v1.5 the score equals the
/// cosine similarity: `score = 1.0 - (l2_distance² / 2.0)`.
#[derive(Debug, Clone)]
pub struct ScoredChunk {
    pub chunk: Chunk,
    /// Cosine similarity in [−1, 1]; typically [0, 1] for semantic search.
    pub score: f32,
}

/// Local vector store contract.
///
/// All methods are scoped to a `session_id`. Cross-session queries must
/// never be issued — implementors may assume caller isolation.
///
/// `chunk_count` is synchronous because it is called on the hot path
/// (pre-warm cache decision) and must not add async overhead.
#[async_trait]
pub trait VectorInterface: Send + Sync {
    /// Ingest a batch of pre-embedded chunks into the session's vector table.
    async fn ingest(&self, session_id: Uuid, chunks: Vec<Chunk>) -> Result<()>;

    /// Retrieve the `top_k` most similar chunks (by cosine similarity) to the
    /// given query embedding. Returns an empty vec if no chunks are stored.
    async fn query(
        &self,
        session_id: Uuid,
        embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<ScoredChunk>>;

    /// Drop all data for this session (vector table + metadata rows).
    async fn delete_session(&self, session_id: Uuid) -> Result<()>;

    /// Number of chunks stored for this session. Returns 0 if the session
    /// table does not exist.
    fn chunk_count(&self, session_id: Uuid) -> usize;
}
