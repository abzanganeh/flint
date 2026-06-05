//! Local text embedder backed by `fastembed-rs` (ONNX, fully offline).
//!
//! Model: BAAI/bge-small-en-v1.5 (384 dimensions). The model is downloaded
//! once on first use and cached in the user's fastembed cache directory
//! (typically `~/.cache/fastembed`). All subsequent calls load from disk.
//!
//! The [`Embedder`] is intended to be constructed once at startup and shared
//! (it is `Send + Sync`). Prefer [`embed_batch`](Embedder::embed_batch) over
//! calling [`embed_one`](Embedder::embed_one) in a loop — batch inference is
//! significantly faster due to ONNX runtime vectorisation.

#![allow(dead_code)]

use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use tracing::debug;

/// Fixed output dimension for BAAI/bge-small-en-v1.5.
const DIMENSIONS: usize = 384;

/// Single-instance text embedder. Wraps a loaded `TextEmbedding` model that
/// is reused for every call. Construction downloads the model on first use;
/// all subsequent constructions load from the local cache.
///
/// The inner `TextEmbedding` requires `&mut self` for inference, so it is
/// guarded by a `Mutex` to allow calling from an immutable `&self` reference
/// (required for `Send + Sync` sharing across tokio tasks).
pub struct Embedder {
    model: Mutex<TextEmbedding>,
}

impl Embedder {
    /// Load the `bge-small-en-v1.5` model. Blocks until the model is ready
    /// (download + ONNX load). Download is skipped when the cache is warm.
    pub fn new() -> Result<Self> {
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15).with_show_download_progress(false),
        )
        .context("failed to load bge-small-en-v1.5 embedding model")?;

        Ok(Self {
            model: Mutex::new(model),
        })
    }

    /// Embed a batch of texts in one ONNX inference pass. Prefer this over
    /// calling `embed_one` in a loop.
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let n = texts.len();
        let start = Instant::now();

        let embeddings = self
            .model
            .lock()
            .expect("embedder mutex poisoned")
            .embed(texts, None)
            .context("embedding inference failed")?;

        let duration_ms = start.elapsed().as_millis();
        debug!(batch_size = %n, duration_ms = %duration_ms, "embedding batch complete");

        Ok(embeddings)
    }

    /// Embed a single text string. Internally uses `embed_batch` with a
    /// one-element slice.
    pub fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let mut batch = self.embed_batch(&[text])?;
        batch
            .pop()
            .ok_or_else(|| anyhow::anyhow!("embedder returned empty result for single input"))
    }

    /// Output dimension of the loaded model. Always 384 for bge-small-en-v1.5.
    pub fn dimensions(&self) -> usize {
        DIMENSIONS
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use super::*;

    // Shared across all tests in this module — the model is loaded once and
    // reused. This avoids concurrent download attempts which cause fastembed
    // lock-file contention when tests run in parallel.
    //
    // Returns None when the model is not cached and the download is blocked
    // (e.g. HuggingFace 429 on CI runners). Tests skip gracefully in that case.
    static EMBEDDER: OnceLock<Option<Embedder>> = OnceLock::new();

    fn shared_embedder() -> Option<&'static Embedder> {
        EMBEDDER.get_or_init(|| Embedder::new().ok()).as_ref()
    }

    macro_rules! require_embedder {
        () => {
            match shared_embedder() {
                Some(e) => e,
                None => {
                    tracing::warn!(
                        "SKIP embedder test: fastembed model not cached (offline or rate-limited)"
                    );
                    return;
                }
            }
        };
    }

    /// Verifies the model loads and produces 384-dimensional embeddings.
    ///
    /// Downloads bge-small-en-v1.5 (~24 MB) on first run; subsequent runs
    /// use the local fastembed cache. Requires internet access on the first
    /// run only.
    #[test]
    fn test_embedding_dimensions() {
        let embedder = require_embedder!();
        let embedding = embedder
            .embed_one("test sentence")
            .expect("embed_one should succeed");
        assert_eq!(
            embedding.len(),
            384,
            "bge-small-en-v1.5 must produce 384-dim vectors"
        );
        assert_eq!(embedder.dimensions(), 384);
    }

    #[test]
    fn test_embed_batch_returns_one_vector_per_input() {
        let embedder = require_embedder!();
        let texts = ["hello world", "distributed systems", "Rust programming"];
        let results = embedder
            .embed_batch(&texts)
            .expect("batch embed should succeed");
        assert_eq!(results.len(), 3);
        for vec in &results {
            assert_eq!(vec.len(), 384);
        }
    }

    #[test]
    fn test_embed_one_and_batch_are_consistent() {
        let embedder = require_embedder!();
        let text = "consistency check sentence";
        let via_one = embedder.embed_one(text).expect("embed_one failed");
        let via_batch = embedder
            .embed_batch(&[text])
            .expect("embed_batch failed")
            .into_iter()
            .next()
            .unwrap();
        // Vectors for the same input through different entry points must be identical.
        assert_eq!(via_one, via_batch);
    }

    #[test]
    fn test_embeddings_are_not_zero_vectors() {
        let embedder = require_embedder!();
        let embedding = embedder
            .embed_one("non-trivial sentence about machine learning")
            .expect("embed_one failed");
        let norm_sq: f32 = embedding.iter().map(|x| x * x).sum();
        assert!(norm_sq > 0.0, "embedding must not be a zero vector");
    }

    #[test]
    fn test_empty_batch_returns_empty_vec() {
        let embedder = require_embedder!();
        let results = embedder
            .embed_batch(&[])
            .expect("empty batch should not error");
        assert!(results.is_empty());
    }
}
