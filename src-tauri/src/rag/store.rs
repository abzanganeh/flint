//! SQLite-backed dual vector store using the `sqlite-vec` extension.
//!
//! ## Dual-store model (Phase 9, design doc §16.5)
//!
//! Each session owns two independent sqlite-vec virtual tables:
//!
//! | Table name | Metadata table | Content |
//! |---|---|---|
//! | `vec_chunks_<id>` | `chunk_meta` | Context: JD, resume, notes, user-provided Q&A |
//! | `vec_qa_<id>` | `chunk_meta_qa` | Q&A: quality-gated AI-generated answers |
//!
//! Separating the tables prevents Q&A chunks from outranking context chunks
//! during retrieval (Q&A embeddings are semantically very close to the question
//! they answer and would consistently score higher than JD text).
//!
//! ## Score formula
//!
//! sqlite-vec's `MATCH` operator returns the L2 (Euclidean) distance.
//! bge-small-en-v1.5 embeddings are unit-normalised, so for two unit
//! vectors `a` and `b`:
//!
//! ```text
//! |a − b|²  = 2 − 2 · (a · b) = 2 − 2 · cosine_similarity
//! ⟹  cosine_similarity = 1 − |a − b|² / 2
//! ```
//!
//! `ScoredChunk::score` is set to this cosine similarity value.
//!
//! ## WAL mode
//!
//! `PRAGMA journal_mode = WAL` is set at connection open. In-memory
//! databases (`:memory:`) silently stay in memory mode — this is expected
//! and logged at WARN level.
//!
//! ## Thread safety
//!
//! `rusqlite::Connection` is `Send` but not `Sync`. It is guarded by a
//! `Mutex` so a single `SqliteVecStore` instance is safe to share across
//! `tokio::spawn` tasks via `Arc`. All blocking SQLite operations are
//! dispatched to the thread pool via `tokio::task::spawn_blocking`.

#![allow(dead_code)]

use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use async_trait::async_trait;
use bytemuck::cast_slice;
use rusqlite::params;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::interfaces::vector::{Chunk, ScoredChunk, VectorInterface};

// ──────────────────────────────────────────────────────────────────────────────
// Extension registration
// ──────────────────────────────────────────────────────────────────────────────

static VEC_EXTENSION_REGISTERED: OnceLock<()> = OnceLock::new();

