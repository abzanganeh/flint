# M3 Linux Manual QA — Findings & Re-test Tracker

Branch: `feature/m3-mock-interview`  
Environment: Linux Wayland, Groq, espeak-ng TTS  
Last updated: 2026-06-16 (M3 Linux manual gate signed off)

## Phase 8 — First Pass (completed)

| # | Test | Result | Notes |
|---|------|--------|-------|
| 1 | Mock entry | Pass | |
| 2 | State MOCK_INTERVIEW | Pass | Verify via SQLite, not DevTools (`window.__TAURI__` unavailable in Tauri 2) |
| 3 | TTS | Pass | Piper neural (8.11 D1); espeak fallback if Piper absent |
| 4 | Suggested answer streaming | Pass | Study mode required; Practice hides until after answer |
| 5 | Mic + transcript | Pass | |
| 6 | Coach feedback | Pass | |
| 7 | Skip | Pass | |
| 8 | Full run | Partial | Used End & review (139 Q bank) |
| 9 | Mid-session exit | Pass | Cancel → Back to rehearsal |
| 10 | Persistence | Pass | WAV on disk + coach_json; duplicate mock_turn rows noted (fixed in code) |
| 11 | Draft recovery | Pass | Silent restore to REHEARSING, no modal (expected per `draft.rs`) |
| 14 | Rehearsal → Go live | Pass | |

## Phase 5 Hotkeys — First Pass (completed)

Hotkeys: **Ctrl+Alt+Space** (tap / hold / double), **Ctrl+Alt+Shift+Space** (panic).

| # | Test | Result | Notes |
|---|------|--------|-------|
| 1 | Tap trigger | Pass | After fixes; Flint must be focused on Wayland |
| 2 | Hold → Answer Now | Pass | |
| 3 | Double-tap cancel | Pass | Works in REHEARSING after fix |
| 4 | Panic hide | Pass | PanicRestoreShell — full chrome hide; re-test C1 2026-06-16 |
| 5 | OBS capture | Partial | Full monitor capture includes Flint (Wayland limitation) |
| 6a | 1920×1080 | Pass | |
| 6b | 4K | Partial | Readable but small at 3840×2160 |
| 7 | Stealth 3 tools | Partial | Screenshot tools capture Flint on Wayland |

**Wayland constraint:** Global hotkeys only work when Flint is focused unless portal/CLI integration is added.

## Fixes Landed on Branch

| Area | Change |
|------|--------|
| 8.10 MockSummary | Skipped badge + count; WAV playback via asset protocol + single mock_turn row |
| 8.9 Shuffle | `shuffle.rs` + UI toggle; follow-up turns in conductor |
| Pre-warm | Guard against raw digest JSON in Directional/Depth panels |
| Panic | `PanicRestoreShell` — only "Show Flint" pill when overlay hidden |
| Hotkeys | Tap on keyup, hold at 2s; cancel in REHEARSING; Wayland focused fallback |
| Rehearsal UX | Prep-before-live copy, skip confirm, de-emphasized go-live |
| Settings | Sign out button (Privacy tab) — was missing despite `logout` command |

## Post-Implementation Re-test Checklist

Run after rebuild (`npm run tauri dev`). One block at a time; report pass/fail before next.

### Block A — 8.10 MockSummary

- [x] **A1** Skipped turns show "Skipped" badge on summary cards + skipped count in header
- [x] **A2** Play button plays WAV (no Error state)

**A1+A2 pass (2026-06-17):** Recording via data URL (`ae1dd6f`). Stale turns from prior runs on summary fixed — clear mock_turns on `start_mock`.

**A1 fail (2026-06-16):** Skip on Q1 scored without answer; Q2 recording failed — turn_n drift (fixed `f233de5`).

**A1+A2 pass (2026-06-16 retest):** Skip labels OK; TTS kept playing after Skip; live recording unreliable until TTS finished.

**Follow-up:** TTS stop on skip, `mock_question_spoken` before mic, skip only drains mic when recording.

Loop message when both pass:
```
/flint-loop resume — 8.10-summary fixed, re-test MockSummary playback + skip label passed (Linux)
```

