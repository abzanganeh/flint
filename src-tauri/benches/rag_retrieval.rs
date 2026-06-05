//! RAG retrieval P95 — NFR < 50ms, CI gate < 60ms.
//!
//! Drives the real `retrieve()` function against an in-memory `SqliteVecStore`
//! seeded with synthetic 384-dim embeddings. The benchmark covers the full
//! query → MMR de-dup → top-k path that runs on every detected question.

use std::sync::Arc;
use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use uuid::Uuid;

use flint_lib::interfaces::vector::{Chunk, VectorInterface};
use flint_lib::rag::retriever::retrieve;
use flint_lib::rag::store::SqliteVecStore;

const EMBEDDING_DIM: usize = 384;
const TOP_K: usize = 10;
const LAMBDA: f32 = 0.7;

/// Deterministic pseudo-random embedding generator. Uses a linear
/// congruential RNG so the same seed always produces the same vector —
/// keeps the benchmark reproducible without pulling in a randomness crate.
fn pseudo_embedding(seed: u64) -> Vec<f32> {
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    let mut out = Vec::with_capacity(EMBEDDING_DIM);
    let mut norm_sq = 0.0_f32;
    for _ in 0..EMBEDDING_DIM {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1442695040888963407);
        let normalised = ((state >> 33) as f32) / (u32::MAX as f32) - 0.5;
        norm_sq += normalised * normalised;
        out.push(normalised);
    }
    let norm = norm_sq.sqrt().max(f32::EPSILON);
    for v in out.iter_mut() {
        *v /= norm;
    }
    out
}

/// Build a populated in-memory store. Returns the store handle and the
/// session id the chunks were ingested under.
fn build_store(corpus_size: usize) -> (Arc<dyn VectorInterface>, Uuid) {
    let store: Arc<dyn VectorInterface> =
        Arc::new(SqliteVecStore::new(":memory:").expect("in-memory store"));
    let session_id = Uuid::new_v4();
    let chunks: Vec<Chunk> = (0..corpus_size)
        .map(|i| Chunk {
            id: Uuid::new_v4(),
            session_id,
            text: format!("synthetic chunk number {i}"),
            embedding: pseudo_embedding(i as u64 + 1),
        })
        .collect();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        store.ingest(session_id, chunks).await.expect("ingest");
    });
    (store, session_id)
}

fn bench_rag_retrieval(c: &mut Criterion) {
    let query = pseudo_embedding(u64::MAX);

    let mut group = c.benchmark_group("rag_retrieval");
    group.sample_size(100);
    group.measurement_time(Duration::from_secs(15));

    for corpus_size in [100_usize, 500, 1_000] {
        let (store, session_id) = build_store(corpus_size);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        group.throughput(Throughput::Elements(corpus_size as u64));
        group.bench_with_input(
            BenchmarkId::new("retrieve_top_k", corpus_size),
            &corpus_size,
            |b, _| {
                b.iter(|| {
                    rt.block_on(async {
                        let scored =
                            retrieve(store.as_ref(), session_id, black_box(&query), TOP_K, LAMBDA)
                                .await
                                .expect("retrieve");
                        black_box(scored);
                    });
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_rag_retrieval);
criterion_main!(benches);
