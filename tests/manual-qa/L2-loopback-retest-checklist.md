# L2 — System audio loopback retest checklist

One-page manual QA for **L2** after the pulse-first loopback fix (PR #34).
Use this after merge — do **not** mark PASS without device evidence.

**Related:** [stealth-audio-validation-runbook.md](./stealth-audio-validation-runbook.md) (root-cause section) · [M13 live pipeline checklist](./m13-live-pipeline-checklist.md)

---

## Prerequisites

- Linux with **PipeWire** + **Wayland** session (Flint v1 target)
- `pipewire-pulse` installed (HealthCheck `system_audio_isolation` depends on pulse-first selection)
- Headphones optional but recommended to avoid speaker→mic bleed
- Flint build: `npm run dev:clean` (dev) or a signed/release build
- `RUST_LOG=info` available if you need to capture logs on failure

---

## Step 1 — HealthCheck

1. Launch Flint and open **Health Check** (onboarding or Settings).
2. Confirm **`system_audio_isolation`** is **Pass** or an acceptable **Warn** with actionable fix text.
3. If **Fail:** follow the fix instruction (typically install `pipewire-pulse` / verify PipeWire). **Stop here** — do not proceed to live audio until this passes or is explained.

Also note: `system_audio_loopback`, `microphone_access`, `stealth_api` — record pass/warn/fail.

---

## Step 2 — Start audio path

1. Create or open a session and enter **Rehearsal** or start a **Live** session (Rehearsal is enough for loopback verification).
2. Open a browser tab with **YouTube** or any site that plays audio (Zoom/Meet also valid).
3. Play continuous speech or music at moderate volume.

---

## Step 3 — Pass criteria

In the **Transcript** panel:

- Audio transcription appears on the **System / INTERVIEWER** channel only.
- Your microphone is **not** picking up the same browser audio as duplicate lines on **Mic / YOU**.
- Real mic speech (if you speak) appears on **Mic** only, separate from system audio.

**PASS** only if you observe the above with your own eyes on device.

---

## Step 4 — Fail criteria

Mark **FAIL** if any of the following occur:

- The same utterance appears on **both** System and Mic channels.
- Browser/YouTube audio never appears on System (only on Mic).
- Transcript is empty despite audible browser audio and HealthCheck pass.

On fail, capture:

```bash
RUST_LOG=info npm run tauri dev 2>&1 | tee /tmp/flint-l2-retest.log
```

Include the log snippet (device names, `system_audio_isolation`, capture start) in your QA notes.

---

## Step 5 — Record result

1. Update the **L2** row in [stealth-audio-validation-runbook.md](./stealth-audio-validation-runbook.md):

   `| L2 | System audio loopback | … | ☑ PASS / ☑ FAIL — <date>, <evidence link or log path> |`

2. Optionally file `tests/manual-qa/stealth-audio-validation-YYYY-MM-DD.md` with OS version and HealthCheck screenshot.

---

## Explicit rule

**Do not mark PASS without device evidence.** CI and unit tests cannot validate real loopback routing.
