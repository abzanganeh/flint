# M10 — Live Session Reliability Fix

## Context

First real Zoom live session (Interview Kickstart mock) produced **zero responses**.
Root-cause analysis of `docs/error_logs.txt` revealed three cascading failures:

1. **Echo suppression killed all content** — 11/13 mic chunks and 2/13 system chunks
   were suppressed. The Jaccard threshold (0.5) with a 10s window treated overlapping
   Zoom audio as echo, so nothing reached the question detector.
2. **Question detector fired 14 times on empty content** — Pass 2 returned generic
   JSON every time. Burned Groq free-tier tokens on garbage.
3. **Groq 429 at session end** — `retry_after_secs=4901` (81 min). Post-session
   summary called `resolve_primary()` → Groq directly, bypassing the FailoverManager.
   DeepSeek was never tried even though the failover chain supports it.

Additional structural issues:
- Whisper skipped 34 chunks (`single timestamp ending - skip`).
- No manual "question ended" signal from the user.
- No speaker assignment UI (who is the interviewer?).
- Conversation memory unbounded for 1-2h sessions.
- **Phone mode is fundamentally broken**: `start_phone_mode()` opens two cpal
  streams on the same mic device. Both channels receive identical mixed audio
  (user + interviewer). Channel = speaker identity is impossible. Echo suppression
  kills all content. Auto question detection fires on user's own voice.
- **LLM provider order is hardcoded**: `PRIMARY_PROVIDERS` in `stack.rs` is
  `["groq", "openai", "anthropic", "deepseek"]`. Users cannot choose their
  default or configure fallback priority.

## Competitor Research

| Product | Speaker separation approach | Question boundary |
|---------|---------------------------|-------------------|
| **Verve Copilot** | pyannote diarization + user picks which speaker is interviewer | Auto + manual Assist button |
| **OphyAI** | pyannote.audio on server, labels turns | DeBERTa classifier + VAD silence threshold |
| **Final Round AI** | No diarization — trusts mic for question capture | Auto from transcript, no manual button |
| **CorpoDrone** | Dual capture (mic + loopback) = channel identity | Post-session pyannote refinement |
| **OpenWhispr** | WebRTC AEC3 sidecar + pyannote diarization | Transcript-level dedupe (mic duplicates retracted when system confirms) |
| **MeetScribe** | Dual-channel energy analysis: YOU (mic) vs REMOTE (system) | WhisperX forced alignment |
| **Clutch AI** | Single mic, RMS VAD, no diarization | BiLSTM/MLP classifier |

**Key insight from competitors:**
- Best approach for Flint's dual-channel setup: **channel identity IS speaker identity**
  (system = interviewer, mic = user). Echo suppression should only remove *exact duplicates*,
  not suppress original content.
- Several competitors offer a manual **"Assist" / "Question ended"** button as fallback.
- OpenWhispr's approach: retract mic duplicate AFTER system confirms — not pre-emptively kill.
- Nobody uses an LLM for question detection. They all use local classifiers (DeBERTa/BERT)
  or VAD silence thresholds. LLM-based detection wastes tokens and adds latency.

## Cost Analysis

| Component | Current cost | Fixed cost |
|-----------|-------------|------------|
| Question detection (14 calls on empty text) | ~7,000 tokens wasted | $0 (local DeBERTa + VAD) |
| Groq free tier (llama-3.3-70b) | 1,000 RPD / 100K TPD | Upgrade to Developer (free, just add card) for 10x |
| Post-session summary failover | Failed — no fallback tried | $0 extra (uses existing DeepSeek/Ollama key) |
| `speakrs` diarization (phone mode) | N/A | $0 — local ONNX, no LLM, ~200MB models |
| DeBERTa question classifier | N/A | $0 — local ONNX, ~600MB model, <50ms/check |
| Manual Ctrl+Q | N/A | $0 |
| Hybrid LLM verification (1 call/question) | N/A | ~$0.003/session (15-25 calls × ~200 tokens) |

