# Loop slice: m8-input-quality

**Branch:** `feature/m8-input-quality`  
**Spec:** `docs/ROADMAP.md` §M8 · `docs/flint_system_design_v3.md` §17 Step 3, §26 `initial_prompt`  
**Policy:** One task per iteration. Commit after each green gate. Do not stop until CI green or blocker.  
**End:** push → PR → CI fix loop until green.

---

## Pre-flight (iteration 0 — before task 1)

1. `git checkout main && git pull`
2. `git checkout -b feature/m8-input-quality`
3. Reset `.cursor/flint-loop-state.json`:
   - `current_milestone`: `"M8"`
   - `milestone_branch.flint`: `"feature/m8-input-quality"`
   - `current_task_id`: `"m8-3.5.11-wer"`
   - `loop_stopped`: `false`
   - `completed_tasks`: `[]`
   - `ci_fix_attempts`: `0`
4. Read `src-tauri/src/transcription/engine.rs`, `session/persistence.rs` (`app_preferences`), `App.tsx` routing (`session-focus` gate).

---

## Tasks (in order — one commit per task on success)

### 1. `m8-3.5.11-wer` — WER utility + unit tests

**Implement**
- New module `src-tauri/src/transcription/wer.rs`
- `normalize_for_wer(text) -> String` — lowercase, strip punctuation, collapse whitespace
- `word_error_rate(reference: &str, hypothesis: &str) -> f32` — token-level Levenshtein / ref length
- Export from `transcription/mod.rs`

**Tests** (must pass before commit)
- Known pairs from design doc sample paragraph with deliberate typos
- Empty reference → 0.0 or defined edge case
- Perfect match → 0.0
- `cargo test wer --lib`

**Commit message**
```
Add WER utility for mic calibration scoring.
```

**Gates:** `cargo test`, `cargo clippy -- -D warnings`

---

### 2. `m8-3.5.2-migration` — SQLite v14 + whisper_initial_prompt column

**Implement**
- `SCHEMA_VERSION = 14`
- Migration: `ALTER TABLE sessions ADD COLUMN whisper_initial_prompt TEXT NOT NULL DEFAULT ''`
- `load_whisper_initial_prompt(session_id)` / `save_whisper_initial_prompt(session_id, prompt)` on `SessionPersistence`

**Tests**
- Fresh DB migration test
- Round-trip save/load

**Commit message**
```
Add v14 migration for session whisper initial prompt storage.
```

**Gates:** `cargo test persistence --lib`

---

### 3. `m8-3.5.1-whisper-prompt` — Session-aware initial_prompt builder + injection

**Implement**
- New `src-tauri/src/transcription/prompt.rs` — `build_whisper_initial_prompt(digest, context_text) -> String` per design doc §26 (220 char cap, fallback string for pre-digest)
- Call from `confirm_digest` → persist to SQLite
- Refactor `WhisperEngine` to accept `initial_prompt: String` (or `Arc<str>`) per session — thread through `audio/pipeline.rs` and `mock/mic_capture.rs`
- Replace static `INITIAL_PROMPT` in `build_full_params(initial_prompt: &str)`

**Tests**
- Unit test: digest with company/role/skills produces expected prefix + tokens
- Truncation at 220 chars
- `cargo test prompt --lib`

**Commit message**
```
Inject session-specific Whisper initial prompt from digest.
```

**Gates:** `cargo test`, `cargo clippy`

---

### 4. `m8-3.5.6-device-prefs` — Per-device calibration persistence

**Implement**
- `device_fingerprint()` — stable hash from OS + default mic name + default output name (cpal enumeration)
- Keys in `app_preferences`: `mic_calibration_passed_{fingerprint}`, `mic_calibration_wer_system_{fingerprint}`, `mic_calibration_wer_mic_{fingerprint}`, `mic_calibration_at_{fingerprint}`
- Tauri commands: `get_mic_calibration_status`, `mark_mic_calibration_passed`, `clear_mic_calibration` (for re-test)

**Tests**
- Preference round-trip
- Fingerprint stable across calls in same process

**Commit message**
```
Persist mic calibration results per device fingerprint.
```

**Gates:** `cargo test`

---

### 5. `m8-3.5.4-system-audio-test` — Phase 1 backend (system loopback WER)

**Implement**
- Static ground-truth clip text + bundled or generated WAV (~30s) under `src-tauri/resources/calibration/` (or synthesise via existing TTS in test-only path)
- Command `run_system_audio_calibration` — play clip (cpal output or `rodio`/platform), capture loopback, transcribe, return `{ wer, passed, transcript }`
- Pass threshold: WER < 0.20
- Emit `calibration_system_complete` event

**Tests**
- WER against mocked transcript (inject mock WhisperEngine in test cfg if needed)
- **Park manual gate:** real loopback on device → `manual_gate_backlog`

**Commit message**
```
Add system audio calibration test with WER scoring.
```

**Gates:** `cargo test` (automated only)

---

### 6. `m8-3.5.5-mic-test` — Phase 2 backend (mic WER)

