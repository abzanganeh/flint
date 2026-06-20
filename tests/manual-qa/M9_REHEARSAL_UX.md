# M9 — Rehearsal UX, Session Focus, Preferred Answers

Manual QA for rehearsal polish, session focus filtering, preferred-answer matching,
and window/display controls.

## Prerequisites

- `npm run tauri dev` (Vite at `http://127.0.0.1:1420`)
- Session past digest review with question bank populated
- At least one LLM provider configured

---

## 1. Window resize grip

| Step | Expected |
|------|----------|
| Open any Flint screen | Small **triangle grip** visible at bottom-right corner |
| Hover grip | Cursor becomes diagonal resize; grip brightens |
| Drag grip | Frameless window resizes from bottom-right |

---

## 2. Display zoom (Settings → Account)

| Step | Expected |
|------|----------|
| Settings → Account → **Display zoom** | Slider 85%–130%, default 100% |
| Move slider to 115% | All UI scales up immediately |
| Move slider to 90% | All UI scales down |
| **Reset to 100%** | Returns to normal size |
| Restart app | Zoom preference persists |

---

## 3. Settings back navigation

| Step | Expected |
|------|----------|
| Rehearsal → Settings (Session Focus tab) | Opens settings |
| Click Settings in title bar again | Stays on settings; return target unchanged |
| **← Back** | Returns to **Rehearsal**, not session design |

---

## 4. Rehearsal Ask button label

| Step | Expected |
|------|----------|
| Ask first question | Button reads **Ask** |
| After response, same question in box | Button reads **Ask again** |
| Change to different question in box | Button reads **Ask** (not Ask again) |

---

## 5. Session focus tags

| Step | Expected |
|------|----------|
| After digest → Session focus gate | Tag chips visible; select at least one to continue |
| Settings → Session Focus | Same chips; selected tags show summary text |
| Select `behavioral` + `competency` | Question bank filters to matching tags (OR) |
| **Tell me about yourself** | Tagged `self-assessment` — include that tag to see it in bank |
| Notes field | Free text only — not where tags live |

Competency-based phone screens (e.g. Fisher/iCIMS) map best to **`competency` + `behavioral` + `culture`**, not behavioral alone.

---

## 6. Preferred answers

| Step | Expected |
|------|----------|
| Rehearsal → ask question → tailor → **Save as preferred answer** | Success toast; bank shows **Live** badge |
| Re-ask **exact same** wording | Directional panel serves saved script (no new LLM draft) |
| Re-ask with `?` or `Can you …` prefix | Still hits preferred (normalized key match) |
| Re-ask paraphrase (e.g. walk me through background vs tell me about yourself) | Hits preferred if cosine ≥ **0.85** (requires save after v15 embedding) |
| Mock Study mode, same question | Suggested answer matches preferred |
| Live session, same question | Instant preferred script |

**Note:** Preferred answers saved before schema v15 need a one-time **re-save** to store question embeddings for semantic matching.

---

## 7. Dev startup (Linux)

| Check | Expected |
|-------|----------|
| `npm run tauri dev` | No `Failed to open session persistence DB` (schema v15) |
| WebView loads | No `Could not connect to localhost` (dev URL uses `127.0.0.1:1420`) |

---

## Pass criteria

- [ ] Resize grip works on frameless window
- [ ] Zoom slider persists and scales UI
- [ ] Settings back returns to origin screen
- [ ] Ask / Ask again label correct per question
- [ ] Focus tags filter rehearsal bank
- [ ] Preferred answer hits on exact + minor rephrase
- [ ] App starts cleanly on Linux dev
