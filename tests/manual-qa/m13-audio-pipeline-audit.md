# M13 Audio Pipeline Deep Review — Audit Report (Slice 1)

Branch: `feature/m13-audio-pipeline-deep-review`
Scope: Live and Mock audio pipelines from cpal capture through orchestrator/coach prompt assembly.
Method: Read-only walk of every stage; no code changes in this slice.

This is the ground-truth report against which the M10 design and the rules in
`.cursor/rules/flint-core.mdc` and `.cursor/rules/flint-performance.mdc` are
compared. Each finding has a severity (P0 catastrophic, P1 high, P2 medium)
and points to the slice that fixes it.

---

## 1. Live audio path

```
cpal callback (48 kHz native)
  -> mono + rubato resample (capture.rs)
  -> ring buffer (16_384 f32)
  -> AudioFrame { 480 samples @ 48 kHz, source }
  -> Mic only:  RNNoiseProcessor::process_frame   (480 @ 48 kHz, in place)
  -> Downsampler::process                         (480 -> 160 @ 16 kHz)
  -> VadChunker::process_frame                    (320-sample frames, mode 3)
  -> VadChunk { samples, source, duration_ms }
  -> WhisperEngine::transcribe (spawn_blocking)
  -> CrossChannelDedup::should_suppress           (Jaccard 0.85, 500 ms)
  -> emit_transcription_chunk + persist
  -> if Microphone: stop here (mic quality monitor only)
  -> if System and not phone-mode-manual-only:
       SystemTranscriptBuffer::append
       HybridQuestionDetector::ingest_transcript
       -> ConfirmPlan::Immediate | WithLlm
       -> question_tx -> orchestrator
```

Wiring confirmation:
- `commands.rs:2971-2984` spawns `run_audio_pipeline` with
  `echo_suppression_enabled = !is_phone_call_mode` and
  `phone_mode_manual_only = is_phone_call_mode`. Both flags reach the runner.
- `audio/pipeline.rs` honours both: echo gate active in non-phone mode,
  hybrid detection skipped in phone-manual mode.
- `ChannelProcessor::new_system()` deliberately skips RNNoise on the
  loopback (digital signal). Mic uses RNNoise. Matches design.
- `tokio::select!` over `system_rx` and `mic_rx` is unbiased. No
  starvation under continuous YouTube/Zoom audio.

## 2. Mock audio path

```
Conductor turn loop (mock/conductor.rs:340-417)
  tokio::select! {
    user command (Skip/Abort/etc) => stop_active TTS, branch on command
    spawn run_suggested_answer LLM in parallel
    async {
      tts::speak_best_effort(&question).await    // BLOCKS until playback done
      emit mock_question_spoken
      mic_listen_tx.send(turn_n)                 // arms listening AFTER TTS
    }
  }

MicCapture (mock/mic_capture.rs)
  cpal mock mic stream (PipeWire/pulse plugin on Linux, raw default elsewhere)
  -> 480 samples @ 48 kHz
  -> RNNoise (denoise)
  -> Downsampler 48 -> 16 kHz
  -> VAD process_frame (Microphone source tag)
  -> on speech_in_progress in Listening: enter_answering immediately
  -> on Answering with VadChunk: WhisperEngine::transcribe_with_context
  -> emit_mock_user_transcribed + append to transcript_buf
  -> end_turn drains frames, returns (text, wav_path, avg_logprob)
  -> commands::end_mock_turn applies sanitize_mock_transcript
     -> CoachFeedback prompt uses sanitized text + suggested_answer
```

Wiring confirmation:
- TTS playback completes before `mic_listen_tx.send` is reached. The
  outer `select!` can interrupt with a user command, but during normal
  flow TTS finishes first, then mic begins listening. So Listening does
  not race against active TTS subprocesses.
- Mic stream is opened on every turn and closed on EndTurn. The cpal
  device handle is released between turns so other apps can use it.
- Whisper engine instance is shared between Live and Mock paths
  (`AppState::whisper`). Same params, same model file.

## 3. Speaker labeling — channel mapping and break points

Today's invariant: source channel == speaker.
- `AudioSource::System` -> `speaker = "System"` (interviewer / loopback)
- `AudioSource::Microphone` -> `speaker = "Microphone"` (user mic)
- Mock turns force `AudioSource::Microphone` because there is only one
  stream and it is always the user.
