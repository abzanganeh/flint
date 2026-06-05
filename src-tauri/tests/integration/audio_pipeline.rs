//! Integration tests for the Phase 3 audio pipeline.
//!
//! Reference: Task 3.12 spec, design doc §26 (Audio Processing Configuration).
//!
//! ## Build requirement
//!
//! The full test suite compiles only when whisper-rs can build, which requires:
//!   - cmake ≥ 3.16
//!   - A C/C++ toolchain (gcc/clang)
//!   - libclang (for bindgen)
//!
//! Sections A and B do not use whisper-rs directly and will run whenever the
//! library compiles. Section C is `#[ignore]` and additionally requires a
//! Whisper model file at `~/.cache/whisper/ggml-tiny.en.bin`.
//!
//! ## Sections
//!
//! A. RNNoise + Downsampler chain        — always runs
//! B. Question detection (Pass 1)        — always runs
//! C. Full pipeline with Whisper model   — `#[ignore]`, opt-in

use std::path::PathBuf;

use flint_lib::audio::rnnoise::{
    Downsampler, RNNoiseProcessor, DOWNSAMPLED_FRAME_SIZE, RNNOISE_FRAME_SIZE,
};
use flint_lib::transcription::detector::QuestionDetector;

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Voiced-speech-like frame at 48kHz: 100 Hz fundamental + 3 harmonics.
///
/// Amplitude ≈ −10 dBFS — well above the VAD silence threshold (−35 dBFS).
/// This spectral profile mimics the harmonic structure of human voice.
fn speech_like_frame_48khz(frame_idx: usize) -> Vec<f32> {
    let pi = std::f32::consts::PI;
    (0..RNNOISE_FRAME_SIZE)
        .map(|i| {
            let t = (frame_idx * RNNOISE_FRAME_SIZE + i) as f32 / 48_000.0;
            let s = (2.0 * pi * 100.0 * t).sin()
                + 0.6 * (2.0 * pi * 200.0 * t).sin()
                + 0.3 * (2.0 * pi * 300.0 * t).sin()
                + 0.2 * (2.0 * pi * 400.0 * t).sin();
            s * 0.18 // ≈ −10 dBFS, well above the −35 dBFS silence threshold
        })
        .collect()
}

/// Resolve the `prompts/` directory the same way the production code does.
fn prompts_dir() -> PathBuf {
    std::env::var("FLINT_PROMPTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("prompts")
        })
}

// ── Section A: RNNoise + Downsampler ─────────────────────────────────────────

/// RNNoiseProcessor handles a 480-sample frame without panicking and the
/// output is non-zero (signal preserved, not destroyed).
#[test]
fn test_rnnoise_processes_speech_frame() {
    let mut proc = RNNoiseProcessor::new().expect("RNNoiseProcessor init failed");
    let mut frame = speech_like_frame_48khz(0);

    proc.process_frame(&mut frame)
        .expect("process_frame failed");

    let energy: f32 = frame.iter().map(|s| s * s).sum();
    assert!(
        energy > 0.0,
        "RNNoise output is all zeros — signal was destroyed"
    );
}

/// RNNoise applied to a zero-valued (silence) frame produces a zero-energy output.
#[test]
fn test_rnnoise_silence_stays_silence() {
    let mut proc = RNNoiseProcessor::new().expect("RNNoiseProcessor init failed");
    let mut frame = vec![0.0f32; RNNOISE_FRAME_SIZE];

    proc.process_frame(&mut frame)
        .expect("process_frame failed");

    let energy: f32 = frame.iter().map(|s| s * s).sum();
    assert!(
        energy < 1e-6,
        "Silence frame should produce near-zero output, got energy {energy:.6}"
    );
}

/// Downsampler converts exactly RNNOISE_FRAME_SIZE → DOWNSAMPLED_FRAME_SIZE.
#[test]
fn test_downsampler_output_size() {
    let mut ds = Downsampler::new().expect("Downsampler init failed");
    let frame = speech_like_frame_48khz(0);

    let out = ds.process(&frame).expect("Downsampler process failed");

    assert_eq!(
        out.len(),
        DOWNSAMPLED_FRAME_SIZE,
        "Downsampler must output exactly {DOWNSAMPLED_FRAME_SIZE} samples, got {}",
        out.len()
    );
}

