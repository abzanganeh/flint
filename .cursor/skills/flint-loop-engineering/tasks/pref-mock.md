# Loop slice: `pref-mock` — Preferred answers in Mock Interview

**Milestone:** Rehearsal + Mock alignment (pre-M7 polish)  
**Branch:** `feature/rehearsal-mock-interview-fixes` (continue; do not split unless user asks)  
**Base:** `main` @ latest; includes uncommitted Rehearsal preferred-answer work from prior session  
**Roadmap relation:** Prerequisite for M7 Mock polish; **not** a replacement for M7-M2/M3/M4 tonight

---

## Product rule (middle ground)

| Question type | Mock suggested reference |
|---------------|--------------------------|
| Exact/normalized match (`normalize_question_key`) | **Use `preferred_answer` from DB — no LLM** |
| Same intent, different wording | **Deferred v2** — do not implement semantic nearest-match in this loop |
| AI follow-up / unseen question | **Generate** from RAG + `mock_suggested` prompt (current behavior) |
| No preferred saved | **Generate** (current behavior) |

Coach compares **user spoken answer** vs whatever reference was used (preferred or generated).

---

## Context (already shipped on branch — verify, do not redo)

- Schema v12: `question_attempts.preferred_answer`
- Commands: `get_preferred_answer`, `save_preferred_answer`
- Rehearsal UI: `PreferredAnswerPanel`, question bank **Live** badge
- Live + Rehearsal orchestrator: preferred hit in `orchestrator/mod.rs` → instant directional + depth
- Mock **does not** use preferred yet — `mock/conductor.rs` → `run_suggested_answer` always calls LLM

---

## Task checklist

Run in order. Mark `[x]` in state `completed_tasks` when done.

### pref-mock-0 — Branch hygiene

- [ ] Confirm branch `feature/rehearsal-mock-interview-fixes`
- [ ] Ensure `PreferredAnswerPanel.tsx` is tracked (was untracked)
- [ ] Run `cargo test` + `npm run test` — baseline green before Mock changes
- [ ] **Do not commit** unless user explicitly asks

### pref-mock-1 — Conductor preferred short-circuit

**File:** `src-tauri/src/mock/conductor.rs`

- [ ] Pass `Arc<SessionPersistence>` into `run_suggested_answer` (or load preferred in caller before spawn)
- [ ] Before LLM: `persistence.get_preferred_answer(session_id, question)`
- [ ] If non-empty:
  - Return preferred text immediately
  - Stream tokens same as cache path (`emit_mock_suggested_token` word-by-word) respecting `MockMode`:
    - **Study:** stream during turn (existing behavior for LLM tokens)
    - **Practice:** buffer only — reveal after user answers (existing behavior; do not leak early)
  - Log `event = "mock_preferred_answer_hit"`
  - Skip `complete_stream` call entirely
- [ ] If empty: existing LLM path unchanged
- [ ] AI follow-ups from `generate_follow_up` — **always LLM** (no preferred lookup)

### pref-mock-2 — Coach reference consistency

**Files:** `src-tauri/src/mock/coach.rs`, `commands.rs` (end_mock_turn path if separate)

- [ ] Coach must receive the **same** suggested text the user sees (preferred or generated)
- [ ] Verify `update_mock_turn_user_answer` stores preferred text in `suggested_text` column when preferred hit
- [ ] No change to coach prompt required if it already compares user vs suggested — verify only

### pref-mock-3 — Frontend indicator

**Files:** `src/screens/MockInterview.tsx`, `src/panels/SuggestedAnswerPanel.tsx`, `src/components/rehearsal-enrichment.css` (or mock-specific css)

- [ ] When suggested answer came from preferred (new event or payload flag), show subtle label:
  - **"Using your saved Live script"**
- [ ] Option A (preferred): extend `mock_question_started` payload with `preferred_hit: bool`
- [ ] Option B: infer client-side via `getPreferredAnswer` — avoid extra IPC if event is cleaner
- [ ] Do not show label for LLM-generated suggestions

### pref-mock-4 — Tests

