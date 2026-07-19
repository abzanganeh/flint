# Session Focus Robustness — stable ordering, round-aware tags, discoverable tailoring

> **DO NOT RUN THIS LOOP until the user explicitly says**
> `/flint-loop session-focus-robustness start`
> This file is the plan + prompt only.

## Context — four bugs from live usage, one root cause per fix

Reported during first real DAT recruiter-screen prep. Root-caused against
current code (see below). All four get a proper architectural fix, not a
patch:

1. **Question bank reorders unpredictably after asking a question.**
   Root cause: `shuffle_strings` (`src-tauri/src/session/shuffle.rs`) is a
   position-indexed Fisher–Yates over the *current* pending array. When a
   question is answered well and leaves `pending` (array shrinks by one),
   every remaining item's index shifts, so the whole permutation changes —
   unrelated questions "jump" to the front. Compounded by
   `readShuffleQuestionsPreference()` (`src/lib/shufflePreference.ts`)
   defaulting to **`true`** when unset, so this fires for every user who
   never touched the toggle.

2. **Session Focus only offers 5 tags (`behavioral`, `general`, `logistics`,
   `self-assessment`, `technical`) — no `motivation`, `culture`,
   `competency`.** Root cause: `list_question_bank_tags` returns only tags
   *already present on existing bank entries*
   (`collect_bank_tags`, `src-tauri/src/session/question_bank.rs`). Tags are
   inferred by keyword heuristic per-question — if no bank question happens
   to match a tag's trigger phrases, that tag never exists to select. There
   is no canonical taxonomy exposed independent of current bank content.

3. **Bank questions are technical/production-incident-heavy for a 30-minute
   recruiter screen.** Root cause: `likely_questions` come entirely from an
   LLM read of the pasted Job Description (`src-tauri/src/digest.rs`), which
   has no concept of *which interview round* this session is for. A Staff
   Backend JD reasonably produces deep technical questions — appropriate for
   a technical round, wrong for an HR/recruiter screen. Session Focus can
   only filter tags on what's already generated; it has no mechanism to
   supply round-appropriate questions when the JD-derived set doesn't cover
   the round.

4. **"Save as preferred answer" tailoring appears to have vanished.**
   Root cause: two different buttons look similar but aren't.
   `VisualPanel`'s **"Use This Answer"** (`src/panels/VisualPanel.tsx`) is
   copy-to-clipboard only, for diagrams, no edit path — by design. The real
   tailor/edit/save flow, `PreferredAnswerPanel`
   (`src/components/PreferredAnswerPanel.tsx`), is rendered in Rehearsal
   with `defaultCollapsed` (`src/screens/Rehearsal.tsx` line ~489) — it
   starts collapsed behind a small "Tailor for Live" header that's easy to
   miss the first time.

## Non-negotiable rules (from `.cursor/rules/flint-*.mdc`, restated)

1. React never holds authoritative session state — all new state
   (round type, tag catalog) lives in Rust, read via Tauri commands.
2. SQLite migrations are **additive only** — new nullable/defaulted columns,
   no drops, no renames (`flint-data.mdc`).
3. No session content (transcript text, AI answers) in logs above DEBUG.
4. Minimal diff — no drive-by refactors outside what each slice specifies.
5. Session Focus continues to affect **rehearsal and mock only** — live
   sessions always use the full bank, unfiltered. Do not change this.

---

## Architecture (read before any slice)

### 1. Stable, order-preserving shuffle (fixes bug 1)

Replace the Fisher–Yates-over-current-array approach with a **stable
per-item sort key** derived from the question text itself, independent of
what else is in the array or the array's length:

```rust
// src-tauri/src/session/shuffle.rs

/// Deterministic per-item sort key: same (item, seed) always produces the
/// same key, regardless of what other items are present or absent. This is
/// what makes shuffle order *stable* under insertion/removal — removing one
/// question never reorders the rest.
pub fn stable_shuffle_key(item: &str, seed: u64) -> u64 {
    // FNV-1a over the seed bytes + normalized item bytes.
    let mut hash: u64 = 0xcbf29ce484222325 ^ seed;
    for byte in item.trim().to_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
```

`get_question_bank` (`src-tauri/src/commands.rs`, the `shuffle` branch)
changes from building a `HashMap<question, rank>` via `shuffle_strings` to:

