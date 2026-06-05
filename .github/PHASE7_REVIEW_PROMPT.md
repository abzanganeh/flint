# Phase 7 Implementation Review — Agent Prompt

> **Purpose:** End-to-end review of Phase 7 hardening (tasks 7.1–7.7) on branch `chore/phase7-security-audit`. All 7.1–7.7 commits live on this single branch.
> **Audience:** High-capability agent (`claude-opus-4-7-thinking-xhigh` or equivalent).
> **Do not skip fixes:** Every finding must be remediated on the same branch, verified with tests, committed, and pushed before you mark the review complete.

---

## Required attachments

`docs/` is gitignored. Before starting, ask the user to attach the files below (or open them as `@`-references in Cursor). Do not proceed without them.

- `ROADMAP.md` — Phase 7 tasks table + Phase 7 Review Gate
- `flint_system_design_v3.md` — §7/§11 NFRs, §22 failure handling, §33 GDPR, §35 observability, §36 feature flags, §31 CI/CD

The following workspace rules autoload from `.cursor/rules/` and need no attachment:

- `flint-core.mdc`, `flint-rust.mdc`, `flint-security.mdc`, `flint-performance.mdc`, `flint-testing.mdc`, `flint-git-workflow.mdc`

---

## Copy-paste prompt (start here)

The fenced block below is the prompt itself. The outer fence uses **four** backticks so inner triple-backtick code blocks render correctly when pasted.

````
You are the Phase 7 release reviewer for Flint. Perform a full implementation review — NOT a design brainstorm — against shipped code on branch `chore/phase7-security-audit`.

## Authoritative references (must be attached before starting)

- ROADMAP.md — Phase 7 tasks table + Phase 7 Review Gate
- flint_system_design_v3.md — §7/§11 NFRs, §22 failure handling, §33 GDPR, §35 observability, §36 feature flags, §31 CI/CD

Workspace rules autoload from `.cursor/rules/`: flint-core, flint-rust, flint-security, flint-performance, flint-testing, flint-git-workflow.

## Scope — what Phase 7 delivered

| Task | Expected capability | Primary files |
|------|---------------------|---------------|
| 7.3 | Performance benchmark suite + `bench_gate` NFR gates | `src-tauri/benches/`, `src-tauri/src/bin/bench_gate.rs`, `.github/workflows/bench.yml` |
| 7.4 | Cost cap — suspend inference at threshold | `src-tauri/src/cost.rs`, `orchestrator/mod.rs`, `commands.rs`, `src/hooks/useCostCap.ts` |
| 7.5 | GDPR delete + export | `src-tauri/src/gdpr.rs`, `tests/integration/gdpr.rs`, `commands.rs`, `src/commands/index.ts` |
| 7.6 | Feature flags — remote + 24h cache kill switch | `src-tauri/src/flags.rs`, `tests/integration/feature_flags.rs`, `src/hooks/useFeatureFlag.ts` |
| 7.7 | Security audit remediation | log redaction, `supabase/config.rs` env override, provider key commands, tracing init |
| Stretch | Crash-recovery hardening | `session/persistence.rs`, `session/recovery.rs`, `tests/integration/crash_recovery.rs` |
| Stretch | Cross-platform CI matrix | `.github/workflows/ci.yml`, `scripts/install-*-deps.*` |

7.1 (coverage) and 7.2 (eval harness) are verified at the Review Gate level only — no per-task functional review section here.

Out of scope for this review: **7.8 Distribution/installers** (not started), and any dependency upgrade not directly tied to a finding.

## Review protocol

### Step 0 — Pre-flight (environmental)

Verify the host can build before running anything else:

- `clang` / `libclang` (required by `whisper-rs-sys`)
- `cmake`, `pkg-config`, `ripgrep` (`rg`), Node ≥ 20, Rust stable
- Linux: `./scripts/install-linux-deps.sh`
- macOS: `./scripts/install-macos-deps.sh`
- Windows: `./scripts/install-windows-deps.ps1`

