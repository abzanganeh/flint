# M11 — Mock Coach Quality

## Context

Mock coach scores cluster at 40–50 with generic `too_hesitant` feedback. Session Design
already collects company overview, leadership principles, and role expectations (plus Smart
Resume company intel), but mock coach and suggested-answer prompts only receive `{rag_chunks}`
from vector retrieval — not the structured employer context.

## Fix Plan — 7 Slices

### Slice 1: Company context in mock prompts

**Goal:** Wire `SessionContextFields` company sections into mock coach + suggested prompts.

- Add `mock/context.rs` — `format_company_context_for_prompt()`
- Pass `{company_context}` into `mock_coach` and `mock_suggested` prompts
- Load context from persistence in `commands.rs` (coach) and `conductor.rs` (suggested)

**Gate:** `cargo test`, `cargo clippy -- -D warnings`, `npm run test`

---

### Slice 2: Multi-axis coach schema

**Goal:** Replace single score + `too_hesitant` with structured rubric axes.

- Extend `CoachFeedback` with axes: content, specificity, company_alignment, delivery
- Update coach prompt JSON schema and frontend Coach panel rendering
- Keep backward compat for persisted `coach_json` rows

---

### Slice 3: Smarter echo guardrail

**Goal:** Reduce false positives when user shares IAM/domain vocabulary with suggested script.

- Tokenize with stop-word list; ignore domain terms from session context
- Raise threshold or use bigram overlap for short answers
- Only cap score when overlap is clearly scripted reading

---

### Slice 4: Speaking style preference

**Goal:** Session setup captures natural vs polished voice; coach and suggested honor it.

- Add `speaking_style` field to session context (UI + persistence)
- Inject into mock prompts as `{speaking_style}`

---

### Slice 5: Session vocabulary → Whisper

**Goal:** User-editable per-session terms improve STT for domain acronyms.

- Session Design field → Whisper initial prompt / domain vocab path

---

### Slice 6: Suggested answer quality v2

**Goal:** Stronger suggested answers tied to leadership principles and role expectations.

- Prompt rules: cite LP by name, inverted pyramid, no invented metrics

---

### Slice 7: Coach prompt v2

**Goal:** Sophisticated judgment — not generic hesitation flags.

- Rubric examples, company-specific gap detection, voice preference

---

## Manual gate (post-merge)

Run mock session with Fisher Investments / IAM Architect prep — verify coach references
leadership principles and scores vary by answer quality.
