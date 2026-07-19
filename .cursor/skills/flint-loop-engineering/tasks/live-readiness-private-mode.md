# Live Readiness + Private Mode — Combined Milestone

> **Kickoff:** `/flint-loop live-readiness-private-mode start`
> **One branch, one PR, one merge.**

## Context

User-requested UX + legal/consent work, plus a terminology rename:

1. **Private Mode rename** — "Stealth Mode" reads as concealment/deception; rename to Private Mode everywhere (already started on branch).
2. **Rehearsal setup popup** — once-ever modal on first Rehearsal entry explaining equipment/settings check before Live (phone + non-phone).
3. **Test Live Session** — button in Rehearsal runs a readiness pass (health checks + live gates) then optional 60s audio preview; button disables after success until config changes (phone mode, mic calibration, OS audio routing fingerprint).
4. **Remove Mock Interview checkbox** — "Practice with sample call audio" was instructions-only and confusing; remove entirely.
5. **Recording consent gate** — before first Live entry **per session**, user must confirm legal right to record/transcribe; cancel → warning + return to Rehearsal (not Mock Interview).
6. **Crash fix** — revert broken uncommitted WIP (`onDirectionalToken` import) that white-screens the app on open.

**Branch:** `feature/live-readiness-private-mode` (from latest `main`, includes rename commit).

**Repos:** Flint only.

### Hard guardrails

- React is a dumb renderer — session state in Rust.
- API keys in keychain; no session content in INFO logs.
- Minimal diff — no drive-by refactors.
- Do not commit `docs/`, loop state, or secrets.

---

## Agent ladder

| Complexity | Agent |
|------------|-------|
| Simple (fmt, copy, tests wiring) | composer-2.5-fast |
| Medium (frontend modals, fingerprint) | cursor-grok-4.5-high-fast |
| Complex (state machine, new commands) | claude-sonnet-5-thinking-high |

Review each slice with one level up before commit.

---

## Slices

### Slice 0: `lrpm-s0-branch-hygiene` (SIMPLE)

- Revert broken WIP on working tree (`LiveSessionStatusBar` / `onDirectionalToken`).
- Fix `cargo fmt` module order in `lib.rs`.
- Rename branch to `feature/live-readiness-private-mode`.
- Close superseded PR #44; reopen as milestone PR at end.

**Gate:** `cargo fmt --check`, app opens without white screen.

---

### Slice 1: `lrpm-s1-private-mode-rename` (SIMPLE)

Already committed: `stealth.rs` → `private_mode.rs`, `StealthApi` → `PrivateModeApi`, UI copy, runbooks.

**Gate:** `cargo test --lib health::checks`, `npx vitest run src/screens/HealthCheck.test.tsx`.

---

### Slice 2: `lrpm-s2-live-readiness-backend` (COMPLEX)

**Goal:** Curated readiness check command for Rehearsal "Test Live Session".

Add to `health/checks.rs`:

- `pub async fn run_live_readiness_check(phone_call_mode: bool) -> LiveReadinessReport`
- Subset: private mode self-test, whisper model, primary LLM key, system audio loopback/isolation (skip isolation fail block when phone mode), echo cancellation (warn), ollama (warn), headphone gate evaluation.
- Optional lightweight LLM ping via existing failover stack (warn on fail, don't block if cloud key present).
- `LiveReadinessConfigFingerprint` struct: phone_call_mode, headphone_override, mic_calibration_ok, pulse routing snapshot (Linux pactl probe when available).

Add Tauri command `run_live_readiness_check(session_id: String) -> LiveReadinessReportDto`.

**Gate:** unit tests for report shape + fingerprint stability.

---

### Slice 3: `lrpm-s3-rehearsal-live-preview-state` (COMPLEX)

**Goal:** Allow Test Live Session from REHEARSING without completing rehearsal.

- State machine: add `(Rehearsing, LivePreview)` and `(LivePreview, Rehearsing)`.
- `LivePreviewTaskHandles.return_state: SessionState` (Ready | Rehearsing).
- `start_live_preview(session_id, rehearsal_test: Option<bool>)`:
  - When `rehearsal_test == true`: allow from REHEARSING, skip `is_rehearsal_completed`, set `return_state = Rehearsing`.
  - Default path unchanged (READY, requires rehearsal completed).
- `teardown_live_preview` transitions to `return_state`.
- `commit_live_preview` still requires recording consent (slice 5) before LIVE.

**Gate:** `cargo test --lib session::state`, integration test for new transitions.

---

### Slice 4: `lrpm-s4-recording-consent-backend` (MEDIUM)

**Goal:** Per-session recording consent before Live.

- SQLite column or session metadata flag `recording_consent_accepted_at`.
- Commands: `get_recording_consent_status(session_id)`, `accept_recording_consent(session_id)`, `clear_recording_consent(session_id)` (on session clone only if needed).
- `start_session` and `commit_live_preview` reject if consent not accepted for session.

**Gate:** persistence round-trip test.

---

### Slice 5: `lrpm-s5-rehearsal-setup-modal` (MEDIUM)

**Goal:** Once-ever popup on Rehearsal entry.

- `RehearsalSetupModal.tsx`: explains mic/system audio/phone mode/settings check; checkbox "I understand"; Continue dismisses.
- `localStorage` key `flint.rehearsalSetupAck.v1`.
- Wire in `Rehearsal.tsx` on mount.

**Gate:** vitest for modal show/dismiss.

---

### Slice 6: `lrpm-s6-test-live-session-ui` (MEDIUM)

**Goal:** Test Live Session button in Rehearsal header/footer.

- Calls `run_live_readiness_check`; show pass/warn/fail modal with fix instructions.
- On all-pass (no Fail rows): enable "Run audio preview" → `start_live_preview(sessionId, true)`.
- Store `{ fingerprint, testedAt }` in localStorage per sessionId; disable button when fingerprint matches.
- Re-enable when fingerprint changes (phone mode toggle, mic recalibration, settings save).

Remove Mock Interview "Practice with sample call audio" block entirely.

**Gate:** vitest for button disable/reenable + mock removal.

---

### Slice 7: `lrpm-s7-recording-consent-ui` (MEDIUM)

**Goal:** Consent gate at Live entry (once per session).

- `RecordingConsentModal.tsx` before `commit_live_preview` / direct `start_session`.
- Copy: user confirms legal right to record/transcribe in their jurisdiction; Flint captures system audio + mic locally.
- Confirm → `accept_recording_consent` → proceed.
- Cancel → alert warning; stay on Rehearsal (or Live Preview back).

Wire in `LivePreview.tsx` Go Live and any direct live start path.

**Gate:** vitest for cancel/confirm flows.

---

### Slice 8: `lrpm-s8-manual-qa-doc` (SIMPLE)

Add `tests/manual-qa/live-readiness-checklist.md` with Rehearsal popup, test button, consent gate steps.

---

### Slice 9: `lrpm-s9-pr-ci-merge` (SIMPLE)

- Push branch, open PR, CI loop until green, squash merge.
- Update loop state `ci_green`, `loop_stopped: true`.

---

## Stop conditions

- PR merged, CI green
- Blocker after 3 slice attempts or 6 CI fix attempts
- Manual device gate → park in `manual_gate_backlog`

## Manual gate backlog (post-merge)

- Full Test Live Session on real Zoom/phone call
- Recording consent copy review by user (non-lawyer disclaimer)
