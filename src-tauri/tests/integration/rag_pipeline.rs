//! End-to-end integration test for the full RAG pipeline.
//!
//! Exercises: chunking → embedding → vector-store ingestion → MMR retrieval →
//! session isolation → cleanup.
//!
//! Reference: Task 2.11 spec, design doc §11 (RAG), §13 (RAG rules).

use flint_lib::interfaces::vector::{Chunk, VectorInterface};
use flint_lib::rag::chunker::chunk_text;
use flint_lib::rag::embedder::Embedder;
use flint_lib::rag::retriever::retrieve;
use flint_lib::rag::store::SqliteVecStore;
use uuid::Uuid;

// ──────────────────────────────────────────────────────────────────────────────
// Test fixture: ~500-word job description centred on distributed systems
// ──────────────────────────────────────────────────────────────────────────────

const JOB_DESCRIPTION: &str = "\
Senior Software Engineer — Distributed Systems

About Us
We are a fast-growing infrastructure company building the next generation of \
real-time data platforms. Our systems process billions of events per day and \
require rock-solid reliability, low latency, and seamless horizontal \
scalability. We are looking for a Senior Software Engineer with deep \
expertise in distributed systems to join our core platform team.

Role Overview
In this role you will design, implement, and operate distributed components \
that underpin our entire product. You will work closely with staff engineers \
to evolve our consensus layer, improve our replication protocols, and harden \
our fault-tolerance guarantees. You will own critical subsystems end-to-end — \
from initial design through production observability.

Key Responsibilities
- Design and implement distributed data pipelines using event streaming \
  technologies such as Apache Kafka and Apache Pulsar.
- Build and maintain high-availability services with consensus algorithms \
  (Raft, Paxos) to ensure strong consistency across replicas.
- Develop internal tooling for tracing, metrics collection, and automated \
  incident response in large distributed environments.
- Collaborate with the infrastructure team to improve service mesh \
  configuration, load-balancing policies, and circuit-breaker behaviour.
- Conduct rigorous code reviews focused on correctness of concurrent and \
  distributed algorithms, paying special attention to race conditions, \
  network partitions, and split-brain scenarios.
- Participate in on-call rotation for tier-one distributed services.
- Mentor junior and mid-level engineers on distributed systems principles, \
  including CAP theorem trade-offs, eventual consistency, and idempotency.

Required Qualifications
- 6+ years of software engineering experience, with at least 3 years focused \
  on distributed systems.
- Proven track record building production-grade distributed systems at scale \
  (millions of requests per second or petabytes of data).
- Deep understanding of consensus protocols (Raft, Paxos, Viewstamped \
  Replication) and their practical implications.
- Experience with at least one systems programming language: Rust, Go, or C++.
- Familiarity with Kubernetes, service meshes (Istio, Linkerd), and cloud \
  infrastructure (AWS, GCP, or Azure).
- Strong grasp of networking fundamentals: TCP/IP, gRPC, HTTP/2, and \
  connection pooling.

Nice to Have
- Contributions to open-source distributed systems projects (etcd, TiKV, \
  CockroachDB, etc.).
- Experience with vector databases or embedding-based search at scale.
- Published research or technical blog posts on distributed systems topics.

What We Offer
- Competitive salary and equity package.
- Remote-first culture with optional hub offices in San Francisco and Berlin.
- Generous hardware budget and conference attendance stipend.
- Collaborative engineering culture with a strong emphasis on technical depth, \
  documentation, and sustainable on-call practices.

We are an equal opportunity employer committed to diversity and inclusion.
";

// ──────────────────────────────────────────────────────────────────────────────
// Helper
// ──────────────────────────────────────────────────────────────────────────────

fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

// ──────────────────────────────────────────────────────────────────────────────
// Integration test
// ──────────────────────────────────────────────────────────────────────────────