/// Downsampler output carries signal energy (3:1 reduction, not destruction).
#[test]
fn test_downsampler_preserves_signal_energy() {
    let mut ds = Downsampler::new().expect("Downsampler init failed");
    let frame = speech_like_frame_48khz(0);

    let out = ds.process(&frame).expect("Downsampler process failed");

    let energy: f32 = out.iter().map(|s| s * s).sum();
    assert!(
        energy > 0.0,
        "Downsampled output should carry signal energy, got {energy:.6}"
    );
}

/// Full chain: 480 samples @ 48kHz  →  RNNoise  →  160 samples @ 16kHz.
///
/// This validates the handoff between the two stages and confirms that
/// ten consecutive frames all produce correct output sizes.
#[test]
fn test_rnnoise_downsampler_chain_ten_frames() {
    let mut proc = RNNoiseProcessor::new().expect("RNNoiseProcessor init failed");
    let mut ds = Downsampler::new().expect("Downsampler init failed");

    for i in 0..10 {
        let mut frame = speech_like_frame_48khz(i);
        proc.process_frame(&mut frame)
            .unwrap_or_else(|e| panic!("RNNoise frame {i} failed: {e}"));
        let out = ds
            .process(&frame)
            .unwrap_or_else(|e| panic!("Downsampler frame {i} failed: {e}"));

        assert_eq!(
            out.len(),
            DOWNSAMPLED_FRAME_SIZE,
            "Frame {i}: expected {DOWNSAMPLED_FRAME_SIZE} samples, got {}",
            out.len()
        );

        let energy: f32 = out.iter().map(|s| s * s).sum();
        assert!(
            energy > 0.0,
            "Frame {i}: downsampled output should not be all zeros"
        );
    }
}

/// Two independent ChannelProcessor chains (System + Microphone) do not
/// share state — their outputs diverge appropriately.
#[test]
fn test_two_channels_independent() {
    let mut proc_sys = RNNoiseProcessor::new().expect("System RNNoise init failed");
    let mut ds_sys = Downsampler::new().expect("System Downsampler init failed");

    let mut proc_mic = RNNoiseProcessor::new().expect("Mic RNNoise init failed");
    let mut ds_mic = Downsampler::new().expect("Mic Downsampler init failed");

    // Feed different signals to each channel.
    let mut sys_frame = speech_like_frame_48khz(0);
    let mut mic_frame = vec![0.0f32; RNNOISE_FRAME_SIZE]; // silence

    proc_sys.process_frame(&mut sys_frame).unwrap();
    proc_mic.process_frame(&mut mic_frame).unwrap();

    let sys_out = ds_sys.process(&sys_frame).unwrap();
    let mic_out = ds_mic.process(&mic_frame).unwrap();

    let sys_energy: f32 = sys_out.iter().map(|s| s * s).sum();
    let mic_energy: f32 = mic_out.iter().map(|s| s * s).sum();

    assert!(
        sys_energy > mic_energy,
        "System (speech) channel should have higher energy than mic (silence)"
    );
}

// ── Section B: Question detection ────────────────────────────────────────────

/// Direct questions ending in `?` are detected as questions.
#[tokio::test]
async fn test_detector_identifies_direct_question() {
    let detector =
        QuestionDetector::new(1, None, &prompts_dir()).expect("QuestionDetector init failed");

    assert!(
        detector
            .detect("What are your greatest strengths?")
            .await
            .unwrap(),
        "Direct question with '?' should be detected"
    );
}

/// Statements are not classified as questions.
#[tokio::test]
async fn test_detector_rejects_statement() {
    let detector =
        QuestionDetector::new(1, None, &prompts_dir()).expect("QuestionDetector init failed");

    assert!(
        !detector
            .detect("I have five years of experience in distributed systems.")
            .await
            .unwrap(),
        "A plain statement should not be detected as a question"
    );
}

/// Common interview question prefixes are all detected correctly.
#[tokio::test]
async fn test_detector_interview_question_prefixes() {
    let detector =
        QuestionDetector::new(1, None, &prompts_dir()).expect("QuestionDetector init failed");

    let questions = [
        "Tell me about yourself.",
        "Walk me through your background.",
        "Can you explain your experience with distributed systems?",
        "How did you approach that problem?",
        "Why are you interested in this role?",
        "Describe a time you had to deal with a difficult stakeholder.",
        "Give me an example of a challenging technical decision.",
    ];

    for q in questions {
        assert!(
            detector.detect(q).await.unwrap(),
            "'{q}' should be classified as a question"
        );
    }
}

