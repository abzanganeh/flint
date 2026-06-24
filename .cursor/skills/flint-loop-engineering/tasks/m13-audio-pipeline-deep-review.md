# M13 — Audio pipeline deep review

## Context

Loop-engineered audit + fix of the entire Live and Mock audio pipeline before
the `phone-interview-and-live-preview` UX work begins. User reported:

- STT mis-transcribes ("specifically" -> profanity, names dropped, repeats).
- Channel bleed (interviewer voice on mic, user voice on system loopback).
- Wrong speaker labels in transcript even in non-phone mode.
- Wrong context fed to orchestrator/coach when interviewer vs user speaks.
- Mock coach scoring polluted transcripts that contain TTS playback echoes.

M10 (Live Session Reliability) shipped a redesign on paper but parts may not
be wired or working as designed. M13 confirms ground truth and patches
divergences before any UX work resumes.

## Branch and loop config

- Milestone branch: `feature/m13-audio-pipeline-deep-review`
- One commit per slice. Slice 7 pushes the branch and opens a PR.
- State reset: `current_milestone: "m13-audio-pipeline-deep-review"`,
  `current_slice: 1`, `current_task_id: "m13-s1-audit"`.
- Kickoff: `/flint-loop m13-audio-pipeline-deep-review start`.

## Pre-flight

```bash
git checkout main && git pull
git checkout -b feature/m13-audio-pipeline-deep-review
```

Reset loop state to the milestone above.

## Slice 1 — Audit (read-only, produces report)

Output: `tests/manual-qa/m13-audio-pipeline-audit.md` with severity-ranked
findings (P0 / P1 / P2). Walk every stage:

- `src-tauri/src/audio/capture.rs` — cpal stream config (normal vs phone mode).
- `src-tauri/src/audio/rnnoise.rs` — which streams it runs on, frame timing.
- `src-tauri/src/audio/vad.rs` — aggression, padding, segment durations.
- `src-tauri/src/audio/pipeline.rs` — echo dedup state, Jaccard threshold,
  channel routing.
- `src-tauri/src/transcription/engine.rs` — Whisper params (model, language,
  beam, temperature, no_speech_thold, no_context, single_segment).
- `src-tauri/src/transcription/prompt.rs` — initial_prompt build, vocabulary.
- `src-tauri/src/transcription/detector.rs` and `hybrid.rs` — detection
  layers wired in?
- `src-tauri/src/orchestrator/mod.rs` — filter on `source = System`?
- `src-tauri/src/session/memory.rs` — per-turn speaker tagging.
- `src-tauri/src/mock/mic_capture.rs` — same Whisper config as live? Mic
  muted during TTS?
- `src-tauri/src/mock/tts.rs` — TTS playback path; mic capture concurrency.

Audit doc structure (mandatory):

1. Live audio path (capture -> RNNoise -> VAD -> Whisper -> echo gate ->
   orchestrator).
2. Mock audio path (TTS playback, mic capture, coach prompt building).
3. Speaker labeling: channel mapping, where it can break.
4. Per-speaker context routing: where user-mic text could leak into prompts.
5. Whisper config drift vs M8 design.
6. M10 Slices 1, 2, 7 wiring status (shipped vs partial vs missing).
7. Findings table: severity, area, file:line, fix slice.
8. Manual QA matrix: what needs device test.

Commit: `Audit live and mock audio pipeline (M13 S1)`.

Gate: `cargo check`, `npm run test`. No code changes besides the new doc.

## Slice 2 — STT quality

- Whisper params in `transcription/engine.rs`:
  - `language = Some("en")` explicit.
  - `temperature_inc` set explicitly (avoid silent upstream drift).
  - `suppress_blank = true`, `suppress_nst = true`.
  - `single_segment` decision per audit.
- New `transcription/sanitizer.rs`:
  - `validate_segment(text, duration)` — drop > 5 wps.
  - `is_known_hallucination(text)` — curated stock-string list.
  - `collapse_repeated_ngrams(text)` — fold 3+ consecutive 4-word ngrams.
  - `sanitize_live_transcript(text) -> Option<String>`.
- Move profanity sanitiser out of `mock/transcript.rs` so Live uses it too.
- Wire engine `post_process` and pipeline call.

Tests: unit tests on `validate_segment`, `is_known_hallucination`,
`collapse_repeated_ngrams`, sanitiser. Manual QA gate: real Zoom call.

Commit: `Tighten Whisper config and add hallucination filters (M13 S2)`.

## Slice 3 — Channel bleed (Live + Mock)

- Confirm M10 Slice 1 echo gate is wired (Jaccard 0.85, 500 ms). Audit said
  yes — keep it that way.
