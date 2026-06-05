//! Question detection P95 — NFR ≤ 100ms.
//!
//! Pass 1 is rule-based and runs on the hot path before any LLM call. We
//! bench it without an Ollama provider (Tier 1 config) which forces ambiguous
//! results to resolve via Pass 1 fallback only — matching the latency floor
//! the orchestrator pipeline relies on.

use std::path::PathBuf;
use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};

use flint_lib::transcription::detector::QuestionDetector;

/// Mixed corpus that exercises every Pass 1 branch:
/// question prefixes, statements, ambiguous prefixes, near-noise.
const UTTERANCES: &[&str] = &[
    "What is your approach to scaling distributed systems?",
    "Tell me about a time you handled conflict.",
    "How would you design a rate limiter?",
    "Walk me through your CI pipeline.",
    "I led the migration to Kubernetes last year.",
    "We shipped the redesign in Q3.",
    "Could you describe a failure mode you debugged?",
    "Describe the architecture you'd choose.",
    "The team rolled back after the canary failed.",
    "Why did you choose Rust for that service?",
    "When did you join the platform team?",
    "Which database did you pick and why?",
    "Explain how the cache invalidation works.",
    "I think the timeline was aggressive.",
    "Give me an example of a trade-off you made.",
    "What's your favourite testing strategy?",
    "Walk us through your debugging methodology.",
    "Honestly, the architecture was over-engineered.",
    "Help me understand the failover path.",
    "Where do you see yourself in five years?",
];

fn locate_prompts_dir() -> PathBuf {
    // Benches run from the src-tauri crate root, so prompts/ is one level up.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap().join("prompts")
}

fn bench_question_detection(c: &mut Criterion) {
    let prompts = locate_prompts_dir();
    // Tier 1 + no Ollama keeps the bench focused on Pass 1, which is the
    // path the rolling P95 gate is enforced against in production.
    let detector = QuestionDetector::new(1, None, &prompts).expect("build detector");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("question_detection");
    group.sample_size(100);
    group.measurement_time(Duration::from_secs(10));

    group.bench_function("pass1_mixed_corpus", |b| {
        let mut idx = 0_usize;
        b.iter_batched(
            || {
                let utterance = UTTERANCES[idx % UTTERANCES.len()];
                idx = idx.wrapping_add(1);
                utterance
            },
            |utterance| {
                rt.block_on(async {
                    let is_q = detector.detect(black_box(utterance)).await.expect("detect");
                    black_box(is_q);
                });
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(benches, bench_question_detection);
criterion_main!(benches);
