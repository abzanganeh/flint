# M13 Live Pipeline — Manual QA Checklist

Runs the audio pipeline end-to-end against real hardware. Required gate
between Slice 6 and merging the `feature/m13-audio-pipeline-deep-review`
branch.

This checklist relies on the per-chunk telemetry shipped in Slice 6
(`flint::audio::chunk` target, INFO level) and the session-end summary
written to `~/.flint/metrics.log`. Capture both artefacts for every run
and attach them to the QA report.

## Prerequisites

- Linux Wayland session (X11 will fail the stealth gate).
- Whisper model installed at `~/.cache/whisper/ggml-small.en.bin` (or the
  tier-appropriate model).
- Groq or DeepSeek API key configured in keychain.
- Two devices: laptop running Flint, second device running interviewer
  audio.
- Wayland screencast permission granted to the Flint window.
- `~/.flint/metrics.log` cleared between runs:
  `rm -f ~/.flint/metrics.log` so each run produces exactly one summary
  line.
- Run with `RUST_LOG=info,flint::audio::chunk=info` so per-chunk metrics
  are captured to stderr alongside the structured logs.

## Run matrix

For every scenario, execute a 5-minute live session that contains at
least 8 distinct interviewer questions and 8 candidate answers. Take
notes inline.

### A. Real Zoom call, headphones, Linux PipeWire AEC enabled

Goal: confirm the baseline. Echo gate stays cold; no suspicion events
fire; `metrics.log` summary shows balanced System/Mic counts and a low
suppression rate.

- [ ] HealthCheck shows `echo_cancellation: pass`.
- [ ] Both speakers transcribed verbatim with correct labels.
- [ ] Suppression rate in `metrics.log` ≤ 1%.
- [ ] No `chunk_label_suspicious` event in the frontend log.
- [ ] No "specifically -> profanity" hallucination on candidate side.
- [ ] No "Thanks for watching" tail on either side.

### B. Real Zoom call, laptop speakers (no headphones), AEC enabled

Goal: speaker bleed into mic suppressed by Slice 1 echo gate plus AEC.

- [ ] Most echo events logged as `echo_system_to_mic`.
- [ ] No `echo_mic_to_system` events.
- [ ] Candidate answers still arrive intact (not suppressed by mistake).
- [ ] Suppression rate in `metrics.log` ≤ 10%.

### C. Real Zoom call, laptop speakers, AEC DISABLED

Goal: stress test of the Jaccard echo gate and the new directional
warning.

Disable AEC via `pactl unload-module module-echo-cancel` (or simply
revert any `FLINT_MIC_SOURCE` override and restart Flint).

- [ ] HealthCheck shows `echo_cancellation: warn` with the hint to load
  `module-echo-cancel`.
- [ ] Echo events are still classified `echo_system_to_mic` (Jaccard gate
  is doing the work AEC would).
- [ ] If `echo_mic_to_system` events appear, the WARN line
  "Mic audio appears to be looping back" is logged exactly once.
- [ ] Candidate answers still arrive intact.

### D. Phone call on speaker, single laptop mic

Goal: phone-call mode end-to-end. Manual Ctrl+Q drives the orchestrator.

- [ ] HealthCheck steps required (mic, stealth) all pass before LIVE.
- [ ] Auto question detection is off (`phone_mode_manual_only = true`).
- [ ] Pressing Ctrl+Q dispatches the buffered interviewer text exactly
  once per question.
- [ ] Echo gate is disabled (no echo events in `metrics.log` at all —
  `suppressions = 0`).
- [ ] Candidate answers labelled `Microphone` (single channel, but the
  pipeline still tags every frame as System per phone-mode rules — note
  the divergence from non-phone runs).

### E. Mock interview, headphones

Goal: baseline mock path. No TTS bleed possible.

- [ ] Mock turn quiet window dropped no real user speech (manually
  verify the first 0.3s of each answer is silence in the captured WAV).
- [ ] Coach feedback never accuses the user of profanity that they did
  not say.
- [ ] Audit summary shows mic-only chunks, no system chunks at all.

### F. Mock interview, laptop speakers

Goal: TTS->mic bleed gate. Hardest scenario before this milestone.

- [ ] First user-answer chunk is NOT a transcription of the AI's
  spoken question.
- [ ] No "specifically -> profanity" hallucination after a TTS turn ends
  with the word "specifically".
- [ ] If speakers are loud enough to bleed past the quiet window, the
  bleed text is dropped by the live sanitiser (engine post_process for
  known hallucinations, or sanitizer for profanity).

### G. 30-minute long session

Goal: stability + memory growth + repeat-collapse.

- [ ] No `KnownHallucination` storms (more than 3 per minute is a fail).
- [ ] No process memory growth above 200 MB after 30 minutes.
- [ ] If Whisper enters a long-silence loop, the repeat-ngram collapse
  shrinks the chunk to a single occurrence (verify in transcript).
- [ ] Conversation memory rolling summary distinguishes interviewer
  asks from candidate answers (look for "Interviewer asked" /
  "Candidate answered" in compression payloads — debug-level only).

### H. Speaker with strong accent

Goal: low-confidence transcript handling.

- [ ] `mean_logprob` for the affected channel in `metrics.log` is
  below -0.5 (matches expectations for a hard speaker).
- [ ] Suspicion detector does not flag normal accented speech as the
  wrong speaker (false positives < 5% of chunks).
- [ ] Manual relabel via `relabel_transcript_chunk` updates the persisted
  speaker and emits `transcript_chunk_relabeled` for the UI.

## Pass criteria

A scenario passes only if every box is ticked. The full M13 milestone
passes only when scenarios A, B, D, E and F all pass on Linux. C, G, H
are advisory but block any "stable" claim.

## Triage

When a scenario fails, attach:

1. The relevant `metrics.log` line (`session_audit_summary`).
2. The last 200 lines of the Flint stderr log.
3. The audio capture WAV from the affected mock turn (if mock).
4. A short note on which slice the regression appears tied to.

Open a follow-up issue rather than blocking the M13 PR if the failure
is in scope of a future slice (e.g. SpeakerPicker UI regressions are
phone-interview-and-live-preview territory).