```rust
if shuffle {
    let seed = crate::session::shuffle::session_shuffle_seed(sid);
    pending.sort_by_key(|e| crate::session::shuffle::stable_shuffle_key(&e.question, seed));
}
```

Keep `session_shuffle_seed` unchanged (still session-scoped). Remove
`shuffle_strings` (Fisher–Yates) entirely once nothing references it — grep
first; if a test file references it directly, update the test to exercise
`stable_shuffle_key` instead.

**Also flip the default.** `readShuffleQuestionsPreference()`
(`src/lib/shufflePreference.ts`) currently returns `true` when unset. Change
the unset default to `false` — natural (insertion) order is the safer,
less-surprising default; shuffle becomes explicit opt-in.

**Required regression test** (this is the bug, encode it directly):

```rust
#[test]
fn stable_shuffle_key_is_unaffected_by_other_items() {
    let seed = 42u64;
    let questions = vec!["a", "b", "c", "d", "e"];
    let keys_before: Vec<u64> = questions.iter().map(|q| stable_shuffle_key(q, seed)).collect();
    // Remove "b" — every other item's key must be identical, proving
    // order among survivors cannot change when one item leaves the set.
    let remaining = ["a", "c", "d", "e"];
    for q in remaining {
        let key = stable_shuffle_key(q, seed);
        let original_index = questions.iter().position(|x| *x == q).unwrap();
        assert_eq!(key, keys_before[original_index]);
    }
}
```

### 2. Canonical focus-tag taxonomy (fixes bug 2)

New module `src-tauri/src/session/focus_tags.rs`:

```rust
/// Canonical focus tags, independent of any single session's bank content.
/// This is the taxonomy `infer_question_tags` (question_bank.rs) already
/// implicitly targets — making it an explicit, exposed list is the fix.
pub struct FocusTagDef {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

pub const FOCUS_TAG_TAXONOMY: &[FocusTagDef] = &[
    FocusTagDef { id: "self-assessment", label: "Self-assessment", description: "Tell me about yourself, strengths/weaknesses" },
    FocusTagDef { id: "motivation",      label: "Motivation",      description: "Why this role, why this company" },
    FocusTagDef { id: "behavioral",      label: "Behavioral",      description: "Tell me about a time..., STAR-style" },
    FocusTagDef { id: "competency",      label: "Competency",      description: "Leadership, conflict, prioritization, stakeholders" },
    FocusTagDef { id: "culture",         label: "Culture fit",     description: "Values, team fit, work style" },
    FocusTagDef { id: "technical",       label: "Technical",       description: "System design, coding, architecture" },
    FocusTagDef { id: "logistics",       label: "Logistics",       description: "Location, availability, compensation" },
    FocusTagDef { id: "general",         label: "General",         description: "Process, timeline, other" },
];
```

New DTO + command — the catalog always lists all 8 tags, with a live count
so the UI can show "0 questions yet" instead of hiding the tag entirely:

```rust
// dto.rs
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusTagCatalogEntryDto {
    pub id: String,
    pub label: String,
    pub description: String,
    pub question_count: usize,
}
```

```rust
// commands.rs — new command, ADDITIVE (keep list_question_bank_tags as-is,
// it's still used for other callers/tests — do not remove it).
#[tauri::command]
pub async fn get_focus_tag_catalog(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<crate::dto::FocusTagCatalogEntryDto>, String> {
    let sid = validate_session_id(&state, &session_id).await?;
    let entries = state.persistence.load_question_bank_entries(sid).map_err(|e| e.to_string())?;
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for e in &entries {
        for t in &e.tags {
            *counts.entry(t.as_str()).or_insert(0) += 1;
        }
    }
    Ok(crate::session::focus_tags::FOCUS_TAG_TAXONOMY
        .iter()
        .map(|def| crate::dto::FocusTagCatalogEntryDto {
            id: def.id.to_string(),
            label: def.label.to_string(),
            description: def.description.to_string(),
            question_count: counts.get(def.id).copied().unwrap_or(0),
        })
        .collect())
}
```

