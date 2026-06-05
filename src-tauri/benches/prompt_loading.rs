//! Prompt loading P95 — monitored, not gated.
//!
//! Every LLM call lazily reads its template from `prompts/<category>/<model>.txt`.
//! The path lookup must stay cheap because it runs on the hot directional and
//! depth paths.

use std::path::PathBuf;
use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use flint_lib::orchestrator::load_prompt;

fn prompts_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap().join("prompts")
}

fn bench_load_prompt(c: &mut Criterion) {
    let dir = prompts_dir();
    let mut group = c.benchmark_group("prompt_loading");
    group.sample_size(100);
    group.measurement_time(Duration::from_secs(8));

    let cases: &[(&str, &str)] = &[
        ("directional", "llama"),
        ("depth", "llama"),
        ("clarifying", "llama"),
        ("question_detection", "llama"),
        ("digest", "llama"),
    ];

    for (category, model) in cases {
        let label = format!("{category}__{model}");
        group.bench_function(&label, |b| {
            b.iter(|| {
                let prompt = load_prompt(black_box(category), black_box(model), black_box(&dir))
                    .expect("load prompt");
                black_box(prompt);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_load_prompt);
criterion_main!(benches);
