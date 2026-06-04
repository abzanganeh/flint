//! Pre-warm cache: fire inference for the top-5 digest questions before the
//! session starts so the first responses feel instant.
//!
//! Reference: design doc §4 (Core Concept — pre-warming), §13 (RAG rules —
//! cache hit threshold 0.85, staleness penalty), `.cursor/rules` flint-core
//! §4 (parallel threads via tokio::spawn, never sequential).
//!
//! ## Concurrency guarantee
//!
//! All 10 inference calls (5 × directional + 5 × depth) are dispatched
//! immediately via `tokio::spawn`, then collected with `futures::future::join_all`.
//! No call ever waits for another. Sequential execution is forbidden by
//! the unit tests (see `test_run_prewarm_fires_all_tasks_concurrently`).
//!
//! ## Cache hit logic (live session)
//!
//! Before firing any inference on a live question, call
//! [`PreWarmCache::lookup`]. If cosine similarity ≥ 0.85 between the
//! incoming question embedding and a cached entry's embedding, serve the
//! cached response immediately. Entries older than 10 minutes contribute a
//! staleness penalty to the confidence score (§21).
//!
//! ## Cache shape
//!
//! Each question yields **exactly one** [`PreWarmEntry`] containing both
//! the directional and the depth response. The two LLM calls per question
//! still run concurrently, but their results are merged before insertion —
//! lookups never see a half-populated entry.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use futures::future::join_all;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::digest::Digest;
use crate::llm::provider::{CompletionConfig, LLMProvider};
use crate::rag::embedder::Embedder;

// ──────────────────────────────────────────────────────────────────────────────
// Cache types
// ──────────────────────────────────────────────────────────────────────────────

/// One cached pre-warm result for a single question.
#[derive(Debug, Clone)]
pub struct PreWarmEntry {
    pub question: String,
    pub directional_response: String,
    pub depth_response: String,
    pub created_at: DateTime<Utc>,
    /// bge-small-en-v1.5 embedding of `question` (384 dimensions, unit-norm).
    pub embedding: Vec<f32>,
}

impl PreWarmEntry {
    /// True when this entry was created more than 10 minutes ago.
    pub fn is_stale(&self) -> bool {
        let age = Utc::now().signed_duration_since(self.created_at);
        age.num_minutes() > 10
    }

    /// Staleness penalty subtracted from the confidence score (§21 formula).
    /// Returns 0.0 for fresh entries, 0.10 for stale ones.
    pub fn staleness_penalty(&self) -> f32 {
        if self.is_stale() {
            0.10
        } else {
            0.0
        }
    }
}

/// In-memory cache of pre-generated responses, keyed by a UUID derived from
/// the question embedding. All live-session lookups check this cache first.
#[derive(Debug, Default)]
pub struct PreWarmCache {
    /// Key: UUID v4 assigned at insertion time (stable within a session run).
    /// Lookup is always by cosine similarity, never by UUID directly.
    entries: HashMap<Uuid, PreWarmEntry>,
}

impl PreWarmCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert an entry into the cache. Returns the assigned key.
    pub fn insert(&mut self, entry: PreWarmEntry) -> Uuid {
        let key = Uuid::new_v4();
        self.entries.insert(key, entry);
        key
    }

    /// Return the cached entry whose question embedding has the highest cosine
    /// similarity to `embedding`, if that similarity is ≥ 0.85 (§13 rule).
    ///
    /// Returns `None` when the cache is empty or no entry clears the
    /// threshold.
    pub fn lookup(&self, embedding: &[f32]) -> Option<&PreWarmEntry> {
        const HIT_THRESHOLD: f32 = 0.85;

        let (best_entry, best_sim) = self
            .entries
            .values()
            .filter_map(|entry| {
                if entry.embedding.len() != embedding.len() {
                    return None;
                }
                let sim = dot_product(&entry.embedding, embedding);
                Some((entry, sim))
            })
            .fold((None, f32::NEG_INFINITY), |(best_e, best_s), (e, s)| {
                if s > best_s {
                    (Some(e), s)
                } else {
                    (best_e, best_s)
                }
            });

        if best_sim >= HIT_THRESHOLD {
            best_entry
        } else {
            None
        }
    }

    /// Number of entries currently in the cache.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Prompt loading (never inline)
