# Deepgram STT provider — opt-in, consent-gated, Whisper-fallback

## Context

Flint's only STT today is local whisper-rs (`transcription/engine.rs` +
`audio/live_whisper_worker.rs`), driven per closed VAD chunk. Add Deepgram as
an optional, user-selectable transcription provider. Whisper stays the
default and the mandatory fallback — Deepgram is never the only path.

Audio delivery: batch-per-VAD-chunk against Deepgram's pre-recorded
`/v1/listen` REST endpoint (NOT continuous websocket streaming). Same
trigger point Whisper uses today — do not touch RNNoise/VAD/chunking.

Gate: choosing Deepgram in Settings requires (1) a Deepgram API key (BYOK,
same keychain pattern as every other LLM provider) and (2) an explicit
legal-consent checkbox modal, before the option becomes selectable. Re-check
both server-side before `READY -> LIVE` (never trust the frontend alone).

**Branch:** `feature/deepgram-stt-provider` (from `main`)

## Slices

### Slice 1: deepgram-s1-provider-trait

- Add `transcription/provider.rs`: `TranscriptionProvider` async trait
  (`transcribe`, `name`, `is_available`, `health_check`) — mirror
  `llm/provider.rs::LLMProvider` shape.
- Add `WhisperTranscriptionProvider` wrapping `Arc<WhisperEngine>`; move the
  `tokio::task::spawn_blocking(... transcribe_with_context ...)` call from
  `audio/live_whisper_worker.rs::worker_loop` into this provider's
  `transcribe()` impl.
- Refactor `LiveWhisperWorker` to hold `Arc<dyn TranscriptionProvider>`
  instead of `Arc<WhisperEngine>`; `worker_loop` calls
  `provider.transcribe(chunk, rolling_context).await` directly (no manual
  spawn_blocking left at the call site).
- Zero behavior change for existing Whisper-only users — full existing
  test suite (unit + the mock/live audio integration tests) must still pass
  unmodified in behavior (signatures will change; update call sites/tests
  accordingly).

### Slice 2: deepgram-s2-deepgram-provider

- Add `transcription/deepgram.rs`: `DeepgramTranscriptionProvider`
  implementing `TranscriptionProvider`.
  - PCM16LE encoding of `VadChunk.samples: Vec<f32>`.
  - `POST {base_url}/v1/listen?model=nova-3&language=en&smart_format=true&punctuate=true&encoding=linear16&sample_rate=16000&channels=1`,
    header `Authorization: Token {key}` (`SecretString`, never logged).
  - `base_url` configurable in a `DeepgramConfig` struct (mirror
    `OpenAiCompatConfig` in `llm/openai_compat.rs`) so tests point at a
    `wiremock` server.
  - Parse `results.channels[0].alternatives[0].{transcript,words[]}` into the
    existing `TranscriptionResult` (word_timestamps from `words[].start/end`
    in seconds -> ms; `avg_logprob: None` — document why in a doc comment,
    do not conflate Deepgram confidence with Whisper logprob).
  - Reuse `sanitizer::collapse_repeated_ngrams` + `validate_segment` on the
    returned text; drop empty/invalid results.
  - Short request timeout (3s) via `reqwest::Client` builder — a slow
    network call must never stall the pipeline.
  - Unit tests with `wiremock`: happy path, empty transcript, HTTP 401,
    HTTP 5xx, malformed JSON, timeout.

### Slice 3: deepgram-s3-router-failover

- Add `transcription/router.rs`: `TranscriptionRouter` mirroring
  `llm/failover.rs::FailoverManager` structurally (retry-once ->
  fail over -> background health ping -> restore), but simplified to exactly
  two tiers (primary = Deepgram when configured, local = Whisper, always
  present). No primary configured => call local directly, no overhead.
- Add `TranscriptionFailoverTriggeredPayload` / `TranscriptionPrimaryRestoredPayload`
  to `events.rs` + `emit_transcription_failover_triggered` /
  `emit_transcription_primary_restored`, event names
  `transcription_failover_triggered` / `transcription_primary_restored`.
- Unit tests mirroring the `llm/failover.rs` test suite: hard failure falls
  back, health-check ping restores, already-in-fallback short-circuits.

### Slice 4: deepgram-s4-consent-and-wiring

- `keychain.rs`: add `"deepgram"` to `KNOWN_API_PROVIDERS`; add
  `DEEPGRAM_CONSENT_ENTRY` + `set_deepgram_consent_accepted()` +
  `is_deepgram_consent_accepted()` (mirror `*_legal_consent_accepted`); add
  to `clear_account_secrets()`.
- `session/persistence.rs`: `get_transcription_provider_preference()` /
  `set_transcription_provider_preference()` via the existing
  `get_app_preference`/`set_app_preference` KV store (mirror
  `get_headphone_gate_override`), default `"whisper"`.
- `commands.rs`: `get_transcription_provider_preference`,
  `set_transcription_provider_preference` (server-side rejects `"deepgram"`
  unless key + consent both present), `get_deepgram_consent_status`,
  `accept_deepgram_consent`. Add `require_transcription_provider_ready`
  gate, call it alongside `require_recording_consent` in the live-session
  start path and `commit_live_preview`.
- Replace both `init_whisper_engine(...)` call sites feeding
  `run_audio_pipeline` with a new `build_transcription_router(...)` that
  always builds Whisper and conditionally attaches Deepgram as primary.
  Update `run_audio_pipeline`'s `whisper: Arc<WhisperEngine>` param to
  `router: Arc<TranscriptionRouter>`.
- `cargo test` full pass, `cargo clippy -- -D warnings`.

### Slice 5: deepgram-s5-settings-ui

- `commands/index.ts`: add `"deepgram"` to `ApiKeyProvider`; typed wrappers
  for the four new commands.
- `events/index.ts`: `onTranscriptionFailoverTriggered` /
  `onTranscriptionPrimaryRestored`.
- `src/components/DeepgramConsentModal.tsx` (mirror
  `RecordingConsentModal.tsx`): discloses that audio including the other
  participant's voice leaves the device to Deepgram's cloud, differs from
  Whisper's fully-local processing, and that the user is responsible for any
  required third-party consent. Checkbox required before confirm is enabled.
- `src/screens/TranscriptionSettings.tsx` (new, mirror
  `ProviderSettings.tsx` structure): Whisper/Deepgram choice, opens consent
  modal on first switch to Deepgram, reuses `ProviderEntry` for the Deepgram
  key row, option disabled until key + consent present.
- `Settings.tsx`: add `"transcription"` tab wired to the new component.
- Live-session status indicator on `transcription_failover_triggered` /
  `transcription_primary_restored` (mirror the existing LLM failover
  indicator).
- `npm run test` pass.

### Slice 6: deepgram-s6-pr-ci

- `cargo test`, `cargo clippy -- -D warnings`, `npm run test`.
- Add a short `docs/ROADMAP.md` entry documenting the feature and the
  batch-per-chunk (not streaming) scope decision.
- Push, open PR to `main`, CI loop until green, merge.

## Do NOT change

- RNNoise/VAD constants, chunk boundaries, or `audio/pipeline.rs`'s
  RNNoise -> Downsampler -> VadChunker flow.
- Whisper.cpp decode parameters in `transcription/engine.rs` (`flint-audio.mdc`
  §26 contract).
- Any LLM provider/failover code in `llm/` — this is a parallel, separate
  trait hierarchy for transcription, not a reuse of `LLMProvider`.
- Do not implement continuous/streaming websocket ingestion to Deepgram —
  batch-per-VAD-chunk only, per the confirmed scope decision above.
