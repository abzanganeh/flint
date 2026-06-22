# M12 — Mock Interview Turn UX

## Context

Mock turns currently require "Start Answering" after each question. M12 automates
listen → answer → pause → coach, adds mid-answer Retry, and moves save-for-live
behind a confirm modal.

**Branch:** `feature/m11-mock-coach-quality` (extends PR #24)  
**Do NOT commit:** `tests/manual-qa/fisher-iam-*.json`, `scripts/import_preferred_answers.py`, `apply_bank_aliases.py`, `dedupe_question_attempts.py`, `setup_prep_order.py`

## Voice flow

Phases: `speaking` → `listening` → `answering` → `paused` → `reviewing`

- After `mock_question_spoken`: **listening** (mic open, no REC/STT)
- Speech detected → **answering** (REC + STT)
- Remove Start Answering (guided + continuous)
- 3000ms turn-level silence → **paused** (not coach); min ~2s speech before pause
- Speech during paused → resume **answering** (same `turn_n`)
- Done in answering/paused → `end_mock_turn` → coach
- Do not change live 600ms VAD

## Mid-answer Retry (slice 3)

- Retry (answering/paused): discard partial take, same `turn_n`, back to listening
- Try again / Re-grade (reviewing): keep existing
- New command `abort_mock_turn`
- Footer answering/paused: Retry · Skip · Done

## Save for Live (slice 4)

- Mock reviewing only: one editor (`editTranscript`)
- Footer reviewing: Re-grade · Try again · Next question — no save
- Below coach: Review & save for Live → confirm modal → `savePreferredAnswer`
- Rehearsal unchanged (one-click save)

---

## Slices

### Slice 1: m12-s1-listen-answer-pause

**Goal:** Rust/events — auto listen, speech→answer, 3s pause, resume, Done, tests

- `mock/turn_phase.rs` — silence/pause constants + unit tests
- `mock_turn_phase` Tauri event (`listening` | `answering` | `paused`)
- `mic_capture` — listening mode, turn-level 3s silence, pause/resume
- Conductor triggers listen after `mock_question_spoken`
- `start_mock_turn` no-op (auto flow); `end_mock_turn` from answering/paused
- `skip_mock_turn` ends listen phase too

**Gate:** `cargo test`, `cargo clippy -- -D warnings`, `npm run test`

---

### Slice 2: m12-s2-mock-frontend-phases

**Goal:** MockInterview phases, remove Start Answering, wire `mock_turn_phase` events

---

### Slice 3: m12-s3-mid-answer-retry

**Goal:** `abort_mock_turn`, Retry button, tests

---

### Slice 4: m12-s4-save-confirm-ux

**Goal:** Review & save confirm modal, Vitest

---

### Slice 5: m12-s5-pr-ci

**Goal:** push, PR #24 green, merge, `ci_green`

---

## Done when

PR #24 merged, all CI green. Report: Ready for user to return to Fisher interview prep.
