//! Vector store trait and associated types (design doc §27, §16.5).
//!
//! Phase 9 introduces a dual-store model per session:
//!   - **Context store** (`vec_chunks_<id>`): JD, resume, notes, and
//!     user-provided Q&A pairs. Immutable after ingest. Always filled first.
//!   - **Q&A store** (`vec_qa_<id>`): quality-gated AI-generated Q&A pairs
//!     from rehearsal and live turns (confidence ≥ 0.65 only). Grows during
//!     the session. Used for slot-allocated supplemental retrieval.
//!
//! The old `ingest` / `query` methods are kept as aliases to their `_context`
//! equivalents so existing call sites compile without modification.
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

/// Slot-allocated chunks ready for prompt injection (design doc §16.5.3).
///
/// Context slots are always filled (6 max, MMR-de-duplicated). Q&A slots are
/// supplemental and may be empty if the Q&A store has no entries above the
/// 0.80 relevance threshold.
#[derive(Debug, Default)]
pub struct PromptChunks {
    /// Up to 6 context chunks (JD, resume, notes). Always filled first.
    pub context: Vec<ScoredChunk>,
    /// Up to 2 Q&A pair chunks (past rehearsal/live answers, score ≥ 0.80).
    pub qa: Vec<ScoredChunk>,
}

impl PromptChunks {
    /// Total chunk count across both stores.
    pub fn total(&self) -> usize {
        self.context.len() + self.qa.len()
    }

    /// True when at least one Q&A chunk is present (controls prompt label injection).
    pub fn has_qa(&self) -> bool {
        !self.qa.is_empty()
    }
}

/// Minimum cosine similarity for a Q&A chunk to be included in retrieval.
///
/// Below this threshold the past answer is too weakly related to the current
/// question to be useful — it would add noise rather than signal.
pub const QA_RETRIEVAL_THRESHOLD: f32 = 0.80;

/// Minimum confidence score for a generated answer to be embedded into the
/// Q&A store. Maps to the green/blue confidence band boundary.
pub const QA_EMBED_CONFIDENCE_THRESHOLD: f32 = 0.65;

/// Local vector store contract (dual-store, design doc §16.5).
///
/// All methods are scoped to a `session_id`. Cross-session queries must
/// never be issued — implementors may assume caller isolation.
///
/// `chunk_count` is synchronous because it is called on the hot path
/// (pre-warm cache decision) and must not add async overhead.
#[async_trait]
pub trait VectorInterface: Send + Sync {
    // ── Context store ───────────────────────────────────────────────────────

    /// Ingest a batch of pre-embedded chunks into the session's context store.
    ///
    /// Use for: JD, resume, company notes, web research, and user-provided
    /// Q&A pairs (trusted ground truth — not AI-generated).
    async fn ingest_context(&self, session_id: Uuid, chunks: Vec<Chunk>) -> Result<()>;

    /// Retrieve the `top_k` most similar chunks from the context store.
    ///
    /// Returns an empty vec if no chunks are stored for this session.
    async fn query_context(
        &self,
        session_id: Uuid,
        embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<ScoredChunk>>;

    // ── Q&A store ───────────────────────────────────────────────────────────

    /// Ingest a quality-gated AI-generated Q&A pair into the session's Q&A
    /// store.
    ///
    /// Callers are responsible for the quality gate: only call this when
    /// confidence ≥ [`QA_EMBED_CONFIDENCE_THRESHOLD`]. The store does not
    /// enforce the threshold — the decision belongs to the orchestrator.
    async fn ingest_qa(&self, session_id: Uuid, chunks: Vec<Chunk>) -> Result<()>;

    /// Retrieve the `top_k` most similar Q&A chunks.
    ///
    /// Returns an empty vec if the Q&A store is empty for this session.
    /// Callers should filter results by [`QA_RETRIEVAL_THRESHOLD`] before
    /// injecting into the prompt.
    async fn query_qa(
        &self,
        session_id: Uuid,
        embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<ScoredChunk>>;

    // ── Session lifecycle ───────────────────────────────────────────────────

    /// Drop all data for this session — both context and Q&A tables.
    async fn delete_session(&self, session_id: Uuid) -> Result<()>;

    /// Number of chunks stored in the context store for this session.
    ///
    /// Returns 0 if the session table does not exist.
    fn chunk_count(&self, session_id: Uuid) -> usize;

    // ── Backward-compat aliases ─────────────────────────────────────────────

    /// Alias for [`ingest_context`]. Kept for call sites that pre-date Phase 9.
    async fn ingest(&self, session_id: Uuid, chunks: Vec<Chunk>) -> Result<()> {
        self.ingest_context(session_id, chunks).await
    }

    /// Alias for [`query_context`]. Kept for call sites that pre-date Phase 9.
    async fn query(
        &self,
        session_id: Uuid,
        embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<ScoredChunk>> {
        self.query_context(session_id, embedding, top_k).await
    }
}
