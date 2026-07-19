# Visual Robustness + Manual Diagram Trigger — Manual QA Checklist

Manual gate for the `visual-robustness-and-triggers` milestone before merging
`feature/visual-robustness-and-triggers`: SSE byte-buffering fix (all 5 LLM
providers), Visual non-streaming, Mermaid auto-retry, prompt guardrails, and
Generate diagram buttons on Live, Rehearsal, and Mock Interview.

## Prerequisites

- `npm run tauri dev` on Linux Wayland.
- Groq (or another configured cloud provider) with a valid API key in Settings.
- A session past Digest Review with a job description mentioning system design
  or architecture (helps Visual auto-fire in section 1).
- Whisper model installed (`ggml-small.en.bin` in `~/.cache/whisper/`).

---

## 1. Live — original bug regression

| Step | Expected |
|------|----------|
| Start a LIVE session | Four-panel overlay; Visual panel visible |
| Ask (system audio or manual trigger): "Draw a system design for a notification microservice" | Answer panel streams a textual response |
| Visual panel (auto or manual) | Mermaid diagram renders without red bomb / "Syntax error in text" |
| Repeat the same prompt 3–5 times across the session | Diagrams stay syntactically valid; no missing arrows, dropped newlines, or truncated node labels (e.g. `Twilio` → `ilio`) |

---

## 2. Live — manual Generate diagram

| Step | Expected |
|------|----------|
| Ask a behavioral question (e.g. "Tell me about a conflict with a teammate") | Visual panel stays idle (classifier skips) |
| Click **Generate diagram** in Visual panel | Diagram generates for the current question without re-asking |
| Mermaid render succeeds | Diagram shown; no raw fallback |

---

## 3. Rehearsal — Generate diagram

| Step | Expected |
|------|----------|
| Open Rehearsal for a configured session | Visual panel visible with session wired |
| Ask a behavioral question | Answer streams; Visual may stay empty |
| Click **Generate diagram** | Turn re-runs with Visual forced; diagram appears |
| `asking` / generating state | UI shows generating during the forced turn; no duplicate error toasts |

---

## 4. Mock Interview — Generate diagram

| Step | Expected |
|------|----------|
| Start Mock Interview (practice or study mode) | Question appears; suggested answer flow unchanged |
| Expand/collapse Visual panel (if toggle present) | Panel shows below suggested answer area |
| Click **Generate diagram** while a question is active | `trigger_mock_visual_response` fires; diagram renders via `visual_token` |
| Complete turn / coach feedback | TTS question flow and coach grading unaffected by Visual panel |

---

## 5. Mermaid auto-retry (optional)

| Step | Expected |
|------|----------|
| If a diagram fails Mermaid parse once | Panel silently retries once (same "Generating…" state) |
| Second failure for same question | Raw fenced text fallback; no infinite retry loop |

---

## 6. Folded-in fixes (smoke)

| Step | Expected |
|------|----------|
| Installation health check → Whisper row missing model | Fix copy shows install script + curl one-liner (not vague "download before first session" only) |
| Mic calibration paragraph | Generic project-management text (no stale SecureAuth/IAM fixture copy) |
| Settings → download speaker separation models | Download completes or times out gracefully; no white-screen crash on eager ONNX load |

---

## Sign-off

| Platform | Tester | Date | Pass / Fail | Notes |
|----------|--------|------|-------------|-------|
| Linux Wayland | | | | |