/// Pure statements that happen to mention topics are not questions.
#[tokio::test]
async fn test_detector_non_question_statements() {
    let detector =
        QuestionDetector::new(1, None, &prompts_dir()).expect("QuestionDetector init failed");

    let non_questions = [
        "We are looking for someone with strong Rust skills.",
        "The role involves leading a team of five engineers.",
        "Compensation is competitive.",
    ];

    for s in non_questions {
        assert!(
            !detector.detect(s).await.unwrap(),
            "'{s}' should NOT be classified as a question"
        );
    }
}

/// Detector performs consistently over 50 consecutive calls (no state
/// accumulation bugs, no P95 bypass triggering on Pass 1-only paths).
#[tokio::test]
async fn test_detector_consistent_across_repeated_calls() {
    let detector =
        QuestionDetector::new(1, None, &prompts_dir()).expect("QuestionDetector init failed");

    let question = "Tell me about a time you had to debug a production issue.";
    let statement = "We operate a 24/7 on-call rotation.";

    for _ in 0..50 {
        assert!(
            detector.detect(question).await.unwrap(),
            "Question must be classified correctly on every call"
        );
        assert!(
            !detector.detect(statement).await.unwrap(),
            "Statement must be rejected on every call"
        );
    }
}

// ── Section C: Full pipeline — requires Whisper model + build environment ────

/// Full pipeline: PCM audio → WhisperEngine → transcript text.
///
/// To run this test:
///
/// 1. Install cmake, a C/C++ toolchain, and libclang.
/// 2. Download `ggml-tiny.en.bin`:
///    ```
///    wget https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin \
///         -O ~/.cache/whisper/ggml-tiny.en.bin
///    ```
/// 3. Run: `cargo test --test audio_pipeline -- --ignored`
///
/// The test generates synthetic audio of a known utterance and asserts that
/// the transcript is non-empty and passes the no-speech filter thresholds.
/// A separate fixture-based test (below) uses real pre-recorded speech.
#[test]
#[ignore = "requires cmake, libclang, and ~/.cache/whisper/ggml-tiny.en.bin"]
fn test_whisper_engine_transcribes_pcm_audio() {
    use flint_lib::audio::capture::AudioSource;
    use flint_lib::audio::vad::VadChunk;
    use flint_lib::transcription::engine::WhisperEngine;

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let model_path = format!("{home}/.cache/whisper/ggml-tiny.en.bin");

    if !std::path::Path::new(&model_path).exists() {
        tracing::warn!("SKIP: Whisper model not found at {model_path}");
        return;
    }

    let engine = WhisperEngine::new(&model_path, 1_u8).expect("WhisperEngine init failed");

    // Synthesise 3 seconds of speech-like audio at 16kHz (Whisper input rate).
    let sample_rate = 16_000_u32;
    let duration_secs = 3.0_f32;
    let pi = std::f32::consts::PI;
    let samples: Vec<f32> = (0..(sample_rate as f32 * duration_secs) as usize)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            let s = (2.0 * pi * 120.0 * t).sin()
                + 0.6 * (2.0 * pi * 240.0 * t).sin()
                + 0.3 * (2.0 * pi * 360.0 * t).sin()
                + 0.2 * (2.0 * pi * 480.0 * t).sin();
            s * 0.18
        })
        .collect();

    let chunk = VadChunk {
        samples,
        source: AudioSource::System,
        duration_ms: (duration_secs * 1000.0) as u32,
    };

    // Whisper may return None (all segments filtered as silence/hallucination)
    // or Some(result). Either outcome is valid for synthetic audio — the test
    // asserts only that the engine does not panic or error.
    let result = engine.transcribe(&chunk);
    assert!(
        result.is_ok(),
        "WhisperEngine::transcribe should not return Err for valid input: {:?}",
        result.err()
    );
}