---

## Fix Plan — 10 Slices

### Slice 1: Fix Echo Suppression (Critical — enables everything else)

**Branch:** `fix/echo-suppression-overhaul`

**Problem:** Jaccard 0.5 threshold + 10s window is too aggressive. In Zoom, both channels
carry overlapping audio. The "first arrival wins" strategy kills whichever channel transcribes
second — often the one with the actual question.

**Fix — switch to "channel identity = speaker identity" model:**

1. **Remove `CrossChannelDedup` from the hot path entirely for the default loopback mode.**
   In loopback mode:
   - System channel = interviewer's voice (what plays through speakers)
   - Mic channel = user's voice (what the mic picks up)
   - These ARE different speakers. Echo is a hardware/acoustic problem, not a content problem.

2. **Replace with lightweight energy-based echo gate:**
   - When both channels produce a transcript within 500ms of each other AND
     the Jaccard similarity is >= 0.85 (near-identical, not 0.5):
     - Keep the **System** channel version (interviewer takes priority for question detection)
     - Suppress the **Mic** version (it's acoustic bleed, not the user speaking)
   - This only triggers on true echo (same words), not on concurrent conversation.

3. **Echo suppression is DISABLED in phone-call mode.** Phone mode uses a completely
   different pipeline (see Slice 7). The two-channel dedup is irrelevant there.

4. **Add a `suppress_own_voice` flag per source:**
   - Mic transcripts that match >85% of a recent System transcript → suppress (user's speaker leaked)
   - System transcripts that match >85% of a recent Mic transcript → suppress (Zoom fed user's voice back)
   - This is directional: system→mic echo and mic→system echo are handled separately.

**Files:** `src-tauri/src/audio/pipeline.rs`

**Tests:**
- Existing echo tests updated for new thresholds
- New test: two different speakers talking concurrently → neither suppressed
- New test: exact echo at 90% Jaccard → suppressed correctly
- New test: 50% Jaccard overlap (different sentences) → NOT suppressed

---

### Slice 2: Hybrid Question Detection (DeBERTa + VAD + LLM verify + Ctrl+Q)

**Branch:** `feature/hybrid-question-detection`

**Problem:** Current detection uses an LLM (Groq) for every check. It fires every ~3s,
has no cooldown, no minimum word count, and burns tokens on empty text. 14 calls in 90s
all returned `ROLE=UNKNOWN`. Nobody in the industry uses an LLM for question detection.

**Fix — four-layer hybrid detection pipeline:**

**Layer 1: DeBERTa/BERT local classifier (always running)**
- Use a fine-tuned DeBERTa-base (~600MB ONNX model) or a smaller BERT variant
  to classify each new System-channel transcript chunk as question/not-question.
- Runs locally via ONNX Runtime (already a dependency via `speakrs`).
- Inference time: <50ms per chunk on CPU.
- Output: confidence score 0.0–1.0 and question type
  (behavioral/technical/situational/competency/general).
- If confidence > 0.7 → mark as "candidate question" and wait for VAD confirmation.
- If confidence < 0.3 → definitely not a question, skip.
- Between 0.3–0.7 → ambiguous, wait for more text.

**Layer 2: VAD silence threshold (event-driven)**
- After a candidate question (DeBERTa confidence > 0.7), monitor the System channel
  for silence.
- When VAD detects 1.5–2.0s of continuous silence on the System channel
  (interviewer stopped talking) → "question boundary confirmed."
- This prevents triggering mid-sentence when the interviewer pauses to think.

**Layer 3: Single LLM verification (optional, 1 call per question)**
- When DeBERTa + VAD agree → send the buffered System text to the LLM once.
- Prompt: "Given this transcript segment, confirm: is this an interview question?
  If yes, return the question text cleaned up. If no, return null."
- This call is small (~200 tokens) and happens only 15-25 times per hour.
- If LLM confirms → fire orchestrator with the cleaned question.
- If LLM says "not a question" → discard, don't fire orchestrator.
- **Skip this layer if LLM is rate-limited or unavailable** — DeBERTa + VAD alone
  are sufficient. The LLM verification is a quality boost, not a hard requirement.

**Layer 4: Manual Ctrl+Q override (always available)**
- User presses Ctrl+Q → grab the System transcript buffer since last Ctrl+Q.
- Bypass all three layers above.
- Send directly to orchestrator as a confirmed question.
- Visual flash on the "Q" button to confirm.
- This is the safety net that always works.

**Guard rails (prevent the token waste that happened):**
- Don't call any detector if no new System-channel text since last call.
- Minimum 5 new words on System channel before any detection attempt.
- Minimum 8s cooldown between LLM verification calls.
- Cache the last LLM result. If UNKNOWN, don't re-fire for 30s.
- If LLM returns a generic template response, treat as "no detection."

**Files:**
- `src-tauri/src/transcription/detector.rs` — replace LLM-only detection with hybrid pipeline
- `src-tauri/src/audio/pipeline.rs` — VAD silence tracking, transcript buffer management
- `src-tauri/src/commands.rs` — new `signal_question_ended` command for Ctrl+Q
- `src/screens/LiveOverlay.tsx` — Ctrl+Q button, rolling transcript display
- `src/hooks/useHotkeys.ts` — register Ctrl+Q
- New: `src-tauri/src/transcription/classifier.rs` — ONNX DeBERTa wrapper

**Model sourcing:**
- Fine-tuned DeBERTa-base on question detection datasets (SQuAD, QuALITY, custom
  interview Q&A). Export to ONNX. Bundle with the app (~600MB) or download on first use.
- Alternative: use a smaller model like `distilbert-base-uncased` (~250MB) if
  DeBERTa is too heavy. Trade accuracy for size.

**Tests:**
- Unit test: no new system text → no detection attempt
- Unit test: DeBERTa confidence < 0.3 → skip
- Unit test: DeBERTa confidence > 0.7 + VAD silence → triggers LLM verification
- Unit test: Ctrl+Q bypasses all layers
- Unit test: LLM rate-limited → DeBERTa + VAD alone fires orchestrator
- Unit test: generic UNKNOWN response → cached, skip for 30s

---

### Slice 3: Groq Rate Limit Handling + Summary Failover

**Branch:** `fix/groq-rate-limit-failover`

**Problem:** Two bugs:
1. Detection calls exhausted Groq free tier.
2. `generate_session_summary()` calls `resolve_primary()` → Groq directly,
   bypassing `FailoverManager`. When Groq returned 429, DeepSeek was never tried.

**Fix:**

1. **Route `generate_session_summary()` through the FailoverManager.**
   Replace the direct `provider.complete()` call with
   `state.failover_manager.complete()`. This automatically cascades
   Groq → DeepSeek → OpenRouter → Ollama.

2. **Handle 429 gracefully during live session:**
   - On 429, immediately switch to next provider in the failover chain.
   - Show a toast: "Groq rate limited — using [DeepSeek/Ollama] for this session."
   - Don't retry Groq for `retry_after_secs` duration.

3. **Post-session summary failover:**
   - If Groq 429 at session end → try DeepSeek → OpenRouter → Ollama → skip summary
     with message: "Summary unavailable — rate limited. Retry from Past Sessions."
   - Don't block session ending on summary failure.

4. **Separate token budgets:** Track token usage per-provider, not globally.
   Show remaining Groq budget in the UI.

**Files:**
- `src-tauri/src/commands.rs` — `generate_session_summary` uses FailoverManager
- `src-tauri/src/llm/failover.rs` — non-streaming `complete()` also cascades
- `src-tauri/src/llm/groq.rs` — respect retry_after, flag provider as exhausted

**Tests:**
- Unit test: summary 429 → falls back to DeepSeek
- Unit test: all cloud providers 429 → falls back to Ollama
- Unit test: summary failure doesn't block session end

---

### Slice 4: Configurable LLM Provider Priority

**Branch:** `feature/configurable-llm-priority`

**Problem:** `PRIMARY_PROVIDERS` is hardcoded as `["groq", "openai", "anthropic", "deepseek"]`.
Users cannot choose their default provider or configure fallback order.

**Fix:**

1. **New Settings UI section: "LLM Providers"**
   - Show all configured providers (those with API keys stored).
   - Drag-and-drop or up/down arrows to reorder.
   - Label slots: "Default", "Fallback 1", "Fallback 2", "Local (Ollama)".
   - Ollama is always the last resort and cannot be reordered.

2. **Persist the order in SQLite:**
   - New table or setting: `provider_priority` with ordered list.
   - `resolve_primary()` reads this instead of the hardcoded `PRIMARY_PROVIDERS`.
   - `resolve_cloud_tiers()` reads the same list, skipping the primary.

3. **Show active provider in the Live overlay:**
   - Small badge: "Groq" / "DeepSeek" / "Ollama" with color indicator.
   - Changes in real-time when failover triggers.

4. **Tauri commands:**
   - `get_provider_priority() → Vec<String>` — current order
   - `set_provider_priority(order: Vec<String>)` — save new order
   - `get_configured_providers() → Vec<{name, has_key, is_reachable}>` — for UI

**Files:**
- `src-tauri/src/llm/stack.rs` — read priority from persistence, not const
- `src-tauri/src/session/persistence.rs` — store/load provider priority
- `src-tauri/src/commands.rs` — new commands
- `src/screens/Settings.tsx` — new "LLM Providers" tab/section
- `src/commands/index.ts` — TypeScript wrappers

**Tests:**
- Unit test: custom order respected by resolve_primary
- Unit test: fallback chain follows user order
- Unit test: provider without key is skipped

---

### Slice 5: Whisper Chunk Skip Reduction

**Branch:** `fix/whisper-chunk-skip`

**Problem:** 34 out of ~48 Whisper chunks were skipped (`single timestamp ending`).
This means most audio was never transcribed.

**Fix:**

1. **Increase VAD pre-buffer:** Extend the speech segment by 200ms before and after
   VAD boundaries. Whisper struggles with segments that start/end abruptly.

2. **Lower beam search complexity for short segments:** Use greedy decoding for
   segments < 2 seconds. Beam search on short noisy segments causes timestamp
   confusion.

3. **Fall back to greedy decode when beam search produces `single timestamp ending`.**
   Re-transcribe the chunk with `strategy=0` (greedy) instead of discarding.

4. **Minimum segment duration:** Don't send segments shorter than 0.5s to Whisper.
   They're almost always noise or partial words.

**Files:**
- `src-tauri/src/audio/vad.rs` — pre/post buffer padding
- `src-tauri/src/transcription/engine.rs` — fallback decoding strategy

**Tests:**
- Unit test: short segments get greedy decoding
- Unit test: failed beam search retries with greedy

---

### Slice 6: Long Session Memory Management

**Branch:** `feature/long-session-memory`

**Problem:** 1-2 hour interviews generate unbounded transcript and conversation
memory. Context budget will overflow.

**Fix:**

1. **Rolling transcript window:** Keep last 5 minutes of raw transcript in memory.
   Older turns get compressed into a rolling summary via the compression prompt.

2. **Compression trigger:** When conversation memory exceeds 60% of context budget,
   run the compression prompt on the oldest 50% of turns.

3. **Transcript persistence:** Write transcript chunks to SQLite as they arrive
   (already done). Don't keep the full transcript in memory — read from DB
   if needed for post-session summary.

4. **Memory pressure indicator:** Show a subtle badge in the Live overlay when
   memory usage exceeds 80% of budget.

**Files:**
- `src-tauri/src/session/memory.rs` — compression trigger
- `src-tauri/src/orchestrator/mod.rs` — enforce rolling window
- `src/screens/LiveOverlay.tsx` — memory pressure badge

**Tests:**
- Unit test: compression triggers at 60% budget
- Unit test: old turns are removed after compression

---

### Slice 7: Phone Mode Overhaul (single-channel + Ctrl+Q)

**Branch:** `fix/phone-mode-overhaul`

**Problem:** Current `start_phone_mode()` opens two cpal streams on the **same mic**.
Both channels get identical audio (user + interviewer mixed). This means:
- Echo suppression kills everything (both channels have the same words).
- Channel = speaker identity is impossible (both speakers on both channels).
- Question detector fires on user's own voice from the System channel.
- The mode is fundamentally broken.

**Fix — single-channel capture + manual-only question detection:**

1. **Rewrite `AudioCapture::start_phone_mode()`:**
   - Open a **single** cpal stream on the default mic.
   - Route all frames to the **System channel only**.
   - Do NOT open a Mic channel stream.

2. **Disable automatic question detection in phone mode.**
   Set a flag `phone_mode_manual_only = true` in AppState.
   When this flag is set:
   - DeBERTa classifier is skipped (can't distinguish speakers).
   - Only `signal_question_ended` (Ctrl+Q from Slice 2) triggers responses.

3. **Disable echo suppression in phone mode.**
   There's only one channel — no cross-channel dedup needed.

4. **Live transcript display (essential for Ctrl+Q UX):**
   - Rolling "Incoming transcript" pane in the Live overlay.
   - User presses Ctrl+Q when interviewer finishes a question.
   - Transcript pane clears and starts fresh for the next question.

5. **Setup guidance in the UI:**
   - "Place your phone on speaker near your laptop microphone.
      Press Ctrl+Q when the interviewer finishes a question."
   - Recommend USB audio adapter as alternative for auto-detection.

**Files:**
- `src-tauri/src/audio/capture.rs` — rewrite `start_phone_mode()` to single stream
- `src-tauri/src/audio/pipeline.rs` — skip echo dedup and auto-detection in phone mode
- `src-tauri/src/state.rs` — add `phone_mode_manual_only: Mutex<bool>`
- `src-tauri/src/commands.rs` — set flag during `start_session`
- `src/screens/LiveOverlay.tsx` — rolling transcript pane, setup card

**Tests:**
- Unit test: phone mode opens only one stream
- Unit test: echo suppression disabled when phone_mode = true
- Unit test: auto question detector skipped when phone_mode_manual_only = true
- Unit test: Ctrl+Q fires orchestrator with transcript buffer

---

### Slice 8: Phone Mode Diarization with `speakrs`

**Branch:** `feature/phone-diarization-speakrs`

**Problem:** Slice 7 gives phone mode a manual-only Ctrl+Q workflow. This works but
requires user attention during the interview. Competitors (Verve) offer automatic
speaker separation via diarization. We should match or exceed that.

**Why `speakrs` over `polyvoice`:**
- 7.1% DER vs mid-20s% — accuracy matters for 2-speaker phone calls with
  compressed audio from a phone speaker.
- Pure Rust, no Python runtime. ONNX Runtime is already a dependency.
- 50x realtime on CPU — can process a 3s window in 60ms.
- 200MB models are acceptable for a desktop app already bundling Whisper (~150MB).

**Implementation:**

1. **Add `speakrs` crate as a dependency:**
   ```toml
   speakrs = { version = "0.4", features = ["cpu"] }
   ```
   On macOS also enable `coreml` feature. On NVIDIA systems, `cuda`.

2. **Diarization manager (`src-tauri/src/audio/diarizer.rs`):**
   - Holds a `speakrs::Pipeline` instance.
   - Receives raw PCM audio from the single phone-mode mic stream.
   - Processes in 3-second rolling windows every 2 seconds.
   - Returns speaker labels with timestamps: `[(Speaker_0, 0.0-2.3), (Speaker_1, 2.5-4.8)]`.

3. **Speaker picker UI (first 15-30 seconds of session):**
   - After diarization detects 2+ speakers, show a modal in the Live overlay:
     "We detected two speakers. Select the interviewer."
   - Show sample text from each speaker.
   - User clicks to assign: "This is the interviewer" / "This is me."
   - Can reassign mid-session if wrong.

4. **Route diarized segments to the correct channel:**
   - Once the user assigns speakers:
     - Interviewer segments → System channel pipeline (question detection fires on these)
     - User segments → Mic channel pipeline (ignored by question detector)
   - The hybrid detection pipeline (Slice 2) then works as normal on the
     diarized interviewer segments.

5. **Fallback to Ctrl+Q:**
   - If diarization can't separate speakers (too similar voices, noisy audio),
     show a message: "Speaker separation couldn't distinguish voices. Use Ctrl+Q."
   - Ctrl+Q always works regardless of diarization status.

6. **Model download:**
   - On first use, download `speakrs` models (~200MB) via `ModelManager`.
   - Show progress bar in Settings → "Audio Models" section.
   - Store models in `~/.flint/models/speakrs/`.

**Files:**
- `src-tauri/Cargo.toml` — add `speakrs` dependency
- New: `src-tauri/src/audio/diarizer.rs` — speakrs wrapper
- `src-tauri/src/audio/pipeline.rs` — integrate diarizer in phone mode
- `src-tauri/src/commands.rs` — `assign_speaker`, `download_diarization_models`
- New: `src/components/SpeakerPicker.tsx` — speaker assignment modal
- `src/screens/LiveOverlay.tsx` — show SpeakerPicker, diarization status
- `src/screens/Settings.tsx` — model download UI

**No extra LLM cost.** Diarization is 100% local ONNX inference.

**Tests:**
- Unit test: diarizer returns 2 speakers with timestamps
- Unit test: speaker assignment routes segments to correct channel
- Unit test: fallback to Ctrl+Q when diarization fails
- Integration test: phone mode with diarization → question detection fires

---

### Slice 9: Live Overlay UX Improvements

**Branch:** `feature/live-overlay-ux`

**Problem:** No visibility into what Flint is hearing or doing during a live session.
User had no idea nothing was being transcribed.

**Fix:**

1. **Rolling transcript pane:**
   - Small scrolling area showing the last 30s of System-channel transcript.
   - Color-coded: interviewer text in blue, user text in grey.
   - Helps user see what Flint heard and when to press Ctrl+Q.

2. **Active provider badge:**
   - Show "Groq" / "DeepSeek" / "Ollama" with green/amber/red indicator.
   - Changes in real-time when failover triggers.

3. **Detection activity indicator:**
   - Small pulsing dot when the question detector is processing.
   - Turns green when a question is detected.
   - Shows "Listening..." / "Question detected" / "Generating response..."

4. **Ctrl+Q button with visual feedback:**
   - Button labeled "Q" with tooltip.
   - Pulses/flashes when pressed to confirm.
   - Shows the captured question text briefly.

5. **Token usage indicator:**
   - Compact display: "Groq: 42K/100K tokens" or percentage bar.
   - Turns amber at 70%, red at 90%.

**Files:**
- `src/screens/LiveOverlay.tsx` — all UI additions
- `src/components/rehearsal-enrichment.css` — styles
- `src-tauri/src/commands.rs` — expose transcript buffer, token usage

**Tests:**
- Frontend test: transcript pane updates on event
- Frontend test: Ctrl+Q visual feedback

---

### Slice 10: Speaker Assignment UI (Standard Mode)

**Branch:** `feature/speaker-assignment-ui`

**Problem:** Flint assumes System = interviewer, Mic = user. In some setups
(unusual routing), this assumption breaks.

**Fix (Verve Copilot pattern):**

1. **During first 30s of live session:** Show detected speakers with sample text.
2. **User clicks to assign:** "This is the interviewer" / "This is me."
3. **Swap channel labels if needed:** If user assigns Mic as interviewer,
   swap the source labels in the pipeline.

**Lower priority** — echo suppression fix (Slice 1) and hybrid detection (Slice 2)
solve the immediate problem for standard mode. Speaker assignment is for edge cases
in standard mode (phone mode has its own diarization in Slice 8).

**Files:**
- `src-tauri/src/audio/pipeline.rs` — swappable source labels
- `src/screens/LiveOverlay.tsx` — speaker assignment modal
- New component: `SpeakerAssignment.tsx`

---

## Execution Order

| Order | Slice | Branch | Depends on | Effort |
|-------|-------|--------|------------|--------|
| 1 | Echo suppression fix | `fix/echo-suppression-overhaul` | None | Large |
| 2 | Hybrid question detection | `feature/hybrid-question-detection` | None (parallel with 1) | X-Large |
| 3 | Groq rate limit + summary failover | `fix/groq-rate-limit-failover` | None (parallel) | Medium |
| 4 | Configurable LLM priority | `feature/configurable-llm-priority` | Slice 3 | Medium |
| 5 | Whisper chunk skip | `fix/whisper-chunk-skip` | None (parallel) | Medium |
| 6 | Long session memory | `feature/long-session-memory` | None (parallel) | Medium |
| 7 | Phone mode overhaul (Ctrl+Q) | `fix/phone-mode-overhaul` | Slices 1, 2 | Large |
| 8 | Phone diarization (speakrs) | `feature/phone-diarization-speakrs` | Slices 2, 7 | X-Large |
| 9 | Live overlay UX | `feature/live-overlay-ux` | Slices 2, 3, 7 | Medium |
| 10 | Speaker assignment (standard) | `feature/speaker-assignment-ui` | Slices 1, 2 | Large |

**Phase 1 (critical fixes):** Slices 1, 2, 3 in parallel → Slice 7.
**Phase 2 (UX + features):** Slices 4, 5, 6, 9 in parallel.
**Phase 3 (diarization):** Slice 8 → Slice 10.

## Git Workflow

1. Create each slice on its own branch from `main`.
2. Implement → `cargo test` → `cargo clippy` → commit.
3. Run Bugbot review on the branch.
4. Fix all findings → commit.
5. Push branch to GitHub.
6. After all slices pass review: create a single integration branch
   `feature/m10-live-reliability` that merges slices 1-9.
7. Run full test suite on integration branch.
8. Fix any cross-slice issues.
9. Open PR to `main`.
10. Fix CI failures, rerun until green.

## Acceptance Criteria

- [ ] Echo suppression allows concurrent speakers (Jaccard < 0.85 → no suppression)
- [ ] Hybrid detection: DeBERTa + VAD + optional LLM verify replaces LLM-only detection
- [ ] Ctrl+Q manually marks question boundary and triggers response
- [ ] Groq 429 triggers automatic failover to DeepSeek/Ollama
- [ ] Post-session summary uses FailoverManager, not direct provider call
- [ ] User can reorder LLM providers (default + fallback 1 + fallback 2) in Settings
- [ ] Active provider badge visible in Live overlay
- [ ] Whisper chunk skip rate < 10% on typical Zoom audio
- [ ] 1-hour session memory stays within context budget via compression
- [ ] Phone mode: single mic stream, no echo dedup, Ctrl+Q for questions
- [ ] Phone mode with diarization: `speakrs` separates speakers, user picks interviewer
- [ ] Phone mode fallback: Ctrl+Q always works if diarization fails
- [ ] Rolling transcript visible in Live overlay
- [ ] Token usage indicator in Live overlay
- [ ] All existing tests pass
- [ ] New tests cover each fix