- Phone-call mode (`AudioCapture::start_phone_mode`) opens a single mic
  cpal stream and tags every frame as `AudioSource::System`. In this
  mode, channel identity is meaningless — both interviewer (over phone
  speaker) and user share one channel. The system therefore disables
  the cross-channel echo gate (`echo_suppression_enabled = false`) and
  the auto question detector (`phone_mode_manual_only = true`). User
  must press Ctrl+Q to mark question end. Confirmed wired.

Where it can break in non-phone mode:
- a. User wears speakers (no headphones): interviewer audio plays
  through speakers, mic picks it up. After echo gate, the duplicate is
  dropped IF Jaccard >= 0.85 within 500 ms. Otherwise it leaks into
  Mic transcript and downstream marks the user as having said the
  interviewer's question.
- b. PipeWire loopback config error: user's mic feedback into the
  monitor sink. User voice ends up tagged as System. Echo gate may not
  catch it because the interviewer-tagged user voice arrives first (no
  prior Mic chunk to compare).
- c. Two speakers overlap (both talking at once): Jaccard < 0.85, both
  emitted. This is intentional — but each is mis-attributed to its
  channel rather than its true speaker.
- d. Whisper assigns the wrong text to a chunk (hallucination, drop)
  — channel is correct, content is wrong, looks like wrong speaker
  to the user.

There is no provenance tag on emitted chunks today. The frontend cannot
distinguish "channel-derived" labels from any future heuristic/manual
override. This is the foundation Slice 4 needs.

## 4. Per-speaker context routing — leak surfaces

Where user mic text could leak into prompts that should only see
interviewer text:

a. Hybrid question detector: only ingests when
   `source == AudioSource::System` (`pipeline.rs:415-456`). Mic chunks
   short-circuit with a quality-monitor update. **OK by code.**

b. Orchestrator question stream: the only producer of
   `DetectedQuestion` is `send_detected_question` inside
   `dispatch_confirm_plan` (System-only branch). The orchestrator
   itself never inspects `source`. **OK by construction**, but there
   is no defensive assertion. Any future bug that routes Mic content
   into `question_tx` would silently dispatch responses to the user's
   own utterance. **P1 — add defensive check (Slice 5).**

c. Conversation memory: `Turn` struct stores
   `{ question, directional_response, depth_response, created_at_ms }`.
   No user-spoken-answer field. The user's own speech never enters
   memory at all. The LLM has no record of what the user already said
   in prior turns. **P1 — design gap (Slice 5).**

d. Compression / depth prompt rolling summary: built from
   `serialise_turns`. Since Turn has no user-answer, the summary cannot
   leak user mic text — but it also cannot remember it. Same finding
   as (c).

e. RAG retrieval: confirm in Slice 5 that the embedding query is
   `question.text` (interviewer), not anything contaminated by mic
   bleed.

f. Mock coach: `commands::end_mock_turn` already wraps the transcript
   in `sanitize_mock_transcript` (M11 work shipped). However, the
   transcript is the raw mic transcription. If TTS bleed leaked into
   it, the coach grades a polluted answer. The sanitiser handles
   profanity hallucinations only, not full TTS bleed text. **P1 — see
   Slice 3 quiet window + Slice 5 verification.**

## 5. Whisper config drift vs design

Spec (§26 / `flint-audio.mdc`):
- model: tier-dependent (tiny/small/base.en)
- language: en
- beam_size: 5
- temperature: 0.0
- no_speech_thold: 0.6
- compression_ratio_thold: 2.4
- logprob_thold: -1.0
- initial_prompt: session-specific
- max_context: -1 (no carry-over)

Current (`transcription/engine.rs:35-44`, 211-244):
- BEAM_SIZE = 5  ✓
- TEMPERATURE = 0.0  ✓
- LANGUAGE = "en"  ✓
- NO_SPEECH_THRESHOLD = 0.6  ✓
- COMPRESSION_RATIO_THRESHOLD = 2.4  ✓
- LOGPROB_THRESHOLD = -1.0  ✓
- `set_initial_prompt(prompt)` per call  ✓
- `set_no_context(true)`  ✓ (whisper.cpp KV-cache off; rolling context
  injected via `initial_prompt` instead — see `build_context_prompt`)
- `MAX_TEXT_CONTEXT_TOKENS = 0`  ✓

Drift / missing:
- `temperature_inc_on_fallback`: NOT set. Whisper.cpp default is the
  fallback ladder 0.0 -> 0.2 -> 0.4 -> 0.6 -> 0.8 -> 1.0 when a segment
  fails the no_speech / compression / logprob thresholds. Confirm
  whisper-rs default carries this through; if not, set explicitly.
  **P2 (Slice 2).**
