//! Global knowledge base — loads and queries static domain packs.
//!
//! Packs live in text files under the `knowledge/` directory (next to `prompts/`).
//! On first launch each pack is chunked, embedded with bge-small-en-v1.5, and
//! stored in a dedicated SQLite vector DB (`flint_knowledge.db`).  Subsequent
//! launches skip ingestion when `chunk_count > 0` for that pack UUID.
//!
//! The embedder is accessed via a shared slot (`Arc<RwLock<Option<Embedder>>>`)
//! so loading can wait for the async embedder init to complete without blocking
//! the Tauri startup path.

use std::path::PathBuf;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use anyhow::Result;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::interfaces::vector::{Chunk, ScoredChunk, VectorInterface};
use crate::rag::chunker::chunk_text;
use crate::rag::embedder::Embedder;

use super::packs::PackId;

const CHUNK_TOKENS: usize = 200;
const OVERLAP_TOKENS: usize = 50;

/// Resolve the knowledge base directory.
///
/// `FLINT_KNOWLEDGE_DIR` env-var overrides the default (dev/CI use).
/// Production uses `<CARGO_MANIFEST_DIR>/../knowledge`.
pub fn knowledge_base_dir() -> PathBuf {
    std::env::var("FLINT_KNOWLEDGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("knowledge")
        })
}

/// Manages static interview knowledge packs backed by a global SQLite vector DB.
///
/// Each `PackId` is mapped to a stable UUID acting as the `session_id` key in
/// the shared `SqliteVecStore`.  This is safe because pack UUIDs are in a
/// reserved namespace (`f17fffff-…`) that cannot collide with real v4 UUIDs.
pub struct GlobalKnowledgeBase {
    store: Arc<dyn VectorInterface>,
    embedder_slot: Arc<StdRwLock<Option<Arc<Embedder>>>>,
    packs_dir: PathBuf,
}

impl GlobalKnowledgeBase {
    pub fn new(
        store: Arc<dyn VectorInterface>,
        embedder_slot: Arc<StdRwLock<Option<Arc<Embedder>>>>,
        packs_dir: PathBuf,
    ) -> Self {
        Self {
            store,
            embedder_slot,
            packs_dir,
        }
    }

    /// Spawn a background task that waits for the embedder then loads every pack.
    ///
    /// Safe to call multiple times — already-loaded packs are no-ops.
    pub fn spawn_background_load(self: &Arc<Self>) {
        let kb = Arc::clone(self);
        tokio::spawn(async move {
            let embedder = kb.wait_for_embedder(Duration::from_secs(120)).await;
            let embedder = match embedder {
                Some(e) => e,
                None => {
                    warn!("knowledge base init: embedder not ready after 120s; skipping");
                    return;
                }
            };
            for &pack in PackId::all() {
                if kb.store.chunk_count(pack.uuid()) > 0 {
                    debug!(
                        pack = pack.dir_name(),
                        "knowledge pack already embedded; skipping"
                    );
                    continue;
                }
                if let Err(e) = kb.load_pack(pack, &embedder).await {
                    warn!(pack = pack.dir_name(), error = %e, "failed to embed knowledge pack");
                }
            }
            info!("knowledge base background load complete");
        });
    }

    /// Query the given packs and return up to `top_k` best-scoring chunks.
    ///
    /// Distributes `top_k` evenly across packs, then merges and re-ranks by
    /// cosine similarity so the most relevant chunks float to the top.
    pub async fn query_packs(
        &self,
        pack_ids: &[PackId],
        query_vec: &[f32],
        top_k: usize,
    ) -> Vec<ScoredChunk> {
        if pack_ids.is_empty() {
            return vec![];
        }
        let per_pack = top_k.div_ceil(pack_ids.len()).max(2);
        let mut all: Vec<ScoredChunk> = Vec::new();

        for &pack in pack_ids {
            if self.store.chunk_count(pack.uuid()) == 0 {
                continue;
            }
            match self.store.query(pack.uuid(), query_vec, per_pack).await {
                Ok(chunks) => all.extend(chunks),
                Err(e) => warn!(pack = pack.dir_name(), error = %e, "knowledge query error"),
            }
        }

        all.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all.truncate(top_k);
        all
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    async fn wait_for_embedder(&self, timeout: Duration) -> Option<Arc<Embedder>> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Ok(guard) = self.embedder_slot.read() {
                if let Some(e) = guard.clone() {
                    return Some(e);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn load_pack(&self, pack: PackId, embedder: &Embedder) -> Result<()> {
        let dir = self.packs_dir.join(pack.dir_name());
        if !dir.exists() {
            warn!(pack = pack.dir_name(), path = %dir.display(), "pack directory not found");
            return Ok(());
        }

        let mut combined = String::new();
        let mut entries: Vec<_> = std::fs::read_dir(&dir)?
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("txt"))
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            match std::fs::read_to_string(entry.path()) {
                Ok(text) => {
                    combined.push_str(&text);
                    combined.push_str("\n\n");
                }
                Err(e) => {
                    warn!(path = %entry.path().display(), error = %e, "failed to read pack file")
                }
            }
        }

        if combined.trim().is_empty() {
            warn!(pack = pack.dir_name(), "pack directory is empty; skipping");
            return Ok(());
        }

        let raw_chunks = chunk_text(&combined, CHUNK_TOKENS, OVERLAP_TOKENS);
        if raw_chunks.is_empty() {
            warn!(
                pack = pack.dir_name(),
                "no chunks produced; skipping ingest"
            );
            return Ok(());
        }

        let pack_uuid: Uuid = pack.uuid();

        // Embed the entire pack in one batched ONNX inference call — much faster
        // than calling embed_one() per chunk and avoids redundant model lock/unlock.
        const EMBED_BATCH_SIZE: usize = 64;
        let mut chunks = Vec::with_capacity(raw_chunks.len());

        for batch in raw_chunks.chunks(EMBED_BATCH_SIZE) {
            let refs: Vec<&str> = batch.iter().map(String::as_str).collect();
            let embeddings = embedder.embed_batch(&refs).unwrap_or_default();

            for (text, embedding) in batch.iter().zip(embeddings) {
                if embedding.is_empty() {
                    warn!(
                        pack = pack.dir_name(),
                        "empty embedding for chunk; skipping"
                    );
                    continue;
                }
                chunks.push(Chunk {
                    id: Uuid::new_v4(),
                    text: text.clone(),
                    embedding,
                    session_id: pack_uuid,
                });
            }
        }

        let n = chunks.len();
        if n == 0 {
            warn!(
                pack = pack.dir_name(),
                "all chunks produced empty embeddings; skipping ingest"
            );
            return Ok(());
        }
        self.store.ingest(pack_uuid, chunks).await?;
        info!(
            pack = pack.dir_name(),
            chunks = n,
            "knowledge pack embedded and stored"
        );
        Ok(())
    }
}
