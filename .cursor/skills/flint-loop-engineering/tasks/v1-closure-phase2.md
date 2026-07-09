# V1 Closure Phase 2 — Remaining Work + Uncommitted Hardening

> **DO NOT RUN THIS LOOP until the user explicitly says**  
> `/flint-loop v1-closure-phase2 start`  
> This file is the plan + prompt only.

## Context (audited 2026-07-08)

### Already done (do not re-implement)

| Item | Evidence |
|------|----------|
| v1-closure phase 1 (slices 1–16) | PR #32 merged → `main` (`3f5ceef`) |
| Calibration / mock / OAuth fix | PR #33 merged (`47e192b`) |
| M3–M12 code on `main` | See `docs/ROADMAP.md` milestones |
| BYOK + flat Pro, speakrs ONNX scaffold, eval baseline, bench_gate, coverage gate | v1-closure phase 1 |
| System-audio isolation root-cause **code** (prefer ALSA `pulse` over `pipewire`, fail-fast same-device, `system_audio_isolation` health check) | **Uncommitted on dirty `main`** — must land via this milestone |

### Dirty working tree on `main` (must leave `main` clean)

Uncommitted / untracked work that belongs in this milestone (include):

- `src-tauri/src/audio/capture.rs` — pulse-first loopback + same-device fail-fast
- `src-tauri/src/health/checks.rs` — `SystemAudioIsolation` check + tests
- `src-tauri/src/commands.rs` — block `start_session` on isolation Fail
- `src-tauri/src/health/headphone_gate.rs` + integration test — Linux headphone detection
- `src-tauri/src/hotkeys.rs`, `src/hooks/useHotkeys.ts`, `src/screens/Rehearsal.tsx`
- `src-tauri/capabilities/default.json` — global-shortcut permissions
- `src/screens/HealthCheck.tsx`, `src/commands/index.ts`, `src/screens/LiveOverlay.tsx`
- `src/components/WindowResizeGrip.*`
- `scripts/dev-clean.sh`, `package.json` (`dev:clean`), `README.md`
- `tests/manual-qa/stealth-audio-validation-runbook.md`, `hosted-supabase-runbook.md` (tracked QA)

**Explicitly OUT OF SCOPE — never stage/commit:**

- `tests/manual-qa/fisher-iam-*.json`
- `scripts/apply_bank_aliases.py`, `dedupe_question_attempts.py`, `import_fisher_phone_screen.py`, `import_preferred_answers.py`, `recover_fisher_session.py`, `setup_prep_order.py`
- Any `*.code-workspace` changes
- Secrets / `.env`

### Docs are gitignored (`docs/` in `.gitignore`)

Edit locally on every completed slice; **never `git add docs/`**. Still update them so the local plan stays truthful.

---

## What's left (priority order)

### A — Automatable in Flint repo (this milestone)

1. **Clean slate** — leave `main` identical to `origin/main`; all WIP on a feature branch.
2. **Land uncommitted hardening** — audio isolation, headphone gate, hotkeys, resize grip, `dev-clean.sh`, runbook notes.
3. **Local docs pass** — refresh `ROADMAP.md`, `flint_system_design_v3.md`, `STRATEGY_B_INTEGRATION_PLAN.md` to match reality (v1-closure merged; audio isolation shipped; manual gates still open).
4. **Consolidated review** → fix findings → **one PR** → **CI green** → **merge**.

### B — Manual gates (park in `manual_gate_backlog`; do not fake pass)

| Gate | Doc | Notes |
|------|-----|-------|
| L1 HealthCheck screenshot | `stealth-audio-validation-runbook.md` | Include `system_audio_isolation` |
| L2 System audio loopback | same | Retest after pulse-first fix; YouTube → INTERVIEWER only |
| L5 OBS / screen capture | same | Document Wayland outcome |
| Installer signing / notarization | `installer-signing-runbook.md` | Needs Apple + Windows certs |
| Hosted Supabase project | `hosted-supabase-runbook.md` | Human in dashboard |
| M8 device WER | `M8_INPUT_QUALITY.md` | Calibration thresholds |
| M10 live Zoom/Teams + phone Ctrl+Q | `M10_LIVE_RELIABILITY.md` | Device |
| M6 provider P1–P3 | `M6_LLM_PROVIDERS.md` | Device / keys |
| M7 macOS BlackHole + Windows WASAPI | ROADMAP §M7 | Device |
| Wayland hotkeys unfocused | Accepted P2 | Do not invent a portal fix in v1 |

