# Visual Robustness + Manual Diagram Trigger Everywhere — Combined Milestone

> **Kickoff:** `/flint-loop visual-robustness-and-triggers start`
> **One branch, one PR, one merge** — Part A (streaming correctness) then Part B (manual trigger UX) sequentially; full local gates after Part B; then senior review → fix → re-test → Composer PR/CI/merge.

## Context

Root-caused during manual testing: a live session asked "draw a system design for a
notification microservice" and the returned Mermaid diagram had corrupted syntax
(missing arrows, a dropped `\n`, `Twilio` truncated to `ilio`) — mermaid.js then
rendered a hard parse-error instead of the diagram.

**Root cause (confirmed by code read, not guessed):** every LLM provider
(`groq.rs`, `openai_compat.rs` — OpenAI/DeepSeek, `openrouter.rs`, `anthropic.rs`,
`ollama.rs`) parses its SSE/NDJSON stream the same buggy way:

```rust
let line_stream = byte_stream
    .map(|chunk| chunk.context("... stream read error"))
    .flat_map(|chunk_result| {
        let lines: Vec<Result<String>> = match chunk_result {
            Ok(bytes) => String::from_utf8_lossy(&bytes).lines()...collect(),
            Err(e) => vec![Err(e)],
        };
        futures::stream::iter(lines)
    })
```

`response.bytes_stream()` yields raw network chunks — one chunk does **not**
guarantee one complete `data: {...}` line. When a line spans two chunks, `.lines()`
is called independently per chunk with no carried-over buffer, so:
- the first half of the split line is incomplete JSON → parse fails → **silently
  dropped**
- the second half no longer starts with `data: ` → also fails → **silently
  dropped**

That token (an arrow, a newline, a few characters of a word) just vanishes. This
corrupts Mermaid's strict grammar visibly; it can just as easily drop a word from
an Answer response, just less noticeably. This is a correctness bug in the core
LLM streaming layer, not a Mermaid limitation — **do not** respond by downgrading
Visual to plain text.

**Decision (confirmed with user, do not re-litigate):**
1. Fix the root byte-buffering bug across all 5 providers (benefits every thread).
2. Switch the Visual thread specifically to non-streaming (`stream: false`) —
   Visual already buffers internally and never shows partial tokens (it waits for
   a complete fenced block before emitting anything), so there is zero UX cost,
   and every provider already has a correct single-JSON-body decode path for
   `!config.stream` that completely bypasses the buggy line-splitter.
3. Add a client-side auto-retry-once on Mermaid render failure as a safety net for
   genuine model mistakes (independent of the network bug).
4. Do **not** build a Rust-side Mermaid validator/auto-repair — regenerating is
   more robust than trying to patch broken graph syntax.
5. Add the manual "Generate diagram" trigger to Rehearsal and Mock Interview (Live
   already has it, working, untouched by this milestone).

**Branch:** `feature/visual-robustness-and-triggers` (from latest `main`, folding in
two already-committed but unmerged fix branches — see Slice 0).

**Repos:** Flint only. Do not touch smart-resume / flint-extension.

### Hard guardrails

- Prompts in `/prompts/` only — never inline LLM prompt string literals in Rust.
- A prompt change (Slice 6) REQUIRES an eval harness smoke run before merging
  (`flint-performance.mdc`).
- React is a dumb renderer — session state + thread orchestration stays in Rust.
- Parallel threads via `tokio::spawn` — Answer + Visual never sequential.
- API keys in keychain; no session content in INFO logs.
- **NEVER** commit Fisher scripts/json, secrets, or staged `docs/`.
- Minimal diff — no drive-by refactors beyond what each slice specifies.

### Leftover-work policy (Slice 0 — read before touching git)

Two bug-fix branches exist from earlier in this session, committed but never
opened as PRs. **Fold both into this milestone's branch** instead of leaving them
as stray unmerged branches:

