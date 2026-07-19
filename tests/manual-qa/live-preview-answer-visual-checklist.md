# Live Preview + Answer/Visual Engine — Manual QA Checklist

Manual gate for the combined `live-preview-answer-visual` milestone before
merging `feature/live-preview-answer-visual`: Part A (Live Preview state,
3-tier speaker ID, per-line relabel, live onboarding) + Part B (Answer +
Visual orchestrator, mermaid/shiki rendering, four-panel overlay).

## Prerequisites

- `npm run tauri dev` on Linux Wayland (X11 fails the private mode gate before
  LIVE is reachable).
- A session past Digest Review with a job description that mentions
  system design or architecture (needed for the Visual classifier to
  auto-fire in section 5).
- Groq or Ollama configured; headphones connected for the non-phone
  sections.
- A second device (or a recorded call) to play interviewer audio for
  sections 2 and 3.

---

## 1. Live Preview flow

| Step | Expected |
|------|----------|
| From READY, start Live Preview | Audio pipeline starts; transcript panel shows live captions; no Answer/Visual panel activity (no orchestrator spawn) |
| Speak a test question during preview | Transcript captures it; no answer is generated |
| Wait 60s without committing | Preview auto-cancels back to READY |
| Start Live Preview again, click **Commit** | Transitions to LIVE without re-initializing audio capture (no visible capture restart/gap) |
| Start Live Preview, click **Cancel** | Returns to READY; pipeline stops cleanly |

---

## 2. Phone relabel

| Step | Expected |
|------|----------|
| Enable phone call mode, go LIVE with phone on speaker | Transcript lines default per phone-mode speaker rules |
| Interviewer line misclassified as `You` (or vice versa) | Per-line swap control relabels it; `speaker_refined` fires; persisted speaker updates in SQLite |
| Relabel a chunk to `You` | That chunk is excluded from any "last interviewer question" span used by Ask now |
| Speaker with a strong accent or noisy line | Uncertain-speaker hint renders in phone mode; manual relabel resolves it |

---

## 3. Ask now

| Step | Expected |
|------|----------|
| LIVE session, interviewer asks a question | Status bar button reads **Ask now** with its keycap hint (not the old bare "Q") |
| Click **Ask now** / press its hotkey | Sends the last Interviewer-labeled span (respecting relabels from section 2) to the orchestrator — not a blind 30s rolling window |
| Phone mode, single mic channel | Ask now still resolves the correct interviewer span despite everything being tagged `System` per phone-mode rules |

---

## 4. Answer stream

| Step | Expected |
|------|----------|
| Ask any question | Answer panel streams tokens live; question heading shows the current question |
| Response completes | Confidence border color + label appear (e.g. green / "Grounded") |
| Click **Answer This** | Text copied to clipboard; button reads "Copied!" briefly; panel enlarges (Answer Now mode) |
| Click **Rephrase** | Buffers clear; a new Answer draft streams for the same question |
| Ask a question with no prior manual question in the box | **Rephrase** stays disabled |

---

## 5. Visual — mermaid on a system design question

| Step | Expected |
|------|----------|
| Ask "How would you design a URL shortener?" (or similar system-design phrasing) | Visual thread auto-fires alongside Answer (classifier match) — no manual trigger needed |
| Visual response completes | A mermaid diagram renders as SVG in the Visual panel, not raw text |
| Ask a purely behavioral question (e.g. "Tell me about a time you disagreed with a teammate") | Visual panel stays empty — classifier correctly skips it |
| Spot-check 2–3 diagram types beyond flowchart (sequence, class, or ER) over a few system-design questions | Each renders as a diagram, not a raw code block |
| Ask a question that returns a non-diagram code snippet | Panel shows syntax-highlighted code (shiki), not a mermaid render attempt |
| Repeat a previously pre-warmed question | **pre-prepared** badge shown next to the Visual panel header |

---

## 6. Manual visual trigger

| Step | Expected |
|------|----------|
| Ask a behavioral question (Visual correctly skipped per section 5) | Visual panel shows a **Generate diagram** button next to the waiting placeholder |
| Click **Generate diagram** | Visual thread fires for the current question despite the classifier's verdict; button hides while generating |
| Diagram completes | Renders exactly like an auto-fired Visual response (mermaid SVG or highlighted code) |
| Open Rehearsal mode | **Generate diagram** button is absent — manual trigger requires a LIVE session (`trigger_visual_response` rejects non-LIVE state) |
| No current question yet (fresh turn) | **Generate diagram** button stays hidden until a question is asked |

---

## Pass criteria

- [ ] Live Preview starts, auto-cancels at 60s, commits and cancels cleanly
- [ ] Phone relabel corrects misclassified lines and updates Ask now's span
- [ ] Ask now sends the correct interviewer span, not a blind time window
- [ ] Answer streams, shows confidence, and Answer This / Rephrase both work
- [ ] Visual auto-fires and renders mermaid on system-design questions, skips behavioral ones
- [ ] Manual "Generate diagram" trigger works from LIVE and is absent from Rehearsal

## Triage

If any box fails, capture:

1. The session ID and the exact question text that triggered the failure.
2. `~/.flint/metrics.log` tail and the last 200 lines of stderr.
3. Which slice of `live-preview-answer-visual` the regression is closest to
   (Part A slices 1–16 vs. Part B slices 17–33) so the fix lands on the
   right branch.
