# Mock STT reliability — decouple capture from Whisper

## Context

Mock "tell me about yourself" answers (~90s) produce garbled, lagging, truncated
transcripts. Root cause: `capture_loop` awaits Whisper inline, blocking frame
drain; `Paused` stops WAV/STT; `EndTurn` never calls `vad.force_end_segment()`.

Related: PR #46 (ALSA enum SIGFPE on mock mic open) ships on same branch.

**Branch:** `feature/mock-stt-reliability` (from `fix/mock-mic-alsa-enum-sigfpe`)

## Slices

### Slice 1: mock-stt-s1-whisper-worker

- Add `mock/whisper_worker.rs` — ordered worker, TurnEpoch, Reset/Transcribe/Flush/Shutdown
- Worker owns transcript_buf, rolling_context, logprob accumulator
- Unit tests for epoch, flush barrier, stale drop

### Slice 2: mock-stt-s2-decouple-capture

- Refactor `mic_capture.rs` — hot path uses `try_transcribe`, never awaits Whisper
- Paused is UI-only: WAV + STT continue during `Paused`
- Wire worker into `MicCapture::start` / `shutdown`

### Slice 3: mock-stt-s3-end-turn-flush

- EndTurn: close cpal → try_recv drain → `force_end_segment` → Flush barrier
- Remove 300/150ms timing heuristics
- Integration-style unit tests in mic_capture / whisper_worker

### Slice 4: mock-stt-s4-pr-ci

- `cargo test`, `cargo clippy -- -D warnings`, `npm run test`
- Push, open PR, CI green

## Do NOT change

- `audio/pipeline.rs` (live path — separate effort)
- VAD constants (600ms gap, mode 3)
- `MOCK_TURN_SILENCE_MS` / `MOCK_MIN_SPEECH_MS`
- MicCapture public Tauri API / mock_turn_phase event strings