**UI change** (`SessionFocusGate.tsx`, `Settings.tsx`
`SessionFocusTab`): replace `listQuestionBankTags` (5-tag list) with
`getFocusTagCatalog` (always-8-tag list with counts). Render all 8 chips;
chips with `questionCount === 0` are still selectable (selecting an empty
tag is harmless — it just yields fewer rehearsal questions until content
exists) but visually muted with a small "0" badge so the user understands
why a chip they pick might not surface anything yet. Keep existing
`focusTags` selection/save logic unchanged — only the tag *source* changes.

**Manual tag override for added questions.** Extend
`add_to_question_bank` to optionally accept explicit tags, so a user can
correct a heuristic miss instead of being stuck with `general`:

```rust
#[tauri::command]
pub async fn add_to_question_bank(
    state: State<'_, AppState>,
    session_id: String,
    question: String,
    tags: Option<Vec<String>>, // NEW — validated against FOCUS_TAG_TAXONOMY; None = heuristic infer (unchanged)
) -> Result<Vec<String>, String> { /* ... */ }
```

UI: in `QuestionBank.tsx`'s "Add a question…" row, add a small collapsible
"Tag (optional)" chip picker sourced from the same catalog — collapsed by
default so it doesn't clutter the common case of adding an untagged
question.

### 3. Round-type-aware supplemental questions (fixes bug 3)

This is the deepest fix: give Flint an explicit concept of **which
interview round this session is for**, decoupled from the full-JD digest,
and use it to seed round-appropriate questions instead of relying solely on
JD-derived `likely_questions`.

**Schema (additive migration, `SCHEMA_VERSION` 17 → 18):**

```sql
ALTER TABLE sessions ADD COLUMN round_type TEXT NOT NULL DEFAULT '';
```

`SessionFocus` struct (`persistence.rs`) and `SessionFocusDto` (`dto.rs`)
both gain `round_type: String` (empty = unspecified, backward compatible —
existing rows read as `""`).

**Canonical round types + supplemental question bank**, new module
`src-tauri/src/session/round_questions.rs`:

```rust
pub const ROUND_TYPES: &[(&str, &str)] = &[
    ("recruiter_screen", "Recruiter / HR screen"),
    ("technical",        "Technical interview"),
    ("hiring_manager",   "Hiring manager"),
    ("onsite_panel",     "Onsite / panel loop"),
    ("final",            "Final / executive round"),
];

/// Heuristic inference from the pasted recruiter brief/agenda — a *suggestion*
/// the user can override, never authoritative on its own.
pub fn infer_round_type(recruiter_brief: &str) -> Option<&'static str> {
    let lower = recruiter_brief.to_lowercase();
    if lower.contains("recruiter") || lower.contains("hr screen")
        || lower.contains("phone screen") || lower.contains("initial screening")
        || lower.contains("internal recruiter") {
        return Some("recruiter_screen");
    }
    if lower.contains("system design") || lower.contains("coding")
        || lower.contains("whiteboard") || lower.contains("technical interview") {
        return Some("technical");
    }
    if lower.contains("hiring manager") { return Some("hiring_manager"); }
    if lower.contains("onsite") || lower.contains("panel") || lower.contains("loop") {
        return Some("onsite_panel");
    }
    if lower.contains("final round") || lower.contains("executive") {
        return Some("final");
    }
    None
}

/// Supplemental, correctly-tagged questions merged into the bank when a round
/// type is confirmed — additive only, never removes JD-derived questions
/// (those stay available for a later round on the same session).
pub fn supplemental_questions_for_round(round_type: &str) -> Vec<crate::session::question_bank::BankQuestionEntry> {
    use crate::session::question_bank::BankQuestionEntry;
    match round_type {
        "recruiter_screen" => vec![
            BankQuestionEntry::new("Tell me about yourself and your background.", vec!["self-assessment".into()]),
            BankQuestionEntry::new("Why are you interested in this role?", vec!["motivation".into()]),
            BankQuestionEntry::new("Why this company?", vec!["motivation".into()]),
            BankQuestionEntry::new("What are you looking for in your next role?", vec!["motivation".into(), "logistics".into()]),
            BankQuestionEntry::new("What's your current location and work setup preference?", vec!["logistics".into()]),
            BankQuestionEntry::new("What questions do you have for me?", vec!["general".into()]),
        ],
        "hiring_manager" => vec![
            BankQuestionEntry::new("How do you like to work with your manager?", vec!["culture".into()]),
            BankQuestionEntry::new("Tell me about a time you disagreed with a decision.", vec!["competency".into(), "behavioral".into()]),
        ],
        "onsite_panel" | "final" => vec![
            BankQuestionEntry::new("How do you handle competing priorities across stakeholders?", vec!["competency".into()]),
            BankQuestionEntry::new("What does success look like for you in the first 90 days?", vec!["motivation".into()]),
        ],
        _ => vec![],
    }
}
```

