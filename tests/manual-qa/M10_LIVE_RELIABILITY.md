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
