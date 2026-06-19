# M6 Multi-Provider LLM — Manual Test Guide

Branch: `feature/m6-multi-provider-llm`  
Platform: Linux first; macOS/Windows same Settings + failover stack  
Design: `docs/ROADMAP.md` Phase 12, `docs/flint_system_design_v3.md` §27

## Prerequisites

1. At least one cloud key in Settings → API Keys (Groq, OpenAI, Anthropic, or DeepSeek).
2. Optional: DeepSeek + OpenRouter keys to exercise cloud fallback tiers.
3. Optional: Ollama on `localhost:11434` for local last-resort failover.
4. Dev builds: keys in project `.env` bootstrap to keychain on first run (`GROQ_API_KEY`, `DEEPSEEK_API_KEY`, etc.).

## Failover stack (default)

```
User-selected primary (default: Groq)
  → DeepSeek (if keyed, and not primary)
  → OpenRouter (if keyed)
  → Ollama (local)
```

## Test P1 — Primary provider selection

| Step | Action | Pass criteria |
|------|--------|---------------|
| 1 | Settings → API Keys → add Groq key | Status shows "Key stored" |
| 2 | Primary LLM → select **Groq** | Radio saves; no error toast |
| 3 | Add OpenAI key; select **OpenAI** | Primary switches; rehearsal uses OpenAI |
| 4 | Start a live session (mock audio OK) | Directional panel streams tokens |
| 5 | During LIVE, try switching primary | Error: not available during live session |

**Pass:** preference persists across app restart; LIVE switch rejected.

## Test P2 — Cloud fallback (DeepSeek)

Requires Groq primary + DeepSeek key. Simulate Groq failure by temporarily removing Groq key or using invalid key.

| Step | Action | Pass criteria |
|------|--------|---------------|
| 1 | Configure Groq primary + DeepSeek fallback key | Health check shows Groq configured |
| 2 | Invalidate Groq key (Settings → Remove) | — |
| 3 | Trigger inference (rehearsal or mock live) | UI shows failover indicator; DeepSeek serves response |
| 4 | Restore Groq key | After ~30s ping cycle, "Primary LLM restored" toast optional |

**Pass:** inference succeeds via DeepSeek without app crash.

## Test P3 — Local fallback (Ollama)

| Step | Action | Pass criteria |
|------|--------|---------------|
| 1 | Remove all cloud keys; ensure Ollama running | Health check warns on cloud; Ollama pass |
| 2 | Trigger inference | Red confidence border; Ollama response streams |
| 3 | Stop Ollama | Clear error message referencing Settings / Ollama |

**Pass:** graceful error when no tier available.

## Test P4 — Live latency gate (TTFT)

Run 15 directional-style prompts per provider. Record TTFT from first token event.

| Provider | P95 TTFT target | Notes |
|----------|-----------------|-------|
| Groq (`llama-3.3-70b-versatile`) | < 900 ms (fail PR > 900 ms) | Baseline |
| DeepSeek (`deepseek-chat`) | < 900 ms; ideal < 800 ms | Blocked if account has no balance |
| OpenAI (`gpt-4o-mini`) | < 900 ms | Optional |
| Anthropic (`claude-3-5-haiku`) | < 900 ms | Optional |

Script helper (dev dashboard or logs):

```bash
# After a rehearsal/live session, inspect ~/.flint/metrics.log for ttft_ms entries
grep directional_thread_complete ~/.flint/metrics.log | tail -15
```

Record results here:

| Provider | Runs | P50 TTFT | P95 TTFT | Pass? |
|----------|------|----------|----------|-------|
| Groq | | | | |
| DeepSeek | | | | |
| OpenAI | | | | |
| Anthropic | | | | |

## Test P5 — Token usage + cost cap

| Step | Action | Pass criteria |
|------|--------|---------------|
| 1 | Settings → Usage Cap → set low cap (e.g. 500 tokens) | Saved |
| 2 | Run session until cap hit | Inference suspended toast; panels idle |
| 3 | Switch primary provider; repeat | Usage accumulates per provider correctly |

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| DeepSeek 402 / insufficient balance | Account credits | Top up DeepSeek account |
| Anthropic 401 | Invalid key | Regenerate in console |
| Failover stuck on Ollama | All cloud tiers failed | Check keys; start Ollama |
| Primary picker disabled | No key for that provider | Add key first |

## Loop message when Block P4 passes

```
/flint-loop resume — m6-llm-providers Block P4 pass (TTFT recorded)
```

**Status:** OPEN — automated tasks 12.1–12.10 + 12.12 complete; live TTFT + eval smoke pending.
