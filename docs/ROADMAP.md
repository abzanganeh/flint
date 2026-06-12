# Flint — Implementation Roadmap

> Source of truth: `docs/flint_system_design_v3.md`  
> Rules: `.cursor/rules/`  
> Approach: **Vertical slices only.** Each phase produces something runnable and testable. Never build a full layer horizontally first.

---

## Current Status (audited 2026-06-11 — `feature/mock-interview`)

> **You are here:** Phase 8 Mock Interview code complete on `feature/mock-interview` (not merged to `main`). Strategy B Phase 1 signed off; Phase 2 code complete — pending manual device test + Chrome Web Store submission. **Next:** merge mock-interview PR → manual Phase 8 gate → Strategy B Phase 3 (SSO + credit ledger).

| Phase | Code on `main` | Review gate | Notes |
|-------|----------------|-------------|-------|
| 0–2 | ✅ Complete | ✅ Passed | — |
| 3 Audio | ✅ Complete | ⏳ Open (`*` device tests) | Hardest remaining manual validation |
| 4 Orchestrator | ✅ Complete | ⏳ Open (NFR benchmarks) | Debounce fixed at 600ms (spec allows 600–1200ms) |
| 5 Five panels | ✅ Complete | ⏳ Partial | Merged via `feature/phase5-ui-panels`; hotkeys/OBS manual |
| 5.5 v1.5 | ✅ Complete | ⏳ Open | Merged via PR #11 — question bank, checklist, research chat, settings, stack layout |
| 6 Post-session | ✅ Complete | ⏳ Open | Summary screen + `get_digest` fallback added in this branch |
| 7 Hardening | 🔄 Partial | ⏳ Open | eval baseline committed; 7.1 coverage + 7.8 installers + NFR run still open |
| **8 Mock Interview** | ⏳ Branch only | ⏳ Open | `feature/mock-interview` @ `3c2aea3` — TTS + coach + mic capture; manual E2E pending |
| Strategy B Ph1 | ✅ Code + manual | ✅ Signed off | 1.A 2026-06-09; 1.B 2026-06-10 |
| Strategy B Ph2 | ✅ On `main` (ext) | ⏳ Open | API gate passed 2026-06-11; Chrome manual + Store submission pending |

### Cross-cutting gaps (not phase-complete)

| Gap | Blocks |
|-----|--------|
| **`get_digest` SQLite fallback** | ✅ Fixed in this branch — loads from SQLite on restart |
| **Post-session summary screen** | ✅ Fixed in this branch — `SessionSummary.tsx`, routes after `ENDED` |
| **Settings: cost cap + GDPR** | ✅ Fixed in this branch — `Settings.tsx` with API Keys / Usage Cap / Privacy tabs |
| **Eval baseline** | ✅ Fixed in this branch — `evals/baseline.json` committed (stub, zero-question run) |
| **Strategy B Phase 1.B** (cold-start deep link) | ✅ Code complete — tests added; manual `flint://` on-device test still open |
| **Cost cap UI** — indicator uses hardcoded 50k tokens | Settings Usage Cap tab now configurable |
| **Strategy B Phase 1.5** (installers) + **Phase 3** (billing/SSO) | Phase 1 sign-off; extension beta; credit display |

### What to implement next

1. **Merge `feature/mock-interview`** → Phase 8 code on `main`; run Phase 8 manual gate (checklist below)
2. **Phase 8 follow-ups** (post-merge) — dynamic follow-up questions, mock summary/replay UI, Piper/ElevenLabs TTS
3. **Merge `feature/extension-mvp`** (flint-extension) + smart-resume test move → Phase 2 code-complete
4. **Manual Phase 2 gate** — load extension in Chrome, LinkedIn job → Save JD → Open in Flint → verify pre-fill; then submit to Chrome Web Store
5. **Strategy B Phase 3** — Supabase SSO migration + unified credit ledger (5–7 weeks)
6. Close Phase 3/4/5 manual review gates in parallel
7. **7.8** signed installers before extension public beta

Companion integration track: `docs/STRATEGY_B_INTEGRATION_PLAN.md`

---

## How to Use This Roadmap

1. Work one phase at a time. Do not start Phase N+1 until Phase N's review gate passes.
2. Each task specifies which Cursor agent or mode to use.
3. **Review gates** are mandatory before moving to the next phase.
4. Keep `docs/flint_system_design_v3.md` open in Cursor and `@`-reference the relevant section for each task.
5. Write the interface/test stub yourself first, then ask Cursor to implement it.

---

## Agent Guide

| Agent / Mode | Best for |
|---|---|
| **Cursor Agent (Claude Sonnet 4.6)** | All standard code generation tasks — implement a trait, write a component, add an endpoint |
| **Cursor Agent (Claude Opus 4.7 Thinking)** | Complex architectural decisions, orchestrator design, session state machine, LLM failover logic, confidence scoring algorithm |
| **`explore` Task agent** | Navigating unfamiliar codebase sections, understanding cross-cutting concerns, searching for patterns across files |
| **`generalPurpose` Task agent** | Research tasks (e.g. "how does cpal handle PipeWire on Linux", "fastembed-rs bge-small-en-v1.5 API") |
| **`shell` Task agent** | Running builds in parallel while you work, CI checks, `cargo test`, scaffolding |
| **`best-of-n-runner` Task agent** | Critical algorithms where correctness matters most: VAD chunking, confidence scoring formula, question detection, MMR de-dup, session state machine transitions |
| **`browser-use` Task agent** | UI testing — testing the overlay at different resolutions, stealth mode self-test, hotkey behaviour |

### When to Use Opus Thinking
Use `claude-opus-4-7-thinking-xhigh` for:
- Designing the `session/state.rs` state machine (Phase 0)
- Designing the orchestrator thread management (Phase 4)
- The failover logic decision tree (Phase 4)
- Debugging any silent failure in audio or IPC (Phase 3, Phase 4)
- The confidence scoring implementation (Phase 4)

### When to Review Every Line Yourself
- **Audio pipeline** (`src-tauri/src/audio/`, `src-tauri/src/transcription/`) — AI fails silently here
- **Tauri IPC layer** (`commands.rs`, `events.rs`) — type mismatches surface only at runtime
- **State machine transitions** (`session/state.rs`) — invalid transitions corrupt session state
- **Security-sensitive code** (`keychain.rs`, `supabase/auth.rs`) — no shortcuts

---

## Phase 0 — Project Skeleton

**Goal:** Compiles, CI passes, folder structure matches the design doc. Nothing works yet.  
**Duration:** 1–2 days

### Tasks

| # | Task | Agent | Review? | Status |
|---|---|---|---|---|
| 0.1 | `cargo tauri init` — Tauri 2.x project. Choose React + TypeScript frontend. | Shell agent | Check `tauri.conf.json` window config (always_on_top, transparent, decorations, skip_taskbar) | [x] Complete |
| 0.2 | Create full folder structure from `flint-core.mdc` | Cursor Agent (Sonnet) — give it the folder tree from the rules | Verify every directory exists | [x] Complete |
| 0.3 | `Cargo.toml`: add all Rust dependencies — tokio, anyhow, thiserror, tracing, rusqlite, serde, uuid, secrecy, async-trait | Cursor Agent — ask it to add deps one module at a time | Check versions are current | [x] Complete |
| 0.4 | `package.json`: add React 18, Tailwind CSS, Zustand, Vitest, ESLint, Prettier | Cursor Agent | Check TypeScript strict mode in `tsconfig.json` | [x] Complete |
| 0.5 | Create `src/types/index.ts` — all shared TypeScript types (SessionState enum, PanelId, etc.) | Cursor Agent — reference design doc Section 25 and Section 28 | Read every type definition | [x] Complete |
| 0.6 | Stub `commands.rs` and `events.rs` with empty functions — just the signatures | Cursor Agent — reference Section 9 of rules | Check all command names match exactly | [x] Complete |
| 0.7 | GitHub Actions CI skeleton: `cargo fmt --check`, `cargo clippy`, `cargo test`, `vitest run` | Cursor Agent | Run CI locally first | [x] Complete |
| 0.8 | Create Supabase project. Enable GoTrue auth. Set up local dev environment. | Manual | Confirm local Supabase CLI working | [x] Complete — see `docs/supabase-setup.md` |
| 0.9 | First migration: `supabase/migrations/YYYYMMDDHHMMSS_initial_schema.sql` — all 8 tables with RLS enabled | Cursor Agent — reference design doc Section 16 | Review every RLS policy | [x] Complete |
| 0.10 | Stub all five panels as empty React components. App renders without errors. | Cursor Agent | — | [x] Complete |