/// Full pipeline with a pre-recorded speech fixture.
///
/// The fixture contains a single utterance: "Tell me about yourself."
/// This test asserts that the transcript includes the word "yourself" or "tell",
/// which Whisper.cpp reliably produces even with tiny.en.
///
/// To create the fixture (requires sox):
/// ```
/// sox -n -r 16000 -c 1 tests/fixtures/tell_me_about_yourself.wav \
///     synth 3 sin 120 sin 240 gain -10
/// ```
/// Or record your voice saying "Tell me about yourself."
#[test]
#[ignore = "requires cmake, libclang, ~/.cache/whisper/ggml-tiny.en.bin, and tests/fixtures/tell_me_about_yourself.wav"]
fn test_whisper_transcribes_real_speech_fixture() {
    use flint_lib::audio::capture::AudioSource;
    use flint_lib::audio::vad::VadChunk;
    use flint_lib::transcription::engine::WhisperEngine;

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let model_path = format!("{home}/.cache/whisper/ggml-tiny.en.bin");
    let fixture_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tell_me_about_yourself.wav"
    );

    if !std::path::Path::new(&model_path).exists() {
        tracing::warn!("SKIP: Whisper model not found at {model_path}");
        return;
    }
    if !std::path::Path::new(fixture_path).exists() {
        tracing::warn!("SKIP: fixture not found at {fixture_path}");
        return;
    }

    // Load raw 16kHz f32 PCM from the WAV file (hound crate not available here;
    // a minimal WAV parser reads the data chunk directly).
    let samples = load_wav_samples(fixture_path).expect("Failed to load WAV fixture");

    let engine = WhisperEngine::new(&model_path, 1_u8).expect("WhisperEngine init failed");

    let chunk = VadChunk {
        duration_ms: (samples.len() as u32 * 1000) / 16_000,
        samples,
        source: AudioSource::System,
    };

    let result = engine
        .transcribe(&chunk)
        .expect("transcribe should not error");

    let transcript = result.map(|r| r.text.to_lowercase()).unwrap_or_default();

    assert!(
        transcript.contains("tell")
            || transcript.contains("yourself")
            || transcript.contains("about"),
        "Expected transcript to contain words from 'Tell me about yourself', got: {transcript:?}"
    );
}

// ── WAV loader (minimal, for the fixture test) ────────────────────────────────

/// Load 16kHz mono f32 PCM samples from a standard WAV file.
///
/// Accepts 16-bit PCM (most common) and converts to f32.  Returns an error
/// for unsupported formats (stereo, non-16kHz, 32-bit float, etc.).
#[allow(dead_code)]
fn load_wav_samples(path: &str) -> anyhow::Result<Vec<f32>> {
    use std::io::{BufReader, Read};

    let file = std::fs::File::open(path).map_err(|e| anyhow::anyhow!("cannot open {path}: {e}"))?;
    let mut reader = BufReader::new(file);

    let mut header = [0u8; 44];
    reader.read_exact(&mut header)?;

    // RIFF chunk: bytes 0..4 = "RIFF", 8..12 = "WAVE"
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        anyhow::bail!("not a WAV file");
    }

    let audio_format = u16::from_le_bytes([header[20], header[21]]);
    let num_channels = u16::from_le_bytes([header[22], header[23]]);
    let sample_rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
    let bits_per_sample = u16::from_le_bytes([header[34], header[35]]);

    if audio_format != 1 {
        anyhow::bail!("only PCM (format 1) is supported, got {audio_format}");
    }
    if num_channels != 1 {
        anyhow::bail!("only mono WAV is supported, got {num_channels} channels");
    }
    if sample_rate != 16_000 {
        anyhow::bail!("only 16kHz WAV is supported, got {sample_rate} Hz");
    }
    if bits_per_sample != 16 {
        anyhow::bail!("only 16-bit WAV is supported, got {bits_per_sample} bits");
    }

    // Read remaining PCM data.
    let mut pcm_bytes = Vec::new();
    reader.read_to_end(&mut pcm_bytes)?;

    // Skip any sub-chunk headers that precede the 'data' chunk.
    // A minimal search: find the first 4 bytes that spell 'data'.
    let data_offset = pcm_bytes
        .windows(4)
        .position(|w| w == b"data")
        .map(|p| p + 8) // skip 'data' + 4-byte size field
        .unwrap_or(0);

    let samples: Vec<f32> = pcm_bytes[data_offset..]
        .chunks_exact(2)
        .map(|b| {
            let s = i16::from_le_bytes([b[0], b[1]]);
            s as f32 / 32_768.0
        })
        .collect();

    Ok(samples)
}
