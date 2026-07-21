# Multi-round interview prep — vertical slice

## Goal

After Round 1 (e.g. DAT recruiter screen), let the user stay on the same job session,
ingest a debrief into RAG, advance `round_type`, merge supplemental questions, and
re-enter rehearsal for Round 2.

## Existing infra (do not re-build)

- `round_type` + `save_session_focus` supplemental merge
- `mark_needs_focus_refresh` on `stop_session`
- `routeAfterDigestOrLive` → `SessionFocusGate` when `needsFocusRefresh`
- `reopen_past_session` — ENDED → REHEARSING

## Slices

| Slice | Work |
|-------|------|
| S1 | Technical supplemental questions in `round_questions.rs` |
| S2 | `ingest_round_debrief` Tauri command (chunk + embed + RAG append) |
| S3 | SessionSummary "Prepare for next round" → reopen + focus gate |
| S4 | SessionFocusGate next-round mode: banner, debrief field, ingest on save |
| S5 | Tests + local CI gates |

## CI gates

- `cargo test --lib --tests`
- `cargo clippy --all-targets -- -D warnings`
- `npx vitest run`

## Manual gate (deferred)

- Full Round 1 stop → summary → prepare → focus → rehearsal flow on device