### Phase 0 Review Gate
- [x] `cargo build` succeeds with zero warnings
- [x] `cargo clippy -- -D warnings` passes
- [x] `vitest run` passes
- [ ] CI passes on first push
- [x] All folder structure matches `.cursor/rules/flint-core.mdc`
- [x] Supabase migrations run locally without errors

---

## Phase 1 — Auth + Onboarding

**Goal:** A real user can sign up, log in, see the legal consent screen, complete hardware tier assessment, and have their credentials stored in the OS keychain. No AI yet.  
**Duration:** 2–3 days  
**Status:** ✅ Complete (audited & verified — all 16 Rust tests + 5 frontend tests pass, `cargo clippy -- -D warnings` clean)

### Tasks

| # | Task | Agent | Review? |
|---|---|---|---|
| ~~1.1~~ | ~~Implement `AuthInterface` trait in `interfaces/auth.rs`~~ | ✅ Done | `#[async_trait]`, `SecretString` for tokens, `User`/`Plan`/`AuthToken` structs verified |
| ~~1.2~~ | ~~Implement Supabase GoTrue auth in `supabase/auth.rs` (signup, login, logout, refresh)~~ | ✅ Done | 10s timeout, config-driven URL/key, error mapping (400/429/5xx), no secrets logged |
| ~~1.3~~ | ~~Implement `keychain.rs` — OS keychain read/write using the `keyring` crate~~ | ✅ Done | Service `"flint"`, `SecretString` throughout, user-facing error messages, legal-consent helpers |
| ~~1.4~~ | ~~`hardware.rs` — Tier 1–4 hardware assessment (CPU cores, RAM, GPU presence)~~ | ✅ Done | `sysinfo` + OS-specific GPU detection, `calculate_tier` / `calculate_tier_detailed`, logged at startup |
| ~~1.5~~ | ~~`health/checks.rs` — installation health check (audio devices, Whisper model file, Ollama availability)~~ | ✅ Done | 12 checks, Ollama 2s timeout, Supabase 5s timeout, SQLite R/W/D round-trip, X11 Fail |
| ~~1.6~~ | ~~Tauri commands for auth: `login`, `logout`, `get_current_user`~~ | ✅ Done | All commands registered in `lib.rs`; DTOs free of `SecretString`; `map_user_error` guards all paths |
| ~~1.7~~ | ~~`screens/Onboarding.tsx` — signup/login form, legal consent screen, Supabase auth integration~~ | ✅ Done | Legal gate un-bypassable; `disabled={!consentChecked \|\| submitting}`; acceptance in keychain; no `invoke()` directly |
| ~~1.8~~ | ~~`screens/HealthCheck.tsx` — hardware tier display, health check wizard, BlackHole/PipeWire guidance~~ | ✅ Done | Loading spinner; Pass/Warn/Fail icons; expandable warn fix; "Start anyway" disabled on any Fail; X11 hard-red banner |
| ~~1.9~~ | ~~Store auth token securely in keychain after login. Clear on logout.~~ | ✅ Done | `persist_auth_token` on login; `restore_auth_from_keychain` + refresh on startup; `clear_auth_token` on logout |
| ~~1.10~~ | ~~Unit tests: `AuthInterface` mock implementation, keychain read/write round-trip~~ | ✅ Done | `MockAuth` (4 async tests), keychain round-trip, hardware tier boundaries, auth-session expiry |

### Phase 1 Review Gate
- [ ] Signup → login → logout flow works end-to-end *(manual verification on device required)*
- [x] Auth token in OS keychain, not on disk or in memory as `String` — `SecretString` enforced; `persist_auth_token` + `clear_auth_token` verified
- [x] Legal consent cannot be bypassed — `disabled={!consentChecked || submitting}` + Vitest test confirms button state
- [ ] Health check correctly identifies hardware tier on your machine *(manual verification on device required)*
- [x] All unit tests pass for auth module — 16 Rust tests + 5 TypeScript tests, all green

---

## Phase 2 — Session Design + RAG

**Goal:** A user can create a session, paste context, trigger embedding + storage, review the digest, and pre-warm responses. This is the knowledge foundation everything else depends on.  
**Duration:** 3–4 days

> **v1.5 extension:** Session Design now uses structured context fields (see §10 Module 3). The ingest pipeline combines all fields into one RAG blob with section labels. SQLite stores each field as a separate column. Draft sessions persist and restore across restarts.

### Tasks

| # | Task | Agent | Review? | Status |
|---|---|---|---|---|
| 2.1 | `session/state.rs` — full state machine with all valid and invalid transitions | **Cursor Agent (Opus Thinking)** — this is the most critical Rust file in the project | Read every transition. Write tests first. | [x] Complete |
| 2.2 | State machine unit tests — 100% coverage: all valid transitions + all invalid transitions rejected | Write test stubs yourself, then `best-of-n-runner` (3 attempts) | Run `cargo test` — must be 100% | [x] Complete — 43 tests (19 valid + 6 named invalid + 3 extra invalid + 2 SQLite + 2 recovery + 2 ENDED divergence + 9 smoke) |
| 2.3 | `rag/embedder.rs` — fastembed-rs integration with `bge-small-en-v1.5` | Cursor Agent (Sonnet) — research with `generalPurpose` agent first if unfamiliar with fastembed-rs API | Test embedding dimensions are correct | [x] Complete — 384-dim verified |
| 2.4 | `rag/store.rs` — sqlite-vec `VectorInterface` implementation (ingest, query, delete_session) | Cursor Agent — write `VectorInterface` trait first, then implement | Test session isolation with two sessions | [x] Complete — `vec_chunks_{hex}` per-session, WAL enforced |
| 2.5 | `rag/retriever.rs` — dot product similarity, top-8-10 chunks, MMR de-duplication | **`best-of-n-runner`** (3 attempts) — correctness matters here | Verify MMR actually removes near-duplicates | [x] Complete — real dot-product inter-chunk sim + 0.99 hard threshold |
| 2.6 | `digest.rs` — extract top entities and top 5 likely questions from pasted context | Cursor Agent (Sonnet) — give it the prompt from `/prompts/digest/` | Test with a real job description | [x] Complete — prompts loaded from `/prompts/digest/{provider}.txt`, no inline strings |
| 2.7 | `session/persistence.rs` — SQLite write-through: write state on every transition, every transcript chunk | Cursor Agent — check WAL mode is set | Simulate crash — verify recovery data | [x] Complete — WAL verified, crash-survival test passes |
| 2.8 | Pre-warm logic in `orchestrator/prewarm.rs` — fire top-5 questions before session starts | Cursor Agent — reference the question bank pre-warm priority order from `flint-data.mdc` | Verify cache entries exist in sqlite-vec | [x] Complete — all 10 LLM calls spawned concurrently; one merged entry per question |
| 2.9 | Tauri commands: `create_session`, `ingest_context`, `confirm_digest`, `get_digest` | Cursor Agent | Check state transitions fire correctly | [x] Complete — strict ownership + state-machine preconditions |
| 2.10 | `screens/SessionDesign.tsx` and `screens/DigestReview.tsx` — UI for context paste, spinner during ingestion, digest display | Cursor Agent (Sonnet) | Test with a 500-word job description | [x] Complete — fully event-driven, no `any` types |
| 2.11 | Integration test: paste JD text → embed → store → query → assert top chunk relevance | `best-of-n-runner` | — | [x] Complete — end-to-end test asserts score > 0.5 and zero near-duplicates survive MMR |