- `suppress_blank`: NOT explicitly set. Default is true in whisper.cpp.
  Confirm and set explicitly to lock contract. **P2 (Slice 2).**
- `suppress_non_speech_tokens`: NOT explicitly set. Default false.
  Should be `true` to suppress applause/music tokens that Whisper emits
  on silence. **P1 (Slice 2).**
- `single_segment`: not addressed. For Mock per-turn, where the chunk
  is a complete utterance, `single_segment = true` reduces hallucinated
  inter-segment fragments. For Live VAD chunks (already pre-segmented
  by VAD) it is also reasonable. **P2 (Slice 2).**
- Per-segment hallucination filters: only `compression_ratio` +
  `no_speech_prob` + `avg_logprob`. Known-string list ("Thanks for
  watching", "Subscribe to my channel", lone "you", lone "the",
  "thank you for watching") is NOT applied. These pass all three
  numeric filters and emit through. **P0 (Slice 2).**
- `validate_segment(text, duration)`: NOT IMPLEMENTED. A 250 ms VAD
  chunk that produces 60 words is impossible (>4 words/sec is hard
  ceiling, 5+ guarantees hallucination loop). No such guard today.
  **P0 (Slice 2).**
- Profanity sanitiser: lives in `mock/transcript.rs` and runs only on
  mock transcripts (`commands::end_mock_turn`). Live transcripts never
  see it. The "specifically -> F-word" hallucination affects Live too.
  **P1 (Slice 2).**
- Repeat-collapse: 4-word ngram appearing 3+ times consecutively (long
  silence loop bug) is not collapsed. Compression ratio catches the
  worst cases but not the moderate ones. **P2 (Slice 2).**

## 6. M10 Slice 1 / 2 / 7 wiring status

Slice 1 — cross-channel echo gate.
- `CrossChannelDedup` exists in `audio/pipeline.rs:69-192`. Jaccard
  threshold 0.85, window 500 ms, min 3 words. **Wired** at
  `pipeline.rs:372-386` behind `echo_suppression_enabled`.
- Tests: 7 unit tests covering symmetric suppression, distinct
  content, partial overlap, and window expiry. **Pass.**
- Directional `suppress_own_voice` (mic -> system bleed): NOT
  IMPLEMENTED. The current dedup is symmetric — it cannot distinguish
  "user voice leaked onto System" from "interviewer leaked onto Mic"
  and just drops whichever arrives second. In practice this means
  when user voice leaks onto System and arrives BEFORE the user's own
  Mic chunk (rare but possible with high system-channel latency),
  the user's actual Mic chunk is suppressed. **P2 (Slice 3).**

Slice 2 — hybrid question detection.
- `HybridQuestionDetector` in `transcription/hybrid.rs` with Pass1
  rule classifier + VAD silence + optional LLM verify + manual Ctrl+Q.
  **Wired**.
- Phone mode disables auto detection (`phone_mode_manual_only`).
  **Wired**.
- Min 5 new words, 8 s LLM cooldown, 30 s UNKNOWN cache, skip
  unchanged text. **Implemented.**
- `signal_question_ended` Tauri command exists for manual Ctrl+Q.
  **Confirmed via grep.**

Slice 7 — phone-call mode.
- `AudioCapture::start_phone_mode` opens one cpal stream from the
  mic device, tags every frame as System, and leaves the mic stream
  field as `None`. **Wired.**
- `is_phone_mode_single_stream()` helper exists for downstream
  branching. **Wired.**
- `set_phone_call_mode` Tauri command toggles state. **Wired.**

Slice 5 — VAD pre/post padding + max chunk.
- Pre-roll deque (200 ms = 10 frames at 20 ms/frame) prepended to
  every emitted chunk. **Wired.**
- 200 ms zero post-padding appended on finalise. **Wired.**
- Max chunk = 30 s; `flush_and_continue` mid-utterance. **Wired.**

## 7. Findings table

