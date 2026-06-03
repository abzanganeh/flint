//! SQLite-backed vector store using the `sqlite-vec` extension.
//!
//! Each session gets its own `vec0` virtual table named
//! `vec_chunks_<session_id_hex>` (32-char lowercase hex UUID without
//! hyphens). Chunk metadata (text, UUID) lives in a shared `chunk_meta`
//! table keyed by the same rowid as the vec0 table entry.
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

/// SQLite + sqlite-vec backed vector store.
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

        let conn =
            rusqlite::Connection::open(db_path).context("failed to open vector store DB")?;

        // WAL mode for durability and concurrent read access.
        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .context("failed to set journal_mode")?;
        if mode != "wal" {
            warn!(mode = %mode, "WAL mode not active (expected for in-memory DBs)");
        }

        // Shared metadata table: one row per chunk, rowid matches the vec0 entry.
        // `embedding` is stored as raw little-endian f32 bytes so the retriever
        // can compute real inter-chunk dot-product similarity for MMR.
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

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// The `vec0` table name for a session.
    fn vec_table(session_id: Uuid) -> String {
        format!("vec_chunks_{}", session_id.simple())
    }

    /// Create the session's `vec0` virtual table if it does not yet exist.
    fn ensure_vec_table(
        conn: &rusqlite::Connection,
        session_id: Uuid,
    ) -> Result<()> {
        let table = Self::vec_table(session_id);
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS {table}
             USING vec0(embedding float[384])"
        ))
        .with_context(|| format!("failed to create vec table for session {session_id}"))?;
        Ok(())
    }

    /// Check whether the session's `vec0` table exists in the schema.
    fn vec_table_exists(conn: &rusqlite::Connection, session_id: Uuid) -> bool {
        let table = Self::vec_table(session_id);
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            params![table],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
            > 0
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// VectorInterface implementation
// ──────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl VectorInterface for SqliteVecStore {
    async fn ingest(&self, session_id: Uuid, chunks: Vec<Chunk>) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("vec store mutex poisoned");
            Self::ensure_vec_table(&conn, session_id)?;

            let vec_table = Self::vec_table(session_id);
            let mut meta_stmt = conn
                .prepare(
                    "INSERT INTO chunk_meta (chunk_uuid, session_id, text, embedding)
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .context("prepare chunk_meta insert")?;
            let mut vec_stmt = conn
                .prepare(&format!(
                    "INSERT INTO {vec_table}(rowid, embedding) VALUES (?1, ?2)"
                ))
                .context("prepare vec insert")?;

            for chunk in &chunks {
                let emb_bytes: &[u8] = cast_slice(&chunk.embedding);
                meta_stmt
                    .execute(params![
                        chunk.id.to_string(),
                        chunk.session_id.to_string(),
                        chunk.text,
                        emb_bytes,
                    ])
                    .context("insert chunk_meta")?;
                let rowid = conn.last_insert_rowid();

                let bytes: &[u8] = cast_slice(&chunk.embedding);
                vec_stmt
                    .execute(params![rowid, bytes])
                    .context("insert vec embedding")?;
            }

            debug!(
                session_id = %session_id,
                count = chunks.len(),
                "ingested chunks into vector store",
            );
            Ok(())
        })
        .await
        .context("ingest spawn_blocking panicked")?
    }

    async fn query(
        &self,
        session_id: Uuid,
        embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<ScoredChunk>> {
        let conn = self.conn.clone();
        let embedding = embedding.to_vec();

        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("vec store mutex poisoned");

            if !Self::vec_table_exists(&conn, session_id) {
                return Ok(vec![]);
            }

            let vec_table = Self::vec_table(session_id);
            let bytes: &[u8] = cast_slice(&embedding);

            // sqlite-vec's vec0 requires `k = ?` in the WHERE clause (not a
            // parameterised LIMIT) for the KNN query planner to activate.
            // Return the stored embedding so the retriever can compute real
            // inter-chunk dot-product similarity for MMR de-duplication.
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT v.rowid, v.distance, m.chunk_uuid, m.text, m.embedding
                     FROM {vec_table} v
                     JOIN chunk_meta m ON m.id = v.rowid
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

                    // Deserialise the stored f32 bytes back to a Vec<f32>.
                    let embedding: Vec<f32> = if emb_bytes.len() % 4 == 0 {
                        cast_slice::<u8, f32>(&emb_bytes).to_vec()
                    } else {
                        vec![]
                    };

                    ScoredChunk {
                        chunk: Chunk {
                            id: Uuid::parse_str(&chunk_uuid)
                                .unwrap_or_else(|_| Uuid::nil()),
                            text,
                            embedding,
                            session_id,
                        },
                        score,
                    }
                })
                .collect();

            debug!(
                session_id = %session_id,
                returned = scored.len(),
                "vector store query complete",
            );
            Ok(scored)
        })
        .await
        .context("query spawn_blocking panicked")?
    }

    async fn delete_session(&self, session_id: Uuid) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("vec store mutex poisoned");
            let vec_table = Self::vec_table(session_id);

            if Self::vec_table_exists(&conn, session_id) {
                conn.execute_batch(&format!("DROP TABLE {vec_table}"))
                    .with_context(|| {
                        format!("failed to drop vec table for session {session_id}")
                    })?;
            }

            conn.execute(
                "DELETE FROM chunk_meta WHERE session_id = ?1",
                params![session_id.to_string()],
            )
            .context("delete chunk_meta for session")?;

            debug!(session_id = %session_id, "deleted session from vector store");
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

    static EMBEDDER: OnceLock<Embedder> = OnceLock::new();

    fn embedder() -> &'static Embedder {
        EMBEDDER.get_or_init(|| Embedder::new().expect("embedder should load"))
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
        let emb = embedder();

        let chunk = make_chunk("distributed systems and fault tolerance", session, emb);
        store.ingest(session, vec![chunk]).await.unwrap();

        let query = emb
            .embed_one("Tell me about distributed systems")
            .unwrap();
        let results = store.query(session, &query, 5).await.unwrap();

        assert_eq!(results.len(), 1);
        assert!(
            results[0].score > 0.5,
            "expected score > 0.5 for semantically related query, got {}",
            results[0].score
        );
    }

    /// Session isolation: chunks ingested under session A must not appear in
    /// queries against session B (design doc §11 and RAG rules §13).
    #[tokio::test]
    async fn test_session_isolation() {
        let store = make_store();
        let session_a = Uuid::new_v4();
        let session_b = Uuid::new_v4();
        let emb = embedder();

        let chunk = make_chunk("machine learning and neural networks", session_a, emb);
        store.ingest(session_a, vec![chunk]).await.unwrap();

        let query = emb.embed_one("machine learning").unwrap();
        let results = store.query(session_b, &query, 5).await.unwrap();

        assert!(
            results.is_empty(),
            "session B must not see chunks ingested under session A"
        );
    }

    #[tokio::test]
    async fn test_chunk_count() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = embedder();

        assert_eq!(store.chunk_count(session), 0);

        let chunks: Vec<Chunk> = ["first chunk", "second chunk", "third chunk"]
            .iter()
            .map(|t| make_chunk(t, session, emb))
            .collect();
        store.ingest(session, chunks).await.unwrap();

        assert_eq!(store.chunk_count(session), 3);
    }

    #[tokio::test]
    async fn test_delete_session_removes_all_data() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = embedder();

        let chunk = make_chunk("Rust ownership and borrowing", session, emb);
        store.ingest(session, vec![chunk]).await.unwrap();
        assert_eq!(store.chunk_count(session), 1);

        store.delete_session(session).await.unwrap();

        assert_eq!(store.chunk_count(session), 0);
        let query = emb.embed_one("Rust ownership").unwrap();
        let results = store.query(session, &query, 5).await.unwrap();
        assert!(results.is_empty(), "query after delete must return empty");
    }

    #[tokio::test]
    async fn test_query_on_empty_session_returns_empty() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = embedder();
        let query = emb.embed_one("anything").unwrap();
        let results = store.query(session, &query, 5).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_scores_are_in_valid_range() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = embedder();

        let texts = [
            "software engineering best practices",
            "product management and roadmaps",
            "distributed systems architecture",
        ];
        let chunks: Vec<Chunk> = texts.iter().map(|t| make_chunk(t, session, emb)).collect();
        store.ingest(session, chunks).await.unwrap();

        let query = emb
            .embed_one("software engineering and architecture")
            .unwrap();
        let results = store.query(session, &query, 3).await.unwrap();

        for r in &results {
            assert!(
                r.score >= -1.0 && r.score <= 1.0,
                "score must be in [-1, 1], got {}",
                r.score
            );
        }
    }

    #[tokio::test]
    async fn test_multi_chunk_batch_ingest() {
        let store = make_store();
        let session = Uuid::new_v4();
        let emb = embedder();

        let texts: Vec<&str> = (0..10).map(|_| "unique chunk text").collect();
        let chunks: Vec<Chunk> = texts.iter().map(|t| make_chunk(t, session, emb)).collect();
        store.ingest(session, chunks).await.unwrap();

        assert_eq!(store.chunk_count(session), 10);
    }
}