### Phase 2 Review Gate
- [x] State machine: `cargo test` 100% on state machine module
- [x] Paste 500-word JD → digest generated → top 5 questions extracted correctly
- [x] RAG query returns semantically relevant chunks (integration test: top score > 0.5)
- [x] MMR de-duplication removes obvious near-duplicates (integration test: 0 pairs ≥ 0.99 cosine)
- [x] Pre-warm cache populated before session starts (one entry per question, both responses merged)
- [x] Crash simulation: kill process mid-ingest → restart → data intact in SQLite
- [x] Session isolation: two sessions do not share vectors

### Phase 2 Audit — File-by-File Verification

Comprehensive audit run on 2026-05-27 against `.cursor/rules` and `docs/flint_system_design_v3.md`.

| File | Audit Focus | Verdict | Notes |
|---|---|---|---|
| `src-tauri/src/session/state.rs` | 13 states, hard-error invalid transitions, write-through SQLite, tracing | ✅ Pass | `SessionStateMachine` is the sole mutator; persists before in-memory commit; rolls back on persistence failure |
| `src-tauri/src/session/persistence.rs` | WAL mode, write-through, crash recovery, `StatePersister` impl | ✅ Pass | WAL verified at startup + in tests; crash-simulation test confirms data survives connection drop |
| `src-tauri/src/rag/embedder.rs` | `fastembed-rs` + `bge-small-en-v1.5`, 384 dims, single-instance | ✅ Pass | `Mutex<TextEmbedding>` for `Send + Sync`; tests assert dims == 384 |
| `src-tauri/src/interfaces/vector.rs` | `#[async_trait] VectorInterface` contract | ✅ Pass | `Chunk` / `ScoredChunk` documented; session isolation contractually enforced |
| `src-tauri/src/rag/store.rs` | `vec_chunks_{hex}` virtual tables, WAL, embeddings BLOB storage | ✅ Pass | `simple()` UUID hex naming; cosine-from-L2 score; embeddings round-trip for retriever |
| `src-tauri/src/rag/retriever.rs` | 2×top_k candidates, λ=0.7 MMR, hard 0.99 dedup threshold | ✅ Pass — fixed | Stale doc comment corrected; algorithm verified end-to-end |
| `src-tauri/src/digest.rs` | Prompts loaded from `/prompts/digest/{provider}.txt`, JSON validation, universal question bank fallback | ✅ Pass | Zero inline prompts; raw response logged on parse failure; pad to 5 questions |
| `src-tauri/src/orchestrator/prewarm.rs` | 10 fully-concurrent `tokio::spawn` LLM calls, ≥ 0.85 cache threshold, 10-min staleness | ✅ Pass — fixed | Eliminated directional/depth race that produced duplicate cache entries; `embed_batch` now via `spawn_blocking`; `join_all` for collection |
| `src-tauri/src/commands.rs` | Session-ID ownership + state-machine preconditions, event emission, no raw Rust errors | ✅ Pass — fixed | `confirm_digest` now awaits `run_prewarm` directly (no `spawn_blocking` + `block_on` dance) |
| `src-tauri/src/state.rs` | `AppState` wires persistence as `StatePersister`, embedder/vector_store/llm singletons | ✅ Pass | All Phase 2 dependencies live behind `Arc`; auth interop preserved |
| `src-tauri/src/dto.rs` | `SessionConfigDto`, `DigestDto`, `SessionSnapshotDto` with `From` impls | ✅ Pass | Strict serde, no untyped maps |
| `src-tauri/tests/integration/rag_pipeline.rs` | Chunk → embed → ingest → MMR query → cleanup → isolation | ✅ Pass | Asserts top score > 0.5 and 0 pairs ≥ 0.99 cosine after MMR |
| `prompts/digest/default.txt`, `prompts/directional/default.txt`, `prompts/depth/default.txt` | External prompt files | ✅ Pass | All placeholders (`{pasted_context}`, `{question}`, `{role}`, `{domain}`, `{key_skills}`) honoured by loaders |
| `src/commands/index.ts` | Single typed bridge for `createSession`, `ingestContext`, `confirmDigest`, `getDigest`, `getSessionSnapshot` | ✅ Pass | All components route through this layer; no direct `invoke` in screens |
| `src/screens/SessionDesign.tsx` | UI driven solely by `session_state_change` events | ✅ Pass | Navigation triggered by `DIGEST_REVIEW` event, not command result |
| `src/screens/DigestReview.tsx` | Event-driven pre-warm progress, inline-editable digest | ✅ Pass | `REHEARSING` event drives `onComplete`; no `any` types |

#### Hard Constraints
- ✅ Zero `println!` / `print!` / `eprintln!` in `src-tauri/src` or `src-tauri/tests` (grep verified)
- ✅ `cargo build` and `cargo clippy --all-targets -- -D warnings` pass cleanly with 0 warnings
- ✅ `cargo test` — 119 unit + 3 integration = 122 tests, 0 failures
- ✅ `npx tsc --noEmit` clean, `npx vitest run` 5/5 passing
- ✅ Zero `any` types in `src/` (grep verified)

#### Fixes Applied During Audit
1. **Pre-warm duplicate-entry race** (`orchestrator/prewarm.rs`) — directional and depth tasks were each inserting separate entries, leading to up to 10 cache entries for 5 questions with some half-populated. Refactored to use a per-question coordinator that `tokio::join!`s the two spawned tasks and inserts exactly one merged entry. Added regression test `test_run_prewarm_one_entry_per_question_with_both_fields`.
2. **Embedder blocking call now off-runtime** (`orchestrator/prewarm.rs`) — `embed_batch` was being called synchronously inside the async `run_prewarm` function, forcing callers to wrap with `spawn_blocking` + `block_on`. Moved the `spawn_blocking` inside `run_prewarm` and updated the signature to take `Arc<Embedder>`. `confirm_digest` is now a clean `await`.
3. **Stale doc comments** (`rag/retriever.rs`, `orchestrator/prewarm.rs`) — corrected references to the old geometric-mean approximation and the imprecise `tokio::join!` description.
4. **Redundant test assertion** (`orchestrator/prewarm.rs`) — removed a duplicate `!c.is_empty()` check in `test_run_prewarm_populates_cache`.

---

## Phase 3 — Audio + Transcription

**Goal:** Real-time audio capture from both channels, noise suppression, VAD chunking, Whisper transcription, transcript panel rendering. This is the hardest platform-specific work.  
**Duration:** 3–4 days  
**Status:** ✅ Code complete — pending on-device hardware validation (Review Gate items marked with `*`)

> **Caution:** Review every line of audio pipeline code yourself. AI fails silently here.

### Tasks