### C — Strategy B / cross-repo (out of Flint-only loop; document only)

| Phase | Status | Action in this milestone |
|-------|--------|--------------------------|
| 1 Link | ✅ Signed off | Docs: mark done |
| 1.5 Installers | Manual | Point at signing runbook; no code |
| 2 Extension Chrome Store | Open | Note in STRATEGY_B; work lives in `flint-extension` |
| 3 Full credit ledger + SSO | Partial (BYOK/Pro shipped) | Docs: v1 simplified; full ledger deferred |
| 4 Autofill | ❌ Phase 1 blocker, not revenue-gated | **Separate prompts** in smart-resume + flint-extension — not this loop |
| 5.5 Agent corpus | Deferred | Docs only |
| 6 Backend unify | Deferred | Docs only |

### D — Explicitly deferred (do not implement)

ElevenLabs TTS, file upload, URL scraping, mid-session model switch UI, voice I/O beyond Piper, conversation phase tracking, medical domain, OAuth beyond current Google SSO path, cert pinning, admin corpus write UI (11.5), agent-enriched bank (9.5).

---

## Three-layer agent model (mandatory)

| Layer | Model slug | Role |
|-------|------------|------|
| **L1 Implementer (cheap)** | `composer-2.5-fast` | SIMPLE slices: docs text, runbook checkboxes, README/`package.json`, small TS labels, CI flake fixes, merge |
| **L2 Implementer / reviewer** | `grok-4.5-fast-xhigh` | MEDIUM slices: health checks, hotkeys TS, headphone gate helpers, capture device selection, PR body |
| **L3 Implementer / senior reviewer** | `claude-sonnet-5-thinking-high` | COMPLEX slices: audio capture isolation, `start_session` gates, security/compliance-adjacent; final pre-PR review |

### Review ladder (every slice)

```
SIMPLE  → Composer 2.5 implements  → Grok 4.5 reviews      → fix → commit
MEDIUM  → Grok 4.5 implements      → Sonnet 5 reviews      → fix → commit
COMPLEX → Sonnet 5 implements      → Sonnet 5 2nd-pass review (different checklist) → fix → commit
CI/merge → Composer 2.5 only (cheap) until green + merged
```

**Reviewer rules:**

- Reviewer is **read-only** except for writing a findings list; implementer applies fixes.
- Max **one** review round per slice (fix → re-check once → commit or blocker).
- Reviewer must cite file paths and reject scope creep / secrets / Fisher scripts.
- Parent agent launches Task subagents with the model slugs above; do not substitute other models.

### Complexity tags per slice

| Slice | Complexity | Implementer | Reviewer |
|-------|------------|-------------|----------|
| 0 Clean slate | SIMPLE | Composer 2.5 | Grok 4.5 |
| 1 Land audio isolation + health gate | COMPLEX | Sonnet 5 | Sonnet 5 (2nd pass) |
| 2 Headphone gate + LiveOverlay copy | MEDIUM | Grok 4.5 | Sonnet 5 |
| 3 Hotkeys + capabilities + Rehearsal | MEDIUM | Grok 4.5 | Sonnet 5 |
| 4 Resize grip + `dev-clean.sh` + README | SIMPLE | Composer 2.5 | Grok 4.5 |
| 5 Tracked QA runbook updates | SIMPLE | Composer 2.5 | Grok 4.5 |
| 6 Local docs pass (gitignored) | MEDIUM | Grok 4.5 | Sonnet 5 |
| 7 Consolidated pre-PR review | COMPLEX | — | Sonnet 5 (review only) |
| 8 Fix review findings | MEDIUM | Grok 4.5 | Sonnet 5 |
| 9 PR + CI + merge | SIMPLE | Composer 2.5 | Grok 4.5 (PR text only) |