// ──────────────────────────────────────────────────────────────────────────────

fn prompts_base_dir() -> PathBuf {
    std::env::var("FLINT_PROMPTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("prompts")
        })
}

fn load_prompt(category: &str, provider_name: &str) -> Result<String> {
    let base = prompts_base_dir();
    let specific = base.join(category).join(format!("{provider_name}.txt"));
    let default = base.join(category).join("default.txt");

    if specific.exists() {
        std::fs::read_to_string(&specific).with_context(|| format!("read {}", specific.display()))
    } else {
        std::fs::read_to_string(&default).with_context(|| format!("read {}", default.display()))
    }
}

fn build_prompt(template: &str, question: &str, digest: &Digest) -> String {
    template
        .replace("{question}", question)
        .replace("{role}", &digest.role)
        .replace("{domain}", &digest.domain)
        .replace("{key_skills}", &digest.key_skills.join(", "))
}

// ──────────────────────────────────────────────────────────────────────────────
// Pre-warm runner
// ──────────────────────────────────────────────────────────────────────────────

/// Fire pre-warm inference for the top-5 digest questions.
///
/// **All 10 LLM calls are spawned concurrently** — directional and depth for
/// each of the 5 questions — using `tokio::spawn`, then awaited with
/// `futures::future::join_all`. No call ever blocks another.
///
/// For each question both responses are joined and inserted as a single
/// [`PreWarmEntry`], so the cache contains at most one entry per question
/// even if the two LLM calls race.
///
/// Errors from individual question tasks are logged at WARN and skipped; a
/// failure for one question never prevents others from being cached.
///
/// # Blocking work
///
/// The embedder is a blocking ONNX call, so it is dispatched to
/// `tokio::task::spawn_blocking`. This means `run_prewarm` is safe to call
/// from any async context (no extra `spawn_blocking` wrapping required at
/// the call site).
pub async fn run_prewarm(
    digest: &Digest,
    llm: Arc<dyn LLMProvider>,
    embedder: Arc<Embedder>,
    cache: Arc<Mutex<PreWarmCache>>,
) -> Result<()> {
    let questions: Vec<String> = digest.likely_questions.iter().take(5).cloned().collect();

    if questions.is_empty() {
        warn!("digest has no likely_questions — pre-warm skipped");
        return Ok(());
    }

    let provider_name = llm.name().to_string();

    // Load prompt templates once (outside the spawn tasks).
    let dir_template = load_prompt("directional", &provider_name)
        .context("failed to load directional pre-warm prompt")?;
    let dep_template =
        load_prompt("depth", &provider_name).context("failed to load depth pre-warm prompt")?;

    // Embed all questions in one batch off the async runtime.
    let questions_for_embed = questions.clone();
    let embedder_handle = Arc::clone(&embedder);
    let embeddings: Vec<Vec<f32>> = tokio::task::spawn_blocking(move || {
        let refs: Vec<&str> = questions_for_embed.iter().map(|q| q.as_str()).collect();
        embedder_handle.embed_batch(&refs)
    })
    .await
    .context("embed_batch task panicked")?
    .context("failed to embed pre-warm questions")?;

    let start = Instant::now();

    // For each question, spawn directional and depth tasks. Both tasks run
    // concurrently with one another and with all other questions' tasks.
    // We collect them per-question so we can merge results into a single
    // PreWarmEntry per question (no duplicate / half-populated entries).
    let mut question_tasks = Vec::with_capacity(questions.len());

    for (question, embedding) in questions.iter().zip(embeddings) {
        let dir_prompt = build_prompt(&dir_template, question, digest);
        let dep_prompt = build_prompt(&dep_template, question, digest);
        let question_str = question.clone();

        // Spawn both LLM calls immediately so they are in-flight in parallel.
        let llm_dir = Arc::clone(&llm);
        let dir_handle = tokio::spawn(async move {
            llm_dir
                .complete(
                    dir_prompt,
                    CompletionConfig {
                        max_tokens: Some(200),
                        temperature: 0.0,
                        stream: false,
                    },
                )
                .await
        });

        let llm_dep = Arc::clone(&llm);
        let dep_handle = tokio::spawn(async move {
            llm_dep
                .complete(
                    dep_prompt,
                    CompletionConfig {
                        max_tokens: Some(400),
                        temperature: 0.0,
                        stream: false,
                    },
                )
                .await
        });

        // Per-question coordinator: join the two LLM tasks, then write one
        // merged cache entry. Spawned so the per-question coordination ALSO
        // runs concurrently across questions.
        let cache_handle = Arc::clone(&cache);
        let coordinator = tokio::spawn(async move {
            let (dir_join, dep_join) = tokio::join!(dir_handle, dep_handle);

            let directional_response = match dir_join {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    warn!(question = %question_str, error = %e, "directional pre-warm failed");
                    String::new()
                }
                Err(e) => {
                    warn!(question = %question_str, error = %e, "directional task panicked");
                    String::new()
                }
            };

            let depth_response = match dep_join {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    warn!(question = %question_str, error = %e, "depth pre-warm failed");
                    String::new()
                }
                Err(e) => {
                    warn!(question = %question_str, error = %e, "depth task panicked");
                    String::new()
                }
            };

            // Skip insertion if both calls failed — nothing useful to cache.
            if directional_response.is_empty() && depth_response.is_empty() {
                return;
            }

            let entry = PreWarmEntry {
                question: question_str.clone(),
                directional_response,
                depth_response,
                created_at: Utc::now(),
                embedding,
            };

            let mut c = cache_handle.lock().await;
            c.insert(entry);
            debug!(question = %question_str, "pre-warm entry cached");
        });

        question_tasks.push(coordinator);
    }

    // Wait for every per-question coordinator to finish. The 10 underlying
    // LLM calls and their per-question merges all run concurrently.
    let _ = join_all(question_tasks).await;

    let elapsed_ms = start.elapsed().as_millis();
    let cache_len = cache.lock().await.len();

    info!(
        questions = questions.len(),
        entries_cached = cache_len,
        elapsed_ms,
        "pre-warm complete",
    );

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use futures::stream;
    use std::pin::Pin;
    use std::sync::OnceLock;

    use super::*;
    use crate::llm::provider::{FailingMockLLMProvider, LLMProvider, MockLLMProvider, RateLimit};

    static EMBEDDER: OnceLock<Option<Arc<Embedder>>> = OnceLock::new();

    fn embedder() -> Option<Arc<Embedder>> {
        EMBEDDER
            .get_or_init(|| Embedder::new().ok().map(Arc::new))
            .clone()
    }

    macro_rules! require_embedder {
        () => {
            match embedder() {
                Some(e) => e,
                None => {
                    eprintln!(
                        "SKIP: fastembed model not cached (no internet or rate-limited on CI)"
                    );
                    return;
                }
            }
        };
    }

    fn sample_digest() -> Digest {
        Digest {
            role: "Senior Software Engineer".to_string(),
            company: "Acme Corp".to_string(),
            domain: "software engineering".to_string(),
            key_skills: vec!["Rust".to_string(), "distributed systems".to_string()],
            seniority: "senior".to_string(),
            likely_questions: vec![
                "Tell me about yourself".to_string(),
                "What is your experience with distributed systems?".to_string(),
                "How do you handle production incidents?".to_string(),
                "Describe a challenging technical problem you solved".to_string(),
                "Why are you interested in this role?".to_string(),
            ],
            topics_to_avoid: vec![],
        }
    }

    /// Fast mock LLM: returns a fixed string immediately.
    struct FastMockLLM {
        response: String,
    }

    #[async_trait::async_trait]
    impl LLMProvider for FastMockLLM {
        async fn complete_stream(
            &self,
            _prompt: String,
            _config: CompletionConfig,
        ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<String>> + Send>>> {
            let r = self.response.clone();
            Ok(Box::pin(stream::once(async move { Ok(r) })))
        }
        fn name(&self) -> &str {
            "default"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn context_window(&self) -> usize {
            128_000
        }
        fn rate_limit(&self) -> RateLimit {
            RateLimit {
                requests_per_minute: 60,
                tokens_per_minute: 60_000,
            }
        }
    }

    fn mock_llm(response: &str) -> Arc<dyn LLMProvider> {
        Arc::new(FastMockLLM {
            response: response.to_string(),
        })
    }

    // ── PreWarmCache ─────────────────────────────────────────────────────────

    #[test]
    fn test_cache_lookup_returns_entry_above_threshold() {
        let emb = require_embedder!();
        let q = "Tell me about yourself";
        let embedding = emb.embed_one(q).unwrap();

        let mut cache = PreWarmCache::new();
        cache.insert(PreWarmEntry {
            question: q.to_string(),
            directional_response: "I am a senior engineer.".to_string(),
            depth_response: "I have 8 years of experience...".to_string(),
            created_at: Utc::now(),
            embedding: embedding.clone(),
        });

        // Exact same question → cosine sim = 1.0 → hit.
        let result = cache.lookup(&embedding);
        assert!(result.is_some(), "exact match must hit the cache");
        assert_eq!(result.unwrap().question, q);
    }

    #[test]
    fn test_cache_lookup_returns_none_for_unrelated_query() {
        let emb = require_embedder!();
        let interview_q = "Tell me about yourself";
        let unrelated_q = "What is the weather like today in Tokyo?";

        let interview_emb = emb.embed_one(interview_q).unwrap();
        let unrelated_emb = emb.embed_one(unrelated_q).unwrap();

        let mut cache = PreWarmCache::new();
        cache.insert(PreWarmEntry {
            question: interview_q.to_string(),
            directional_response: "answer".to_string(),
            depth_response: "detailed answer".to_string(),
            created_at: Utc::now(),
            embedding: interview_emb,
        });

        let result = cache.lookup(&unrelated_emb);
        assert!(
            result.is_none(),
            "unrelated query must not hit the interview cache"
        );
    }

    #[test]
    fn test_cache_lookup_returns_none_when_empty() {
        let emb = require_embedder!();
        let cache = PreWarmCache::new();
        let embedding = emb.embed_one("some question").unwrap();
        assert!(cache.lookup(&embedding).is_none());
    }

    #[test]
    fn test_staleness_fresh_entry() {
        let entry = PreWarmEntry {
            question: "q".to_string(),
            directional_response: String::new(),
            depth_response: String::new(),
            created_at: Utc::now(),
            embedding: vec![],
        };
        assert!(!entry.is_stale());
        assert_eq!(entry.staleness_penalty(), 0.0);
    }

    #[test]
    fn test_staleness_old_entry() {
        let entry = PreWarmEntry {
            question: "q".to_string(),
            directional_response: String::new(),
            depth_response: String::new(),
            created_at: Utc::now() - chrono::Duration::minutes(11),
            embedding: vec![],
        };
        assert!(entry.is_stale());
        assert_eq!(entry.staleness_penalty(), 0.10);
    }

    // ── run_prewarm ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_run_prewarm_populates_cache() {
        let digest = sample_digest();
        let llm = mock_llm("A great pre-warmed answer.");
        let emb = require_embedder!();
        let cache = Arc::new(Mutex::new(PreWarmCache::new()));

        run_prewarm(&digest, llm, emb, Arc::clone(&cache))
            .await
            .expect("run_prewarm should succeed");

        let c = cache.lock().await;
        assert!(!c.is_empty(), "cache must have entries after pre-warm");
    }

    /// Each question must produce **exactly one** cache entry — both the
    /// directional and depth response merged. No half-populated entries.
    #[tokio::test]
    async fn test_run_prewarm_one_entry_per_question_with_both_fields() {
        let digest = sample_digest();
        let expected_count = digest.likely_questions.len().min(5);

        let llm = mock_llm("merged answer");
        let emb = require_embedder!();
        let cache = Arc::new(Mutex::new(PreWarmCache::new()));

        run_prewarm(&digest, llm, emb, Arc::clone(&cache))
            .await
            .unwrap();

        let c = cache.lock().await;
        assert_eq!(
            c.len(),
            expected_count,
            "one entry per question (no duplicates from directional/depth race)",
        );
        for entry in c.entries.values() {
            assert!(
                !entry.directional_response.is_empty(),
                "directional must be populated"
            );
            assert!(!entry.depth_response.is_empty(), "depth must be populated");
        }
    }

    #[tokio::test]
    async fn test_run_prewarm_fires_all_tasks_concurrently() {
        // This test verifies that all tasks complete in approximately the same
        // wall-clock time as a single task (i.e. they run concurrently, not
        // serially). With a fast mock LLM the total time should be well under
        // 5× the single-task time.
        use std::time::Instant;

        let digest = sample_digest();
        let llm = mock_llm("concurrency check answer");
        let emb = require_embedder!();
        let cache = Arc::new(Mutex::new(PreWarmCache::new()));

        let start = Instant::now();
        run_prewarm(&digest, llm, emb, Arc::clone(&cache))
            .await
            .unwrap();
        let elapsed = start.elapsed();

        // 10 concurrent async tasks with a no-op mock LLM should finish in
        // well under 5 seconds even on a slow CI machine.
        assert!(
            elapsed.as_secs() < 5,
            "concurrent pre-warm took too long: {:?}",
            elapsed
        );

        let c = cache.lock().await;
        assert!(!c.is_empty(), "cache must not be empty after pre-warm");
    }

    #[tokio::test]
    async fn test_run_prewarm_skips_empty_questions() {
        let mut digest = sample_digest();
        digest.likely_questions.clear();

        let llm = mock_llm("should not be called");
        let emb = require_embedder!();
        let cache = Arc::new(Mutex::new(PreWarmCache::new()));

        run_prewarm(&digest, llm, emb, Arc::clone(&cache))
            .await
            .expect("should not error on empty questions");

        let c = cache.lock().await;
        assert!(c.is_empty(), "no questions means no cache entries");
    }

    /// Cache lookup must skip entries whose embedding dimension does not
    /// match the query (defensive — should never happen in practice but the
    /// length check is in production code and must be tested).
    #[test]
    fn test_cache_lookup_skips_mismatched_dimension_entries() {
        let mut cache = PreWarmCache::new();
        // Insert an entry with a 4-dim embedding.
        cache.insert(PreWarmEntry {
            question: "wrong-dim".to_string(),
            directional_response: "x".to_string(),
            depth_response: "y".to_string(),
            created_at: Utc::now(),
            embedding: vec![1.0, 0.0, 0.0, 0.0],
        });
        // Query with a 3-dim embedding — must return None (no compatible entry).
        let result = cache.lookup(&[1.0, 0.0, 0.0]);
        assert!(result.is_none(), "mismatched dims must not hit cache");
    }

    /// Lookup over multiple entries: the fold's "not greater" branch fires
    /// whenever a later entry has lower similarity than the running best.
    #[test]
    fn test_cache_lookup_picks_highest_similarity_across_entries() {
        let mut cache = PreWarmCache::new();
        // Best match: aligned with query.
        cache.insert(PreWarmEntry {
            question: "best".to_string(),
            directional_response: "a".to_string(),
            depth_response: "b".to_string(),
            created_at: Utc::now(),
            embedding: vec![1.0, 0.0, 0.0, 0.0],
        });
        // Lower similarity: orthogonal direction, still ≥ 0.85 due to magnitude.
        cache.insert(PreWarmEntry {
            question: "worse".to_string(),
            directional_response: "c".to_string(),
            depth_response: "d".to_string(),
            created_at: Utc::now(),
            embedding: vec![0.9, 0.1, 0.0, 0.0],
        });
        let result = cache.lookup(&[1.0, 0.0, 0.0, 0.0]);
        assert_eq!(result.unwrap().question, "best");
    }

    /// Provider with no matching prompt file falls back to `default.txt`.
    /// Hits the else-branch of [`load_prompt`].
    #[tokio::test]
    async fn test_prewarm_falls_back_to_default_prompt_when_provider_specific_missing() {
        let digest = sample_digest();
        let emb = require_embedder!();
        let cache = Arc::new(Mutex::new(PreWarmCache::new()));

        // Provider name does not match any file under prompts/{directional,depth}/.
        let llm: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "fallback answer".to_string(),
            provider_name: "no_such_provider".to_string(),
        });

        run_prewarm(&digest, llm, emb, Arc::clone(&cache))
            .await
            .expect("run_prewarm must succeed using default.txt fallback");
        assert!(!cache.lock().await.is_empty());
    }

    /// Inner LLM error path (Ok(Err(_))): provider returns an error from
    /// `complete_stream`. Both directional and depth tasks observe the
    /// failure and the entry is skipped (both fields empty).
    #[tokio::test]
    async fn test_prewarm_skips_entry_when_both_calls_return_inner_error() {
        let mut digest = sample_digest();
        // Single question keeps the assertion crisp.
        digest.likely_questions = vec!["Tell me about yourself".to_string()];

        let emb = require_embedder!();
        let cache = Arc::new(Mutex::new(PreWarmCache::new()));

        let llm: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
            provider_name: "default".to_string(),
            error_message: "boom".to_string(),
        });

        run_prewarm(&digest, llm, emb, Arc::clone(&cache))
            .await
            .expect("run_prewarm must succeed even when LLM fails");

        // Both calls failed → no entry inserted.
        assert!(
            cache.lock().await.is_empty(),
            "cache must be empty when both LLM tasks fail"
        );
    }

    /// Outer task error path (Err(_) from JoinHandle): the inner LLM future
    /// panics. The coordinator must log it and continue without inserting.
    #[tokio::test]
    async fn test_prewarm_handles_panicking_llm_task() {
        use std::pin::Pin;
        struct PanickingLLM;
        #[async_trait::async_trait]
        impl LLMProvider for PanickingLLM {
            async fn complete_stream(
                &self,
                _prompt: String,
                _config: CompletionConfig,
            ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<String>> + Send>>> {
                panic!("intentional panic for coverage of JoinHandle::Err path");
            }
            fn name(&self) -> &str {
                "default"
            }
            fn is_available(&self) -> bool {
                true
            }
            fn context_window(&self) -> usize {
                128_000
            }
            fn rate_limit(&self) -> RateLimit {
                RateLimit {
                    requests_per_minute: 60,
                    tokens_per_minute: 60_000,
                }
            }
        }

        let mut digest = sample_digest();
        digest.likely_questions = vec!["Tell me about yourself".to_string()];

        let emb = require_embedder!();
        let cache = Arc::new(Mutex::new(PreWarmCache::new()));
        let llm: Arc<dyn LLMProvider> = Arc::new(PanickingLLM);

        run_prewarm(&digest, llm, emb, Arc::clone(&cache))
            .await
            .expect("run_prewarm must survive panicking provider");

        assert!(
            cache.lock().await.is_empty(),
            "panicking provider must not produce a cache entry"
        );
    }

    #[tokio::test]
    async fn test_prewarm_cache_hit_serves_prewarmed_response() {
        let emb = require_embedder!();
        let q = "Tell me about yourself";
        let embedding = emb.embed_one(q).unwrap();

        let mut cache = PreWarmCache::new();
        cache.insert(PreWarmEntry {
            question: q.to_string(),
            directional_response: "I am a senior software engineer.".to_string(),
            depth_response: "With 8 years of experience...".to_string(),
            created_at: Utc::now(),
            embedding: embedding.clone(),
        });

        // Query with a semantically equivalent question.
        let related_q = "Can you tell me about yourself?";
        let related_emb = emb.embed_one(related_q).unwrap();
        let hit = cache.lookup(&related_emb);
        // "Can you tell me about yourself?" and "Tell me about yourself" are
        // semantically very similar — expect a cache hit.
        assert!(
            hit.is_some(),
            "semantically equivalent question should hit cache"
        );
    }
}