| # | Task | Agent | Review? | Status |
|---|---|---|---|---|
| ~~3.1~~ | ~~Research: cpal API for loopback capture on Linux PipeWire~~ | ✅ Done | Read the output carefully before implementing | [x] Complete |
| ~~3.2~~ | ~~`audio/capture.rs` — cpal audio capture, dual-channel (system loopback + mic)~~ | ✅ Done | **You review every line.** Test both channels independently. | [x] Complete — 48kHz capture, ring buffer, gap recovery, `!Send` OS-thread pattern |
| ~~3.3~~ | ~~`audio/rnnoise.rs` — RNNoise preprocessing. Frame size 480 samples. < 5ms per frame.~~ | ✅ Done | Test with noisy audio sample | [x] Complete — `nnnoiseless`, 480-sample frames at 48kHz; Downsampler 480→160 via `rubato` |
| ~~3.4~~ | ~~`audio/vad.rs` — WebRTC VAD, mode 3, all parameters from `flint-audio.mdc`. Produces tagged chunks.~~ | ✅ Done | Test: 200ms speech minimum, 600ms silence gap | [x] Complete — 43 unit tests; energy gate + WebRTC VAD mode 3; all §26 parameters exact |
| ~~3.5~~ | ~~`transcription/engine.rs` — whisper-rs integration, all Whisper params from `flint-audio.mdc`~~ | ✅ Done | Test with a 30-second audio sample | [x] Complete — beam=5, per-segment silence filter, `spawn_blocking`, hardware-tier model selection |
| ~~3.6~~ | ~~`transcription/detector.rs` — two-pass question detection: rule-based patterns first, Ollama 1B classifier if ambiguous~~ | ✅ Done | Test: 100ms P95 target. Log detection latency. | [x] Complete — Pass 1 regex + Pass 2 Ollama; P95 window 20 samples; auto-bypass > 200ms rolling |
| ~~3.7~~ | ~~Audio pipeline integration: cpal → RNNoise → VAD → Whisper → emit `transcription_chunk` event~~ | ✅ Done | **You review every line of the pipeline assembly.** | [x] Complete — `run_audio_pipeline` in `audio/pipeline.rs`; parallel system + mic `ChannelProcessor`s |
| ~~3.8~~ | ~~Ring buffer management: 16KB per channel. Never flush to disk. Clear on session end.~~ | ✅ Done | Verify zero bytes written to disk | [x] Complete — `RingBuffer<f32>` in `capture.rs`; `stop()` zeros both channels; zero disk writes |
| ~~3.9~~ | ~~Audio gap recovery: detect cpal stream drop → reinitialise within 5s → insert `[audio gap - Ns]` marker~~ | ✅ Done | Simulate cpal drop — verify recovery | [x] Complete — atomic error flag; 5-attempt backoff; `[audio gap - Ns]` marker emitted via Tauri event |
| ~~3.10~~ | ~~`panels/TranscriptPanel.tsx` — real-time transcript rendering, System vs Microphone colour coding~~ | ✅ Done | Test with fast token stream | [x] Complete — per-instance ID counter, `behavior: "instant"` scroll, listener-leak guard |
| ~~3.11~~ | ~~Tauri commands: `start_session`, `stop_session`. Events: `transcription_chunk`.~~ | ✅ Done | **You review every IPC type.** | [x] Complete — zeroing ack (`zeroed_rx`), startup rollback on transition failure, `abort_live_tasks` |
| ~~3.12~~ | ~~Integration test: mock audio file → full pipeline → assert transcript text output~~ | ✅ Done | — | [x] Complete — Section A (RNNoise+DS chain), Section B (QuestionDetector), Section C (Whisper, `#[ignore]`) |

### Phase 3 Review Gate
- [ ] `*` Dual-channel audio capture working on Linux (your dev machine) — requires device test
- [ ] `*` RNNoise: < 5ms per frame (log timing) — requires device test
- [ ] `*` VAD: correctly splits at silence boundaries (200ms min speech, 600ms silence gap) — unit tests pass; device verification pending
- [ ] `*` Whisper: transcribes a 30-second sample with < 2s lag — requires cmake + model + device test
- [ ] `*` Question detection: rule-based fires < 100ms; Ollama classifier fallback fires correctly — unit tests pass; Ollama device test pending
- [ ] `*` Zero bytes of audio written to disk (verify with `strace` or equivalent) — code verified (no disk writes in ring buffer); `strace` run pending
- [ ] `*` Audio gap recovery: kill/restart cpal stream, transcript shows gap marker — requires device test
- [ ] `*` Transcript panel renders System vs Microphone tagged chunks with correct colour — requires device test

#### What can be verified now (without device)
- [x] All §26 VAD parameters exact — unit tests enforce 200ms min / 600ms silence gap / mode 3
- [x] Ring buffer zeroed on session end — `zeroed_rx` handshake confirmed before ENDED state
- [x] No inline prompts — `prompts/question_detection/llama.txt` loaded from disk
- [x] `cpal::Stream !Send` — OS-thread pattern; AppState holds only `Send` types
- [x] State machine transitions respected — `start_session` / `stop_session` unit-tested
- [x] Listener-leak guard in `TranscriptPanel` — cancelled flag pattern
- [x] `cargo clippy -- -D warnings` clean (pending cmake availability for full build)

---

## Phase 4 — Orchestrator + LLM Threads

**Goal:** Parallel AI response threads, token streaming to UI, provider abstraction, failover logic, confidence scoring. This is the core engine.  
**Duration:** 3–4 days  
**Status:** ✅ Code complete on `main` (merged via `feature/orchestrator-llm-threads`, `fix/phase4-review-findings`) — NFR review gate open

### Tasks

| # | Task | Agent | Review? | Status |
|---|---|---|---|---|
| 4.1 | `llm/provider.rs` — `LLMProvider` trait definition (from `flint-rust.mdc` Section 27) | Write the trait yourself — it's the contract everything else depends on | — | [x] Complete |
| 4.2 | `llm/groq.rs` — Groq streaming implementation | Cursor Agent (Sonnet) | Test streaming with a real question | [x] Complete |
| 4.3 | `llm/ollama.rs` — Ollama local implementation | Cursor Agent (Sonnet) | Test with llama3.2:1b model | [x] Complete |
| 4.4 | Token-bucket rate limiter for providers (80% of free-tier limits) | **`best-of-n-runner`** (3 attempts) — correctness matters | Test 429 → Retry-After honoured → no immediate failover | [x] Complete — `rate_limiter.rs` + unit tests |
| 4.5 | Failover logic: `network_failure → retry → Ollama → ping_primary_every_30s → primary_restored` | **Cursor Agent (Opus Thinking)** — complex decision tree | Test: mock 500 → assert Ollama fires → assert `failover_triggered` event emitted | [x] Complete — `failover.rs` + unit tests |
| 4.6 | `orchestrator/mod.rs` — `tokio::spawn` thread management. All three threads spawned concurrently, never sequentially. | **Cursor Agent (Opus Thinking)** | **You review every line.** Verify no `.await` between spawns. | [x] Complete |
| 4.7 | `orchestrator/directional.rs` — directional response thread. TTFT target < 800ms. | Cursor Agent — load prompt from `/prompts/directional/`. | Measure TTFT with `tracing`. Fail if > 900ms P95. | [x] Complete |
| 4.8 | `orchestrator/depth.rs` — depth response thread. Fully streamed < 8s. | Cursor Agent | Measure stream_complete_ms. | [x] Complete |
| 4.9 | `orchestrator/clarifying.rs` — clarifying question detection thread. | Cursor Agent | — | [x] Complete |
| 4.10 | Silence debounce: 600–1200ms after VAD end-of-speech before firing threads | Cursor Agent — reference the VAD config from `flint-audio.mdc` | Test with rapid speech | [x] Partial — fixed **600ms** (`SILENCE_DEBOUNCE` in `orchestrator/mod.rs`); upper range not configurable |
| 4.11 | `confidence.rs` — confidence scoring formula from `flint-data.mdc`. Computed locally, no LLM round-trip. | **`best-of-n-runner`** (3 attempts) — formula must be exact | Unit test all five score bands | [x] Complete |
| 4.12 | Token streaming to React: `directional_token`, `depth_token` events emitted per token | Cursor Agent | Test: tokens appear in UI incrementally | [x] Complete |
| 4.13 | `session/memory.rs` — conversation memory: full history for cloud providers, compression for Ollama | Cursor Agent (Sonnet) — reference `flint-data.mdc` memory section | Test compression with Ollama 4K context window | [x] Complete |
| 4.14 | Prompt loading: load from `/prompts/` directory, never inline in Rust | Cursor Agent | Verify no inline prompts anywhere | [x] Complete |
| 4.15 | Integration test: mock provider → full orchestrator → assert `directional_token` + `depth_token` events fired concurrently | `best-of-n-runner` | — | [x] Complete — `tests/integration/orchestrator.rs` |

