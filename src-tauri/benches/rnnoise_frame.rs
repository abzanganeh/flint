//! RNNoise per-frame P95 — NFR < 5ms.
//!
//! One 480-sample (10ms @ 48kHz) frame is the smallest unit the audio
//! pipeline ever processes. We bench in two configurations:
//!   - `denoise_only` — RNNoise alone (matches the NFR spec line item)
//!   - `denoise_plus_downsample` — pipeline-realistic 48kHz → 16kHz path

use std::f32::consts::PI;
use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};

use flint_lib::audio::rnnoise::{Downsampler, RNNoiseProcessor, RNNOISE_FRAME_SIZE};

/// Synthetic 10ms frame: composite sine (voice fundamental + harmonics)
/// plus white-noise floor so RNNoise has real work to do per frame.
fn synthetic_frame() -> [f32; RNNOISE_FRAME_SIZE] {
    let mut buf = [0.0_f32; RNNOISE_FRAME_SIZE];
    let mut noise_state: u64 = 0x9E37_79B9_7F4A_7C15;
    for (i, sample) in buf.iter_mut().enumerate() {
        let t = i as f32 / 48_000.0;
        let voice = 0.6 * (2.0 * PI * 220.0 * t).sin()
            + 0.3 * (2.0 * PI * 440.0 * t).sin()
            + 0.1 * (2.0 * PI * 880.0 * t).sin();
        noise_state = noise_state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1442695040888963407);
        let noise = ((noise_state >> 40) as f32) / (u32::MAX as f32) - 0.5;
        *sample = voice * 0.6 + noise * 0.05;
    }
    buf
}

fn bench_rnnoise(c: &mut Criterion) {
    let mut group = c.benchmark_group("rnnoise_frame");
    group.sample_size(200);
    group.measurement_time(Duration::from_secs(10));

    let frame = synthetic_frame();

    group.bench_function("denoise_only", |b| {
        // Fresh processor per outer iteration to avoid first-call warmup
        // skewing P95 — the production pipeline reuses the same instance
        // across thousands of frames so we want steady-state numbers, which
        // is exactly what `iter_batched` with `SmallInput` gives us.
        let mut proc = RNNoiseProcessor::new().expect("rnnoise");
        b.iter_batched(
            || frame,
            |mut samples| {
                proc.process_frame(black_box(&mut samples))
                    .expect("denoise");
                black_box(samples);
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("denoise_plus_downsample", |b| {
        let mut proc = RNNoiseProcessor::new().expect("rnnoise");
        let mut down = Downsampler::new().expect("downsampler");
        b.iter_batched(
            || frame,
            |mut samples| {
                proc.process_frame(&mut samples).expect("denoise");
                let downsampled = down.process(&samples).expect("downsample");
                black_box(downsampled);
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group!(benches, bench_rnnoise);
criterion_main!(benches);