| Branch | Commit | What it fixes |
|--------|--------|----------------|
| `fix/whisper-health-check-install-copy` | `6e78602` | Whisper health-check Fix copy now gives install-script + curl steps |
| `fix/mic-calibration-text-diarizer-crash` | `2de2e5f` | Generic mic-test paragraph (was hardcoded SecureAuth fixture text) + lazy-load speakrs ONNX (was crashing on eager load after model download) |

There is also one untracked file at repo root worth keeping —
`scripts/run-dev-stack.sh` (generic dev tooling: starts Smart Resume + extension +
Flint together; not Fisher-specific, not a loop/cursor file). Add and commit it in
Slice 0.

**Do NOT add to git, ever, in this milestone** (leave untouched/untracked):
- `.cursor/skills/flint-loop-engineering/tasks/post-v1-ship-prep.md`
- `.cursor/skills/flint-loop-engineering/tasks/v1-closure-phase1.md`
- `scripts/apply_bank_aliases.py`, `scripts/dedupe_question_attempts.py`,
  `scripts/import_fisher_phone_screen.py`, `scripts/import_preferred_answers.py`,
  `scripts/recover_fisher_session.py`, `scripts/setup_prep_order.py`
- `tests/manual-qa/fisher-iam-*.json` (all 5 files)
- Any gitignored path (`docs/`, etc.)

These are personal-use-case artifacts (a real "Fisher" interview prep case) and
internal loop-engineering scratch files — not project features.

### Explicitly OUT OF SCOPE