### Phase 4 Review Gate
- [x] Threads are spawned concurrently — verified by `tokio::spawn` pattern in code
- [x] Confidence scores computed correctly for all five bands — unit tests in `confidence.rs`
- [ ] Directional TTFT: measure P95 over 20 runs → must be < 900ms (Groq) — `bench_gate` wired; production run pending
- [ ] Depth: fully streamed in < 8s P95
- [ ] One thread crash does not affect other threads (kill a thread mid-run, others continue) — manual
- [ ] Failover: mock Groq returning 500 → Ollama fires within 2s → `failover_triggered` event in UI
- [ ] Rate limit: mock 429 → Retry-After honoured → no immediate switch to Ollama — unit tests pass; live test open
- [ ] `@docs/flint_system_design_v3.md` Section 20 eval harness: run 10-question smoke test

---

## Phase 5 — UI: Five Panels

**Goal:** The complete stealth overlay with all five panels, token streaming rendering, confidence colours, hotkey system, and all panel interactions.  
**Duration:** 3–4 days  
**Status:** ✅ Code complete on `main` (merged via `feature/phase5-ui-panels`) — pending on-device validation (Review Gate items marked with `*`)

> **v1.5 layout change (Phase 5.5.9 — not started):** Vertical full-width resizable stack is **not** implemented yet. Current `OverlayLayout` is horizontal grid only. Stack + grid toggle tracked in Phase 5.5.9.

**Merged via:** `feature/phase5-ui-panels` (`cc23d44` → `f3067d4` Wayland capture hint).

### Tasks

| # | Task | Agent | Review? | Status |
|---|---|---|---|---|
| ~~5.1~~ | ~~Panel layout system — five-panel grid, resize, collapse. Layout state in Zustand only.~~ | Cursor Agent (Sonnet) | Test resize at different window sizes | [x] Complete — `src/store/ui.ts`, `src/components/OverlayLayout.tsx`; Vitest store + viewport tests |
| ~~5.2~~ | ~~`panels/DirectionalPanel.tsx` — token stream, 4px confidence left-border, Answer This / Rephrase~~ | Cursor Agent (Sonnet) | 4px left border only | [x] Complete — `useDirectionalStream`; `triggerResponse` / `rephraseResponse` via `commands/`; buffers cleared per turn |
| ~~5.3~~ | ~~`panels/DepthPanel.tsx` — structured rendering, pre-prepared label, Use This Answer~~ | Cursor Agent (Sonnet) | pre-prepared label on cache hit | [x] Complete — `useDepthStream` + `response_metadata`; section split; clipboard copy |
| ~~5.4~~ | ~~`panels/ClarifyingPanel.tsx` — clarifying questions ranked list~~ | Cursor Agent | — | [x] Complete — `onClarifyingQuestion`, rank-sorted in Zustand (hook extraction optional) |
| ~~5.5~~ | ~~`panels/ContextPanel.tsx` — RAG chunks + session digest summary~~ | Cursor Agent | — | [x] Complete — `useRagChunks` + `rag_chunks_update`; digest from `getSessionSnapshot` |
| ~~5.6~~ | ~~Hotkey system: Ctrl+Option/Alt tap, 2s hold, double-tap, +Shift panic hide~~ | Cursor Agent + Rust | **You test every hotkey on your machine.** | [x] Complete — Rust `Control+Alt` / `Control+Alt+Shift`; React `useHotkeys` timing layer |
| ~~5.7~~ | ~~Stealth overlay in `tauri.conf.json`~~ | Manual | OBS capture test | [x] Complete — window flags; runtime exclusion in `src-tauri/src/stealth.rs` (Windows + macOS) |
| ~~5.8~~ | ~~Stealth self-test before `READY → LIVE`~~ | Cursor Agent | Wayland pass | [x] Complete — `run_stealth_self_test()` in `checks.rs`; called from `start_session` |
| ~~5.9~~ | ~~`screens/Rehearsal.tsx` — mandatory before first live session~~ | Cursor Agent (Sonnet) | blocks skip to live | [x] Complete — `run_rehearsal_turn`, `complete_rehearsal`, keychain flag; App route enforced |
| ~~5.10~~ | ~~Token budget indicator in overlay~~ | Cursor Agent | cost cap | [x] Partial — `token_usage_update` + `useCostCap` wired; backend suspends on cap (Phase 7.4); indicator uses **hardcoded 50k warn**; no Settings UI for `setCostCap` |
| ~~5.11~~ | ~~Overlay at 1920×1080 and 2560×1440~~ | `browser-use` agent | — | [x] Partial — Vitest viewport render tests; **`browser-use` visual pass still manual** |

### Phase 5 Review Gate
- [x] All five panels render and respond to Tauri events (Rehearsal + `LiveOverlay`; Transcript/Clarifying use `events/` directly)
- [x] Token streaming: directional/depth tokens append incrementally via Zustand `append*Token`
- [x] Confidence left border: 4px `borderLeft` only, no background fill
- [ ] `*` Hotkeys: tap, hold, double-tap, panic — code wired; **requires on-device OS shortcut registration test**
- [x] Stealth self-test gates `start_session` (X11 fail, Wayland warn/pass)
- [ ] `*` Overlay not captured by OBS — Windows/macOS APIs wired; **manual OBS test required**; Wayland = PipeWire portal + one-time hint banner
- [x] Rehearsal cannot be skipped on first session (`keychain::is_rehearsal_completed` + `start_session` guard)
- [x] Multi-monitor: `stealth::place_on_non_primary_monitor` at app setup (top-right inset on first non-primary)

#### What can be verified now (without device)
- [x] App flow: `DigestReview → Rehearsal → LiveOverlay` (`src/App.tsx`)
- [x] No raw `invoke()` in `src/panels/` — all via `src/commands/index.ts`
- [x] Event hooks: `useDirectionalStream`, `useDepthStream`, `useRagChunks`, `useTokenUsage`, `useHotkeys`
- [x] New Tauri commands: `get_rehearsal_completed`, `run_rehearsal_turn`, `complete_rehearsal`
- [x] New events: `rag_chunks_update`, `response_metadata`, `overlay_visibility`, `hotkey_trigger`
- [x] `npx tsc --noEmit` clean; `npx vitest run` 23/23 passing (RTL cleanup in `src/test/setup.ts`)
- [x] `cargo clippy -- -D warnings` — passes in CI on `main`

### Phase 5 Audit — File-by-File Verification

Comprehensive audit run 2026-06-03 against `.cursor/rules` and `docs/flint_system_design_v3.md` (Module 4, FR-5.11, §16.1).