**Implement**
- Static calibration paragraph (design doc §17 sample text) in `resources/calibration/mic_paragraph.txt`
- Command `run_mic_calibration` — record until user stops or 45s timeout, transcribe, return WER
- Pass threshold: WER < 0.25
- Emit `calibration_mic_complete` event

**Tests**
- Unit test with injected transcript
- **Park manual gate:** real mic on device

**Commit message**
```
Add mic calibration test with domain-heavy reference paragraph.
```

**Gates:** `cargo test`

---

### 7. `m8-3.5.3-ui` — MicCalibration screen + skip flow

**Implement**
- `src/screens/MicCalibration.tsx` — two phases, progress, WER display, pass/fail
- Skip path: if `get_mic_calibration_status().passedOnDevice` → show *"You've already passed on this device. Is your setup the same?"* → **Run again** | **Skip — nothing changed**
- Wire commands in `src/commands/index.ts`
- Vitest: render phases, skip button visible when passed

**Commit message**
```
Add MicCalibration gate UI with per-device skip option.
```

**Gates:** `npm run test`, `npx tsc --noEmit`

---

### 8. `m8-3.5.7-failure-ux` — Warning + recommendations + proceed anyway

**Implement**
- Red warning panel on fail (amber category, strong copy per design doc)
- Fix list: headset, quieter room, device selection, wired vs BT
- **I understand — continue anyway** → `mark_mic_calibration_passed` with `forced: true` or separate `skipped_despite_fail` flag

**Commit message**
```
Add strong warning UX when calibration WER exceeds threshold.
```

**Gates:** `npm run test`

---

### 9. `m8-3.5.10-routing` — App gate: session-focus → mic-calibration → rehearsal

**Implement**
- Extend `App.tsx`: after `SessionFocusGate.onComplete`, check calibration status → route to `mic-calibration` or `rehearsal`
- Add `"mic-calibration"` to `AppScreen` + `SHELL_SCREENS`

**Commit message**
```
Route through mic calibration gate before first rehearsal per device.
```

**Gates:** `npm run test`

---

### 10. `m8-3.5.8-quality-badge` — Rolling avg_logprob monitor + badge

**Implement**
- Track rolling window of segment `avg_logprob` in audio pipeline (last N segments or 30s)
- Threshold: equivalent to WER > 30% (calibrate: logprob < -0.5 rolling mean — tune in test)
- Emit `audio_quality_status { level: "ok"|"low" }` event
- UI: amber badge bottom-right in Rehearsal, MockInterview, LiveOverlay — label **Mic quality low**
- During calibration Phase 2 completion copy: explain badge location for Live

**Tests**
- Unit test: low logprob sequence triggers `low`
- Vitest: badge renders when event fired

**Commit message**
```
Show mic quality badge when transcription confidence drops.
```

**Gates:** `cargo test`, `npm run test`

---

### 11. `m8-3.5.9-settings-retest` — Settings re-test entry point

**Implement**
- Settings tab or Audio section: **Re-test mic and audio** → navigates to `MicCalibration` (clears pass flag or runs in force mode)

**Commit message**
```
Add Settings entry to re-run mic calibration.
```

**Gates:** `npm run test`

---

### 12. `m8-3.5.12-logging` — Structured audio_quality_event metrics

**Implement**
- On calibration complete: log JSON to metrics per `flint-performance.mdc` schema
- Fields: `event: "audio_quality_calibration"`, `wer_phase1`, `wer_phase2`, `passed`, `device_id` (fingerprint hash, not PII)

**Commit message**
```
Emit structured audio quality metrics on calibration complete.
```

**Gates:** `cargo test`

---

### 13. `m8-review` — Code review + fix findings

**Run**
- Self-review / Bugbot on branch diff
- Fix all findings (clippy, fmt, test gaps)
- Full gate: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `npm run test`
- Coverage gate if CI runs it: `./scripts/check-coverage-gates.sh`

**Commit message** (if fixes needed)
```
Address review findings for M8 input quality.
```

---

### 14. `m8-ci` — Push, PR, CI until green

**Steps**
1. `git push -u origin feature/m8-input-quality`
2. `gh pr create --title "M8: Whisper context injection + mic calibration" --body "..."`
3. Watch CI: `gh pr checks --watch`
4. On failure: fix → commit → push → repeat (max 6 attempts per loop state)
5. Set `milestone_status: "ci_green"` when all checks pass

**Do NOT merge** unless user asks.

---

## Manual gates (park — do not block loop)

| Task | Platform | Notes |
|------|----------|-------|
| m8-3.5.4-system-audio-test | linux/macos/windows | Real loopback WER with speakers |
| m8-3.5.5-mic-test | all | User reads paragraph aloud |
| m8-3.5.8-quality-badge | device | Subjective badge timing |

Append to `manual_gate_backlog` in loop state; write `tests/manual-qa/M8_INPUT_QUALITY.md` checklist at slice end.

---

## Stop conditions

- All tasks 1–14 complete + CI green → stop, report PR URL
- Same task fails 3× → stop, report blocker
- CI fix loop > 6 → stop, report last failing check

---

## Kickoff

```
/flint-loop m8-input-quality start
```

Resume after manual QA:

```
/flint-loop resume — m8-3.5.4-system-audio-test linux pass|fail (<notes>)
```