If any tool is missing, surface as `Environmental — not a code finding` and ask the user. Do **not** classify a build break caused by a missing system dep as BLOCKER.

### Step 1 — Baseline verification (run every command; capture output)

From repo root:

```bash
git checkout chore/phase7-security-audit
git pull origin chore/phase7-security-audit

cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cd ..

npx tsc --noEmit
npx vitest run
```

`cargo test` runs lib + bin + integration + doc tests in one pass. Do not split into `--lib` and `--tests`; that recompiles for no gain.

If `cargo fmt --check` fails: run `cargo fmt --all`, commit as `chore: cargo fmt`, continue. Not a finding.

Any other failure is a **BLOCKER** finding — fix before continuing.

### Step 2 — Task-by-task functional review

For each task below, read the listed files, trace the call path from Tauri command → Rust → event → React (where applicable), and confirm tests exist and assert real behaviour (not tautologies).

#### 7.3 Performance benchmarks

- [ ] Every NFR in `flint-performance.mdc` has a criterion bench OR documented exemption.
- [ ] `bench_gate` parses `sample.json`, computes P95 with nearest-rank, exits non-zero on FAIL gates.
- [ ] `.github/workflows/bench.yml` runs on PR or schedule; document if not yet green on main.
- [ ] Orchestrator TTFT bench uses infinite rate limits so it measures orchestrator overhead only.

#### 7.4 Cost cap enforcement

- [ ] `CostTracker::record_turn_with_transition` emits events only on state change (Ok → Warning80 → Reached).
- [ ] `lift_cost_suspension` clears `suspended` without immediate re-suspend until next `record_turn`.
- [ ] Orchestrator pre-check + `trigger_response` guard both block when suspended.
- [ ] `stop_session` resets tracker; frontend `useCostCap` subscribes to events.
- [ ] Integration tests: `dispatch_turn_short_circuits_when_cost_tracker_is_suspended`, `lifting_cost_suspension_re_enables_inference`.

#### 7.5 GDPR delete + export

- [ ] `gdpr::delete_account` orchestrates Supabase + vector + SQLite + keychain (injectable purge for tests).
- [ ] Partial failure: local wipe continues; `DeleteAccountReport` surfaces per-step status.
- [ ] `delete_account` command guards against LIVE session; resets in-memory state on completion.
- [ ] `export_user_data` returns JSON string; no session content in INFO+ logs.
- [ ] Integration tests in `tests/integration/gdpr.rs` pass without touching real keychain in parallel tests.

#### 7.5 Stretch — Crash recovery hardening

- [ ] `PRAGMA synchronous = FULL`, integrity_check, schema version check on open.
- [ ] `write_state_transition` is transactional; ordering `(updated_at DESC, rowid DESC)` deterministic.
- [ ] `check_for_recovery` refuses when state machine ≠ IDLE; marks stale sessions CRASHED.
- [ ] `discard_session` clears vector store; integration tests cover multi-session + double-check guard.

#### 7.6 Feature flags

- [ ] `evaluate()` matches `flint-rust.mdc`: enabled → plan → `stable_hash(id) % 100 < rollout`.
- [ ] FNV-1a hash is deterministic cross-process (not `DefaultHasher`).
- [ ] Kill switch: fresh cache → Remote; <24h cache → Cache; stale/missing/corrupt → compiled GA defaults.
- [ ] Failed refresh leaves prior bundle authoritative.
- [ ] Tauri commands: `is_feature_enabled`, `refresh_feature_flags`, `get_feature_flags_snapshot`.
- [ ] Startup background refresh in `lib.rs` setup; does not block app boot.
- [ ] Unit tests cover `evaluate()`, kill switch, and refresh ordering; integration tests cover the Tauri command surface. Counts are approximate — assert `≥ N`, not `= N`.

#### 7.7 Security audit remediation

Re-run the security searches (mandatory — do not trust prior audit). These are bash patterns; in zsh prefix with `noglob` or single-quote the whole argument:

```bash
# Session content in logs (must be empty in release paths)
rg 'info!.*question|warn!.*question|error!.*raw_response|error!.*transcript' src-tauri/src --glob '!**/bin/**'

# Plaintext secrets on disk in committed config
rg 'anonKey|api_key|sk-' tauri.conf.json src-tauri/tauri.conf.json

# eprintln in lib (allowed only in bin/bench_gate with exemption comment)
rg 'eprintln!|println!' src-tauri/src --glob '!**/bin/**'

# expose_secret in logs
rg 'expose_secret' src-tauri/src -A2 | rg 'info!|warn!|error!|debug!'

# Audio write to disk
rg 'fs::write|File::create|BufWriter' src-tauri/src/audio src-tauri/src/transcription src-tauri/src/session
```

Confirm:

- [ ] `tauri.conf.json` ships empty `plugins.supabase.url` / `anonKey`; runtime uses `FLINT_SUPABASE_URL` + `FLINT_SUPABASE_ANON_KEY` via `supabase/config.rs`.
- [ ] `.env.example` documents required dev vars; README Development section explains export before `npm run tauri dev`.
- [ ] `AuthInterface::refresh(&SecretString)` — no String clone of refresh token in `auth_session.rs`.
- [ ] `signup`/`login` wrap password in `SecretString` before auth call.
- [ ] `tracing_subscriber` initialised in `lib.rs::run()` with `FLINT_LOG` override.
- [ ] Provider keys: `save_provider_key`, `is_provider_key_present`, `clear_provider_key` with `KNOWN_API_PROVIDERS` allowlist.
- [ ] Digest prompt uses `[data]` block for `{pasted_context}`, not system role injection.
- [ ] Stealth window flags in `tauri.conf.json`; X11 fails health check / stealth self-test.

### Step 3 — Architecture rule compliance (spot-check)

- [ ] React never holds authoritative session state (only Zustand UI state).
- [ ] No inline prompts in Rust — all loaded from `/prompts/`.
- [ ] Parallel threads via `tokio::spawn` + `join!`, not sequential await chains in orchestrator turn dispatch.
- [ ] Directional/depth/clarifying fire only on `source = System` (verify in orchestrator/audio pipeline).
- [ ] API keys as `SecretString`; keychain-only persistence for secrets.

### Step 4 — Phase 7 Review Gate (ROADMAP)

Update ROADMAP checkboxes ONLY for items you verified with evidence (command output, test name, or file:line). Do not check items you did not run (e.g. OBS stealth on device, clean VM installers).

| Gate | How to verify | If unverifiable in this environment |
|------|---------------|-------------------------------------|
| CI gates (TTFT, RAG, transcription lag) | Inspect latest `bench.yml` artifact | Mark `Manual — needs CI artifact access` in handoff; do not block |
| Eval harness win rate ≥ 50%, conciseness ≥ 95% | `cargo run -p evals --release -- --limit 10` smoke; full run if API keys available | Mark `Manual — needs LLM API keys` in handoff; do not block |
| Coverage targets | `cargo tarpaulin` or `cargo llvm-cov` — state machine must be 100% | Mark `Manual` if neither tool installed |
| Zero audio on disk | Code audit + optional `strace` during live session | Code audit alone is sufficient for this review |
| GDPR deletion E2E | `cargo test --test gdpr` | — |
| Crash recovery E2E | `cargo test --test crash_recovery` | — |
| Installers / stealth capture | **Manual** — note as open, do not block on 7.1–7.7 | — |

Items marked `Manual` do not count as open BLOCKER/HIGH findings. Surface them in the handoff note instead.

### Step 5 — Fix all findings

Severity rubric with examples:

- **BLOCKER** — security violation, NFR breach, data corruption.
  *e.g.* plaintext API key on disk; audio bytes flushed to disk; new Supabase table without RLS; TTFT P95 > 900ms gate breached.
- **HIGH** — wrong contract, missing required test, broken event payload.
  *e.g.* `directional_token` event payload type drift between Rust and TS; integration test asserts a tautology.
