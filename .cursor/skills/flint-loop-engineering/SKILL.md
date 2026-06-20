---
name: flint-loop-engineering
description: >-
  Run the Flint vertical-slice engineering loop: read loop state, implement one
  roadmap task at a time, run tests, update state, stop only on blocker or slice
  complete. Use when the user says /flint-loop, flint-loop start/resume/stop,
  or asks to loop on a Flint milestone task before manual gates.
---

# Flint loop engineering

Autonomous implementation loop for Flint milestones. Work continuously until the milestone is merged or a **blocker** stops the loop.

## Kickoff commands

| Command | Action |
|---------|--------|
| `/flint-loop <milestone> start` | Read state, load task prompt, begin loop on milestone branch |
| `/flint-loop resume` | Continue from `current_slice` / `current_task_id` in state |
| `/flint-loop stop` | Set `loop_stopped: true`, report status |
| `/flint-loop status` | Print milestone, branch, current slice, attempts |

`<milestone>` examples: `m10-live-session-reliability`, `pref-mock`, `M7-M2`

## Milestone workflow (single branch)

For multi-slice milestones (e.g. M10):

1. **One branch** for the entire milestone — e.g. `feature/m10-live-session-reliability`.
2. Branch from latest `main` once at milestone start. **Never commit on `main`.**
3. **One commit per slice** (or coherent sub-part) with a message that names the slice and explains why.
4. Implement slices in dependency order from the task prompt; do **not** open per-slice PRs.
5. After each slice: run local gates (`cargo test`, `cargo clippy -- -D warnings`, `npm run test`), update loop state, **continue immediately** to the next slice.
6. When all in-scope slices are committed: **push branch**, **open one PR to `main`**, CI loop until green, **merge PR**.
7. Do **not** stop between slices unless a stop condition is hit.

## Every iteration (strict order)

1. **Read** `.cursor/flint-loop-state.json` and the active task prompt under `.cursor/skills/flint-loop-engineering/tasks/`.
2. **Read** `docs/ROADMAP.md` only for the current slice scope — do not expand scope.
3. **Branch** — confirm on `milestone_branch.flint`; create from `main` if missing.
4. **Implement** the current slice from the task prompt checklist.
5. **Verify** — run gates (minimum: `cargo test`, `npm run test`, `cargo clippy` if Rust changed).
6. **Commit** on the milestone branch with slice-scoped message.
7. **Update state** — append slice to `completed_slices`, advance `current_slice`, reset attempt counters.
8. **Continue** to next slice or PR/CI phase — do not ask the user between slices unless blocked.

## CI and merge phase (end of milestone)

When all slices are committed on the milestone branch:

1. `git push -u origin HEAD`
2. Open **one** PR to `main` with milestone summary + test plan.
3. Poll `gh pr checks` until all green (fix failures, commit, push — increment `ci_fix_attempts`).
4. `gh pr merge` when green.
5. Set `milestone_status: "ci_green"`, `loop_stopped: true`.

## Stop conditions

**Stop the loop (set `loop_stopped: true`) only when:**

- Milestone PR merged to `main` and CI green
- **Blocker:** same slice failed `max_task_attempts` (default 3)
- **Blocker:** CI fix loop exceeded `max_ci_fix_attempts` (default 6) on the milestone PR
- **Blocker:** requires manual device test — park in `manual_gate_backlog`, stop loop
- User said `/flint-loop stop`

**Do NOT stop for:** formatting nits, optional refactors, tasks outside the active prompt, completing one slice when more slices remain.

## State file (`.cursor/flint-loop-state.json`)

```json
{
  "current_milestone": "m10-live-session-reliability",
  "milestone_status": "in_progress | ci_green | blocked_on_<slice>",
  "milestone_branch": { "flint": "feature/m10-live-session-reliability" },
  "current_slice": 1,
  "completed_slices": [],
  "current_task_id": "m10-s1-echo",
  "loop_stopped": false,
  "ci_fix_attempts": 0,
  "max_ci_fix_attempts": 6,
  "task_attempts": 0,
  "max_task_attempts": 3,
  "open_prs": {}
}
```

Increment `task_attempts` / `same_task_streak` on slice failure; reset on slice success.

## Flint rules (non-negotiable)

- `.cursor/rules/flint-*.mdc` — architecture, security, performance, git workflow
- Prompts live in `/prompts/` — never inline Rust string literals for LLM prompts
- React is a dumb renderer — session state in Rust only
- API keys in keychain only; no session content in INFO logs
- Minimal diff — no drive-by refactors

## Task prompts

```
.cursor/skills/flint-loop-engineering/tasks/<milestone>.md
```

## End-of-milestone deliverable

1. Branch name, PR URL, merge confirmation
2. `git diff main --stat` summary
3. Slices completed vs deferred
4. Manual QA file paths (if any)
5. **"Ready for manual live session test"** or **"Blocked on …"**

## Resume format (manual QA)

```
/flint-loop resume — <slice-id> <platform> pass|fail (<notes>)
```