/// Register the sqlite-vec extension with SQLite's auto-extension mechanism.
///
/// Must be called before opening any connection that will use `vec0` tables.
/// Idempotent — safe to call from multiple threads; the extension is
/// registered at most once per process lifetime.
fn register_vec_extension() {
    VEC_EXTENSION_REGISTERED.get_or_init(|| {
        // SAFETY: `sqlite3_vec_init` is the correct entrypoint for the
        // sqlite-vec extension. The transmute is required because
        // `sqlite3_auto_extension` expects a C function pointer type that Rust
        // cannot name directly; this is the canonical pattern shown in the
        // official sqlite-vec Rust documentation.
        #[allow(clippy::missing_transmute_annotations)]
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

// ──────────────────────────────────────────────────────────────────────────────
// Store
// ──────────────────────────────────────────────────────────────────────────────

/// SQLite + sqlite-vec backed dual vector store.
///
/// Construct with [`SqliteVecStore::new`] and wrap in `Arc` for sharing
/// across tasks.
pub struct SqliteVecStore {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

impl SqliteVecStore {
    /// Open (or create) a vector store at `db_path`.
    ///
    /// Pass `":memory:"` for an ephemeral in-process store (useful in tests).
    pub fn new(db_path: &str) -> Result<Self> {
        register_vec_extension();

        let conn = rusqlite::Connection::open(db_path).context("failed to open vector store DB")?;

        // WAL mode for durability and concurrent read access.
        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .context("failed to set journal_mode")?;
        if mode != "wal" {
            warn!(mode = %mode, "WAL mode not active (expected for in-memory DBs)");
        }

        // Context metadata table (existing schema — unchanged for backward compat).
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS chunk_meta (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 chunk_uuid  TEXT    NOT NULL,
                 session_id  TEXT    NOT NULL,
                 text        TEXT    NOT NULL,
                 embedding   BLOB    NOT NULL DEFAULT ''
             );
             CREATE INDEX IF NOT EXISTS idx_chunk_meta_session
                 ON chunk_meta(session_id);",
        )
        .context("failed to create chunk_meta table")?;

        // Q&A metadata table (Phase 9 — separate table prevents schema entanglement
        // and makes session cleanup atomic per store type).
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS chunk_meta_qa (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 chunk_uuid  TEXT    NOT NULL,
                 session_id  TEXT    NOT NULL,
                 text        TEXT    NOT NULL,
                 embedding   BLOB    NOT NULL DEFAULT ''
             );
             CREATE INDEX IF NOT EXISTS idx_chunk_meta_qa_session
                 ON chunk_meta_qa(session_id);",
        )
        .context("failed to create chunk_meta_qa table")?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    // ── Table name helpers ──────────────────────────────────────────────────

    fn context_vec_table(session_id: Uuid) -> String {
        format!("vec_chunks_{}", session_id.simple())
    }

    fn qa_vec_table(session_id: Uuid) -> String {
        format!("vec_qa_{}", session_id.simple())
    }

    // ── Table creation ──────────────────────────────────────────────────────

    fn ensure_context_table(conn: &rusqlite::Connection, session_id: Uuid) -> Result<()> {
        let table = Self::context_vec_table(session_id);
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS {table}
             USING vec0(embedding float[384])"
        ))
        .with_context(|| format!("failed to create context vec table for session {session_id}"))
    }

    fn ensure_qa_table(conn: &rusqlite::Connection, session_id: Uuid) -> Result<()> {
        let table = Self::qa_vec_table(session_id);
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS {table}
             USING vec0(embedding float[384])"
        ))
        .with_context(|| format!("failed to create Q&A vec table for session {session_id}"))
    }

    // ── Table existence checks ──────────────────────────────────────────────

    fn table_exists(conn: &rusqlite::Connection, table: &str) -> bool {
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            params![table],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
            > 0
    }

    // ── Shared ingest logic ─────────────────────────────────────────────────

    /// Core ingest routine. `vec_table` and `meta_table` control which store
    /// the chunks land in. `ensure_fn` creates the vec0 table if absent.
    fn ingest_into(
        conn: &rusqlite::Connection,
        session_id: Uuid,
        chunks: &[Chunk],
        vec_table: &str,
        meta_table: &str,
        ensure_fn: impl Fn(&rusqlite::Connection, Uuid) -> Result<()>,
    ) -> Result<()> {
        ensure_fn(conn, session_id)?;

        let mut meta_stmt = conn
            .prepare(&format!(
                "INSERT INTO {meta_table} (chunk_uuid, session_id, text, embedding)
                 VALUES (?1, ?2, ?3, ?4)"
            ))
            .context("prepare meta insert")?;
        let mut vec_stmt = conn
            .prepare(&format!(
                "INSERT INTO {vec_table}(rowid, embedding) VALUES (?1, ?2)"
            ))
            .context("prepare vec insert")?;

        for chunk in chunks {
            let emb_bytes: &[u8] = cast_slice(&chunk.embedding);
            meta_stmt
                .execute(params![
                    chunk.id.to_string(),
                    chunk.session_id.to_string(),
                    chunk.text,
                    emb_bytes,
                ])
                .context("insert meta row")?;
            let rowid = conn.last_insert_rowid();

            let bytes: &[u8] = cast_slice(&chunk.embedding);
            vec_stmt
                .execute(params![rowid, bytes])
                .context("insert vec embedding")?;
        }

        Ok(())
    }

    // ── Shared query logic ──────────────────────────────────────────────────

    fn query_from(
        conn: &rusqlite::Connection,
        session_id: Uuid,
        embedding: &[f32],
        top_k: usize,
        vec_table: &str,
        meta_table: &str,
    ) -> Result<Vec<ScoredChunk>> {
        if !Self::table_exists(conn, vec_table) {
            return Ok(vec![]);
        }

        let bytes: &[u8] = cast_slice(embedding);

        let mut stmt = conn
            .prepare(&format!(
                "SELECT v.rowid, v.distance, m.chunk_uuid, m.text, m.embedding
                 FROM {vec_table} v
                 JOIN {meta_table} m ON m.id = v.rowid
                 WHERE v.embedding MATCH ?1
                   AND k = ?2
                 ORDER BY v.distance"
            ))
            .context("prepare query")?;

        let results = stmt
            .query_map(params![bytes, top_k as i64], |row| {
                let distance: f64 = row.get(1)?;
                let chunk_uuid: String = row.get(2)?;
                let text: String = row.get(3)?;
                let emb_bytes: Vec<u8> = row.get(4)?;
                Ok((distance, chunk_uuid, text, emb_bytes))
            })
            .context("execute query")?
            .collect::<Result<Vec<_>, _>>()
            .context("collect query rows")?;

        let scored: Vec<ScoredChunk> = results
            .into_iter()
            .map(|(distance, chunk_uuid, text, emb_bytes)| {
                // Cosine similarity from L2 distance for unit-norm vectors:
                // cos_sim = 1 - dist² / 2
                let dist = distance as f32;
                let score = 1.0 - (dist * dist) / 2.0;

                let embedding: Vec<f32> = if emb_bytes.len() % 4 == 0 {
                    cast_slice::<u8, f32>(&emb_bytes).to_vec()
                } else {
                    vec![]
                };

                ScoredChunk {
                    chunk: Chunk {
                        id: Uuid::parse_str(&chunk_uuid).unwrap_or_else(|_| Uuid::nil()),
                        text,
                        embedding,
                        session_id,
                    },
                    score,
                }
            })
            .collect();

        Ok(scored)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// VectorInterface implementation
// ──────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl VectorInterface for SqliteVecStore {
    async fn ingest_context(&self, session_id: Uuid, chunks: Vec<Chunk>) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("vec store mutex poisoned");
            let vec_table = SqliteVecStore::context_vec_table(session_id);
            let meta_table = "chunk_meta";
            SqliteVecStore::ingest_into(
                &conn,
                session_id,
                &chunks,
                &vec_table,
                meta_table,
                SqliteVecStore::ensure_context_table,
            )?;
            debug!(
                session_id = %session_id,
                count = chunks.len(),
                "ingested chunks into context store",
            );
            Ok(())
        })
        .await
        .context("ingest_context spawn_blocking panicked")?
    }

    async fn query_context(
        &self,
        session_id: Uuid,
        embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<ScoredChunk>> {
        let conn = self.conn.clone();
        let embedding = embedding.to_vec();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("vec store mutex poisoned");
            let vec_table = SqliteVecStore::context_vec_table(session_id);
            let results = SqliteVecStore::query_from(
                &conn,
                session_id,
                &embedding,
                top_k,
                &vec_table,
                "chunk_meta",
            )?;
            debug!(
                session_id = %session_id,
                returned = results.len(),
                "context store query complete",
            );
            Ok(results)
        })
        .await
        .context("query_context spawn_blocking panicked")?
    }

    async fn ingest_qa(&self, session_id: Uuid, chunks: Vec<Chunk>) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("vec store mutex poisoned");
            let vec_table = SqliteVecStore::qa_vec_table(session_id);
            let meta_table = "chunk_meta_qa";
            SqliteVecStore::ingest_into(
                &conn,
                session_id,
                &chunks,
                &vec_table,
                meta_table,
                SqliteVecStore::ensure_qa_table,
            )?;
            debug!(
                session_id = %session_id,
                count = chunks.len(),
                "ingested Q&A pair into Q&A store",
            );
            Ok(())
        })
        .await
        .context("ingest_qa spawn_blocking panicked")?
    }

    async fn query_qa(
        &self,
        session_id: Uuid,
        embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<ScoredChunk>> {
        let conn = self.conn.clone();
        let embedding = embedding.to_vec();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("vec store mutex poisoned");
            let vec_table = SqliteVecStore::qa_vec_table(session_id);
            let results = SqliteVecStore::query_from(
                &conn,
                session_id,
                &embedding,
                top_k,
                &vec_table,
                "chunk_meta_qa",
            )?;
            debug!(
                session_id = %session_id,
                returned = results.len(),
                "Q&A store query complete",
            );
            Ok(results)
        })
        .await
        .context("query_qa spawn_blocking panicked")?
    }

    async fn delete_session(&self, session_id: Uuid) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("vec store mutex poisoned");

            let ctx_table = SqliteVecStore::context_vec_table(session_id);
            let qa_table = SqliteVecStore::qa_vec_table(session_id);

            if SqliteVecStore::table_exists(&conn, &ctx_table) {
                conn.execute_batch(&format!("DROP TABLE {ctx_table}"))
                    .with_context(|| {
                        format!("failed to drop context vec table for session {session_id}")
                    })?;
            }
            if SqliteVecStore::table_exists(&conn, &qa_table) {
                conn.execute_batch(&format!("DROP TABLE {qa_table}"))
                    .with_context(|| {
                        format!("failed to drop Q&A vec table for session {session_id}")
                    })?;
            }

            conn.execute(
                "DELETE FROM chunk_meta WHERE session_id = ?1",
                params![session_id.to_string()],
            )
            .context("delete chunk_meta for session")?;

            conn.execute(
                "DELETE FROM chunk_meta_qa WHERE session_id = ?1",
                params![session_id.to_string()],
            )
            .context("delete chunk_meta_qa for session")?;

            debug!(session_id = %session_id, "deleted session from both vector stores");
            Ok(())
        })
        .await
        .context("delete_session spawn_blocking panicked")?
    }

    fn chunk_count(&self, session_id: Uuid) -> usize {
        let conn = self.conn.lock().expect("vec store mutex poisoned");
        conn.query_row(
            "SELECT COUNT(*) FROM chunk_meta WHERE session_id = ?1",
            params![session_id.to_string()],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) as usize
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use super::*;
    use crate::rag::embedder::Embedder;

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
                        "SKIP store test: fastembed model not cached (offline or rate-limited)"
                    );
                    return;
                }
            }
        };
    }

    fn make_chunk(text: &str, session_id: Uuid, embedder: &Embedder) -> Chunk {
        let embedding = embedder.embed_one(text).expect("embed failed");
        Chunk {
            id: Uuid::new_v4(),
            text: text.to_string(),
            embedding,
            session_id,
        }
    }

    fn make_store() -> SqliteVecStore {
        SqliteVecStore::new(":memory:").expect("store should open")
    }

    #[tokio::test]
    async fn test_ingest_and_query_returns_results() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = require_embedder!();

        let chunk = make_chunk("distributed systems and fault tolerance", session, emb);
        store.ingest_context(session, vec![chunk]).await.unwrap();

        let query = emb.embed_one("Tell me about distributed systems").unwrap();
        let results = store.query_context(session, &query, 5).await.unwrap();

        assert_eq!(results.len(), 1);
        assert!(
            results[0].score > 0.5,
            "expected score > 0.5 for semantically related query, got {}",
            results[0].score
        );
    }

    /// Session isolation: context chunks ingested under session A must not
    /// appear in context queries against session B.
    #[tokio::test]
    async fn test_context_session_isolation() {
        let store = make_store();
        let session_a = Uuid::new_v4();
        let session_b = Uuid::new_v4();
        let emb = require_embedder!();

        let chunk = make_chunk("machine learning and neural networks", session_a, emb);
        store.ingest_context(session_a, vec![chunk]).await.unwrap();

        let query = emb.embed_one("machine learning").unwrap();
        let results = store.query_context(session_b, &query, 5).await.unwrap();

        assert!(
            results.is_empty(),
            "session B must not see context chunks from session A"
        );
    }

    /// Q&A store isolation: Q&A chunks must not appear in context queries and
    /// vice versa for the same session.
    #[tokio::test]
    async fn test_qa_context_store_isolation() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = require_embedder!();

        let ctx_chunk = make_chunk("Identity and access management fundamentals", session, emb);
        store.ingest_context(session, vec![ctx_chunk]).await.unwrap();

        let qa_chunk = make_chunk(
            "Q: Tell me about yourself\nA: I am an IAM architect with 8 years experience",
            session,
            emb,
        );
        store.ingest_qa(session, vec![qa_chunk]).await.unwrap();

        // Context query should not return the Q&A chunk.
        let ctx_query = emb.embed_one("Tell me about yourself").unwrap();
        let ctx_results = store.query_context(session, &ctx_query, 5).await.unwrap();
        assert!(
            ctx_results
                .iter()
                .all(|r| !r.chunk.text.contains("Tell me about yourself")),
            "context store must not return Q&A chunk"
        );

        // Q&A query should return the Q&A chunk, not the context chunk.
        let qa_results = store.query_qa(session, &ctx_query, 5).await.unwrap();
        assert_eq!(qa_results.len(), 1);
        assert!(qa_results[0].chunk.text.contains("Tell me about yourself"));
    }

    #[tokio::test]
    async fn test_chunk_count_context_only() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = require_embedder!();

        assert_eq!(store.chunk_count(session), 0);

        let chunks: Vec<Chunk> = ["first chunk", "second chunk", "third chunk"]
            .iter()
            .map(|t| make_chunk(t, session, emb))
            .collect();
        store.ingest_context(session, chunks).await.unwrap();

        // chunk_count reflects context store only.
        assert_eq!(store.chunk_count(session), 3);
    }

    #[tokio::test]
    async fn test_delete_session_removes_both_stores() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = require_embedder!();

        let ctx_chunk = make_chunk("Rust ownership and borrowing", session, emb);
        store.ingest_context(session, vec![ctx_chunk]).await.unwrap();

        let qa_chunk = make_chunk(
            "Q: What is ownership?\nA: Ownership is a set of rules that govern memory",
            session,
            emb,
        );
        store.ingest_qa(session, vec![qa_chunk]).await.unwrap();

        store.delete_session(session).await.unwrap();

        assert_eq!(store.chunk_count(session), 0);

        let query = emb.embed_one("Rust ownership").unwrap();
        let ctx_results = store.query_context(session, &query, 5).await.unwrap();
        let qa_results = store.query_qa(session, &query, 5).await.unwrap();
        assert!(ctx_results.is_empty(), "context store must be empty after delete");
        assert!(qa_results.is_empty(), "Q&A store must be empty after delete");
    }

    #[tokio::test]
    async fn test_query_empty_qa_store_returns_empty() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = require_embedder!();
        let query = emb.embed_one("anything").unwrap();
        let results = store.query_qa(session, &query, 5).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_scores_are_in_valid_range() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = require_embedder!();

        let texts = [
            "software engineering best practices",
            "product management and roadmaps",
            "distributed systems architecture",
        ];
        let chunks: Vec<Chunk> = texts
            .iter()
            .map(|t| make_chunk(t, session, emb))
            .collect();
        store.ingest_context(session, chunks).await.unwrap();

        let query = emb
            .embed_one("software engineering and architecture")
            .unwrap();
        let results = store.query_context(session, &query, 3).await.unwrap();

        for r in &results {
            assert!(
                r.score >= -1.0 && r.score <= 1.0,
                "score must be in [-1, 1], got {}",
                r.score
            );
        }
    }

    /// Backward-compat alias: `ingest` routes to context store.
    #[tokio::test]
    async fn test_compat_ingest_alias() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = require_embedder!();

        let chunk = make_chunk("IAM architect background", session, emb);
        // Old call site — uses the alias, should land in context store.
        store.ingest(session, vec![chunk]).await.unwrap();
        assert_eq!(store.chunk_count(session), 1);
    }
}