| File | Audit Focus | Verdict | Notes |
|---|---|---|---|
| `src/store/ui.ts` | UIState, panel layout, streaming buffers, token accumulation | ✅ Pass | Session state never stored here |
| `src/components/OverlayLayout.tsx` | Five-panel grid, resize, collapse, panic hide | ✅ Pass | `panicHideActive` returns null |
| `src/panels/DirectionalPanel.tsx` | 4px border, hooks, Answer This / Rephrase | ✅ Pass | `clearBuffersForNewTurn` before manual trigger |
| `src/panels/DepthPanel.tsx` | pre-prepared badge, clipboard, sections | ✅ Pass | Badge from `response_metadata` |
| `src/panels/ClarifyingPanel.tsx` | Ranked clarifying list | ✅ Pass | Minor: inline event listener vs dedicated hook |
| `src/panels/ContextPanel.tsx` | RAG + digest | ✅ Pass | `useRagChunks` |
| `src/screens/Rehearsal.tsx` | Orchestrator without audio pipeline | ✅ Pass | `runRehearsalTurn` + `completeRehearsal` |
| `src/screens/LiveOverlay.tsx` | Live overlay shell, `start_session` | ✅ Pass | Wayland hint banner on Linux |
| `src/hooks/useHotkeys.ts` | Tap / hold / double-tap / panic sync | ✅ Pass | Hold fires Answer Now + trigger; resets on panic |
| `src-tauri/src/hotkeys.rs` | Global shortcuts | ✅ Pass | `Control+Alt`, `Control+Alt+Shift` |
| `src-tauri/src/stealth.rs` | Capture exclusion + monitor placement | ✅ Pass (partial Linux) | Win raw FFI; macOS `NSWindowSharingNone`; Wayland = log + UX hint |
| `src-tauri/src/commands.rs` | Rehearsal + live gates | ✅ Pass | `READY` only; rehearsal + stealth checks |
| `src-tauri/src/orchestrator/mod.rs` | Per-turn `rag_chunks_update`, `token_usage_update` | ✅ Pass | `response_metadata` on pre-warm cache hit |

#### Fixes applied during Phase 5 (branch `feature/phase5-ui-panels`)
1. **App routing** — `DigestReview → Rehearsal → LiveOverlay`; removed shell placeholder.
2. **Rehearsal IPC** — `run_rehearsal_turn` / `complete_rehearsal`; keychain `rehearsal_completed`; no `start_session` during rehearsal.
3. **Live start gate** — `start_session` requires `READY`, rehearsal completed, `run_stealth_self_test()`.
4. **Hotkeys** — Rust shortcuts + `useHotkeys` debounce/hold/double-tap; `overlay_visibility` syncs `panicHideActive`.
5. **Panel events** — `rag_chunks_update`, `response_metadata`, accumulated `token_usage_update`.
6. **Review follow-ups** — token indicator single subscriber; buffer clear on live trigger; macOS capture exclusion; multi-monitor placement; Wayland capture hint; Vitest viewport tests.

---

## Phase 5.5 — v1.5 Rehearsal Enrichment

**Goal:** Question bank, prep checklist, research chat, vertical panel layout, per-session usage widget, structured Session Design fields. All Flint-desktop-internal work.  
**Branch:** merged to `main` via PR #11 (`6132431`)  
**Depends on:** Strategy B Phase 1 complete ✅ — Smart Resume handoff merged (`main` @ `a8d9727` / SR `4fbd506`)

> **Billing work is owned by Strategy B Phase 3.** Tasks 5.5.8 (unified credit ledger), 5.5.10 (`product_mode` entitlement), 5.5.11 (admin panel), and 5.5.12 (free trial limits) are tracked in `STRATEGY_B_INTEGRATION_PLAN.md` §§3.2–3.7. Phase 5.5 here covers only Flint-internal feature work that does not depend on the credit API being live. The usage widget (5.5.7) ships in this phase but in BYOK-token-only mode; credit display activates automatically when Strategy B Phase 3 lands.

### Tasks

| # | Task | Agent | Notes | Status |
|---|---|---|---|---|
| 5.5.1 | Structured Session Design fields — separate DB columns + RAG concat with section headers | Cursor Agent (Sonnet) | Required: JD + profile. Recommended: overview, values, tech, strategy. Search guide per field. | [x] Complete — merged to `main` via PR #10 |
| 5.5.2 | Draft session persistence on restart — `session/draft.rs`, `restore_draft_session` command | ✅ Done | Routes to correct screen on restart; digest + context_text + state persisted in SQLite | [x] Complete on `main` |
| 5.5.3 | Question bank in Rehearsal — digest Qs + universal bank + user add/remove | Cursor Agent | `question_bank_json` column on session; `get_question_bank`, `add_to_question_bank`, `remove_from_question_bank` commands | [x] Complete — migration v7, Rust commands, `QuestionBank.tsx`, Qs tab in Rehearsal sidebar |
| 5.5.4 | Prep checklist sidebar — field fill status, search guides, link back to Session Design field | Cursor Agent (Sonnet) | Amber/green per field; updates reactively as user fills fields | [x] Complete — `PrepChecklist.tsx`, Prep tab in Rehearsal sidebar |
| 5.5.5 | First-run Rehearsal modal — explains RAG-only grounding, lists empty fields, shows search queries | Cursor Agent (Sonnet) | Dismissable; "Don't show again" per session | [x] Complete — `FirstRunRehearsalModal.tsx`, `localStorage` dismiss flag |
| 5.5.6 | Research chat in Rehearsal — `thread_type: research`, RAG-only, chunk citations | Cursor Agent (Sonnet) | Tab/slide-over in Rehearsal; `run_research_chat` command; emits `research_token` + `research_citation` events | [x] Complete — Rust command, `ResearchChat.tsx`, events in `events/index.ts`, Chat tab in Rehearsal sidebar |
| 5.5.7 | Per-session usage widget — BYOK token mode (credit display activates with Strategy B Phase 3) | Cursor Agent | `token_usage_update` includes `usage_category`; BYOK shows tokens + USD | [x] Complete — `usage_category` field on payload, `UsageWidget.tsx`, breakdown in Zustand store |
| 5.5.8 | Settings — Groq/provider API key entry | Cursor Agent (Sonnet) | Backend commands exist; **no UI** — blocks real digest extraction | [x] Complete — `ProviderSettings.tsx` screen, Settings nav item wired in `App.tsx` |
| 5.5.9 | Vertical panel layout (default) + grid toggle — Zustand; preference in `localStorage` | Cursor Agent | `layoutMode: "stack" \| "grid"` in UIState; drag handles; default heights per FR-4.6 | [x] Complete — `layoutMode` in UIState/store, `StackPanelSlot`/`StackResizeHandle`, toggle in `OverlayLayout` |

### Phase 5.5 Review Gate

- [ ] Session Design: JD + profile block proceed; missing recommended fields show amber in checklist
- [ ] Question bank: add question → runs rehearsal turn → persists across restart
- [ ] Research chat: asks about pasted context → gets answer with chunk citation; asks about internet fact → honest "not in my context" response
- [ ] Usage widget: Rehearsal turn emits `usage_category: "rehearsal_turn"`; BYOK shows tokens not credits
- [ ] Layout: stack layout default; toggle to grid; preference survives restart; Directional panel is 30% default (visually dominant)
- [ ] handoff `export_version: 2` payload: JD + profile + company_overview all pre-fill Session Design fields
- [ ] Settings: enter Groq key → HealthCheck primary_llm turns pass → digest extraction populates Digest Review fields

---

## Phase 6 — Session Storage + Post-Session