- Rust-side Mermaid validator/auto-repair (see decision #4 above)
- Excalidraw Tier 2a / Visual Tier 2b deeper AI re-pass (already stubbed, untouched)
- Domain-plugin trait / Meeting runtime
- macOS/Windows manual QA (park in `manual_gate_backlog`)
- Answer thread's own non-streaming switch — Answer legitimately streams
  token-by-token to the UI today and must keep doing so; only Visual moves to
  non-streaming (decision #2 is Visual-specific, not a blanket provider change)

---

## Three-layer agent model (mandatory)

| Layer | Model slug | Role |
|-------|------------|------|
| **L1 — cheap** | `composer-2.5-fast` | SIMPLE slices, Vitest UI, prompt text edits, fmt/clippy/CI fixes, PR poll + merge |
| **L2 — medium** | `grok-4.5-fast-xhigh` | MEDIUM slices: event/prop wiring, panel UI, TS command wrappers |
| **L3 — complex** | `claude-sonnet-5-thinking-high` | COMPLEX: LLM streaming internals, orchestrator signature changes, new backend subsystem (Mock Visual), final consolidated review |

### Review ladder (every slice)

```
SIMPLE  → Composer 2.5 implements → Grok 4.5 reviews       → fix → commit → gates
MEDIUM  → Grok 4.5 implements     → Sonnet 5 reviews       → fix → commit → gates
COMPLEX → Sonnet 5 implements     → Sonnet 5 2nd-pass review → fix → commit → gates
```

**Final phase (last 4 slices):**

```
Sonnet 5 — full diff read-only review (blocking vs nit)
Grok 4.5 or Sonnet 5 — apply blocking fixes
Full gates — cargo test, clippy, vitest, eval smoke (prompts changed in Slice 6)
Composer 2.5 ONLY — push, gh pr create, poll checks, fix flakes, merge
```

**Parent agent:** launch Task subagents with the model slugs above; do not
substitute. **Reviewer rules:** read-only findings; max one review round per
slice; cite paths; reject Fisher/secrets/docs staged.

---

## Milestone structure

| Part | Slices | Deliverable |
|------|--------|-------------|
| **A — Streaming correctness** | 0–6 | Root SSE/NDJSON fix (5 providers), Visual non-streaming, client retry, prompt guardrails |
| **B — Manual trigger everywhere** | 7–10 | `force_visual` plumbing, Rehearsal button, Mock Interview Visual support |
| **C — Review + ship** | 11–14 | Senior review, fix, full gates, PR merged CI green |

**Do not stop between slices 0–10** unless a stop condition hits.

---

## Complexity tags (all slices)

| Slice | ID | Complexity | Implementer | Reviewer |
|-------|-----|------------|-------------|----------|
| 0 | `vrt-s0-branch-consolidation` | SIMPLE (parent agent, no subagent) | — | — |
| 1 | `vrt-s1-sse-line-buffering` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 2 | `vrt-s2-wire-buffered-lines-all-providers` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 3 | `vrt-s3-visual-non-streaming` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 4 | `vrt-s4-visual-panel-auto-retry` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 5 | `vrt-s5-visual-prompt-guardrails` | SIMPLE | Composer 2.5 | Grok 4.5 |
| 6 | `vrt-s6-part-a-gates` | SIMPLE | Composer 2.5 | Grok 4.5 |
| 7 | `vrt-s7-dispatch-turn-force-visual-param` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 8 | `vrt-s8-rehearsal-generate-diagram-button` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 9 | `vrt-s9-mock-interview-visual-support` | COMPLEX | Sonnet 5 | Sonnet 5 |
| 10 | `vrt-s10-part-b-gates` | SIMPLE | Composer 2.5 | Grok 4.5 |
| 11 | `vrt-s11-consolidated-review` | COMPLEX | — | Sonnet 5 |
| 12 | `vrt-s12-fix-review-findings` | MEDIUM | Grok 4.5 | Sonnet 5 |
| 13 | `vrt-s13-full-gates-rerun` | SIMPLE | Composer 2.5 | Grok 4.5 |
| 14 | `vrt-s14-pr-ci-merge` | SIMPLE | Composer 2.5 | Grok 4.5 |

---

## Kickoff — Slice 0

Working directory: `/home/alireza/Desktop/projects/Flint`

```bash
git fetch origin
git checkout main && git pull origin main
git checkout -b feature/visual-robustness-and-triggers
git cherry-pick 6e78602
git cherry-pick 2de2e5f
git add scripts/run-dev-stack.sh
git commit -m "chore: add run-dev-stack.sh (starts resume + extension + Flint together)"
git branch -D fix/whisper-health-check-install-copy fix/mic-calibration-text-diarizer-crash
git push origin --delete fix/whisper-health-check-install-copy 2>/dev/null || true
```

If either `cherry-pick` conflicts, resolve using the branch's own version (both
were authored fresh against `main` at roughly this point; conflicts should only
be adjacent-line noise, not logical clashes) — do not drop either fix's content.

Initialize `.cursor/flint-loop-state.json`:

```json
{
  "current_milestone": "visual-robustness-and-triggers",
  "milestone_status": "in_progress",
  "milestone_branch": { "flint": "feature/visual-robustness-and-triggers" },
  "current_slice": 0,
  "completed_slices": [],
  "current_task_id": "vrt-s0-branch-consolidation",
  "loop_stopped": false,
  "ci_fix_attempts": 0,
  "max_ci_fix_attempts": 6,
  "task_attempts": 0,
  "max_task_attempts": 3,
  "open_prs": {},
  "manual_gate_backlog": [
    {
      "gate": "Mermaid diagram device retest",
      "doc": "tests/manual-qa/visual-robustness-checklist.md",
      "notes": "After merge: repeat the original 'draw a system design for a notification microservice' prompt on Live; verify Rehearsal + Mock Interview Generate diagram buttons"
    }
  ],
  "agent_ladder": {
    "simple": "composer-2.5-fast",
    "medium": "grok-4.5-fast-xhigh",
    "complex": "claude-sonnet-5-thinking-high"
  },
  "notes": "Folds in whisper health-check copy fix + mic calibration/diarizer crash fix. Part A streaming correctness, Part B manual trigger everywhere."
}
```

**Gate:** `git status` clean except Fisher/loop-task untracked files listed above
(those are expected and must stay untouched).

---

## Part A — Streaming correctness (slices 1–6)

### Slice 1: `vrt-s1-sse-line-buffering` (COMPLEX)

**Goal:** New shared module that buffers partial lines across network chunk
boundaries so no SSE/NDJSON line is ever silently dropped.

**Create:** `src-tauri/src/llm/sse_lines.rs`

Implement a stream adapter (e.g. via `futures::stream::unfold`) with this shape:

```rust
/// Buffers a byte stream and yields complete newline-terminated lines,
/// carrying partial lines over chunk boundaries so a `data: {...}` line split
/// across two network reads is never silently dropped (see design doc for the
/// bug this fixes — every provider's naive per-chunk `.lines()` call drops
/// half of any line that spans a chunk boundary).
pub fn buffered_lines<S>(byte_stream: S) -> impl Stream<Item = anyhow::Result<String>>
where
    S: Stream<Item = anyhow::Result<bytes::Bytes>>,
```

Requirements:
- Buffer at the **byte** level (`Vec<u8>`), not `String`, so a multi-byte UTF-8
  sequence split across chunks is also handled correctly — only decode to UTF-8
  once a full line (delimited by `\n`) is assembled.
- Skip empty (whitespace-only) lines, matching existing provider behavior.
- On stream end, flush any remaining non-empty buffered bytes as a final line
  (handles a response with no trailing newline).
- Propagate byte-stream errors as `Err` immediately, do not swallow them.

**Tests (in this file):**
- A single `data: {...}` line split across two synthetic chunks reassembles
  into exactly one complete line.
- Multiple complete lines delivered in one chunk all yield correctly.
- Empty/whitespace-only lines are skipped.
- Stream ends without a trailing newline — the last partial line is still
  yielded.
- A byte-stream error is propagated, not dropped.
- A multi-byte UTF-8 character split exactly at the chunk boundary decodes
  correctly once reassembled.

**Commit:** `vrt slice 1: byte-buffering SSE/NDJSON line reader`

---

### Slice 2: `vrt-s2-wire-buffered-lines-all-providers` (COMPLEX)

**Goal:** Replace the buggy per-chunk `.lines()` split in all 5 streaming
providers with `sse_lines::buffered_lines`, keeping each provider's own
`parse_sse_line`/`parse_chunk` line-parser unchanged.

**Files (all in `src-tauri/src/llm/`):**
- `groq.rs` — replace the `.flat_map(...)` block (~line 241) with
  `buffered_lines(byte_stream.map(|c| c.context("Groq stream read error")))`,
  then keep the existing `.filter_map(...Self::parse_sse_line...)` stage.
- `openai_compat.rs` — same pattern (shared by `openai.rs` and `deepseek.rs`
  via `OpenAiCompatProvider`).
- `openrouter.rs` — same pattern.
- `anthropic.rs` — same pattern (note: `parse_sse_line` here treats an empty
  `data:` payload as `None`, keep that check after line reassembly).
- `ollama.rs` — same pattern; note Ollama's `parse_chunk` returns
  `Option<Result<String>>` (different shape from the SSE providers) —
  preserve the `.transpose()` call after swapping in `buffered_lines`.

**Do not change:** any provider's `parse_sse_line`/`parse_chunk` function
signature or logic, the non-streaming (`!config.stream`) branches, rate-limit
handling, or error message text. This slice is purely "how do we get a
complete line to the existing parser," nothing else.

**Tests:** for at least `groq.rs` and `openai_compat.rs`, add an integration-
style unit test that feeds a mocked byte stream with a `data: {...}` line
deliberately split across two chunk boundaries (mid-JSON) and asserts the
provider's token stream yields the correct, undropped content — this is the
regression test for the exact bug that produced the corrupted Mermaid diagram.

**Commit:** `vrt slice 2: wire buffered SSE line reader into all 5 providers`

---

### Slice 3: `vrt-s3-visual-non-streaming` (COMPLEX)

**Goal:** Visual thread requests `stream: false` — since Visual never emits
partial tokens to the UI anyway (it already buffers internally until a
complete fenced block exists), this routes every provider through its
already-correct single-JSON-body decode path, fully bypassing the SSE
line-splitting layer touched in Slices 1–2 for this thread specifically.

**File:** `src-tauri/src/orchestrator/visual.rs`

**Current behavior to replace:** `run_visual` builds `CompletionConfig {
stream: true, ... }`, then loops over `stream.next()` calling
`extract_complete_fence` on each accumulated chunk until a fence closes or a
15s-per-token / 60s-total timeout fires.

**New behavior:**
- Set `CompletionConfig { stream: false, max_tokens: Some(400), temperature: 0.0
  }`.
- `failover.complete_stream(...)` with `stream: false` still returns a
  `Stream`, but every provider's non-stream branch yields it as a single
  `futures::stream::once(...)` item containing the full response — so the
  loop collapses to one `.next().await` call (or use
  `LLMProvider::complete(...)` directly, which already does exactly this
  collection per `provider.rs`'s default trait method — prefer using
  `complete()` if `failover: Arc<FailoverManager>` exposes an equivalent, else
  keep manual single-await on `complete_stream`).
- Keep the existing 60s overall deadline as a request timeout (non-streaming
  calls can still hang on a slow/unresponsive provider).
- Keep the `ctx.turn_cancel` check **before** issuing the call (there's no
  mid-flight cancellation point once a non-streaming HTTP call is in flight,
  which is an accepted, explicitly-documented trade-off — Visual never showed
  partial output during cancellation anyway, so there's no UX regression).
- `extract_complete_fence` is called once on the single complete response
  instead of incrementally — keep the function (still needed for the raw
  fallback path when no fence exists) but simplify the calling loop
  accordingly.
- Update `log_visual_nfr_breach`/`log_visual_complete` call sites — `stream_ms`
  now measures full request latency (there was no meaningful distinction
  before either, since nothing was emitted until the fence closed).

**Tests:** update `visual.rs`'s existing tests for the new non-streaming
config; add a test confirming `run_visual` calls the provider with
`stream: false`; keep the "malformed/no fence → raw fallback" test working
against a single non-streaming mock response instead of a multi-chunk stream.

**Commit:** `vrt slice 3: Visual thread requests non-streaming completion`

---

### Slice 4: `vrt-s4-visual-panel-auto-retry` (MEDIUM)

**Goal:** Client-side safety net — if `mermaid.render()` still fails (a
genuine model mistake, not the network bug fixed in Slices 1–3), silently
retry generation once before showing the raw-text fallback.

**File:** `src/panels/VisualPanel.tsx`

**Implement:**
- On `renderFailed` becoming `true` for a given `block`, if
  `canTriggerManually` is true and this exact `text` has not already been
  auto-retried (track with a ref keyed by the text/question, not state, to
  avoid re-render loops), call `triggerVisualResponse(currentQuestion,
  sessionId)` once automatically.
- Do not auto-retry a second time for the same question — after one retry,
  if it fails again, show the raw fallback as today.
- Do not auto-retry when `sessionId` is absent (Rehearsal before Slice 8,
  or any read-only render) — only where a manual trigger is already possible.
- Surface no additional UI during the silent retry beyond the existing
  "Generating visual response…" state.

**Tests:** `VisualPanel.test.tsx` — render failure once then a valid block on
retry ends with the diagram shown, not the raw fallback; render failure twice
in a row still falls back to raw text without an infinite retry loop.

**Commit:** `vrt slice 4: auto-retry once on Mermaid render failure`

---

### Slice 5: `vrt-s5-visual-prompt-guardrails` (SIMPLE)

**Goal:** Reduce genuine model-side Mermaid mistakes (independent of the
network bug) by tightening the Visual prompt's syntax instructions.

**Files:** `prompts/visual/default.txt`, `claude.txt`, `gpt.txt`, `llama.txt`,
`deepseek.txt`, `openai.txt` (all variants — keep them in sync).

**Add** a short explicit syntax checklist, e.g.:
- Every node must be declared with an id and label before it is referenced by
  an arrow later in the same diagram.
- Exactly one statement (node declaration or edge) per line — never combine
  two statements on one line.
- Every edge is `A --> B` or `A -->|label| B` — never omit the arrow.
- Do not truncate labels mid-word.

**Required per `flint-performance.mdc`:** run the eval harness after this
change, before committing:

```bash
cd evals && cargo run -- --smoke 2>/dev/null || true
```

If no API keys are configured in this environment, note in the commit message
that the smoke run was skipped for that reason and must run in CI (the
`prompt regression gate` workflow already covers this on the PR).

**Commit:** `vrt slice 5: tighten Visual prompt Mermaid syntax guardrails`

---

### Slice 6: `vrt-s6-part-a-gates` (SIMPLE)

```bash
cd src-tauri && cargo fmt --check && cargo clippy -- -D warnings && cargo test
cd .. && npm run test
```

All must pass before Part B starts.

---

## Part B — Manual trigger everywhere (slices 7–10)

### Slice 7: `vrt-s7-dispatch-turn-force-visual-param` (MEDIUM)

**Goal:** `dispatch_turn` and `run_rehearsal_turn` already run the same
classifier logic as Live (Visual already auto-fires in Rehearsal for
system-design questions) — the only gap is the **manual override**. Today
`dispatch_turn`'s Rehearsal call site hardcodes `force_visual: false` with a
comment saying manual trigger is "LIVE-only." Remove that restriction at the
plumbing level (UI wiring for Rehearsal itself is Slice 8).

**Files:**
- `src-tauri/src/orchestrator/mod.rs` — add a `force_visual: bool` parameter
  to `dispatch_turn`'s signature (currently it hardcodes the value inside the
  function body for the Rehearsal caller); thread it through to
  `OrchestratorTurnConfig`.
- `src-tauri/src/commands.rs` — `run_rehearsal_turn` gains an
  `force_visual: Option<bool>` parameter (default `false` when omitted, so
  every existing call site/test keeps working unchanged), passed through to
  `dispatch_turn`.
- `src/commands/index.ts` — `runRehearsalTurn` TS wrapper gains an optional
  4th parameter `forceVisual?: boolean`.

**Tests:** Rust unit/integration test confirming `dispatch_turn` with
`force_visual: true` spawns the Visual thread even for a question the
classifier would judge purely verbal (mirror the existing
`trigger_visual_response` integration test's assertion style in
`tests/integration/orchestrator.rs`).

**Commit:** `vrt slice 7: force_visual parameter through dispatch_turn and run_rehearsal_turn`

---

### Slice 8: `vrt-s8-rehearsal-generate-diagram-button` (MEDIUM)

**Goal:** Make the existing "Generate diagram" button in `VisualPanel`
actually work from Rehearsal, not just Live.

**Problem today:** `VisualPanel`'s `handleGenerateDiagram` hardcodes a call to
`triggerVisualResponse(currentQuestion, sessionId)` — a Tauri command gated to
`SessionState::Live` only (it reaches into `state.live_tasks`, which does not
exist during Rehearsal). Rehearsal has no `live_tasks`; it dispatches turns
through the separate `run_rehearsal_turn` command instead.

**Implement:**
- Change `VisualPanel`'s manual-trigger action from a hardcoded
  `triggerVisualResponse` call to an injected callback prop, e.g.
  `onGenerateDiagram?: (question: string) => Promise<void>`. Keep
  `canTriggerManually`'s existing gating (`sessionId` present, not generating,
  a current question exists) but drive the actual dispatch through the prop.