/// Full RAG pipeline: chunk → embed → ingest → query → MMR dedup → cleanup.
#[tokio::test]
async fn test_rag_pipeline_end_to_end() {
    // ── 1. Chunk the job description (200-token chunks, 50-token overlap) ────

    let raw_chunks = chunk_text(JOB_DESCRIPTION, 200, 50);
    assert!(
        raw_chunks.len() >= 2,
        "expected at least 2 chunks from a ~500-word JD, got {}",
        raw_chunks.len()
    );

    // ── 2 & 3. Embed all chunks in one batch ─────────────────────────────────

    let embedder = Embedder::new().expect("embedder should initialise");

    let refs: Vec<&str> = raw_chunks.iter().map(|s| s.as_str()).collect();
    let embeddings = embedder
        .embed_batch(&refs)
        .expect("embed_batch should succeed");

    assert_eq!(
        embeddings.len(),
        raw_chunks.len(),
        "one embedding per chunk"
    );
    assert_eq!(
        embeddings[0].len(),
        384,
        "bge-small-en-v1.5 produces 384-dimensional vectors"
    );

    // ── 4. Ingest into SqliteVecStore ─────────────────────────────────────────

    let store = SqliteVecStore::new(":memory:").expect("in-memory store should open");
    let session_id = Uuid::new_v4();

    let chunks: Vec<Chunk> = refs
        .iter()
        .zip(embeddings.iter())
        .map(|(text, embedding)| Chunk {
            id: Uuid::new_v4(),
            text: text.to_string(),
            embedding: embedding.clone(),
            session_id,
        })
        .collect();

    store
        .ingest(session_id, chunks.clone())
        .await
        .expect("ingest should succeed");

    assert_eq!(
        store.chunk_count(session_id),
        raw_chunks.len(),
        "chunk_count should equal number of ingested chunks"
    );

    // ── 5. Query with a semantically related question ─────────────────────────

    let query = "Tell me about your experience with distributed systems";
    let query_embedding = embedder
        .embed_one(query)
        .expect("query embedding should succeed");

    let raw_results = store
        .query(session_id, &query_embedding, 5)
        .await
        .expect("query should succeed");

    assert!(
        !raw_results.is_empty(),
        "query should return results for a JD that mentions distributed systems"
    );

    // ── 6. Assert top result is semantically relevant (score > 0.5) ───────────

    let top_score = raw_results[0].score;
    assert!(
        top_score > 0.5,
        "top result score {top_score:.3} should be > 0.5 for a JD rich in distributed-systems content"
    );

    // ── 7. MMR de-duplication ─────────────────────────────────────────────────
    //
    // Strategy: ingest the original chunks *plus* one exact-duplicate of the
    // first chunk (which is likely the most relevant) into a fresh session.
    // The raw query returns both copies; MMR with lambda=0.7 must suppress the
    // duplicate (NEAR_DUPLICATE_THRESHOLD = 0.99, cosine_sim(dup) = 1.0).

    let dedup_session = Uuid::new_v4();

    let first_chunk = &chunks[0];

    let mut dup_dataset: Vec<Chunk> = chunks
        .iter()
        .map(|c| Chunk {
            id: Uuid::new_v4(),
            text: c.text.clone(),
            embedding: c.embedding.clone(),
            session_id: dedup_session,
        })
        .collect();

    // Inject an exact duplicate of the first (most relevant) chunk.
    dup_dataset.push(Chunk {
        id: Uuid::new_v4(),
        text: first_chunk.text.clone(),
        embedding: first_chunk.embedding.clone(),
        session_id: dedup_session,
    });

    let total_with_dup = dup_dataset.len(); // original + 1 duplicate

    store
        .ingest(dedup_session, dup_dataset)
        .await
        .expect("ingest with duplicate should succeed");

    // Raw query: fetch all chunks — should contain both copies of the first chunk.
    let raw_dedup = store
        .query(dedup_session, &query_embedding, total_with_dup)
        .await
        .expect("raw dedup query should succeed");

    let raw_dup_copies = raw_dedup
        .iter()
        .filter(|r| r.chunk.text == first_chunk.text)
        .count();

    assert!(
        raw_dup_copies >= 2,
        "raw results should contain ≥ 2 copies of the duplicate chunk (found {raw_dup_copies})"
    );

    // MMR retrieve: request top_k = original chunk count (≥ 1 fewer than total
    // with duplicate), so MMR must choose between the two identical copies —
    // and must suppress the second via the near-duplicate threshold.
    let top_k = raw_chunks.len();
    let mmr_results = retrieve(&store, dedup_session, &query_embedding, top_k, 0.7)
        .await
        .expect("MMR retrieve should succeed");

    assert!(
        !mmr_results.is_empty(),
        "MMR retrieve must return at least one result"
    );

    // Verify no near-duplicate pair survives MMR (cosine_sim < 0.99 between any
    // two results, verified via dot product on unit-norm embeddings).
    for i in 0..mmr_results.len() {
        for j in (i + 1)..mmr_results.len() {
            let sim = dot_product(
                &mmr_results[i].chunk.embedding,
                &mmr_results[j].chunk.embedding,
            );
            assert!(
                sim < 0.99,
                "MMR results ({i}, {j}) have cosine similarity {sim:.4} ≥ 0.99 — \
                 near-duplicate was not removed"
            );
        }
    }

    // Explicit check: the duplicate text appears exactly once in MMR results.
    let mmr_dup_copies = mmr_results
        .iter()
        .filter(|r| r.chunk.text == first_chunk.text)
        .count();

    assert_eq!(
        mmr_dup_copies, 1,
        "MMR should keep exactly 1 copy of the near-duplicate chunk (found {mmr_dup_copies})"
    );

    // ── 8. Clean up: delete_session ───────────────────────────────────────────

    store
        .delete_session(session_id)
        .await
        .expect("delete_session should succeed");

    // ── 9. Verify session isolation: subsequent query returns empty results ────

    let after_delete = store
        .query(session_id, &query_embedding, 5)
        .await
        .expect("query after delete should not error");

    assert!(
        after_delete.is_empty(),
        "query after delete_session must return empty results (session isolation)"
    );

    // Also verify the dedup session is unaffected by the original session's deletion.
    let dedup_after = store
        .query(dedup_session, &query_embedding, 1)
        .await
        .expect("dedup session query should still work after other session deleted");

    assert!(
        !dedup_after.is_empty(),
        "dedup session must not be affected by deletion of the other session"
    );
}

/// Chunking produces the expected number of chunks for the ~500-word JD.
#[test]
fn test_jd_chunk_count_is_reasonable() {
    let chunks = chunk_text(JOB_DESCRIPTION, 200, 50);
    // ~500 words / ~150 words-per-chunk = roughly 3–6 chunks with overlap.
    assert!(
        chunks.len() >= 2,
        "expected ≥ 2 chunks from ~500-word JD, got {}",
        chunks.len()
    );
    assert!(
        chunks.len() <= 12,
        "expected ≤ 12 chunks from ~500-word JD, got {}",
        chunks.len()
    );
}

/// All chunks together cover every word in the JD (no words dropped).
#[test]
fn test_chunks_cover_all_words() {
    let chunks = chunk_text(JOB_DESCRIPTION, 200, 50);
    let all_words: Vec<&str> = JOB_DESCRIPTION.split_whitespace().collect();
    let combined = chunks.join(" ");

    // Sample every 20th word — should appear somewhere in the combined chunks.
    for (i, word) in all_words.iter().enumerate().step_by(20) {
        assert!(
            combined.contains(word),
            "word #{i} ({word:?}) not found in any chunk"
        );
    }
}