**Goal:** SQLite persistence, crash recovery, Supabase sync, post-session summary.  
**Duration:** 2 days  
**Status:** ✅ Mostly complete on `main` (merged via `feature/phase6-session-storage`) — UI gaps + review gates open

### Tasks

| # | Task | Agent | Review? | Status |
|---|---|---|---|---|
| 6.1 | `session/persistence.rs` complete — write every transcript chunk and response to SQLite as they arrive (not just on session end) | Cursor Agent — reference WAL mode requirement from `flint-data.mdc` | Simulate crash mid-session → verify no data loss | [x] Complete — write-through in `audio/pipeline.rs`, `orchestrator/mod.rs`, state transitions |
| 6.2 | `session/recovery.rs` — on app start: detect LIVE/ENDING/CRASHED in SQLite → offer recovery UI | **Cursor Agent (Opus Thinking)** — recovery edge cases are subtle | Test: kill process at LIVE state → restart → recovery offered | [x] Complete — `Recovery.tsx`, `check_crash_recovery` in App bootstrap |
| 6.3 | Post-session Supabase sync — sync transcript + responses after ENDED state | Cursor Agent | Test sync failure: assert ENDED → CRASHED handled correctly | [x] Partial — fire-and-forget in `stop_session` → `supabase/session.rs`; failures log-only (no ENDED→CRASHED) |
| 6.4 | Post-session summary generation — session insights, usage breakdown, low-confidence topics | Cursor Agent (Sonnet) — load prompt from `/prompts/session_essence/` | Check prompt loaded from file, not inlined | [x] Complete — `generate_session_summary` command + prompts exist; `SessionSummary.tsx` screen routes after ENDED |
| 6.5 | `screens/SessionList.tsx` — list past sessions, promote to permanent, delete | Cursor Agent (Sonnet) | Test data retention: 30-day expiry logic | [x] Complete — list, pin/unpin, delete, clone-via-context |
| 6.6 | Integration test: force CRASHED state → restart → assert RECOVERING → READY with full transcript intact | `best-of-n-runner` | — | [x] Complete — `tests/integration/crash_recovery.rs` |

### Phase 6 Review Gate
- [x] Recovery loads full transcript from SQLite — integration test covers resume path
- [x] Session list shows 30-day sessions; promoted sessions permanent — `SessionList.tsx` + promote/demote commands
- [ ] Kill process at `LIVE` state: on restart, recovery is offered automatically — manual device test
- [ ] Supabase sync: session data in cloud after `ENDED` — requires configured Supabase + manual verify
- [ ] Post-session summary generated correctly — `SessionSummary.tsx` calls `generate_session_summary`; end-to-end screen test pending
- [ ] User can delete a session and it's gone from both SQLite and Supabase — local delete wired; cloud delete manual verify
- [x] `get_digest` SQLite fallback — cold-path fallback added; repopulates in-memory cache on restart

---

## Phase 7 — Hardening (Ongoing)

**Goal:** Production-grade error handling, performance verification against all NFRs, coverage targets hit, eval harness built out.  
**Duration:** Ongoing — minimum 1 week before any release  
**Status:** 🔄 Partial on `main` — backend hardening merged (`chore/phase7-security-audit`, `feature/phase7-hardening`); release gates + installers open

### Tasks

| # | Task | Agent | Review? | Status |
|---|---|---|---|---|
| 7.1 | Achieve coverage targets from `flint-testing.mdc` — state machine 100%, all others at target | `explore` agent to find gaps, then Cursor Agent to fill them | Run `cargo tarpaulin` | [ ] Not started — no coverage gate in CI |
| 7.2 | Eval harness — build 50-question test set, run against all three prompt variants | Cursor Agent (Sonnet) for harness scaffolding, then manual curation of questions | Win rate gate must pass | [x] Partial — `evals/` crate, 200-question bank, `.github/workflows/eval-prompts.yml`; `evals/baseline.json` committed (stub) |
| 7.3 | Performance benchmark suite — measure P95 for all NFR targets | `shell` Task agent — run benchmarks in parallel | All CI gates must pass | [x] Partial — `src-tauri/benches/`, `bench_gate.rs`, `.github/workflows/bench.yml`; first scheduled NFR run pending |
| 7.4 | Cost cap enforcement — configurable limit, suspend inference when exceeded | Cursor Agent | Test cost cap triggers at exact threshold | [x] Complete (backend) — `cost.rs`, orchestrator suspension; [ ] Partial (UI) — no Settings to configure cap |
| 7.5 | GDPR data deletion — Settings → Delete Account end-to-end | Cursor Agent | Test: account deleted → Supabase empty → keychain cleared → SQLite cleared | [x] Complete (backend) — `gdpr.rs`, `tests/integration/gdpr.rs`; [ ] Partial (UI) — **no Settings → Delete Account screen** |
| 7.6 | Feature flag system — Supabase Edge Function `/flags`, local cache, kill switch | Cursor Agent — reference `flint-rust.mdc` evaluation logic | Test: Supabase unreachable → cached flags used | [x] Complete (backend) — `flags.rs` + integration test; [ ] Partial (UI) — `useFeatureFlag` hook unused in screens |
| 7.7 | Final security audit — verify zero audio bytes on disk, all keys in keychain, all logs redacted | `explore` Task agent — search for any `String` holding api keys, any write to disk for audio | Manual review of findings | [x] Complete — merged `chore/phase7-security-audit`; provider key commands added |
| 7.8 | Distribution: build installers for all platforms, sign macOS + Windows | `shell` Task agent — reference CI/CD pipeline from `flint-testing.mdc` | Test installer on a clean VM | [ ] Not started — Strategy B Phase 1.5; blocks extension public beta |

**Phase 7 implementation review:** merged via `chore/phase7-security-audit` → `main`. Prompt archived in `.github/PHASE7_REVIEW_PROMPT.md`.

### Phase 7 Review Gate (Release Criteria)
- [x] GDPR deletion tested end-to-end (backend) — `tests/integration/gdpr.rs`
- [x] Crash recovery tested end-to-end — `tests/integration/crash_recovery.rs`
- [x] Security audit merged — provider keys in keychain; no secrets in INFO+ logs
- [ ] All CI NFR gates pass (TTFT, RAG, transcription lag) — `bench_gate` wired; production baseline run pending
- [ ] Eval harness: win rate ≥ 50%, conciseness ≥ 95%
- [ ] Coverage targets hit for all modules
- [ ] Zero audio bytes on disk (verified with disk monitoring on device)
- [ ] Installers signed and tested on clean macOS/Windows/Linux VMs (7.8)
- [ ] Stealth: not detected by 3 different screen capture tools tested
- [ ] Settings UI: provider keys, cost cap, GDPR delete/export (cross-cutting — blocks v1 UX)

---

## Phase 8 — Guided Mock Interview

**Goal:** Mic-only practice mode where an AI interviewer asks digest questions via TTS, the user answers aloud, and Flint streams a suggested answer plus structured coach feedback (grammar, tone, gaps, polished rewrite, score).  
**Duration:** ~1 week (Phase 1 slice)  
**Branch:** `feature/mock-interview` @ `3c2aea3` — **not merged to `main`**  
**Status:** ✅ Code complete — ⏳ manual device gate open

### Architecture (implemented)