- `src/screens/LiveOverlay.tsx` — pass a callback that calls
  `triggerVisualResponse(question, sessionId)` (today's behavior, unchanged).
- `src/screens/Rehearsal.tsx` — pass `sessionId` into `VisualPanel` (currently
  omitted, which is why the button never renders there today) and a callback
  that calls `runRehearsalTurn(sessionId, question, undefined, true)` (the
  `force_visual` param added in Slice 7) — this re-runs the turn with the
  Visual thread forced on, consistent with Rehearsal's existing "Rephrase"
  pattern of re-asking rather than a side-channel diagram-only request.
- Rehearsal's `asking` state should reflect the diagram regeneration
  (`isGenerating` on `VisualPanel`) the same way it does for a normal ask.

**Tests:**
- `VisualPanel.test.tsx` — clicking Generate diagram calls the injected
  `onGenerateDiagram` prop with the current question, not a hardcoded command.
- `Rehearsal.test.tsx` (or equivalent) — Generate diagram button renders and
  invokes `runRehearsalTurn` with `forceVisual: true`.
- Manual QA checklist update (folded into Slice 13's checklist file, not a
  separate commit here).

**Commit:** `vrt slice 8: Generate diagram works from Rehearsal via injected trigger callback`

---

### Slice 9: `vrt-s9-mock-interview-visual-support` (COMPLEX)

**Goal:** Mock Interview has no Visual capability at all today — it is a
fully separate subsystem from Live/Rehearsal's Answer+Visual orchestrator.

**Read first, before writing any code:** Mock Interview's Q&A loop
(`askMockQuestion`, `advanceMockTurn`, `regradeMockTurn`,
`onMockSuggestedToken`, `onMockCoachFeedback` in `src/screens/MockInterview.tsx`
and their Rust counterparts in `commands.rs` around the `mock_tasks`/
`MockTaskHandles` state and `emit_mock_suggested_token`) does **not** go
through `dispatch_turn`/`OrchestratorTurnConfig`/the Answer+Visual threads at
all — it has its own bespoke suggested-answer generation and its own turn
phase state machine (`idle/answering/listening/paused/reviewing`). This is a
new, small subsystem to add, not a wiring exercise like Slice 8.

**Design constraints:**
- Keep it minimal: a manual "Generate diagram" action for the *current* mock
  question, reusing `visual::run_visual`'s prompt-building and Mermaid/code
  fence contract — do not fork the Visual prompt or invent a second diagram
  format.
- Do not change Mock Interview's core conductor state machine, TTS question
  flow, or coach-feedback grading — this is additive only.
- New Tauri command, e.g. `trigger_mock_visual_response(session_id, question)`
  — valid only from `SessionState::MockInterview`, mirrors
  `trigger_visual_response`'s validation style but sources context from the
  mock session's own digest/RAG state rather than `live_tasks`.
- Reuse the existing `visual_token`/`emit_visual_token` event and
  `VisualPanel` component for rendering — do not build a second diagram
  renderer. `VisualPanel` already accepts an `onGenerateDiagram` callback
  prop (from Slice 8) and a `sessionId`; wire Mock Interview's callback to the
  new command.
- UI: `MockInterview.tsx` has no four-panel `OverlayLayout` — add `VisualPanel`
  as a collapsible/toggleable panel near `SuggestedAnswerPanel` (e.g. below it
  or in a side drawer), gated the same way `showSuggested`/coach panel
  visibility is today. Keep the existing single-column layout for everything
  else unchanged.
- If, after reading the mock conductor code, wiring this turns out to require
  changes to the core turn phase state machine (not just additive plumbing),
  stop and flag it as a blocker in loop state with a clear note — do not
  silently expand scope to rework the conductor.

**Tests:**
- Rust: `trigger_mock_visual_response` validation test (rejects outside
  `MockInterview` state, mirrors `trigger_visual_response`'s existing test
  style).
- Vitest: `MockInterview.test.tsx` (or equivalent) — Generate diagram button
  appears once a question is active, invokes the new command, renders
  `VisualPanel` output on `visual_token`.

**Commit:** `vrt slice 9: Visual/diagram support for Mock Interview`

---

### Slice 10: `vrt-s10-part-b-gates` (SIMPLE)

```bash
cd src-tauri && cargo fmt --check && cargo clippy -- -D warnings && cargo test
cd .. && npm run test
```

Also create `tests/manual-qa/visual-robustness-checklist.md` covering:
- Re-run the exact "draw a system design for a notification microservice"
  prompt on Live — diagram renders without a Mermaid syntax error.
- Rehearsal: ask a behavioral question (Visual correctly skipped), click
  Generate diagram, diagram renders.
- Mock Interview: same manual trigger, diagram renders without disrupting the
  TTS question flow or coach feedback.
- Repeat the original bug scenario 3–5 times across a session to confirm the
  streaming fix holds under longer responses (the bug was more likely on
  large/long diagrams, which cross more chunk boundaries).

**Commit:** `vrt slice 10: manual QA checklist for visual robustness and triggers`

---

## Part C — Review + ship (slices 11–14)

### Slice 11: `vrt-s11-consolidated-review` (COMPLEX — Sonnet 5 review only)

Review full `git diff origin/main...HEAD`:

- `sse_lines::buffered_lines` is actually wired into all 5 providers, no
  provider still has the old per-chunk `.lines()` call
- Visual thread's non-streaming switch didn't silently change Answer's
  streaming behavior
- `force_visual` plumbing doesn't leak into Live's existing
  `trigger_visual_response` path or change its behavior
- Mock Interview additions are additive only — conductor state machine
  untouched
- No secrets, no Fisher files, no stray loop-task `.md` files staged
- Prompts not inlined; eval smoke ran for the Slice 5 prompt change
- Tests meaningful (not trivial), especially the chunk-split regression test
  from Slice 2

Output: blocking vs nit list.

---

### Slice 12: `vrt-s12-fix-review-findings` (MEDIUM)

Apply all **blocking** findings; Sonnet re-check once.

**Commit (if needed):** `vrt slice 12: address consolidated review findings`

---

### Slice 13: `vrt-s13-full-gates-rerun` (SIMPLE)

```bash
cd src-tauri && cargo fmt --check && cargo clippy -- -D warnings && cargo test
cd .. && npm run test
cd evals && cargo run -- --smoke 2>/dev/null || true
```

All must pass before Slice 14.

---

### Slice 14: `vrt-s14-pr-ci-merge` (SIMPLE — Composer 2.5 ONLY)

```bash
git push -u origin HEAD
gh pr create --title "Visual streaming robustness + manual diagram trigger everywhere" --body "$(cat <<'EOF'
## Summary
- Root-caused and fixed a byte-buffering bug in all 5 LLM providers (Groq,
  OpenAI/DeepSeek, OpenRouter, Anthropic, Ollama) that silently dropped
  content whenever an SSE/NDJSON line spanned a network chunk boundary —
  this was corrupting Mermaid diagrams (missing arrows/newlines/characters)
  and could just as easily drop words from Answer responses.
- Visual thread now requests non-streaming completions (no UX cost — it
  never showed partial tokens anyway), fully bypassing the line-splitting
  layer for this thread.
- Added a client-side auto-retry-once on Mermaid render failure as a safety
  net for genuine model mistakes independent of the network fix.
- Tightened the Visual prompt with explicit Mermaid syntax guardrails.
- Manual "Generate diagram" trigger now works from Rehearsal and Mock
  Interview, not just Live.
- Folded in two earlier unmerged fixes: Whisper health-check install-step
  copy, and generic mic-calibration text + lazy-loaded speaker-diarization
  models (was crashing on eager ONNX load after model download).

## Test plan
- [x] cargo test + clippy + fmt
- [x] npm run test
- [x] eval smoke (Visual prompt change)
- [ ] Manual: repeat original notification-microservice diagram prompt on Live
- [ ] Manual: Rehearsal Generate diagram button
- [ ] Manual: Mock Interview Generate diagram button
- [ ] Manual: Whisper health-check Fix copy, mic calibration generic text, phone-mode speaker download

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
- Same slice fails 3x → blocker
- CI fix loop > 6 on slice 14
- Slice 9 (Mock Interview) discovers it requires a core conductor rework →
  blocker, park remainder of Slice 9 in `manual_gate_backlog`, continue with
  Slices 10–14 covering only Rehearsal's trigger (do not let one slice's
  scope creep block the whole milestone)
- User `/flint-loop stop`

**Do NOT stop** between slices 0–13 for nits or user confirmation.

---

## Done when

1. PR merged, CI green
2. `git diff main --stat` summary
3. Manual checklist path committed
4. **"Ready for manual live test"** — repeat the original corrupted-diagram
   prompt on Live, confirm Rehearsal + Mock Interview triggers

## Resume

```
/flint-loop visual-robustness-and-triggers start
/flint-loop resume
/flint-loop status
/flint-loop resume — vrt-s10 linux pass|fail (<notes>)
```