---

## Kickoff — clean slate (Slice 0) — BEFORE any code

Working directory: `/home/alireza/Desktop/projects/Flint`

```bash
# 1. Confirm dirty tree
git status -sb
git fetch origin

# 2. Create WIP branch FROM current dirty main (preserves uncommitted work)
git checkout -b feature/v1-closure-phase2-hardening

# 3. Stage ONLY in-scope files (see list above). Never Fisher scripts/json.
#    Prefer explicit paths over `git add -A`.

# 4. Optional safety commit of WIP so nothing is lost, OR leave unstaged
#    and implement slice-by-slice. Prefer one WIP commit then amend/split
#    only if user rules allow; default: commit per slice below.

# 5. Clean main
git stash push -u -m "phase2-wip" -- <in-scope paths...>   # if not committing yet
# OR after committing WIP on feature branch:
git checkout main
git reset --hard origin/main
git clean -fd   # ONLY if no wanted untracked remain; never delete Fisher files user needs
git checkout feature/v1-closure-phase2-hardening
```

**Hard rules:**

- Never commit on `main`.
- Never force-push `main`.
- After slice 0, `main` must match `origin/main` with a clean status.
- Milestone branch: `feature/v1-closure-phase2-hardening`

Initialize `.cursor/flint-loop-state.json`:

```json
{
  "current_milestone": "v1-closure-phase2",
  "milestone_status": "in_progress",
  "milestone_branch": { "flint": "feature/v1-closure-phase2-hardening" },
  "current_slice": 0,
  "completed_slices": [],
  "current_task_id": "v1p2-s0-clean-slate",
  "loop_stopped": false,
  "ci_fix_attempts": 0,
  "max_ci_fix_attempts": 6,
  "task_attempts": 0,
  "max_task_attempts": 3,
  "open_prs": {},
  "manual_gate_backlog": [],
  "agent_ladder": {
    "simple": "composer-2.5-fast",
    "medium": "grok-4.5-fast-xhigh",
    "complex": "claude-sonnet-5-thinking-high"
  }
}
```

---

## Slices

### Slice 0: `v1p2-s0-clean-slate` (SIMPLE)

**Goal:** `main` == `origin/main`; all work on `feature/v1-closure-phase2-hardening`.

**Gate:** `git status` on `main` clean; feature branch exists; state file written.

---

### Slice 1: `v1p2-s1-audio-isolation` (COMPLEX)

**Goal:** Ship the pulse-first system-audio fix end-to-end.

- Prefer ALSA `"pulse"` over `"pipewire"` in `find_system_device`
- Fail `AudioCapture::start` if system name == mic name
- `HealthCheck::SystemAudioIsolation` + include in `run_health_check`
- `start_session` blocks on Fail (non-phone mode)
- TS: `HealthCheckName` + HealthCheck label
- Unit test: isolation check runs without panic

**Gate:** `cargo test --lib health::checks`, `cargo clippy -- -D warnings`, `npm run test`  
**Docs (local):** ROADMAP — note system_audio_isolation shipped; stealth runbook root-cause section accurate.

---

### Slice 2: `v1p2-s2-headphone-gate` (MEDIUM)

**Goal:** Linux headphone detection so Bluetooth/headset passes gate without requiring echo-cancel module alone; LiveOverlay copy matches.

**Gate:** `cargo test` headphone_gate integration + unit tests.

---

### Slice 3: `v1p2-s3-hotkeys` (MEDIUM)

**Goal:** Focused re-ask + panic hide work; Wayland fallbacks documented; capabilities allow global-shortcut.

**Gate:** `npm run test`; no listener-churn regressions in `useHotkeys`.

---

### Slice 4: `v1p2-s4-dev-clean-ux` (SIMPLE)

**Goal:** `scripts/dev-clean.sh` executable; `npm run dev:clean`; README Development section; WindowResizeGrip polish if already in tree.