- Add directional log/warn on `mic -> system` direction (rare, indicates
  loopback misconfig).
- Mock TTS bleed: in `mock/mic_capture.rs`, drop the first 300 ms of mic
  frames after the Listening phase begins (post-TTS quiet window).
- Linux PipeWire AEC hint: new `EchoCancellation` health check that detects
  `module-echo-cancel`; warns with the exact `pactl load-module` command.

Tests: dedup tests still pass; AEC check runs without panic.

Commit: `Fix audio bleed in Live and Mock pipelines (M13 S3)`.

## Slice 4 — Speaker labeling integrity

- Add `label_source` column to `transcript_chunks` (migration v17, default
  `'channel'`).
- Add `chunk_id` and `label_source` to `TranscriptionChunkPayload` event.
- New `transcription/speaker_suspicion.rs` — regex-only:
  - `evaluate(speaker, text) -> Option<SuspicionVerdict>`.
  - Question-shape on Mic, first-person on System.
- Pipeline emits `chunk_label_suspicious` when echo-suppression mode is on
  (i.e. non-phone) and the heuristic fires.
- New Tauri command `relabel_transcript_chunk(chunk_id, new_speaker)`. Sets
  `label_source = 'user'` and emits `transcript_chunk_relabeled`.
- Frontend types for the new payloads + the `relabelTranscriptChunk` wrapper.

Tests: persistence round-trip; suspicion detector unit tests.

Commit: `Track speaker label provenance and detect suspicious labels (M13 S4)`.

## Slice 5 — Per-speaker context routing

- Add `DetectedQuestionSource` to `audio/pipeline.rs`. Variants: System,
  PhoneManual, UserTriggered, Microphone (sentinel).
- Stamp every `DetectedQuestion` construction site (pipeline, signal_question_ended,
  trigger_response).
- Orchestrator: `is_valid_question_source(source)` check at the top of the
  loop. Drop + log error on Microphone.
- `ConversationMemory::Turn`: add `user_answer` field; `record_user_answer()`.
- `serialise_turns` uses explicit "Interviewer asked / AI suggested /
  Candidate answered" role labels.
- Update `prompts/compression/default.txt` to reference the role labels.

Tests: orchestrator source filter; memory role-label test; user-answer path.

Commit: `Enforce per-speaker context routing in orchestrator and memory (M13 S5)`.

## Slice 6 — Telemetry + manual QA doc

- New `audio/audit.rs`:
  - `AudioAuditCounters` (per-source counts, suppression reasons, suspicion
    counters, mean logprob).
  - `AudioAuditSummary` Serialise impl.
  - `write_summary_to_metrics_log(session_id, summary)` — append JSON line
    to `~/.flint/metrics.log` (path overridable via `FLINT_METRICS_LOG`).
- Plumb `Arc<AudioAuditCounters>` through `run_audio_pipeline` and store on
  `LiveTaskHandles`.
- Per-chunk `tracing::info!(target: "flint::audio::chunk", ...)` line with
  the schema from `flint-performance.mdc`.
- `stop_session` snapshots and writes the summary before dropping handles.
- Manual QA doc `tests/manual-qa/m13-live-pipeline-checklist.md` covering
  the 8 scenarios (Zoom + headphones, Zoom + speakers, Zoom + speakers +
  AEC off, phone-on-speaker, mock + headphones, mock + speakers, 30-min
  long session, accented speaker).

Tests: audit counter unit tests; metrics path env override.

Commit: `Add audio pipeline telemetry and M13 manual QA checklist (M13 S6)`.

## Slice 7 — Push, PR, CI

- `git push -u origin feature/m13-audio-pipeline-deep-review`.
- `gh pr create` to `main` with summary linking the audit doc and QA list.
- CI fix loop until green (max 6 attempts).
- Do NOT merge unless user asks.

## Stop conditions

- Slice fails 3 times in a row → stop and ask.
- CI fix loop > 6 attempts → stop and ask.
- User `/flint-loop stop`.

## Manual gate backlog (post-merge)

Real device tests from
`tests/manual-qa/m13-live-pipeline-checklist.md`. Resume format:

```
/flint-loop resume - m13-s2-stt linux pass|fail (notes)
/flint-loop resume - m13-s3-bleed mock-speakers pass|fail (notes)
```

## Follow-ups

After merge, `phone-interview-and-live-preview` resumes with:

- Audit's recommendations baked into M1 / M2 details.
- SpeakerPicker UI sits on top of the `relabel_transcript_chunk` backend
  shipped in S4.
- Live Preview state reuses the cleaned pipeline + telemetry from M13.