- **MEDIUM** — docs out of date, ergonomic issue, flaky test.
  *e.g.* README missing a required env var; test relies on real keychain in parallel runs.
- **LOW** — naming, formatting that `fmt` doesn't catch, minor comment polish.

For every issue:

1. Fix on `chore/phase7-security-audit` with minimal diff.
2. Add or extend a test when fixing a behaviour bug.
3. Re-run Step 1 commands after fixes.

**Do not** mark review complete with open BLOCKER or HIGH items.

Commit messages: concise, imperative, explain *why*. Example:
`Phase 7.7 fix: redact session content from INFO logs in orchestrator/depth.rs`

### Step 6 — Deliverables

1. **Findings report** (markdown) with: Summary counts, table of findings (severity, location, fix commit), clean checks list.
2. **Commits** on `chore/phase7-security-audit` — one commit per logical fix group, imperative messages explaining *why*.
3. **Push:** `git push origin chore/phase7-security-audit`
4. **ROADMAP update** for verified review-gate items (local file only — `docs/` is gitignored).
5. **Handoff note:** anything still manual (7.8, OBS test, device audio validation, items marked `Manual` in Step 4) for the human release owner.

## Output format

Return your final message as:

```
# Phase 7 Review — Complete

## Summary
- X BLOCKER, Y HIGH, Z MEDIUM, W LOW — all fixed and pushed
- Tests: [paste cargo test / vitest summary]
- Branch: chore/phase7-security-audit @ <short sha>

## Findings (fixed)
| Sev | Title | Location | Fix |
|-----|-------|----------|-----|

## Clean checks
- ...

## Still manual / out of scope
- ...

## Suggested next step
- Merge chore/phase7-security-audit → main OR open PR with link
```

Use code citations `startLine:endLine:path` (Cursor format). In other clients, use `path:lineN`.
````

---

## Branch & commit context

All Phase 7 work (7.1 through 7.7) lives on `chore/phase7-security-audit`. There is no separate stacking branch to merge first.

Expected recent commits on the review branch (verify with `git log --oneline -10`):

- `Add Phase 7 review prompt for high agent + local dev env template`
- `Phase 7.7: security audit fixes (5 CRITICAL + 8 WARN + 2 INFO)`
- `Phase 7.6: feature flag system with Supabase remote + local kill switch`
- `Phase 7.5: GDPR delete-account + export-user-data flow`
- `Phase 7.5: harden crash recovery against multi-session and partial-write faults`
- `Phase 7.4: cross-platform CI matrix (Linux/Windows/macOS)`
- `Phase 7.4: cost cap enforcement (configurable suspend)`
- `Phase 7.3: add performance benchmark suite and NFR gate`
- `feat(phase7.2): prompt eval harness with 200-question bank`
- `test(phase7.1): bring LLM orchestrator coverage to target` (and sibling 7.1 commits)

## Local dev prerequisite (post-7.7)

Supabase credentials are **not** in committed `tauri.conf.json`. Before `npm run tauri dev`:

```bash
cp .env.example .env   # fill in values — never commit .env
export FLINT_SUPABASE_URL=http://127.0.0.1:54321
export FLINT_SUPABASE_ANON_KEY=<your-local-anon-key>
```

Or use `supabase start` and paste the anon key from `supabase status`.

## Notes for the reviewing agent

- Canonical copy: `.github/PHASE7_REVIEW_PROMPT.md` (tracked). The local `docs/PHASE7_REVIEW_PROMPT.md` is a symlink to this file; never edit one without the other.
- `docs/` is gitignored; ROADMAP edits are local-only and won't appear in the PR diff.
- Do **not** implement 7.8 installers in this review unless explicitly asked.
- Do **not** force-push `main`.
- Prefer fixing over documenting-wont-fix for BLOCKER/HIGH security items.
- Dependency upgrades are out of scope unless directly tied to a finding; file follow-up issues for incidental drift.
- Numeric test counts in this prompt are approximate — assert `≥ N`, not `= N`.