**Gate:** `bash scripts/dev-clean.sh --help` exits 0.

---

### Slice 5: `v1p2-s5-tracked-runbooks` (SIMPLE)

**Goal:** Update tracked `tests/manual-qa/stealth-audio-validation-runbook.md` (and hosted-supabase notes if needed): L1 includes `system_audio_isolation`; L2 retest note; root-cause section; do **not** mark L2 PASS without device evidence.

**Gate:** File review only.

---

### Slice 6: `v1p2-s6-docs-pass` (MEDIUM — local only, never commit `docs/`)

**Goal:** Align gitignored docs with reality.

**`docs/ROADMAP.md`:**

- Status: v1-closure phase 1 **merged** (PR #32); phase 2 = this hardening PR
- "What comes after M10" priority 1 → done; next = manual gates + Strategy B
- Phase 3 audio: note Linux pulse-first isolation fix
- Keep manual checkboxes open unless device-signed

**`docs/flint_system_design_v3.md`:**

- §12 / monetization: BYOK + flat Pro for v1; full ledger Strategy B Phase 3
- §29: TLS without pinning = accepted risk
- §14 / excluded: keep file upload, URL scrape, mid-session switch, ElevenLabs as v2+
- Audio: document that Linux loopback must use Pulse-compatible ALSA path (`pulse` plugin + sink `.monitor`); isolation health check is a READY→LIVE precondition

**`docs/STRATEGY_B_INTEGRATION_PLAN.md`:**

- Status table: Phase 1 done; 1.5 manual; 2 open; 3 partial; 4 autofill = Phase 1 blocker ungated
- "Priority after v1-closure" → after phase-2 hardening merge → manual gates → Chrome Store → autofill (separate repos)
- Last audited date → 2026-07-08

**Gate:** Docs read-back by Sonnet 5; no `git add docs/`.

---

### Slice 7: `v1p2-s7-consolidated-review` (COMPLEX — review only)

Sonnet 5 reviews full `git diff origin/main...HEAD` against:

- flint-core / security / performance / git-workflow rules
- No Fisher files, no secrets, no docs/ staged
- Audio isolation actually preferred + fail-fast present
- Health check + start_session gate wired
- Manual gates not falsely closed

Produce findings list (blocking vs nit).

---

### Slice 8: `v1p2-s8-fix-findings` (MEDIUM)

Implementer (Grok) fixes all **blocking** findings; Sonnet re-checks once.

**Gate:** Reviewer ACK "no blocking findings".

---

### Slice 9: `v1p2-s9-pr-ci-merge` (SIMPLE — Composer 2.5)

```bash
git push -u origin HEAD
gh pr create --title "v1 closure phase 2 — audio isolation + live hardening" --body "..."
# Poll gh pr checks; fix CI with Composer; max 6 attempts
gh pr merge  # only when green + no blocking review findings
```

PR body must list:

- What shipped (isolation, headphone gate, hotkeys, dev-clean)
- Manual gates still open (copy from `manual_gate_backlog`)
- Test plan: L2 retest, HealthCheck isolation row, focused hotkeys

After merge:

- `milestone_status: "ci_green"`, `loop_stopped: true`
- Update local ROADMAP: phase 2 merged
- Report PR URL + remaining manual backlog

---

## Stop conditions

Same as `flint-loop-engineering/SKILL.md`:

- Same slice fails 3 attempts → stop
- CI fix loop > 6 → stop
- Hardware/manual needed → park in `manual_gate_backlog`, continue
- User `/flint-loop stop`

**Do NOT stop** between slices 0–9 for optional refactors or Strategy B autofill.

---

## Done when

1. PR merged to `main`, CI green  
2. `main` clean; no WIP left on main  
3. Local docs updated (uncommitted OK)  
4. Clear `manual_gate_backlog` for L1/L2/L5, signing, Supabase, M6–M10 device QA  
5. Explicit note: Strategy B Phase 4 autofill needs **separate** smart-resume + flint-extension prompts  

## Resume

```
/flint-loop v1-closure-phase2 start
/flint-loop resume
/flint-loop status
```