**Wiring (`save_session_focus` in `commands.rs`):** when `focus.round_type`
is non-empty and differs from the previously stored `round_type` for this
session (first confirmation, or the user changed rounds), merge
`supplemental_questions_for_round(&round_type)` into the bank via
`load_question_bank_entries` + dedup-by-normalized-key (reuse
`normalize_question_key`) + `store_question_bank_entries`. Never overwrite
or remove existing entries — additive union only.

**UI (`SessionFocusGate.tsx`):** add a round-type `<select>` above the
recruiter-brief textarea. On brief text change (debounced) or on blur, call
a new lightweight helper (`inferRoundType` exposed via a command or done
client-side by porting the same keyword table — prefer a Tauri command
`infer_round_type_from_brief` so the heuristic lives in one place) and
pre-select the dropdown, clearly marked as a suggestion the user can
change. Saving with a round type selected triggers the merge server-side.

### 4. Preferred-answer tailoring discoverability (fixes bug 4)

- `Rehearsal.tsx`: remove `defaultCollapsed` from the `PreferredAnswerPanel`
  usage (or pass `defaultCollapsed={false}`) — the tailor/save panel starts
  **expanded** the first time a response appears. `expanded` state still
  lives in the component and the user can collapse it manually; because the
  component instance persists across re-asks within one Rehearsal mount,
  a manual collapse stays collapsed for subsequent questions in the same
  session (no per-question remount needed — keep this simple).
- `VisualPanel.tsx`: rename the diagram button from **"Use This Answer"** to
  **"Copy Diagram Text"** (`title` and button label) so it's unambiguous
  that this is a clipboard action for the Visual panel specifically, not the
  tailor-and-save flow that lives under "Tailor for Live".

---

## Milestone structure

| Part | What | Depends on |
|---|---|---|
| **A** | Stable shuffle fix + safer default | None |
| **B** | Canonical focus-tag taxonomy + catalog command + UI + manual tag override | None — parallel to A |
| **C** | `round_type` schema + inference + supplemental question merge | None — parallel to A/B |
| **D** | Round-type UI wiring in Session Focus gate | Depends on C |
| **E** | Preferred-answer discoverability + Visual button rename | None — parallel to A/B/C |
| **F** | Review, fix findings, PR, CI, merge | Depends on A–E |

**Out of scope (do not implement here):**

- Changing what live sessions see (live always uses the full bank — unchanged)
- LLM-based round-type classification (heuristic + user override only, v1)
- Removing/renaming any existing tag or column
- Redesigning the overall Rehearsal layout beyond the two named UI changes

---

## Three-layer agent model

| Slice | Part | Complexity | Implementer | Reviewer |
|---|---|---|---|---|
| 0 | — | SIMPLE | Composer 2.5 | Grok 4.5 |
| 1 | A | COMPLEX | Sonnet 5 | Sonnet 5 (2nd pass) |
| 2 | B | COMPLEX | Sonnet 5 | Sonnet 5 (2nd pass) |
| 3 | C | COMPLEX | Sonnet 5 | Sonnet 5 (2nd pass) |
| 4 | D | MEDIUM | Grok 4.5 | Sonnet 5 |
| 5 | E | MEDIUM | Grok 4.5 | Sonnet 5 |
| 6 | F | COMPLEX | — | Sonnet 5 (review only) |
| 7 | F | MEDIUM | Grok 4.5 | Sonnet 5 |
| 8 | F | SIMPLE | Composer 2.5 | Grok 4.5 |

Use `Task` subagents with the model slugs from `agent_ladder` in
`.cursor/flint-loop-state.json`. Parent agent updates loop state after each
slice, exactly as done for prior milestones (see `autofill-universal-fill`
loop for the pattern).

---

## Kickoff — clean slate (Slice 0)

