# Private Mode & audio hardware validation (manual gate)

These checks require real hardware and OS-specific audio/screen-capture stacks.
They cannot be fully automated in CI. Do **not** mark ROADMAP items 22/24 closed
until each scenario has an explicit pass/fail recorded here or in linked QA notes.

## 2026-07-04 root-cause fix: system audio silently duplicated mic audio

On Linux, cpal's ALSA host can enumerate a native `"pipewire"` PCM plugin
*before* the `"pulse"` PCM plugin. `find_system_device()` used to accept
whichever of the two it found first. The native `pipewire` plugin does not
honour the `PULSE_SOURCE` env var Flint sets to target the sink monitor — it
just opens PipeWire's default *capture* node, which is the same node the
microphone stream opens. Result: System and Mic captured identical audio
(every utterance duplicated on both channels, real loopback/interviewer
audio never appeared), and no manual `pactl`/Bluetooth-profile fix could
change that, because the actual bug was upstream of all of it.

Fixed in `audio/capture.rs` (pulse-first device selection): explicitly prefer `"pulse"` over `"pipewire"`,
and fail fast (`AudioCapture::start`) if system and mic ever resolve to the
literal same device name. Added a new `system_audio_isolation` Health Check
(`health/checks.rs`) that runs the exact same resolution logic and **blocks
`start_session`** before Live if it would collide — this is now caught in
Health Check / Rehearsal, not discovered live via garbled transcripts. No
manual pactl/env var configuration should be required for this class of bug
going forward; if `system_audio_isolation` ever fails, the fix instruction
tells the user to install `pipewire-pulse`, not to hand-edit sources.

## Known limitations (no code fix in v1)

Documented in `tests/manual-qa/M3_LINUX_FINDINGS.md`:

| Item | Status | Notes |
| --- | --- | --- |
| Wayland global hotkeys without focus | **Open P2** | `tauri-plugin-global-shortcut` only fires when Flint is focused on Wayland; xdg-desktop-portal integration is future work — **do not fabricate a fix** |
| OBS / private mode on Wayland | **Accepted** | Full-monitor screencast may include Flint; content protection is best-effort |
| macOS private mode | Requires capture exclusion APIs | Verified via HealthCheck + `run_private_mode_self_test` at live start |

---

## Platform matrix

Run before v1 release. Attach logs (`RUST_LOG=info`) and HealthCheck screenshot.

### Linux (Wayland)

**Prerequisites:** PipeWire, Wayland session (X11 fails private mode gate), screencast permission.

| # | Scenario | Pass criteria | Result |
| --- | --- | --- | --- |
| L1 | HealthCheck | `private_mode_api`, `system_audio_loopback`, `microphone_access`, `global_hotkey`, `system_audio_isolation` all pass/warn acceptably | ☑ 2026-07-10 — all Pass except `microphone_access` Warn (expected stub; never probes mic) |
| L2 | System audio loopback | Play YouTube/browser audio — must appear on **System** channel only (not Mic). Zoom/Meet also valid (see `m13-live-pipeline-checklist.md` §A). **Do not mark PASS without device evidence.** | ☑ **PASS with headphones** (2026-07-10) — interviewer audio on INTERVIEWER only. **FAIL without headphones** — same utterance on YOU + INTERVIEWER (speaker→mic acoustic bleed; not pulse-first device collision). Checklist: [`L2-loopback-retest-checklist.md`](./L2-loopback-retest-checklist.md). Follow-ups (not L2): auto question detect **intermittent** (sometimes fires, sometimes needs UI **Q**); ~2s lag; open-speaker echo garbled YOU lines |
| L3 | Hotkeys **with focus** | Ctrl+Alt+Space re-ask and Ctrl+Alt+Shift+Space panic hide work while Flint/overlay focused; Linux fallbacks (Ctrl+Shift+Space, F8 dev) documented in slice 3 | ☑ **PASS** (2026-07-10, Rehearsal, focused) — Ctrl+Alt+Space trigger OK; Ctrl+Alt+Shift+Space panic hide OK. Fallbacks Ctrl+Shift+Space / F8 = same trigger as primary (no separate UI); if primary already works, pressing them looks like “nothing new.” **Nit:** panic hide leaves shell chrome visible (FLINT / New session / Past sessions / Settings / window − □ ×) — overlay panels hide, title bar does not |
| L4 | Hotkeys **without focus** | Record pass/fail — expected fail on Wayland until portal work lands | ☑ **FAIL (accepted P2)** (2026-07-10) — none of Ctrl+Alt+Space, Ctrl+Shift+Space, F8, or Ctrl+Alt+Shift+Space work when another app has focus. Matches known Wayland global-shortcut limitation |
| L5 | OBS / screen capture | Start OBS full-display capture; note whether Flint overlay is visible (document outcome) | ☑ **VISIBLE in OBS** (2026-07-10, Wayland) — Display Capture showed Flint in Rehearsal and Live preview. Accepted on Wayland (content protection best-effort). Recording file not found after stop (OBS RecFilePath=`~/`, RecEncoder=nvenc — may have failed to write); preview evidence sufficient for L5 |

### Linux run log — 2026-07-10 (Wayland + PipeWire)

