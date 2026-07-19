# Live readiness + recording consent (manual gate)

Post-merge checklist for `feature/live-readiness-private-mode`.

## Rehearsal setup popup

- [ ] First Rehearsal entry shows "Before you rehearse" modal with Test Live Session guidance
- [ ] "Don't show again" + dismiss — modal does not reappear on next Rehearsal entry

## Test Live Session

- [ ] **Test Live Session** button runs readiness checks (Private Mode, Whisper, LLM key, loopback, etc.)
- [ ] Fail rows show fix instructions; button stays enabled until checks pass and modal closes
- [ ] After pass + close (or **Run 60s audio preview**), button shows **Live test complete** and is disabled
- [ ] Change **Phone interview mode** (Settings → Session Focus) — button re-enables
- [ ] Re-run mic calibration — button re-enables
- [ ] 60s preview from Rehearsal returns to Rehearsal (not stuck in READY)

## Recording consent

- [ ] Live Preview **Go Live** shows consent modal when not yet accepted for this session
- [ ] Confirm → session goes Live
- [ ] Cancel → warning alert, preview cancels, returns to Rehearsal
- [ ] New session requires consent again

## Mock Interview

- [ ] "Practice with sample call audio" block is removed from setup screen

## Private Mode rename

- [ ] Health Check row reads **Private mode** (not Stealth mode)
- [ ] X11 warning mentions Private mode