| ID | Severity | Area | File:line | Slice |
|----|----------|------|-----------|-------|
| F1 | P0 | Whisper hallucination strings ("Thanks for watching" etc.) pass all numeric filters | `transcription/engine.rs:246-356` | S2 |
| F2 | P0 | No `validate_segment(text, duration)` — impossible word/sec ratios accepted | `transcription/engine.rs:246-336` | S2 |
| F3 | P1 | Profanity sanitiser only applied to Mock; Live transcripts can still surface hallucinated expletives | `commands.rs end_mock_turn` vs `audio/pipeline.rs:386-411` | S2 |
| F4 | P1 | `suppress_non_speech_tokens` not set — applause/music tokens pass through | `transcription/engine.rs:229-244` | S2 |
| F5 | P2 | `temperature_inc_on_fallback`, `suppress_blank`, `single_segment` not explicitly configured | `transcription/engine.rs:229-244` | S2 |
| F6 | P2 | Repeat-collapse for 4-word ngram >= 3 consecutive missing | `transcription/engine.rs collect_segments` | S2 |
| F7 | P1 | Mock TTS -> mic bleed: no quiet window after TTS finishes before mic accepts speech | `mock/conductor.rs:367-383`, `mock/mic_capture.rs:599-617` | S3 |
| F8 | P2 | Linux PipeWire AEC hint not surfaced even when `module-echo-cancel` is missing | `health/checks.rs`, `audio/capture.rs:651-662` | S3 |
| F9 | P2 | `suppress_own_voice` directional rule not implemented; symmetric dedup can drop the genuine speaker | `audio/pipeline.rs:117-167` | S3 |
| F10 | P0 | No provenance tag on transcript chunks (`source: "channel" / "heuristic" / "llm" / "user"`); frontend cannot show or override | `events.rs TranscriptionChunkPayload`, `audio/pipeline.rs:394-401` | S4 |
| F11 | P1 | No `relabel_transcript_chunk` Tauri command; manual override impossible | `commands.rs` (missing) | S4 |
| F12 | P1 | No non-phone suspicion detector (question-shape on Mic, first-person on System) | new file | S4 |
| F13 | P1 | Persistence schema has no `chunk_label_source` column; provenance not durable | `session/persistence.rs`, `migrations/` | S4 |
| F14 | P1 | Orchestrator has no defensive assertion that incoming `DetectedQuestion` originates from System | `orchestrator/mod.rs:274-357` | S5 |
| F15 | P1 | Conversation memory `Turn` has no user-answer field — LLM cannot recall what user previously said | `session/memory.rs:26-36`, 119-121 | S5 |
| F16 | P2 | Compression / depth prompts have no role-tagging convention for "interviewer said X / I said Y" | `prompts/compression/`, `prompts/depth/` | S5 |
| F17 | P2 | RAG retrieval source not asserted; verify embedding is the interviewer question, not a polluted concat | `orchestrator/run_turn`, `rag/retriever.rs` | S5 |
| F18 | P2 | Per-chunk telemetry incomplete vs `flint-performance.mdc` schema (missing `suppression_reason`, `label_source`, `was_validated`) | `audio/pipeline.rs:394-411` | S6 |
| F19 | P2 | No session-end audit summary (chunk counts, suppression rate, hallucination hits) written to `~/.flint/metrics.log` | `session/persistence.rs end_session` | S6 |
| F20 | P2 | Mock per-turn Whisper params identical to Live; could benefit from `single_segment = true` for known-bounded turn audio | `transcription/engine.rs` | S2 |

## 8. Manual QA matrix — required device tests

These cannot be unit-tested; gate at the end of each fix slice.

| Scenario | Hardware | Validates |
|----------|----------|-----------|
| Real Zoom call, headphones | Linux + PipeWire AEC | Slice 2 hallucination filters; Slice 3 echo gate stays cold |
| Real Zoom call, laptop speakers (no headphones) | Linux + PipeWire AEC | Slice 3 system->mic bleed suppression |
| Real Zoom call, laptop speakers, AEC disabled | Linux, no `module-echo-cancel` | Slice 3 fallback Jaccard gate; Slice 6 healthcheck hint |
| Phone-on-speaker, single laptop mic | Linux | Slice 7 phone mode wiring; Slice 4 manual relabel UI hook |
| Mock interview, headphones | Linux | Slice 3 baseline mic-only path (no TTS bleed possible) |
| Mock interview, laptop speakers | Linux | Slice 3 quiet window + TTS->mic bleed gate |
| 30-minute long session | Linux | Slice 2 repeat-collapse; Slice 5 memory growth |
| Speaker with strong accent | Linux | Slice 2 logprob gate behaviour, manual relabel usage |

## 9. Slice mapping summary

- **S2 STT quality**: F1, F2, F3, F4, F5, F6, F20.
- **S3 Channel bleed**: F7, F8, F9.
- **S4 Speaker labeling**: F10, F11, F12, F13.
- **S5 Per-speaker routing**: F14, F15, F16, F17.
- **S6 Telemetry + manual QA**: F18, F19, plus this audit's QA matrix
  promoted to a checklist doc.

No findings block S1 (this audit). Proceeding to S2.
