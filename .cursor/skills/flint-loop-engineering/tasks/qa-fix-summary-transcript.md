# QA Fix — Post-Session Summary + Transcript UX

> **DO NOT RUN THIS LOOP until the user explicitly says**  
> `/flint-loop qa-fix-summary-transcript start`  
> This file is the plan + prompt only.

## Context (manual QA 2026-07-10)

Device: Linux Wayland + PipeWire. Zoom (standard) + phone-call mode.

### Unexpected bugs — IN SCOPE (fix these)

| ID | Bug | Evidence |
|----|-----|----------|
| **S1** | Post-session summary never usable | Every End Session → `"Summary unavailable for this session."` |
| **S2** | Interviewer questions torn into VAD chunks; each chunk has its own **Q** | Clicking Q answers a fragment |
| **S3** | Per-line **Q** unreliable vs **Ctrl+Q** | Phone: Ctrl+Q works; Q button does not |
| **S4** | Selecting live transcript to copy → white page + websocket error | No safe copy on live overlay |

### Expected / accepted — OUT OF SCOPE

Wayland L4 hotkeys, OBS visible, phone INTERVIEWER-heavy without diarization, BT HFP, open-speaker AEC (stretch only), speakrs P2, ~2s lag / intermittent auto-detect.

---

## Milestone structure

| Part | Repo | Branch | PR |
|------|------|--------|-----|
| **A — Summary + Q + Copy** | Flint | `fix/qa-summary-transcript-ux` | **One PR** → `main` (slices 0–5) |

**Repos:** Flint only. Do not touch smart-resume / flint-extension.

---

## Three-layer agent model (mandatory)

| Layer | Model slug | Role |
|-------|------------|------|
| **L1 Implementer (cheap)** | `composer-2.5-fast` | SIMPLE: runbook line, copy button UI, CI fmt/clippy fixes, merge + poll CI |
| **L2 Implementer / reviewer** | `grok-4.5-fast-xhigh` | MEDIUM: SessionSummary UI, TranscriptPanel Q wiring, JSON extract helper tests |
| **L3 Implementer / senior reviewer** | `claude-sonnet-5-thinking-high` | COMPLEX: summary command + session_id lifecycle, System chunk merge in Rust, final review |

### Review ladder (every slice)

```
SIMPLE  → Composer 2.5 implements  → Grok 4.5 reviews      → fix → commit
MEDIUM  → Grok 4.5 implements      → Sonnet 5 reviews      → fix → commit
COMPLEX → Sonnet 5 implements      → Sonnet 5 2nd-pass review → fix → commit
CI/merge → Composer 2.5 ONLY until gh pr checks all green + merged
```

**Parent agent:** launch Task subagents with model slugs above; do not substitute other models.

**Reviewer rules:** read-only findings; max one review round per slice; cite paths; reject Fisher files / secrets / staged `docs/`.

---

## Complexity tags per slice

| Slice | ID | Complexity | Implementer | Reviewer | Repo |
|-------|-----|------------|-------------|----------|------|
| 0 | `qa-s0-clean-slate` | SIMPLE | Composer 2.5 | Grok 4.5 | Flint |
| 1 | `qa-s1-summary-fix` | COMPLEX | Sonnet 5 | Sonnet 5 | Flint |
| 2 | `qa-s2-q-chunk-merge` | COMPLEX | Sonnet 5 | Sonnet 5 | Flint |
| 3 | `qa-s3-copy-transcript` | SIMPLE | Composer 2.5 | Grok 4.5 | Flint |
| 4 | `qa-s4-runbook-note` | SIMPLE | Composer 2.5 | Grok 4.5 | Flint |
| 5 | `qa-s5-review-pr-ci-merge` | SIMPLE | Composer 2.5 | Grok 4.5 | Flint |

---

## Kickoff — clean slate (Slice 0)

Working directory: `/home/alireza/Desktop/projects/Flint`

```bash
git fetch origin
git checkout main
git pull origin main
git status -sb   # clean except Fisher untracked OK

git checkout -b fix/qa-summary-transcript-ux
```

Initialize `.cursor/flint-loop-state.json`:

```json
{
  "current_milestone": "qa-fix-summary-transcript",
  "milestone_status": "in_progress",
  "milestone_branch": { "flint": "fix/qa-summary-transcript-ux" },
  "current_slice": 0,
  "completed_slices": [],
  "current_task_id": "qa-s0-clean-slate",
  "loop_stopped": false,
  "ci_fix_attempts": 0,
  "max_ci_fix_attempts": 6,
  "task_attempts": 0,
  "max_task_attempts": 3,
  "open_prs": {},
  "agent_ladder": {
    "simple": "composer-2.5-fast",
    "medium": "grok-4.5-fast-xhigh",
    "complex": "claude-sonnet-5-thinking-high"
  }
}
```

