# Live Preview + Answer/Visual Engine — Combined Milestone

> **Kickoff:** `/flint-loop live-preview-answer-visual start`  
> **One branch, one PR, one merge** — Part A then Part B sequentially; full local gates after Part B; then senior review → fix → re-test → Composer PR/CI/merge.

## Context

Combines two planned workstreams into a single engineering run:

| Part | Source plan | Goal |
|------|-------------|------|
| **A** | `phone-interview-and-live-preview` (Jul 2026) | Speaker ID (3 tiers), Live Preview state, live onboarding, Ctrl+Q cleanup |
| **B** | `flint_domain-plugin_architecture` Workstream 2 | Collapse Directional/Depth/Clarifying → **Answer + Visual**; 4-panel layout |

**Branch:** `feature/live-preview-answer-visual` (from latest `main` after PR #37).

**Repos:** Flint only. Do not touch smart-resume / flint-extension.

### Hard guardrails

- Prompts in `/prompts/` only — never inline LLM prompt string literals in Rust.
- React is a dumb renderer — session state + thread orchestration in Rust.
- Parallel threads via `tokio::spawn` — Answer + Visual never sequential.
- API keys in keychain; no session content in INFO logs.
- **NEVER** commit Fisher scripts/json, secrets, or staged `docs/`.
- Domain-plugin trait / Meeting runtime **out of scope** — Answer+Visual only, interview session type unchanged.

### Explicitly OUT OF SCOPE

- macOS/Windows manual QA matrix (park in `manual_gate_backlog`)
- Excalidraw Tier 2a (`@excalidraw/mermaid-to-excalidraw`) — defer to follow-up PR unless Tier 1 green with time left
- Visual Tier 2b deeper AI re-pass — stub button + TODO only
- Full `SessionDomain` trait / Meeting domain
- speakrs model download UX changes beyond existing SpeakerPicker

---

## Three-layer agent model (mandatory)

| Layer | Model slug | Role |
|-------|------------|------|
| **L1 — cheap** | `composer-2.5-fast` | SIMPLE slices, Vitest UI, runbook lines, fmt/clippy/CI fixes, PR poll + merge |
| **L2 — medium** | `grok-4.5-fast-xhigh` | MEDIUM slices: health-adjacent TS, event wiring, classifier stubs, eval harness updates |
| **L3 — complex** | `claude-sonnet-5-thinking-high` | COMPLEX: audio pipeline speaker logic, orchestrator rework, state machine, final consolidated review |

### Review ladder (every slice)

```
SIMPLE  → Composer 2.5 implements  → Grok 4.5 reviews      → fix → commit → gates
MEDIUM  → Grok 4.5 implements      → Sonnet 5 reviews      → fix → commit → gates
COMPLEX → Sonnet 5 implements      → Sonnet 5 2nd-pass review → fix → commit → gates
```

**Final phase (slices 34–37):**

```
Sonnet 5 — full diff read-only review (blocking vs nit)
Grok 4.5 or Sonnet 5 — apply blocking fixes
Full gates — cargo test, clippy, vitest, eval smoke if prompts changed
Composer 2.5 ONLY — push, gh pr create, poll checks, fix flakes, merge
```

**Parent agent:** launch Task subagents with model slugs above; do not substitute.

**Reviewer rules:** read-only findings; max one review round per slice; cite paths; reject Fisher/secrets/docs staged.

---

## Milestone structure

| Part | Slices | Deliverable |
|------|--------|-------------|
| **A — Live preview & speaker UX** | 0–16 | LIVE_PREVIEW, speaker classifier, per-line relabel, onboarding |
| **B — Answer + Visual engine** | 17–33 | 2-thread orchestrator, new prompts/panels/events, eval update |
| **C — Review + ship** | 34–37 | Senior review, fix, full gates, PR merged CI green |

**Do not stop between slices 0–33** unless a stop condition hits.

---

## Complexity tags (all slices)

| Slice | ID | Complexity | Implementer | Reviewer |
|-------|-----|------------|-------------|----------|
| 0 | `lpav-s0-clean-slate` | SIMPLE | Composer 2.5 | Grok 4.5 |
| 1 | `lpav-s1-turn-history-context` | SIMPLE | Composer 2.5 | Grok 4.5 |
| 2 | `lpav-s2-phone-heuristic` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 3 | `lpav-s3-suspicion-detector` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 4 | `lpav-s4-speaker-classifier` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 5 | `lpav-s5-speaker-refined-events` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 6 | `lpav-s6-transcript-relabel-ui` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 7 | `lpav-s7-q-interviewer-span` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 8 | `lpav-s8-live-preview-state` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 9 | `lpav-s9-live-preview-commands` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 10 | `lpav-s10-live-preview-screen` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 11 | `lpav-s11-mock-sample-audio` | SIMPLE | Composer 2.5 | Grok 4.5 |
| 12 | `lpav-s12-first-run-live-modal` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 13 | `lpav-s13-ask-now-phone-toggle` | SIMPLE | Composer 2.5 | Grok 4.5 |
| 14 | `lpav-s14-ctrlq-cleanup` | SIMPLE | Composer 2.5 | Grok 4.5 |
| 15 | `lpav-s15-part-a-rust-tests` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 16 | `lpav-s16-part-a-vitest` | SIMPLE | Composer 2.5 | Grok 4.5 |
| 17 | `lpav-s17-prompts-answer-visual` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 18 | `lpav-s18-answer-thread` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 19 | `lpav-s19-visual-thread` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 20 | `lpav-s20-visual-classifier` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 21 | `lpav-s21-orchestrator-two-thread` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 22 | `lpav-s22-events-dtos` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 23 | `lpav-s23-prewarm-confidence` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 24 | `lpav-s24-answer-panel` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 25 | `lpav-s25-visual-panel-mermaid` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 26 | `lpav-s26-four-panel-layout` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 27 | `lpav-s27-ui-store-streams` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 28 | `lpav-s28-retire-clarifying` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 29 | `lpav-s29-eval-harness-update` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 30 | `lpav-s30-orchestrator-integration-tests` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 31 | `lpav-s31-panel-vitest` | SIMPLE | Composer 2.5 | Grok 4.5 |
| 32 | `lpav-s32-rules-update` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 33 | `lpav-s33-manual-qa-checklist-and-local-docs` | SIMPLE | Composer 2.5 | Grok 4.5 |
| 34 | `lpav-s34-consolidated-review` | COMPLEX | — | Sonnet 5 |
| 35 | `lpav-s35-fix-review-findings` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 36 | `lpav-s36-full-gates-rerun` | SIMPLE | Composer 2.5 | Grok 4.5 |
| 37 | `lpav-s37-pr-ci-merge` | SIMPLE | Composer 2.5 | Grok 4.5 |

---

## Kickoff — Slice 0

Working directory: `/home/alireza/Desktop/projects/Flint`

```bash
git fetch origin
git checkout main && git pull origin main
git checkout -b feature/live-preview-answer-visual
```

Initialize `.cursor/flint-loop-state.json`:

```json
{
  "current_milestone": "live-preview-answer-visual",
  "milestone_status": "in_progress",
  "milestone_branch": { "flint": "feature/live-preview-answer-visual" },
  "current_slice": 0,
  "completed_slices": [],
  "current_task_id": "lpav-s0-clean-slate",
  "loop_stopped": false,
  "ci_fix_attempts": 0,
  "max_ci_fix_attempts": 6,
  "task_attempts": 0,
  "max_task_attempts": 3,
  "open_prs": {},
  "manual_gate_backlog": [
    {
      "gate": "Live Preview device retest",
      "doc": "tests/manual-qa/live-preview-answer-visual-checklist.md",
      "notes": "After merge: headphones path, phone mode relabel, Ask now, Mermaid in Visual panel"
    }
  ],
  "agent_ladder": {
    "simple": "composer-2.5-fast",
    "medium": "grok-4.5-fast-xhigh",
    "complex": "claude-sonnet-5-thinking-high"
  },
  "notes": "Combined Part A (live preview) + Part B (Answer+Visual). One PR."
}
```

**Gate:** `git status` clean except Fisher untracked OK.

---

## Part A — Live Preview & Speaker UX (slices 1–16)

### Slice 1: `lpav-s1-turn-history-context` (SIMPLE)

**Goal:** Move "Earlier questions" `HistoryCard` from DirectionalPanel + DepthPanel → ContextPanel only.

**Files:** `DirectionalPanel.tsx`, `DepthPanel.tsx`, `ContextPanel.tsx`, `TurnCards.tsx`, Vitest if exists.

**Tests:** Vitest — ContextPanel renders turn history; Directional/Depth do not.

**Commit:** `lpav slice 1: move turn history to Context panel`

---

### Slice 2: `lpav-s2-phone-heuristic` (COMPLEX)

**Goal:** Phone mode provisional speaker labels via RMS + pause-before-utterance.

**Implement:**
- `src-tauri/src/audio/speaker_heuristic.rs` — `classify_phone_utterance(rms, pause_ms, prior) -> SpeakerRole`
- Hook in `audio/pipeline.rs` when `phone_call_mode` — tag chunks before emit
- Extend chunk payload with `label_source: "heuristic" | "channel" | "llm" | "user"`

**Tests:** unit tests for RMS thresholds; pipeline test with injected frames.

**Commit:** `lpav slice 2: phone mode energy heuristic speaker labels`

---

### Slice 3: `lpav-s3-suspicion-detector` (COMPLEX)

**Goal:** Route existing suspicion detector into the Tier-2 classifier queue instead of only auto-correcting directly.

**IMPORTANT — do not rebuild from scratch.** `src-tauri/src/transcription/speaker_suspicion.rs` already implements the question-shape-on-Mic / first-person-on-System heuristics and is already called from `audio/pipeline.rs` (`speaker_suspicion::evaluate`), which auto-corrects the label immediately. That part is done.

**What's actually missing:**
- Near-duplicate System/Mic within 1.5s signal (loopback bleed survived echo suppression) — not in `speaker_suspicion.rs` today, add it there.
- Instead of (or in addition to) the immediate auto-correct, suspicious chunks should also enqueue onto the Tier-2 classifier queue built in Slice 4, so a confirmed `interviewer|candidate` verdict can override the plain heuristic guess. Wire this hookup in `audio/pipeline.rs` at the existing `speaker_suspicion::evaluate` call site.
- Extend `label_source` values so a chunk auto-corrected by heuristic but not yet classifier-confirmed is distinguishable from one the classifier has confirmed (ties into Slice 5's event).

**Tests:** table-driven unit test for the new near-duplicate signal; pipeline test asserting a suspicious chunk is enqueued for classification.

**Commit:** `lpav slice 3: route suspicious chunks to Tier-2 classifier queue`

---

### Slice 4: `lpav-s4-speaker-classifier` (COMPLEX)

**Goal:** Async LLM classifier queue (Groq fast tier); phone ≥8 words always; non-phone suspicious only.

**Implement:**
- `SpeakerClassifier` with bounded mpsc, rate limit, drop-on-backpressure
- Prompt in `/prompts/speaker_classification/` (gpt/claude/llama variants)
- Returns `interviewer | candidate | uncertain`

**Tests:** mock LLM provider; queue backpressure drops without panic.

**Commit:** `lpav slice 4: async LLM speaker classifier`

---

### Slice 5: `lpav-s5-speaker-refined-events` (MEDIUM)

**Goal:** `speaker_refined { chunk_id, speaker, source }` event; persist relabel source in SQLite if column missing add migration.

**Files:** `events.rs`, `persistence.rs`, migration, `commands.rs` relabel already exists — wire source.

**Tests:** persistence round-trip; event payload serde test.

**Commit:** `lpav slice 5: speaker_refined event and label_source persistence`

---

### Slice 6: `lpav-s6-transcript-relabel-ui` (MEDIUM)

**Goal:** Per-line swap button in TranscriptPanel → `relabel_transcript_chunk`; user source overrides classifier.

**Note:** Existing `SpeakerPicker.tsx` is diarization bulk picker — keep for diarization banner; add inline swap per line.

**Tests:** TranscriptPanel vitest — swap invokes relabel command.

**Commit:** `lpav slice 6: per-line transcript speaker relabel UI`

---

### Slice 7: `lpav-s7-q-interviewer-span` (MEDIUM)

**Goal:** Fix the *global* Q button's targeting; leave the per-line Q chip alone.

**There are two independent Q mechanisms today — do not conflate them:**
1. `TranscriptPanel.tsx` per-line **Q chip** — already targets the correct merged System utterance (fixed in `qa-fix-summary-transcript` / PR #37). **No change needed here.**
2. `LiveSessionStatusBar.tsx` global **Q button** — still uses a blind `ROLLING_WINDOW_MS = 30_000` window over raw System chunks (`onSystemChunk` accumulator), independent of any relabeling. **This is what this slice fixes.**

**Implement:**
- Replace the 30s rolling-window accumulation in `LiveSessionStatusBar.tsx` with a query against the backend's last Interviewer-labeled span (respecting manual relabels and classifier verdicts from Slices 2–6) — either a new lightweight Rust query command, or reuse the buffer `signalQuestionEnded` already drains.
- Exclude any chunk the user manually relabeled to `You` from the span.
- Phone mode: if the current span's speaker confidence is still uncertain (no classifier/manual confirmation), keep today's raw-30s fallback and show a small "(uncertain speaker)" hint — do not block the button.

**Files:** `LiveSessionStatusBar.tsx` (+ `LiveSessionStatusBar.test.tsx`), Rust query/trigger path in `commands.rs` if a new command is needed.

**Tests:** vitest — span excludes relabeled-to-You chunks, uncertain-speaker hint renders in phone mode; Rust unit for buffer last-interviewer text.

**Commit:** `lpav slice 7: global Q button targets last interviewer span, not raw 30s window`

---

### Slice 8: `lpav-s8-live-preview-state` (COMPLEX)

**Goal:** Add `LivePreview` to `SessionState`; transitions: `READY ↔ LIVE_PREVIEW`, `LIVE_PREVIEW → LIVE`, `LIVE_PREVIEW → READY`.

**Files:** `session/state.rs` — update allow-list + tests for all valid/invalid transitions.

**Tests:** 100% state machine coverage for new transitions.

**Commit:** `lpav slice 8: LIVE_PREVIEW session state and transitions`

---

### Slice 9: `lpav-s9-live-preview-commands` (COMPLEX)

**Goal:** `start_live_preview`, `commit_live_preview`, `cancel_live_preview`.

**Behavior:**
- `start_live_preview`: audio pipeline only, NO orchestrator spawn, 60s timeout auto-cancel
- `commit_live_preview`: transition to LIVE, spawn orchestrator without re-init capture if possible
- `cancel_live_preview`: stop pipeline, READY

**Tests:** integration test with mock audio; state transitions verified.

**Commit:** `lpav slice 9: live preview Tauri commands`

---

### Slice 10: `lpav-s10-live-preview-screen` (MEDIUM)

**Goal:** `LivePreview.tsx` — 60s countdown, TranscriptPanel, Go Live / Back CTAs; wire Rehearsal Complete → LivePreview.

**Files:** `App.tsx`, `Rehearsal.tsx`, commands TS wrappers.

**Tests:** Vitest render + button invokes commands.

**Commit:** `lpav slice 10: LivePreview screen and rehearsal routing`

---

### Slice 11: `lpav-s11-mock-sample-audio` (SIMPLE)

**Goal:** MockInterview toggle + copy for "practice with sample call audio" (instructions only).

**Commit:** `lpav slice 11: mock sample-audio practice guidance`

---

### Slice 12: `lpav-s12-first-run-live-modal` (MEDIUM)

**Goal:** `FirstRunLiveModal.tsx` + `LiveHelpDrawer.tsx`; localStorage `flint_first_run_live_dismissed`; wire LiveOverlay.

**Tests:** Vitest modal sections + dismiss flag.

**Commit:** `lpav slice 12: first-run live modal and help drawer`

---

### Slice 13: `lpav-s13-ask-now-phone-toggle` (SIMPLE)

**Goal:** Rename Q → "Ask now" + keycap; phoneCallMode on SessionDesign.tsx (keep Settings override).

**Commit:** `lpav slice 13: Ask now label and phone mode on session design`

---

### Slice 14: `lpav-s14-ctrlq-cleanup` (SIMPLE)

**Goal:** Remove duplicate Ctrl+Q listener from LiveSessionStatusBar; useHotkeys only.

**Tests:** vitest/hotkeys — single handler path.

**Commit:** `lpav slice 14: unify Ctrl+Q on useHotkeys only`

---

### Slice 15: `lpav-s15-part-a-rust-tests` (MEDIUM)

**Goal:** Fill any Part A Rust test gaps; `cargo test` green for audio/speaker/state modules.

**Commit:** `lpav slice 15: Part A Rust test coverage`

---

### Slice 16: `lpav-s16-part-a-vitest` (SIMPLE)

**Goal:** Part A frontend tests green.

**Gate:**

```bash
cd src-tauri && cargo test && cargo clippy -- -D warnings
cd .. && npm run test
```

**Commit:** `lpav slice 16: Part A frontend test pass`

---

## Part B — Answer + Visual Engine (slices 17–33)

### Slice 17: `lpav-s17-prompts-answer-visual` (MEDIUM)

**Goal:** Create `/prompts/answer/` and `/prompts/visual/` with gpt/claude/llama.txt.

**Answer prompt:** conclusion-first, brief reasoning, one follow-up line appended.  
**Visual prompt:** fenced mermaid or code only; diagram types for system design.

**Scope note — domain hints hardcoded, not dynamic:** the source plan ties the Visual prompt to a `SessionDomain::preferred_diagram_types()` method. That trait is out of scope for this milestone (no domain-plugin architecture here). Hardcode the interview-relevant diagram-type hints (flowchart, sequenceDiagram, classDiagram, erDiagram for system design / API design questions) directly into the `/prompts/visual/` templates instead. This is an intentional scope reduction — leave a `// TODO(domain-plugin): swap for SessionDomain::preferred_diagram_types() when that trait lands` comment, not a silent omission.

**Tests:** prompt loader unit tests; files exist on disk.

**Commit:** `lpav slice 17: answer and visual prompt artifacts`

---

### Slice 18: `lpav-s18-answer-thread` (COMPLEX)

**Goal:** `orchestrator/answer.rs` — merges directional + clarifying behavior; streams tokens.

**File handling:** `git mv src-tauri/src/orchestrator/directional.rs src-tauri/src/orchestrator/answer.rs` and adapt in place — do not leave `directional.rs` behind as a second copy. Update `pub mod directional;` → `pub mod answer;` in `orchestrator/mod.rs` in this same slice.

**Retire clarifying spawn from orchestrator (slice 21 wires removal); `orchestrator/clarifying.rs` itself is deleted in Slice 28, not here — do not delete it prematurely since Slice 21 still needs the old spawn removed cleanly first.**

**Tests:** mock provider streaming; conciseness heuristic.

**Commit:** `lpav slice 18: Answer thread module (renamed from directional.rs)`

---

### Slice 19: `lpav-s19-visual-thread` (COMPLEX)

**Goal:** `orchestrator/visual.rs` — repurposes depth streaming; buffers until closing fence.

**File handling:** `git mv src-tauri/src/orchestrator/depth.rs src-tauri/src/orchestrator/visual.rs` and adapt in place — same rule as Slice 18. Update `pub mod depth;` → `pub mod visual;` in `orchestrator/mod.rs`.

**Tests:** fence detection unit; mock stream defers emit until complete block.

**Commit:** `lpav slice 19: Visual thread with fenced output buffer (renamed from depth.rs)`

---

### Slice 20: `lpav-s20-visual-classifier` (COMPLEX)

**Goal:** Cheap classifier (regex + optional tiny LLM) — `needs_visual: bool`; manual trigger command `trigger_visual_response`.

**Prompt:** `/prompts/visual_classifier/` if LLM path used.

**Tests:** table-driven classifier cases (system design, algorithm, whiteboard).

**Commit:** `lpav slice 20: visual need classifier and manual trigger`

---

### Slice 21: `lpav-s21-orchestrator-two-thread` (COMPLEX)

**Goal:** `orchestrator/mod.rs` spawns Answer + Visual in parallel (Visual skipped when classifier false unless manual).

**Update:** `dispatch_turn`, silence debounce, thread_status events.

**Tests:** integration — both threads spawn; visual skipped on behavioral question.

**Commit:** `lpav slice 21: two-thread orchestrator dispatch`

---

### Slice 22: `lpav-s22-events-dtos` (MEDIUM)

**Goal:** Add `answer_token`, `visual_token` events; keep deprecated aliases `directional_token`/`depth_token` mapping to answer/visual for one release OR update all listeners in same PR.

**Preferred:** clean break — update all TS listeners in this milestone.

**Complete consumer list — every one of these references `directional`/`depth`/`clarifying` today and must be updated in this slice (verified by repo-wide grep, not just the obvious panel files):**

| File | What changes |
|------|--------------|
| `src-tauri/src/events.rs` | `emit_directional_token`→`emit_answer_token`, `emit_depth_token`→`emit_visual_token`; delete `emit_clarifying_question` |
| `src-tauri/src/dto.rs` | payload types renamed to match |
| `src-tauri/src/session/persistence.rs` | `ResponseType` enum: `Directional`→`Answer`, `Depth`→`Visual`, drop `Clarifying` (or map to `Answer` for historical rows — do not lose old data) |
| `src/events/index.ts` | `onDirectionalToken`→`onAnswerToken`, `onDepthToken`→`onVisualToken`; delete `onClarifyingQuestion` |
| `src/hooks/useOrchestratorStreams.ts` | central listener hub — update all five listener registrations |
| `src/hooks/useDirectionalStream.ts` | **dead code, zero importers today — delete this file**, do not migrate it |
| `src/hooks/useDepthStream.ts` | check for importers; if also dead, delete; if used, migrate like `useOrchestratorStreams.ts` |
| `src/components/LiveSessionStatusBar.tsx` (+ `.test.tsx`) | `onDirectionalToken` subscription and `thread === "directional"` phase-detection check both need the new event/thread names |
| `src/store/ui.ts` (+ `ui.test.ts`) | `appendDirectionalToken`/`appendDepthToken` → `appendAnswerToken`/`appendVisualToken`; `streamingBuffers.directional/.depth` → `.answer/.visual`; remove `clarifyingQuestions` state |
| `src/screens/SessionReview.tsx` (+ `.test.tsx`) | `directionalCount`/`depthCount`/`clarifyingCount` stat chips — find and update the Rust-side DTO these are sourced from (likely built in `persistence.rs` alongside the `ResponseType` change above) to `answerCount`/`visualCount` |
| `src/screens/Rehearsal.tsx` | full panel prop wiring (`directional={...}`, `depth={<DepthPanel/>}`, `clarifying={<ClarifyingPanel/>}`) and `streamingBuffers.directional/.depth` reads |
| `src/screens/LiveOverlay.tsx` | same panel prop wiring as Rehearsal |
| `src/components/OverlayLayout.tsx` (+ `.test.tsx`) | panel slot props — also touched by Slice 26, coordinate so this isn't done twice |

**Explicitly NOT renamed (leave as-is, different concept):** `src/screens/HealthCheck.tsx`'s `recommendedLlmConfig.directional`/`.depth` fields describe hardware-tier model recommendations, not orchestrator threads. Leave alone unless it causes a naming collision.

**Commit:** `lpav slice 22: answer and visual event contracts across all consumers`

---

### Slice 23: `lpav-s23-prewarm-confidence` (COMPLEX)

**Goal:** Pre-warm cache keys → answer/visual; confidence scoring uses answer text; **Q&A embedding gate reworked** (this does NOT stay unchanged — see below).

**Q&A embedding rework (real behavior change, not a rename):** `orchestrator/mod.rs` currently gates `ingest_qa` on `confidence_score >= QA_EMBED_CONFIDENCE_THRESHOLD` computed from the directional/depth response pair (`cached_directional`/`cached_depth`, `e.directional_response`/`e.depth_response` in the pre-warm cache hit path around the `is_plausible_cached_response` calls). Every one of these needs to read from the new `answer` response instead:
- `OrchestrationContext.cached_directional`/`cached_depth` → `cached_answer` (drop the visual cache key from this gate — visual output isn't embedded for Q&A recall)
- The pre-warm cache-hit branch that checks `is_plausible_cached_response(&e.directional_response)` / `&e.depth_response` → single check against `e.answer_response`
- Confidence computation feeding the `QA_EMBED_CONFIDENCE_THRESHOLD` comparison must be based on the Answer thread's output, not a directional/depth blend

**Tests:** cache hit serves answer; visual pre-prepared flag; `ingest_qa` fires on high-confidence Answer text (update existing test that asserted this against directional/depth).

**Commit:** `lpav slice 23: prewarm, confidence, and Q&A embedding gate reworked for answer visual model`

---

### Slice 24: `lpav-s24-answer-panel` (MEDIUM)

**Goal:** `AnswerPanel.tsx` replaces DirectionalPanel; shows answer stream + current question.

**Commit:** `lpav slice 24: AnswerPanel replaces Directional`

---

### Slice 25: `lpav-s25-visual-panel-mermaid` (COMPLEX)

**Goal:** `VisualPanel.tsx` replaces DepthPanel slot.

**Deps:** add `mermaid`, `shiki` (or lightweight highlighter).

**Tier 1:** render ```mermaid blocks as SVG; code fences highlighted; raw fallback on parse fail. **Must support all 10 diagram types the source plan requires, not just `flowchart`:** `flowchart`, `sequenceDiagram`, `classDiagram`, `erDiagram`, `stateDiagram`, `quadrantChart`, `timeline`, `mindmap`, `pie`, `xychart-beta`. Mermaid's own parser handles the type dispatch — verify with one test fixture per type, not just flowchart.  
**Tier 2b stub:** disabled "Refine diagram" button with tooltip.

**Tests:** Vitest — one render test per Mermaid diagram type listed above; invalid mermaid shows raw.

**Commit:** `lpav slice 25: VisualPanel with Mermaid and code rendering`

---

### Slice 26: `lpav-s26-four-panel-layout` (MEDIUM)

**Goal:** OverlayLayout — Transcript, Answer, Visual, Context (remove Clarifying slot).

**Update:** `PanelId`, default sizes, Zustand layout, LiveOverlay/Rehearsal wiring.

**Commit:** `lpav slice 26: four-panel overlay layout`

---

### Slice 27: `lpav-s27-ui-store-streams` (MEDIUM)

**Goal:** Zustand `streamingBuffers.answer` / `.visual`; hooks `useOrchestratorStreams`; remove directional/depth/clarifying buffers.

**Tests:** store unit tests for append/clear/startTurn.

**Commit:** `lpav slice 27: UI store and hooks for answer visual streams`

---

### Slice 28: `lpav-s28-retire-clarifying` (MEDIUM)

**Goal:** Delete the clarifying thread end to end — this is the slice that actually removes it (Slice 18/21 only stopped spawning it).

**Delete:**
- `src-tauri/src/orchestrator/clarifying.rs` (file removal, not just unused-import silence) and its `pub mod clarifying;` line in `orchestrator/mod.rs`
- `src/panels/ClarifyingPanel.tsx` (+ any `.test.tsx`)
- `emit_clarifying_question` from `events.rs` if not already removed in Slice 22 — cross-check, don't duplicate
- `prompts/clarifying/` directory (gpt/claude/llama.txt) — confirm Slice 29's eval harness update no longer references it before deleting, to avoid a broken eval fixture
- `clarifyingQuestions` state and `addClarifyingQuestion`/`clearClarifyingQuestions` actions from `store/ui.ts` if Slice 22 left them for this slice

**Verify no dangling references:** `rg -i clarifying src-tauri/src src/ prompts/` should return zero matches when this slice is done (aside from historical comments explaining the retirement, which are fine).

**Commit:** `lpav slice 28: delete clarifying thread, panel, and prompts`

---

### Slice 29: `lpav-s29-eval-harness-update` (COMPLEX)

**Goal:** Eval runner scores `answer` + `visual` threads; update gate thresholds; smoke 10 questions.

**Note:** If no API keys in CI, unit-test gate logic only; document manual eval in backlog.

**Commit:** `lpav slice 29: eval harness answer visual threads`

---

### Slice 30: `lpav-s30-orchestrator-integration-tests` (COMPLEX)

**Goal:** Update `tests/integration/orchestrator.rs` for 2-thread model.

**Commit:** `lpav slice 30: orchestrator integration tests for answer visual`

---

### Slice 31: `lpav-s31-panel-vitest` (SIMPLE)

**Goal:** AnswerPanel.test.tsx, VisualPanel.test.tsx, layout tests.

**Commit:** `lpav slice 31: Answer and Visual panel vitest`

---

### Slice 32: `lpav-s32-rules-update` (MEDIUM)

**Goal:** Update the tracked Cursor rule files — these ARE committed normally, unlike `docs/` (handled separately in Slice 33).

**`.cursor/rules/flint-core.mdc`:**
- Rule 4 ("Parallel Threads via tokio::spawn — Never Sequential") currently shows `run_directional`/`run_depth`/`run_clarifying` in its code example — update to `run_answer`/`run_visual`, two-thread join.
- Folder structure listing under `orchestrator/` — update `directional.rs, depth.rs, clarifying.rs` → `answer.rs, visual.rs`.
- `src/panels/` listing — `DirectionalPanel, DepthPanel, ClarifyingPanel` → `AnswerPanel, VisualPanel`.

**`.cursor/rules/flint-performance.mdc`:**
- NFR table: `Directional TTFT P95 | < 800ms | Fail PR > 900ms` → `Answer TTFT P95 | < 2s | Fail PR > 2.2s` (per the source plan's Answer thread target).
- `Depth response fully streamed P95 | < 8s | Warn` → replace with a Visual thread target (Visual only fires conditionally, so define a P95 for "Visual fully streamed when triggered" — carry the existing 8s figure forward unless the eval harness in Slice 29 says otherwise).
- `thread_type` field in the structured logging schema example: update `directional/depth/clarifying` → `answer/visual`.
- CI performance gates table: `P95 TTFT directional` row → `P95 TTFT answer`.
- User-facing performance alerts table: `TTFT > 1s directional` / `> 2s directional` rows → `answer`.

Both files are tracked — normal commit, no special handling needed.

**Commit:** `lpav slice 32: update flint-core and flint-performance rules for answer visual`

---

### Slice 33: `lpav-s33-manual-qa-checklist-and-local-docs` (SIMPLE)

**Create:** `tests/manual-qa/live-preview-answer-visual-checklist.md`

**Sections:** Live Preview flow, phone relabel, Ask now, Answer stream, Visual mermaid on system design question, manual visual trigger.

**Local docs pass — edit on disk, do NOT `git add docs/` (gitignored on purpose, per `flint-git-workflow.mdc`):**

- `docs/flint_system_design_v3.md`:
  - §4/§6/§7 architecture diagrams — replace directional/depth/clarifying thread references with answer/visual
  - §21 — update NFR targets to match the `flint-performance.mdc` changes from Slice 32
  - §25 — add `LIVE_PREVIEW` to the documented session state machine diagram/table (Slice 8's addition)
- `docs/ROADMAP.md`:
  - Mark the `phone-interview-and-live-preview` and `flint_domain-plugin_architecture` Workstream 2 items as shipped by this milestone's PR
  - Note Excalidraw Tier 2a and deeper-AI-repass Tier 2b as still open (deferred per this milestone's scope)

**Commit (tracked files only):** `lpav slice 33: manual QA checklist for combined milestone` — the docs/ edits above are never staged; verify with `git status` before committing that only the checklist file is added.

---

## Part C — Review + Ship (slices 34–37)

### Slice 34: `lpav-s34-consolidated-review` (COMPLEX — Sonnet 5 review only)

Review full `git diff origin/main...HEAD`:

- flint-core, security, performance, git-workflow rules
- No secrets, no Fisher files
- State machine transitions valid
- Parallel spawn preserved
- Prompts not inlined
- Tests meaningful (not trivial)

Output: blocking vs nit list.

---

### Slice 35: `lpav-s35-fix-review-findings` (MEDIUM)

Apply all **blocking** findings; Sonnet re-check once.

**Commit (if needed):** `lpav slice 35: address consolidated review findings`

---

### Slice 36: `lpav-s36-full-gates-rerun` (SIMPLE)

```bash
cd src-tauri && cargo fmt --check && cargo clippy -- -D warnings && cargo test
cd .. && npm run test
# If prompts changed:
cd evals && cargo run -- --smoke 2>/dev/null || true
```

All must pass before slice 37.

---

### Slice 37: `lpav-s37-pr-ci-merge` (SIMPLE — Composer 2.5 ONLY)

```bash
git push -u origin HEAD
gh pr create --title "Live preview + Answer/Visual engine" --body "$(cat <<'EOF'
## Summary
- Part A: LIVE_PREVIEW, 3-tier speaker ID, per-line relabel, live onboarding
- Part B: Answer + Visual threads, 4-panel layout, Mermaid/code VisualPanel

## Test plan
- [ ] cargo test + clippy
- [ ] npm run test
- [ ] Live Preview 60s dry run
- [ ] System design question → Visual panel Mermaid
- [ ] Phone mode relabel + Ask now

EOF
)"
gh pr checks --watch
# Fix CI with Composer only; max 6 attempts
gh pr merge
```

Set `milestone_status: ci_green`, `loop_stopped: true`.

---

## Stop conditions

- PR merged + CI green
- Same slice fails 3× → blocker
- CI fix loop > 6 on slice 37
- User `/flint-loop stop`

**Do NOT stop** between slices 0–36 for nits or user confirmation.

---

## Done when

1. PR merged, CI green
2. `git diff main --stat` summary
3. Manual checklist path committed
4. **"Ready for manual live test"** — Live Preview + Visual Mermaid on device

## Resume

```
/flint-loop live-preview-answer-visual start
/flint-loop resume
/flint-loop status
/flint-loop resume — lpav-s33 linux pass|fail (<notes>)
```
