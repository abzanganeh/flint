# M10 — Live Session Reliability (manual QA)

## Prerequisites
- Groq + DeepSeek (or Ollama) keys configured
- Headphones optional (echo gate now 0.85 Jaccard)
- Wayland for stealth (not X11)

## Loopback live (standard)
1. Start rehearsal-complete session, go LIVE on Zoom/Teams loopback.
2. Confirm System-channel transcript appears in live status bar (30s rolling).
3. Interviewer asks question — hybrid detection should fire without burning Groq on empty text.
4. Press **Ctrl+Q** when interviewer finishes — confirm directional/depth panels stream.
5. Exhaust Groq quota (or mock 429) — confirm failover toast/badge and post-session summary still completes.

## Phone mode
1. Settings → Session Focus → Phone interview mode.
2. Confirm single mic stream (no duplicate channels in transcript).
3. Ctrl+Q only for questions until speakrs models installed.
4. If speaker picker appears, assign interviewer and verify auto-detection resumes.

## Provider priority
1. Settings → LLM Providers — reorder Groq/DeepSeek.
2. Start live session — badge shows active provider; failover updates badge.

## Deferred (device)
- speakrs model download + real 2-speaker phone diarization (Slice 8 ONNX)
- Subjective WER on real loopback hardware

---

## Device run — 2026-07-10 (Linux Wayland + Zoom, standard mode)

| Check | Result |
| --- | --- |
| Loopback eventually works | **PASS** after leaving Bluetooth HFP as Zoom speaker; wired/headphones path better. Watchdog “No audio captured yet” while Zoom on BT call sink |
| Headphones | Better channel separation than open speakers / BT HFP |
| Transcript WER | **Poor** — heavy garble (“Canvas”/Camas, “ion projects”, mirrored YOU↔INTERVIEWER fragments) |
| Question chunking / Q button | **FAIL UX** — interviewer utterance split across many short VAD lines; each line has its own **Q**, so one click only answers a fragment (“Himself.” / “So...” / half-question) |
| Auto-detect | Fires on fragments; often wrong boundary |
| Answer context quality | **Weak** — RAG/LLM gets fragment + noisy transcript, not full question + clean session context |
| Workaround | Wait until interviewer **finishes**; press **Ctrl+Q** (or trigger hotkey with Flint focused) once — uses rolling System buffer, not a single tiny Q row. Prefer wired headphones; avoid BT HFP for Zoom speaker |

**M10 status:** loopback path workable with correct output device; **question-boundary + WER + context** remain open soft issues (not L2 regression).
