# M10 DAT Interview Reliability — post-mortem fixes

## Root causes (Jul 20 DAT recruiter screen)

1. **Preview→live buffer carryover** — rehearsal/test STT polluted first live Ctrl+Q span
2. **Q vs Ctrl+Q path split** — TranscriptPanel Q used `trigger_response` (UI fragment) not backend buffer
3. **Whisper backlog vs silence confirm** — auto-detect fired before late STT chunks arrived
4. **Uncertain speaker auto-detect** — channel-labeled chunks auto-fired before classifier/relabel

## Slices (this milestone)

| Slice | Fix |
|-------|-----|
| S1 | Clear buffer + reset hybrid on `commit_live_preview` |
| S2 | Store `hybrid_question_detector` on live/preview handles; reset on `signal_question_ended` |
| S3 | Block silence confirm while `whisper_worker.pending_jobs() > 0` |
| S4 | Block auto-detect while `SystemTranscriptBuffer::has_uncertain_speaker()` |
| S5 | Latest System Q chip → `signalQuestionEnded`; older lines → `triggerResponse` |

## CI gates

- `cargo test --lib --tests`
- `cargo clippy --all-targets -- -D warnings`
- `npx vitest run`
- `./scripts/check-coverage-gates.sh` (on PR)

## Manual gate (deferred)

- Real Zoom loopback retest with DAT-style noisy headset — `tests/manual-qa/M10_LIVE_RELIABILITY.md`
