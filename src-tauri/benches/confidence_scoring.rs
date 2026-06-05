//! Confidence scoring P95 — monitored.
//!
//! Runs on every directional/depth completion. Cheap pure-function work but
//! we want to catch regressions if anyone adds heavier heuristics later.

use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use flint_lib::confidence::{compute_confidence, ConfidenceSignals};

fn signals_for_case(case: usize) -> ConfidenceSignals {
    let response = match case % 4 {
        0 => "The migration shipped behind a feature flag with a 1% canary, monitored over 48h."
            .to_string(),
        1 => {
            "I think we maybe could have approached it differently, perhaps with a circuit breaker."
                .to_string()
        }
        2 => "I'm sorry, I don't have enough context to answer that question.".to_string(),
        _ => {
            "We chose tokio because the async runtime gave us first-class cancellation primitives."
                .to_string()
        }
    };
    let rag_texts = vec![
        "Tokio is the de facto async runtime for production Rust services.".to_string(),
        "The canary covered 1% of traffic before we rolled forward.".to_string(),
        "Circuit breakers gate downstream calls when error rates spike.".to_string(),
    ];
    ConfidenceSignals {
        rag_grounding: 0.78,
        response_text: response,
        rag_texts,
        provider_name: "llama-3.3-70b-versatile".to_string(),
        cache_stale: case.is_multiple_of(5),
        local_fallback_active: false,
        turn_number: (case % 8) + 1,
    }
}

fn bench_confidence(c: &mut Criterion) {
    let mut group = c.benchmark_group("confidence_scoring");
    group.sample_size(200);
    group.measurement_time(Duration::from_secs(8));

    group.bench_function("compute_mixed", |b| {
        let mut idx = 0_usize;
        b.iter(|| {
            let signals = signals_for_case(idx);
            idx = idx.wrapping_add(1);
            let scored = compute_confidence(black_box(&signals));
            black_box(scored);
        });
    });
    group.finish();
}

criterion_group!(benches, bench_confidence);
criterion_main!(benches);