```bash
cd /home/alireza/Desktop/projects/Flint
git fetch origin && git checkout main && git pull origin main
git status -sb   # must be clean
git checkout -b feature/session-focus-robustness
```

Write `.cursor/flint-loop-state.json`:

```json
{
  "current_milestone": "session-focus-robustness",
  "milestone_status": "in_progress",
  "milestone_branch": { "flint": "feature/session-focus-robustness" },
  "current_slice": 0,
  "completed_slices": [],
  "current_task_id": "sfr-s0-clean-slate",
  "loop_stopped": false,
  "ci_fix_attempts": 0,
  "max_ci_fix_attempts": 6,
  "task_attempts": 0,
  "max_task_attempts": 3,
  "open_prs": {},
  "manual_gate_backlog": [],
  "agent_ladder": {
    "simple": "composer-2.5-fast",
    "medium": "cursor-grok-4.5-high-fast",
    "complex": "claude-sonnet-5-thinking-high"
  }
}
```

**Commit:** `sfr slice 0: kickoff session-focus-robustness milestone branch`

---

## Slices

### Slice 0: `sfr-s0-clean-slate` (SIMPLE)

Branch created from latest `main`, state file written. No code changes.

**Gate:** `git status -sb` clean; state file valid JSON.

---

### Slice 1: `sfr-s1-stable-shuffle` (COMPLEX)

**Goal:** Fix reordering bug — stable per-item shuffle key, safer default.

**Requirements:**

1. Add `stable_shuffle_key` to `src-tauri/src/session/shuffle.rs` (FNV-1a
   over seed + normalized question text, per Architecture §1).
2. Update `get_question_bank` in `commands.rs` to sort `pending` by
   `stable_shuffle_key` instead of the Fisher–Yates rank map.
3. Remove `shuffle_strings` (Fisher–Yates) if nothing else references it —
   grep the whole repo first; update/replace any test that exercised it.
4. Add the regression test from Architecture §1
   (`stable_shuffle_key_is_unaffected_by_other_items`).
5. Flip `readShuffleQuestionsPreference()` default in
   `src/lib/shufflePreference.ts` from `true` to `false` when unset. Update
   `tests/unit` (Vitest) for the new default.
6. Verify no other caller depends on the old Fisher–Yates output ordering
   (search `shuffle_strings`, `session_shuffle_seed` usages).

**Files:** `src-tauri/src/session/shuffle.rs`, `src-tauri/src/commands.rs`,
`src/lib/shufflePreference.ts`, associated test files.

**Gate:** `cargo test shuffle`, `cargo clippy -- -D warnings`, `npm run test -- shufflePreference`

**Commit:** `sfr slice 1: stable per-question shuffle order, safer default`

**2nd-pass review (Sonnet):** confirm removing one item from a shuffled
pending list truly cannot change survivors' relative order (re-derive the
math, don't just trust the test); confirm no leftover Fisher–Yates code path.

---

### Slice 2: `sfr-s2-focus-tag-taxonomy` (COMPLEX)

**Goal:** Canonical 8-tag taxonomy always visible in Session Focus, plus
manual tag override when adding a question.

**Requirements:**

1. New `src-tauri/src/session/focus_tags.rs` with `FOCUS_TAG_TAXONOMY`
   (8 entries per Architecture §2).
2. New `FocusTagCatalogEntryDto` in `dto.rs`.
3. New Tauri command `get_focus_tag_catalog` in `commands.rs` (keep
   `list_question_bank_tags` — do not remove, other callers may still use
   it). Register in `lib.rs` invoke handler list.
4. Add `getFocusTagCatalog` client wrapper in `src/commands/index.ts`.
5. Update `SessionFocusGate.tsx` and `Settings.tsx` `SessionFocusTab` to
   render all 8 catalog chips (muted + "0" badge when `questionCount === 0`,
   still selectable). Keep existing save/select logic — only the tag
   *source* changes from `listQuestionBankTags` to `getFocusTagCatalog`.