| Item | Result |
| --- | --- |
| L1 HealthCheck | Pass for isolation/loopback/private mode/etc.; `microphone_access` Warn only (stub — never probes mic) |
| L2 with headphones | **PASS** — interviewer/YouTube on INTERVIEWER only; YOU separate when speaking |
| L2 without headphones | **FAIL (acoustic bleed)** — same utterance on YOU + INTERVIEWER; garbled YOU echo. Confirmed 2026-07-10 Zoom: echo lines were from the no-headphones segment only. Follow-up: harden open-speaker path (PipeWire `module-echo-cancel` + HealthCheck AEC + tune Jaccard gate). Not pulse-first device collision |
| Auto question detect | **Intermittent** — sometimes fires after silence; sometimes needs UI **Q** click. Manual Q always works. Parked as M10 soft issue, not L2 fail |
| Transcription lag | ~2s observed — within NFR warn band |
| Post-session summary | **FAIL / unavailable** — after ending Live, UI shows “Summary unavailable for this session.” (reproduced 2026-07-10). Frontend shows this when `generate_session_summary` returns non-JSON or invoke errors; soft fallback JSON would instead show “rate limited or offline…”. Non-blocking for L2/M10; park for summary JSON extraction / provider fix |
| L3 hotkeys (Rehearsal, focused) | **PASS** — Ctrl+Alt+Space trigger OK; panic hide (Ctrl+Alt+Shift+Space) OK. Ctrl+Shift+Space / F8 are **aliases** of the same trigger (for when Ctrl+Alt is blocked); no separate effect when primary already works. Panic hide leaves **title/nav bar** visible (private mode nit on shell screens) |
| L4 hotkeys (unfocused) | **FAIL (accepted P2)** — no hotkeys work when browser/other app focused; expected on Wayland until portal integration |
| L5 OBS Display Capture | **VISIBLE** — Flint shown in OBS preview for Rehearsal + Live. Accepted Wayland outcome. Recording file missing after Stop (check `~/` for `YYYY-MM-DD HH-MM-SS.mkv`; NVENC encoder may have blocked write) |
| M10 Zoom live (standard) | **Partial PASS** (2026-07-10) — works after non-BT output + headphones. Issues: BT HFP → no/watchdog audio; transcript WER poor; **Q per VAD chunk** splits questions so click answers half-question; context/answers weak. Prefer **Ctrl+Q once at end of question**. Details: `M10_LIVE_RELIABILITY.md` |
| Summary / Q / Copy fix | **Engineering** — PR `fix/qa-summary-transcript-ux`: JSON extract for post-session summary; Q sends full merged utterance; live Copy via arboard. Device retest after merge. |

**L2 gate status:** closed for Linux headphones path. Use headphones for all further live tests.

**Linux private mode matrix (L1–L5):** complete for this machine (2026-07-10).

---

### macOS

**Prerequisites:** [BlackHole 2ch](https://existential.audio/blackhole/) + Multi-Output Device (Speakers + BlackHole).

| # | Scenario | Pass criteria | Result |
| --- | --- | --- | --- |
| M1 | HealthCheck | BlackHole detected; private mode + mic checks pass | ☐ |
| M2 | System audio via BlackHole | Interviewer audio on System channel during live session | ☐ |
| M3 | OBS capture | Overlay excluded or documented as visible (record which) | ☐ |
| M4 | Global hotkeys unfocused | Ctrl+Q works while another app is focused | ☐ |

### Windows

**Prerequisites:** Default output device; WASAPI loopback (no virtual cable required).

| # | Scenario | Pass criteria | Result |
| --- | --- | --- | --- |
| W1 | HealthCheck | System audio loopback reported as supported | ☐ |
| W2 | WASAPI loopback | Meet/Zoom/browser audio on System channel | ☐ |
| W3 | OBS capture | Private Mode / capture-exclusion behavior recorded | ☐ |
| W4 | Global hotkeys unfocused | Ctrl+Q works while another app is focused | ☐ |

---

## Phone interview mode (all platforms)

Cross-reference `m13-live-pipeline-checklist.md` phone-mode scenarios and slice 9 diarization:

| # | Scenario | Pass criteria | Result |
| --- | --- | --- | --- |
| P1 | Phone mode live start | Single mic channel; banner + Ctrl+Q manual boundary works | ☑ **Partial** (2026-07-10) — Live started; **Ctrl+Q works**. Almost all lines on INTERVIEWER (expected: single mic → System). Few YOU lines. **Q button does not work** (same chunked half-questions as desktop). After hang-up, selecting transcript to copy → **whole page white** + **websocket failed** (likely Vite HMR / WebView freeze on large selection under `tauri dev`). Process can stay up with session left `LIVE`. |
| P2 | speakrs models | Download via Settings; SpeakerPicker appears when 2 speakers detected | ☐ not exercised this run |
| P3 | Diarization fallback | Ctrl+Q still works when models missing or diarization fails | ☐ Ctrl+Q worked without relying on diarization — treat as soft pass pending explicit model-missing run |

---

## Recording results

For each run, save:

1. OS + version (e.g. Ubuntu 24.04 Wayland, macOS 14, Windows 11)
2. HealthCheck export or screenshot
3. Pass/fail per row above
4. Link to `~/.flint/metrics.log` session summary if live pipeline tested

File results under `tests/manual-qa/` as `private-mode-audio-validation-YYYY-MM-DD.md` when complete.

---

## Manual gate closure

Mark **private mode/audio hardware validation** done in release docs only when:

- [ ] All platform rows L1–L4 / M1–M4 / W1–W4 have recorded pass/fail
- [ ] Wayland hotkey-without-focus outcome explicitly documented (pass or accepted fail)
- [ ] OBS capture outcome documented per platform
- [ ] No open **blocker** severities remain (P2/P3 may stay open with documented acceptance)

Until then, leave in `manual_gate_backlog`.