- [ ] Unit test: `run_suggested_answer` or extracted helper returns preferred without calling failover when DB has answer
  - Mock `SessionPersistence` or test via pure function `resolve_mock_suggested_reference(question, preferred_from_db) -> enum`
- [ ] Unit test: empty preferred → still calls LLM path (existing mock tests may cover; add if missing)
- [ ] Update `OrchestrationContext` test fixtures if touched (already have `from_preferred` from rehearsal work)
- [ ] Run: `cd src-tauri && cargo test mock` and full `cargo test`

### pref-mock-5 — Manual QA doc

**File:** `tests/manual-qa/PREF_MOCK_PREFERRED.md` (new)

- [ ] Steps:
  1. Rehearsal → ask "Tell me about yourself" → save preferred answer
  2. Mock Interview → same question (must be unsatisfied in bank or force-add to digest list)
  3. Assert suggested answer **matches** saved preferred (not a new LLM paragraph)
  4. Assert UI shows "Using your saved Live script"
  5. AI follow-up turn → assert **new** LLM suggestion (no preferred)
  6. Live → same question → still preferred (regression)
- [ ] Park manual execution for user — loop stops after doc is written

### pref-settings-1 — Move Sign out from Privacy tab

**Problem:** Sign out lives under **Settings → Privacy** but it is account/session auth, not a data-rights action. Privacy should stay export + delete only.

**File:** `src/screens/Settings.tsx`

- [x] Add **Account** tab (`account`) to settings tab list (order: Account | API Keys | Usage Cap | Privacy) — **first tab, default on open**
- [x] New `AccountTab` component with Sign out section (move handler + copy from `PrivacyTab`)
- [x] Remove Sign out section from `PrivacyTab` — keep Export + Delete account only
- [x] Privacy heading stays **Your data** (export/delete); Account heading **Account**
- [x] Update manual QA path references:
  - `tests/manual-qa/M3_GMAIL_SSO.md` — Settings → **Account** → Sign out
  - `tests/manual-qa/M3_LINUX_FINDINGS.md` — same
- [x] No backend changes (`logout` command unchanged)
- [ ] Quick smoke: tab renders, sign out still calls `onLoggedOut` (loop agent or user)

**Can run in parallel with pref-mock-1..3** (no file overlap). Code landed on branch — verify smoke only.

---

### pref-mock-6 — Slice complete

- [ ] Full test suite green
- [ ] Update `.cursor/flint-loop-state.json`: `milestone_status: "ci_green"`, `loop_stopped: true`
- [ ] Output handoff summary for user to run ROADMAP M7 leftovers tonight

---

## Explicitly out of scope (this loop)

- Semantic / embedding nearest-preferred for rephrased questions (v2)
- M7-M2 retry same question
- M7-M3 edit transcript before re-grade
- M7-M4 mock coach → session_qa embed
- M7-M5 trends, M7-M6 ElevenLabs
- Commit / push / PR (unless user asks)

**In scope (UX):** `pref-settings-1` — move Sign out to Account tab (see above)

---

## Key files

| Area | Path |
|------|------|
| Mock conductor | `src-tauri/src/mock/conductor.rs` |
| Mock RAG | `src-tauri/src/mock/rag.rs` |
| Preferred persistence | `src-tauri/src/session/persistence.rs` |
| Question key | `src-tauri/src/session/question_attempts.rs` |
| Mock UI | `src/screens/MockInterview.tsx` |
| Suggested panel | `src/panels/SuggestedAnswerPanel.tsx` |
| Rehearsal preferred (reference) | `src/components/PreferredAnswerPanel.tsx` |
| Orchestrator preferred (reference) | `src-tauri/src/orchestrator/mod.rs` |

---

## Verification gates (every task)

```bash
cd src-tauri && cargo test
cd .. && npm run test
```

If Rust public API changed: `cargo clippy -- -D warnings` in `src-tauri/`.

---

## Blocker examples

- Mock question list excludes satisfied questions — manual test step 2 may need unsatisfied question; document workaround, do not redesign mock question selection in this loop
- Practice mode token timing breaks — stop and report; do not ship early leak of preferred script