6. Extend `add_to_question_bank` command with optional `tags: Option<Vec<String>>`
   (validated against the taxonomy ids; invalid ids rejected with a clear
   error; `None` keeps today's heuristic-infer behavior unchanged).
7. Update `addToQuestionBank` TS wrapper signature; add an optional,
   collapsed-by-default tag picker in `QuestionBank.tsx`'s add-question row.
8. Unit tests: Rust (`focus_tags`, `get_focus_tag_catalog` counts correct),
   TS (`SessionFocusGate` renders 8 chips incl. zero-count ones; add-question
   with explicit tags).

**Files:** `src-tauri/src/session/focus_tags.rs` (new), `src-tauri/src/dto.rs`,
`src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`,
`src/commands/index.ts`, `src/screens/SessionFocusGate.tsx`,
`src/screens/Settings.tsx`, `src/components/QuestionBank.tsx`, tests.

**Gate:** `cargo test focus_tags`, `cargo clippy -- -D warnings`, `npm run test`

**Commit:** `sfr slice 2: canonical focus-tag taxonomy + manual tag override`

**2nd-pass review (Sonnet):** tag id validation can't silently accept a
typo'd tag that will never match anything; zero-count chips are clearly
distinguishable, not confusingly identical to populated ones.

---

### Slice 3: `sfr-s3-round-type-questions` (COMPLEX)

**Goal:** Explicit interview-round concept + round-appropriate supplemental
questions, additive to the JD-derived bank.

**Requirements:**

1. Migration: `SCHEMA_VERSION` 17 → 18, `ALTER TABLE sessions ADD COLUMN
   round_type TEXT NOT NULL DEFAULT ''` (additive, per `flint-data.mdc`).
2. `SessionFocus` (persistence.rs) + `SessionFocusDto` (dto.rs) gain
   `round_type: String`. Update `session_focus_to_dto` and
   `save_session_focus` mapping in `commands.rs`.
3. New `src-tauri/src/session/round_questions.rs`: `ROUND_TYPES`,
   `infer_round_type`, `supplemental_questions_for_round` per
   Architecture §3.
4. New Tauri command `infer_round_type_from_brief(recruiter_brief: String) -> Option<String>`
   so the heuristic lives in one place and the frontend doesn't duplicate it.
5. In `save_session_focus`: when incoming `round_type` is non-empty and
   differs from the previously persisted value for this session, merge
   `supplemental_questions_for_round` into the bank (dedup by
   `normalize_question_key`, additive union — never remove existing
   entries). Persist the new `round_type` regardless.
6. Unit tests: `infer_round_type` keyword coverage (recruiter screen,
   technical, hiring manager, onsite/panel, final, none-of-the-above);
   `supplemental_questions_for_round` returns correctly tagged entries;
   migration test (fresh DB lands on v18, old DB upgrades cleanly with
   `round_type = ''`); merge-into-bank is additive and dedups.

**Files:** `src-tauri/src/session/persistence.rs` (migration + struct),
`src-tauri/src/dto.rs`, `src-tauri/src/commands.rs`,
`src-tauri/src/session/round_questions.rs` (new), `src-tauri/src/lib.rs`,
tests.

**Gate:** `cargo test round_questions`, `cargo test persistence`,
`cargo clippy -- -D warnings`

**Commit:** `sfr slice 3: round-type schema + round-appropriate supplemental questions`

**2nd-pass review (Sonnet):** migration is additive-only and idempotent;
merge logic genuinely never drops or mutates existing bank entries; dedup
key matches the one used elsewhere (`normalize_question_key`) so a
supplemental question never duplicates a JD-derived one that means the
same thing but differs in casing/whitespace.

---

### Slice 4: `sfr-s4-round-type-ui` (MEDIUM)

**Goal:** Wire round type into the Session Focus gate.

**Requirements:**

1. `src/commands/index.ts`: add `inferRoundTypeFromBrief` wrapper; extend
   `SessionFocusDto` TS type with `roundType: string`.
2. `SessionFocusGate.tsx`: add a round-type `<select>` (options from
   `ROUND_TYPES` — either hardcode the same 5 pairs client-side or add a
   tiny command to list them; prefer hardcoding since they're stable UI
   copy, consistent with how `SESSION_TYPES` is hardcoded in
   `SessionDesign.tsx`). On recruiter-brief blur/debounce, call
   `inferRoundTypeFromBrief` and pre-select the dropdown if the user hasn't
   already chosen one manually; always let the user override.
3. Saving calls `saveSessionFocus` with `roundType` included — backend
   Slice 3 logic handles the supplemental merge.
4. `Settings.tsx` `SessionFocusTab`: same round-type control, consistent
   with the gate.
5. Unit tests (Vitest): dropdown pre-selects from inference; manual
   override persists; save includes `roundType`.

**Files:** `src/commands/index.ts`, `src/screens/SessionFocusGate.tsx`,
`src/screens/Settings.tsx`, tests.

**Gate:** `npm run test`, `npx tsc --noEmit` (or repo's equivalent Rust/TS type-check step)

**Commit:** `sfr slice 4: round-type selector wired into Session Focus`

---

### Slice 5: `sfr-s5-preferred-answer-discoverability` (MEDIUM)

**Goal:** Fix bug 4 — tailoring panel visible by default, Visual button
disambiguated.

**Requirements:**

1. `Rehearsal.tsx`: `PreferredAnswerPanel` usage drops `defaultCollapsed`
   (or passes `defaultCollapsed={false}`) so it starts expanded on first
   response.
2. `VisualPanel.tsx`: rename button label + `title` from "Use This Answer" /
   "Copy the full visual answer to your clipboard" to "Copy Diagram Text" /
   an equivalently clarified title. Update the `copied` state label
   ("Copied!") — keep as-is, just the idle label changes.
3. Update any snapshot/unit tests referencing the old button text
   (`VisualPanel.test.tsx` if present) and `Rehearsal.test.tsx` collapsed-
   panel expectations.

**Files:** `src/screens/Rehearsal.tsx`, `src/panels/VisualPanel.tsx`,
associated test files.

**Gate:** `npm run test`

**Commit:** `sfr slice 5: expand preferred-answer panel by default, disambiguate Visual copy button`

---

### Slice 6: `sfr-s6-review` (COMPLEX — review only)

Sonnet 5 reviews `git diff origin/main...HEAD`:

- Migration is additive-only, no drops/renames, `round_type` defaults
  correctly for pre-existing rows
- Stable shuffle math actually holds (removing an item cannot reorder
  survivors) — re-derive, don't just trust green tests
- Focus-tag catalog validation rejects unknown tag ids on manual add
- Supplemental-question merge never overwrites/removes existing bank
  entries; dedup works across casing/whitespace variants
- Live session behavior unchanged (still full bank, unfiltered)
- No session content logged above DEBUG
- No drive-by refactors outside the five named files/areas per slice

Output: blocking vs non-blocking findings list with file paths and verdict
(`APPROVE` / `REQUEST_CHANGES`).

---

### Slice 7: `sfr-s7-fix-findings` (MEDIUM)

Fix all blocking findings from Slice 6; one re-check, not a new full review.

**Commit:** `sfr slice 7: fix review findings`

---

### Slice 8: `sfr-s8-pr-ci-merge` (SIMPLE)

1. `git push -u origin HEAD`
2. Open one PR to `main` with milestone summary + test plan (list all four
   bugs fixed, one line each).
3. Poll `gh pr checks` until green (fix, commit, push — increment
   `ci_fix_attempts`, cap at 6).
4. `gh pr merge` when green.
5. Set `milestone_status: "ci_green"`, `loop_stopped: true`.

---

## Stop conditions

Same as `SKILL.md`, plus:

- Any diff that changes live-session (unfiltered) behavior is an immediate
  hard stop, regardless of slice.
- Any migration that is not additive-only (drops/renames a column) is an
  immediate hard stop.

## Done when

1. PR merged, CI green.
2. Regression test proves shuffle stability (bug 1).
3. Session Focus shows all 8 canonical tags with live counts (bug 2).
4. A recruiter-screen round type yields self-assessment/motivation/
   logistics/general questions in the bank without requiring the JD to
   mention them (bug 3).
5. Preferred-answer tailoring panel is visible without extra clicks on the
   first response; Visual's clipboard button no longer reads "Use This
   Answer" (bug 4).
6. Live sessions still always use the full, unfiltered bank.

## Resume commands

```
/flint-loop session-focus-robustness start
/flint-loop resume
/flint-loop status
/flint-loop stop
```

## End-of-milestone deliverable

1. Branch name, PR URL, merge confirmation
2. `git diff main --stat` summary
3. Slices completed vs deferred
4. One line per original bug confirming the fix, referencing the
   regression test or manual check that proves it
5. **"Ready for manual live session test"** or **"Blocked on …"**