---

## Slices

### Slice 0: `qa-s0-clean-slate` (SIMPLE)

**Goal:** `main` current; branch exists; state file written; `loop_stopped: false`.

**Gate:** `git status` on branch clean (Fisher untracked OK).

---

### Slice 1: `qa-s1-summary-fix` (COMPLEX)

**Goal:** End Session always shows usable summary (narrative or soft fallback + stats). Past Sessions can retry.

**Requirements:**

1. Rust — extract JSON from noisy LLM output (strip ``` fences / preamble) before `apply_session_stats`.
2. Rust — if unparseable, return `summary_unavailable_json()` + stats (Ok, never raw prose).
3. Rust — optional `session_id` arg on `generate_session_summary` for retry from list/review.
4. Rust — ensure `session_id` available at ENDED when summary screen mounts.
5. UI — `SessionSummary.tsx`: show soft fallback one-liner; surface invoke errors; Retry button.
6. Tests: JSON extract unit; SessionSummary vitest for Ok(unavailable) vs Err.

**Gate:**

```bash
cd src-tauri && cargo test summary && cargo clippy -- -D warnings
cd .. && npm run test -- SessionSummary
```

**Commit:** `qa-fix slice 1: post-session summary JSON extract + soft fallback`

---

### Slice 2: `qa-s2-q-chunk-merge` (COMPLEX)

**Goal:** Q chip sends same full question text as Ctrl+Q for consecutive System VAD chunks.

**Requirements:**

1. Merge consecutive System chunks (time/silence window) in buffer or TranscriptPanel — prefer Rust `SystemTranscriptBuffer` alignment.
2. Q on merged utterance → full merged text via `trigger_response` (not single VAD fragment).
3. Phone mode: Q uses same path as Ctrl+Q (or hide Q + banner).
4. Tests: merge unit; TranscriptPanel test for burst → one Q target.

**Gate:**

```bash
cd src-tauri && cargo test system_transcript && cargo test transcript_panel
cd .. && npm run test -- TranscriptPanel
```

**Commit:** `qa-fix slice 2: merge System chunks for Q and Ctrl+Q parity`

---

### Slice 3: `qa-s3-copy-transcript` (SIMPLE)

**Goal:** Live overlay Copy transcript without DOM selection (avoids white-screen / websocket death).

**Requirements:**

1. Button on Transcript panel / live chrome.
2. Plain text from lines (match SessionReview format).
3. `copy_text_to_clipboard` (arboard); inline error on failure.
4. Vitest: click invokes copy with expected text.

**Gate:** `npm run test -- TranscriptPanel`

**Commit:** `qa-fix slice 3: live Copy transcript via native clipboard`

---

### Slice 4: `qa-s4-runbook-note` (SIMPLE)

**Goal:** Document fixes in tracked QA; park device retest.

**Update:** `tests/manual-qa/stealth-audio-validation-runbook.md` — Summary fix + Q merge + Copy lines.

**Commit:** `qa-fix slice 4: manual QA runbook notes for summary/Q/copy`

---

### Slice 5: `qa-s5-review-pr-ci-merge` (SIMPLE — Composer 2.5 ONLY)

**Goal:** One PR merged; CI green.

**Steps (Composer 2.5 only — do not escalate unless CI logic bug):**

1. Sonnet 5 read-only final pass on full diff (parent may run once before this slice).
2. `git push -u origin HEAD`
3. `gh pr create` — Summary, Q merge, Copy + test plan
4. Poll `gh pr checks` until all green (fix fmt/clippy/test flakes; increment `ci_fix_attempts`; max 6)
5. `gh pr merge` when green
6. `milestone_status: ci_green`, `loop_stopped: true`

**Commit (if CI fixes only):** `qa-fix slice 5: CI green`

---

## Stop conditions

- PR merged + CI green
- Same slice fails `max_task_attempts` (3)
- CI fix loop > `max_ci_fix_attempts` (6) — Composer 2.5 only for CI slice
- Device-only proof → `manual_gate_backlog`, continue other slices
- User: `/flint-loop stop`

## End deliverable

1. PR URL + merge confirmation
2. `git diff main --stat`
3. Slices done vs deferred
4. **"Ready for manual live retest"** or **"Blocked on …"**