### Block B — 8.9 Shuffle + Follow-ups

- [x] **B1** With shuffle ON, first question differs between two fresh mock starts
- [x] **B2 / B2a** After a full spoken answer, a follow-up question appears (not immediate skip)

**B1+B2a pass (2026-06-16):** M3 Linux manual gate sign-off.

### Block C — P0 Regressions

- [x] **C1** Panic (Ctrl+Alt+Shift+Space): only "Show Flint" pill visible in Rehearsal and LIVE
- [x] **C2** Pre-warm / Directional: no raw digest JSON in panel text
- [x] **C3** Test 14: Rehearsal → Go live still works (+ panel reset — no rehearsal carry-over in live panels)

**C1–C3 pass (2026-06-16):** `PanicRestoreShell`, digest pre-warm guard, `resetOrchestratorPanels` on live entry.

### M3 Linux manual gate — COMPLETE

All blocks A, B, C passed on Linux Wayland. **Block D (8.11) Linux PASS** 2026-06-16. Remaining M3: **m3-gmail-sso**.

### Block D — TTS + platform follow-ups

- [x] **D1** Piper neural TTS speaks mock interviewer questions (Linux)
- [x] **D2** Recording/mic gated until question fully spoken (`mock_question_spoken` timing)
- [x] **D3** Skip stops in-flight TTS immediately (no bleed into next turn)
- [ ] **m3-gmail-sso** Google OAuth (not built)
- [ ] **8-gate-manual-macos-windows** platform TTS paths (say / SAPI)

**8.11 Linux pass (2026-06-16):** Piper backend; D2+D3 verified with skip/timing fixes (`67dd08b`).

### Block E — Gmail SSO (`m3-gmail-sso`)

See `tests/manual-qa/M3_GMAIL_SSO.md` for full setup + E1–E4 checklist.

- [x] **E1** New user — Continue with Google → onboarding completes — **PASS 2026-06-17**
- [x] **E2** Sign out → Google login → local sessions preserved — **PASS**
- [x] **E3** Cancel/deny — `cancel_google_oauth` + Vite error bridge — **PASS**
- [x] **E4** Email/password login still works — **PASS**

**Block E complete** — M3 Linux manual QA closed.

## Session recovery after sign-out (2026-06-16)

**Sign out does not delete local sessions.** Auth token is cleared only; SQLite keeps all session rows.

| Symptom | Cause | Fix |
|---------|-------|-----|
| No Reopen on past session | ENDED sessions had no reopen UI; Resume only for in-progress active draft | **Reopen** button added (select row → Reopen) |
| Start similar empty fields | Clone path used legacy `context_text` blob only; digest fallback requires active session | Fixed: loads structured `contextFields` first |
| Wrong session after login | `restoreDraftSession` picks most recent REHEARSING draft (not Fisher ENDED) | Use **Reopen** on Fisher row, or abandon other draft |
| 139 Q bank missing on Start similar | Start similar creates new session — bank not copied | Use **Reopen** to keep same session + bank |

Verified in SQLite (Linux):
- `Fisher Investments IAM Architect` — ENDED, 139 questions, context intact at `~/.local/share/com.flint.app/flint.db`

## Open Backlog (non-blockers)

| Item | Severity | Notes |
|------|----------|-------|
| Wayland global hotkeys unfocused | P2 | Needs xdg-desktop-portal or similar |
| Overlay text size at 4K | P2 | Per-panel height + font scaling |
| Answer Now badge not clickable | P3 | Badge-only in current UX |
| OBS / stealth on Wayland | Accepted | Document in health check |
| Duplicate mock_turn rows (historical) | Fixed | Old sessions may still have bad rows |

## SQLite / Paths (Linux)

```bash
# Session state
sqlite3 ~/.local/share/com.flint.app/flint.db \
  "SELECT id, state FROM sessions ORDER BY updated_at DESC LIMIT 3;"

# Mock audio
ls ~/.local/share/com.flint.app/mock_audio/
```

## Auth Workaround (pre–Sign out button)

```bash
secret-tool clear service flint account auth_token_*
# restart Flint
```

Sign out is now in **Settings → Privacy → Sign out**.