| Layer | Module / file | Role |
|-------|---------------|------|
| State | `session/state.rs` | `MOCK_INTERVIEW` state; `REHEARSING ↔ MOCK_INTERVIEW → READY` |
| Persistence | `session/persistence.rs` v8 | `mock_turns` table — question, user_text, audio_path, coach_json, suggested, score |
| TTS | `mock/tts.rs` | Platform TTS: macOS `say`, Linux `espeak-ng`/`espeak`, Windows PowerShell |
| Conductor | `mock/conductor.rs` | Sequences `digest.likely_questions`; speaks question; streams suggested answer |
| Mic | `mock/mic_capture.rs` | Mic-only VAD + Whisper; emits `mock_user_transcribed` |
| Audio | `mock/audio_writer.rs` | Per-turn WAV under `{app_data}/mock_audio/` |
| Coach | `mock/coach.rs` | Post-answer LLM → `CoachFeedback` JSON |
| Commands | `commands.rs` | `start_mock`, `start_mock_turn`, `end_mock_turn`, `skip_mock_turn`, `stop_mock`, `get_mock_turns` |
| Events | `events.rs` + `src/events/index.ts` | `mock_question_started`, `mock_user_transcribed`, `mock_suggested_token`, `mock_coach_feedback`, `mock_ended` |
| Prompts | `prompts/mock_coach/`, `prompts/mock_suggested/` | Coach JSON schema + 120-word suggested answer |
| UI | `MockInterview.tsx`, `SuggestedAnswerPanel.tsx`, `CoachPanel.tsx` | Turn loop + merged guidance panels |
| Entry | `Rehearsal.tsx` | Purple **Mock Interview** button → `App.tsx` `mock-interview` screen |

### Tasks

| # | Task | Agent | Review? | Status |
|---|---|---|---|---|
| 8.1 | `MOCK_INTERVIEW` state + transitions in `session/state.rs` | Cursor Agent | State machine tests | [x] Complete — 5 transition tests |
| 8.2 | SQLite v8 `mock_turns` + persistence helpers | Cursor Agent | Migration test on fresh DB | [x] Complete — `SCHEMA_VERSION = 8` |
| 8.3 | Platform TTS for AI interviewer questions | Cursor Agent | Hear question spoken on each OS | [x] Complete — `mock/tts.rs`; Piper/ElevenLabs deferred |
| 8.4 | Conductor — question sequencer + suggested-answer LLM stream | Cursor Agent (Opus) | Questions from `likely_questions` only | [x] Complete — no dynamic follow-ups yet |
| 8.5 | Mic-only capture + per-turn WAV writer | Cursor Agent — reference Phase 3 VAD/Whisper | WAV file exists after turn | [x] Complete — `mock/mic_capture.rs`, `audio_writer.rs` |
| 8.6 | Coach LLM thread — structured JSON feedback + score | Cursor Agent | Coach JSON parser unit tests | [x] Complete — `mock/coach.rs` |
| 8.7 | Tauri commands + events + frontend screen | Cursor Agent | No raw `invoke()` in components | [x] Complete — IPC via `commands/index.ts` |
| 8.8 | Rehearsal entry point — **Mock Interview** button | Cursor Agent | Button visible from REHEARSING | [x] Complete |
| 8.9 | Dynamic follow-up questions (LLM-generated after each answer) | Cursor Agent (Opus) | Conductor generates next Q from context | [ ] Not started — Phase 8.2 |
| 8.10 | Mock session summary + audio replay in `SessionSummary` | Cursor Agent | Replay WAV per turn from summary | [ ] Not started — `get_mock_turns` exists; UI pending |
| 8.11 | Upgrade TTS — Piper (local) or ElevenLabs (cloud) | Cursor Agent | Voice quality vs platform TTS | [ ] Not started — platform TTS is Phase 8.1 default |
| 8.12 | Merge PR + CI green on all platforms | Shell agent | `cargo test`, `vitest`, clippy | [ ] Pending — branch pushed, PR not opened |

### Phase 8 Manual Test Checklist

Prerequisites: Groq API key in Settings; digest confirmed with ≥1 `likely_questions`; mic permission granted; Linux: `espeak-ng` or `espeak` installed for TTS.

- [ ] **Entry** — From Rehearsal, purple **Mock Interview** button visible; click → `MockInterview` screen loads
- [ ] **State** — Session state transitions to `MOCK_INTERVIEW` (check via devtools / `session_state_change` event)
- [ ] **TTS** — First question spoken aloud (platform voice); question text shown in interviewer bubble
- [ ] **Suggested answer** — Tokens stream into Suggested Answer panel while question is displayed
- [ ] **Start answering** — Click **Start Answering** → REC indicator; speak 10–20 s; live transcript appears in Your Answer panel
- [ ] **Done answering** — Click **Done Answering** → coach panel shows "Analyzing…" then score + tone/gaps/grammar/polished rewrite
- [ ] **Skip** — On a later turn, **Skip** advances without recording; conductor moves to next question
- [ ] **Full run** — Complete all `likely_questions` → `mock_ended` fires → returns to Rehearsal
- [ ] **Exit** — Mid-session **Exit** → `stop_mock` → back to Rehearsal, state `REHEARSING`
- [ ] **Persistence** — After a turn, `{app_data}/mock_audio/session_*_turn_*.wav` exists; `mock_turns` row in SQLite has coach_json + score
- [ ] **Draft recovery** — Kill app during mock → restart → draft session restores to Rehearsal (mock state in `DRAFT_STATES`)

### Phase 8 Review Gate

- [x] `cargo test --lib` passes (380 tests incl. mock state, TTS, WAV, coach JSON, mock-turn persistence race)
- [x] `cargo clippy -- -D warnings` passes
- [x] `npx vitest run` passes (31 tests)
- [ ] Manual checklist above passes on Linux (primary dev platform)
- [ ] Manual checklist passes on macOS and Windows (TTS path differs per OS)
- [ ] No regression: Rehearsal → Go live still works after mock session
- [ ] PR merged to `main`

---

## Quick Reference — What to Prompt Cursor With

### Starting a new task

```
I'm implementing [task name] in Flint.
Reference: @docs/flint_system_design_v3.md Section [N]
Rules: @.cursor/rules/[relevant-rule].mdc

The interface is:
[paste the trait or type definition you wrote yourself]

Implement [specific function/module] to make this test pass:
[paste your test stub]
```

### When you hit a bug

Use `explore` Task agent first:
```
In the Flint codebase, find all places where [symptom] could be caused. 
Check src-tauri/src/[module]/ specifically.
```

### Critical review checklist (before merging any PR)

- [ ] No raw `invoke()` or `listen()` in React components
- [ ] No session state written in React
- [ ] No audio bytes written to disk
- [ ] No API keys stored as `String`
- [ ] No inline prompts in Rust
- [ ] No direct state transitions (bypassing state machine)
- [ ] No sequential thread spawning (must be parallel)
- [ ] All new Supabase tables have RLS
- [ ] All new Rust code passes `cargo clippy -- -D warnings`

---

## Technology Ramp-up (If Unfamiliar)

| Technology | What to research | Agent to use |
|---|---|---|
| Tauri 2.x IPC | How to define commands and events, `invoke()` + `emit()` patterns | `generalPurpose` agent |
| fastembed-rs | bge-small-en-v1.5 API, batch embedding, dimension count | `generalPurpose` agent |
| sqlite-vec | Creating vector tables, inserting embeddings, cosine/dot product queries | `generalPurpose` agent |
| whisper-rs | Binding to Whisper.cpp, loading model file, running inference | `generalPurpose` agent |
| cpal | Device enumeration, loopback capture, stream error handling | `generalPurpose` agent |
| rnnoise-rs | Frame processing API, sample rate handling | `generalPurpose` agent |
| tokio::spawn | Cancellation tokens, join handles, error propagation | Cursor Agent — it knows tokio well |
| secrecy crate | `SecretString`, `ExposeSecret`, zeroing on drop | Cursor Agent |
| keyring crate | Platform-specific keychain access | `generalPurpose` agent — API varies by platform |
